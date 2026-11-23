//! End-to-end tests for the Download Engine.
//!
//! A local TCP server exercises real HTTP responses: Range/206, ETag changes,
//! abrupt connection loss, slow transfers and wrong checksums. The tests use
//! the real SQLite migrations, staging sidecars and verification path.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexus_core::{SystemClock, TaskId};
use nexus_platform::db::{Db, DbOptions};
use nexus_platform::download::{
    DownloadEngine, DownloadEngineConfig, DownloadEvent, DownloadEventSink, DownloadKind,
    DownloadRoot, FileStatus, NewDownloadFile, NewDownloadTask, TaskStatus,
};
use nexus_platform::security::{SafeFs, UrlPolicy};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct CaptureSink {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl CaptureSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

impl DownloadEventSink for CaptureSink {
    fn emit(&self, event: DownloadEvent) {
        let name = event.name().to_owned();
        if let Ok(value) = serde_json::to_value(&event) {
            self.events.lock().unwrap().push((name, value));
        }
    }
}

struct Harness {
    paths: nexus_core::DataPaths,
    db: Arc<Db>,
    policy: UrlPolicy,
    sink: Arc<CaptureSink>,
    // Dropped last so the database and data files outlive the engines.
    _temp: TempDir,
}

impl Harness {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = nexus_core::DataPaths::with_root(temp.path().join("data")).unwrap();
        paths.ensure_layout().unwrap();
        let (db, _) = Db::open_initialized(paths.db_path(), DbOptions::default(), &SystemClock)
            .await
            .unwrap();
        Self {
            paths,
            db: Arc::new(db),
            policy: UrlPolicy::nexus_source(true, true, true),
            sink: CaptureSink::new(),
            _temp: temp,
        }
    }

    async fn engine(&self, max_concurrent: usize, max_attempts: u32) -> Arc<DownloadEngine> {
        let fs = SafeFs::open(self.paths.root()).unwrap();
        let config = DownloadEngineConfig {
            max_attempts,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(15),
            max_redirects: 3,
            progress_interval: Duration::from_millis(25),
            persist_interval: Duration::from_millis(50),
            retry_base: Duration::from_millis(10),
            retry_max: Duration::from_millis(60),
            max_file_bytes: 64 * 1024 * 1024,
            user_agent: "NEXUS-Test/1".to_owned(),
        };
        let engine = Arc::new(DownloadEngine::new(
            self.db.pool().clone(),
            Arc::new(SystemClock),
            fs,
            self.policy.clone(),
            Some(self.sink.clone() as Arc<dyn DownloadEventSink>),
            config,
            max_concurrent,
            0,
        ));
        engine.start().await.unwrap();
        engine
    }
}

#[derive(Clone)]
enum Scenario {
    AbruptThenChanged {
        old_body: Vec<u8>,
        new_body: Vec<u8>,
    },
    SlowRange {
        body: Vec<u8>,
        chunk: usize,
        delay: Duration,
    },
    WrongHash {
        body: Vec<u8>,
    },
    Concurrency {
        body: Vec<u8>,
        chunk: usize,
        delay: Duration,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    },
}

struct Server {
    address: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }
}

async fn spawn_server(scenario: Scenario) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let scenario = scenario.clone();
            let index = counter.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::spawn(async move {
                handle_connection(socket, scenario, index).await;
            });
        }
    });
    Server { address, handle }
}

#[derive(Debug)]
struct Request {
    headers: HashMap<String, String>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    fn range_start(&self) -> Option<u64> {
        let raw = self.header("range")?;
        let value = raw.strip_prefix("bytes=")?;
        value.split('-').next()?.parse::<u64>().ok()
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    if method != "GET" || path.is_empty() {
        return None;
    }
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Some(Request { headers })
}

async fn handle_connection(mut socket: TcpStream, scenario: Scenario, index: usize) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };

    match scenario {
        Scenario::AbruptThenChanged { old_body, new_body } => {
            if index == 1 {
                let half = old_body.len() / 2;
                let head = response_head("200 OK", old_body.len(), Some("\"v1\""), None, true);
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&old_body[..half]).await;
            } else {
                let head = response_head("200 OK", new_body.len(), Some("\"v2\""), None, true);
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&new_body).await;
            }
        }
        Scenario::SlowRange { body, chunk, delay } => {
            let start = request.range_start().unwrap_or(0).min(body.len() as u64) as usize;
            let part = &body[start..];
            let status = if start > 0 {
                "206 Partial Content"
            } else {
                "200 OK"
            };
            let range = if start > 0 {
                Some(format!(
                    "bytes {}-{}/{}",
                    start,
                    body.len().saturating_sub(1),
                    body.len()
                ))
            } else {
                None
            };
            let head = response_head(status, part.len(), Some("\"stable\""), range, true);
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            for piece in part.chunks(chunk) {
                if socket.write_all(piece).await.is_err() {
                    return;
                }
                let _ = socket.flush().await;
                tokio::time::sleep(delay).await;
            }
        }
        Scenario::WrongHash { body } => {
            let head = response_head("200 OK", body.len(), Some("\"wrong\""), None, true);
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        }
        Scenario::Concurrency {
            body,
            chunk,
            delay,
            active,
            max_active,
        } => {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(current, Ordering::SeqCst);
            let head = response_head("200 OK", body.len(), Some("\"stable\""), None, true);
            if socket.write_all(head.as_bytes()).await.is_ok() {
                for piece in body.chunks(chunk) {
                    if socket.write_all(piece).await.is_err() {
                        break;
                    }
                    let _ = socket.flush().await;
                    tokio::time::sleep(delay).await;
                }
            }
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    let _ = socket.shutdown().await;
}

fn response_head(
    status: &str,
    content_length: usize,
    etag: Option<&str>,
    content_range: Option<String>,
    accept_ranges: bool,
) -> String {
    let mut head =
        format!("HTTP/1.1 {status}\r\nContent-Length: {content_length}\r\nConnection: close\r\n");
    if let Some(etag) = etag {
        head.push_str(&format!("ETag: {etag}\r\n"));
    }
    if let Some(range) = content_range {
        head.push_str(&format!("Content-Range: {range}\r\n"));
    }
    if accept_ranges {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    head.push_str("\r\n");
    head
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn task_request(url: String, relative_path: &str, size: usize, sha256: String) -> NewDownloadTask {
    NewDownloadTask {
        kind: DownloadKind::Custom,
        label: format!("test {relative_path}"),
        job_id: None,
        priority: 100,
        files: vec![NewDownloadFile {
            url,
            root: DownloadRoot::Downloads,
            relative_path: relative_path.to_owned(),
            size_bytes: Some(size as u64),
            sha1: None,
            sha256: Some(sha256),
            required: true,
            overwrite: true,
        }],
    }
}

async fn wait_for_status(
    engine: &DownloadEngine,
    id: TaskId,
    wanted: TaskStatus,
    timeout: Duration,
) -> nexus_platform::download::DownloadTaskSnapshot {
    let deadline = Instant::now() + timeout;
    loop {
        let task = engine.get_task(id).await.unwrap();
        if task.status == wanted {
            return task;
        }
        if matches!(
            task.status,
            TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Completed
        ) && task.status != wanted
        {
            panic!("task reached {:?}: {:?}", task.status, task.error_message);
        }
        if Instant::now() >= deadline {
            panic!(
                "timeout waiting for {:?}, current {:?}",
                wanted, task.status
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_bytes(engine: &DownloadEngine, id: TaskId, min: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let task = engine.get_task(id).await.unwrap();
        if task.done_bytes >= min {
            return;
        }
        if task.status.is_terminal() {
            panic!(
                "task finished before reaching {min} bytes: {:?}",
                task.status
            );
        }
        if Instant::now() >= deadline {
            panic!("timeout waiting for {min} bytes, got {}", task.done_bytes);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn chunked_body(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_add(seed))
        .collect()
}

#[tokio::test]
async fn interruption_and_changed_etag_do_not_merge_versions() {
    let harness = Harness::new().await;
    let old_body = vec![b'A'; 8192];
    let new_body = chunked_body(8192, 17);
    let server = spawn_server(Scenario::AbruptThenChanged {
        old_body,
        new_body: new_body.clone(),
    })
    .await;
    let engine = harness.engine(2, 5).await;

    let hash = sha256_hex(&new_body);
    let task = engine
        .enqueue(task_request(
            server.url("/file.bin"),
            "resume.bin",
            new_body.len(),
            hash.clone(),
        ))
        .await
        .unwrap();

    wait_for_status(
        &engine,
        TaskId::parse(&task.id).unwrap(),
        TaskStatus::Completed,
        Duration::from_secs(10),
    )
    .await;

    let installed = harness.paths.downloads_dir().join("resume.bin");
    let bytes = std::fs::read(&installed).unwrap();
    assert_eq!(
        bytes, new_body,
        "partial bytes from the old version must not survive"
    );
    assert_eq!(sha256_hex(&bytes), hash);
    assert!(
        !harness
            .paths
            .downloads_dir()
            .join("staging")
            .join(&task.id)
            .exists(),
        "staging must be empty after a successful task"
    );

    let names = harness.sink.names();
    assert!(names.iter().any(|name| name == "download:started"));
    assert!(names.iter().any(|name| name == "download:progress"));
    assert!(names.iter().any(|name| name == "download:completed"));
    assert!(!names.iter().any(|name| name == "download:failed"));
}

#[tokio::test]
async fn pause_and_resume_preserve_exact_offset() {
    let harness = Harness::new().await;
    let body = chunked_body(32 * 1024, 3);
    let server = spawn_server(Scenario::SlowRange {
        body: body.clone(),
        chunk: 2048,
        delay: Duration::from_millis(12),
    })
    .await;
    let engine = harness.engine(1, 4).await;
    let hash = sha256_hex(&body);
    let task = engine
        .enqueue(task_request(
            server.url("/slow.bin"),
            "slow.bin",
            body.len(),
            hash.clone(),
        ))
        .await
        .unwrap();
    let id = TaskId::parse(&task.id).unwrap();

    wait_for_bytes(&engine, id, 4096, Duration::from_secs(5)).await;
    let paused = engine.pause(id).await.unwrap();
    assert_eq!(paused.status, TaskStatus::Paused);
    assert!(paused.done_bytes > 0);
    assert!(paused.done_bytes < body.len() as u64);

    let files = engine.files(id).await.unwrap();
    let file = files.first().unwrap();
    assert_eq!(file.status, FileStatus::Paused);
    let staged = harness
        .paths
        .downloads_dir()
        .join("staging")
        .join(&task.id)
        .join(format!("{}.part", file.id));
    assert!(staged.is_file(), "partial data must stay in staging");

    engine.resume(id).await.unwrap();
    wait_for_status(&engine, id, TaskStatus::Completed, Duration::from_secs(15)).await;

    let installed = std::fs::read(harness.paths.downloads_dir().join("slow.bin")).unwrap();
    assert_eq!(installed, body);
    assert_eq!(sha256_hex(&installed), hash);
}

#[tokio::test]
async fn cancel_removes_staging_and_never_installs_a_partial_file() {
    let harness = Harness::new().await;
    let body = chunked_body(48 * 1024, 5);
    let server = spawn_server(Scenario::SlowRange {
        body: body.clone(),
        chunk: 2048,
        delay: Duration::from_millis(12),
    })
    .await;
    let engine = harness.engine(1, 4).await;
    let task = engine
        .enqueue(task_request(
            server.url("/cancel.bin"),
            "cancel.bin",
            body.len(),
            sha256_hex(&body),
        ))
        .await
        .unwrap();
    let id = TaskId::parse(&task.id).unwrap();

    wait_for_bytes(&engine, id, 4096, Duration::from_secs(5)).await;
    let cancelled = engine.cancel(id).await.unwrap();
    assert_eq!(cancelled.status, TaskStatus::Cancelled);

    let staging = harness.paths.downloads_dir().join("staging").join(&task.id);
    assert!(!staging.exists(), "cancel must delete staging");
    assert!(
        !harness.paths.downloads_dir().join("cancel.bin").exists(),
        "cancel must not install a partial file"
    );
}

#[tokio::test]
async fn shutdown_then_new_engine_restores_paused_task() {
    let harness = Harness::new().await;
    let body = chunked_body(64 * 1024, 9);
    let server = spawn_server(Scenario::SlowRange {
        body: body.clone(),
        chunk: 2048,
        delay: Duration::from_millis(12),
    })
    .await;
    let hash = sha256_hex(&body);

    let engine = harness.engine(1, 4).await;
    let task = engine
        .enqueue(task_request(
            server.url("/restart.bin"),
            "restart.bin",
            body.len(),
            hash.clone(),
        ))
        .await
        .unwrap();
    let id = TaskId::parse(&task.id).unwrap();
    wait_for_bytes(&engine, id, 4096, Duration::from_secs(5)).await;

    engine.shutdown().await;
    let after_shutdown = engine.get_task(id).await.unwrap();
    assert_eq!(after_shutdown.status, TaskStatus::Paused);
    drop(engine);

    let restarted = harness.engine(1, 4).await;
    let tasks = restarted.list_tasks(None).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Paused);

    restarted.resume(id).await.unwrap();
    wait_for_status(
        &restarted,
        id,
        TaskStatus::Completed,
        Duration::from_secs(20),
    )
    .await;

    let installed = std::fs::read(harness.paths.downloads_dir().join("restart.bin")).unwrap();
    assert_eq!(installed, body);
    assert_eq!(sha256_hex(&installed), hash);
}

#[tokio::test]
async fn checksum_mismatch_is_reported_and_never_committed() {
    let harness = Harness::new().await;
    let wrong_body = b"this is not the expected payload".to_vec();
    let server = spawn_server(Scenario::WrongHash {
        body: wrong_body.clone(),
    })
    .await;
    let engine = harness.engine(1, 2).await;
    let expected = sha256_hex(b"expected payload with a different hash");
    let task = engine
        .enqueue(task_request(
            server.url("/wrong.bin"),
            "wrong.bin",
            wrong_body.len(),
            expected,
        ))
        .await
        .unwrap();
    let id = TaskId::parse(&task.id).unwrap();

    wait_for_status(&engine, id, TaskStatus::Failed, Duration::from_secs(10)).await;
    assert!(
        !harness.paths.downloads_dir().join("wrong.bin").exists(),
        "a file with a wrong hash must never reach the destination"
    );
    assert!(
        !harness
            .paths
            .downloads_dir()
            .join("staging")
            .join(&task.id)
            .exists(),
        "checksum failures must discard staged bytes"
    );

    let errors = engine.errors(id, 50).await.unwrap();
    assert!(
        errors
            .iter()
            .any(|error| error.code == "DOWNLOAD_HASH_MISMATCH")
    );
    assert!(
        harness
            .sink
            .names()
            .iter()
            .any(|name| name == "download:failed")
    );
}

#[tokio::test]
async fn concurrency_limit_is_global_across_tasks() {
    let harness = Harness::new().await;
    let body = chunked_body(24 * 1024, 11);
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let server = spawn_server(Scenario::Concurrency {
        body: body.clone(),
        chunk: 1024,
        delay: Duration::from_millis(8),
        active: active.clone(),
        max_active: max_active.clone(),
    })
    .await;
    let engine = harness.engine(1, 3).await;
    let hash = sha256_hex(&body);

    let first = engine
        .enqueue(task_request(
            server.url("/a.bin"),
            "a.bin",
            body.len(),
            hash.clone(),
        ))
        .await
        .unwrap();
    let second = engine
        .enqueue(task_request(
            server.url("/b.bin"),
            "b.bin",
            body.len(),
            hash.clone(),
        ))
        .await
        .unwrap();

    wait_for_status(
        &engine,
        TaskId::parse(&first.id).unwrap(),
        TaskStatus::Completed,
        Duration::from_secs(15),
    )
    .await;
    wait_for_status(
        &engine,
        TaskId::parse(&second.id).unwrap(),
        TaskStatus::Completed,
        Duration::from_secs(15),
    )
    .await;

    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "global max_concurrent=1 must be respected"
    );
}
