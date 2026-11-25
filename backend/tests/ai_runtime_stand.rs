//! Автоматизированный стенд: реальный NEXUS AI Runtime (отдельный процесс) +
//! реальный backend + локальный OpenAI-compatible PolyStrata stub.
//!
//! Это НЕ замена проверке на втором компьютере: оба процесса запускаются на
//! одной машине. Стенд проверяет настоящий WSS, enrollment, heartbeat,
//! streaming, function calling, filesystem tools, usage, cancel и reconnect.
//!
//! Запуск:
//! ```text
//! scripts/ai-runtime-stand.ps1
//! ```
//! Тест помечен `#[ignore]`, потому что требует собранный `nexus-ai-runtime.exe`.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::TestEnv;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

struct Api {
    base: String,
    client: reqwest::Client,
    cookie: Option<String>,
    csrf: Option<String>,
}

impl Api {
    fn new(base: &str) -> Self {
        Self {
            base: base.to_owned(),
            client: reqwest::Client::new(),
            cookie: None,
            csrf: None,
        }
    }

    async fn login(&mut self, username: &str, password: &str) {
        let response = self
            .client
            .post(format!("{}/api/v1/auth/login", self.base))
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await
            .unwrap();
        if let Some(cookie) = response.headers().get("set-cookie") {
            self.cookie = Some(
                cookie
                    .to_str()
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .to_owned(),
            );
        }
        let payload: Value = response.json().await.unwrap();
        self.csrf = payload["data"]["csrfToken"].as_str().map(ToOwned::to_owned);
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .client
            .request(method.parse().unwrap(), format!("{}{}", self.base, path))
            .header("Accept", "application/json");
        if let Some(cookie) = &self.cookie {
            request = request.header("Cookie", cookie);
        }
        if let Some(csrf) = &self.csrf {
            request = request.header("X-CSRF-Token", csrf);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let payload = response.json().await.unwrap_or(Value::Null);
        (status, payload)
    }

    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, payload) = self.call(method, path, body).await;
        assert!(
            (200..300).contains(&status) && payload["success"] == true,
            "{method} {path}: {status} {payload}"
        );
        payload["data"].clone()
    }

    async fn get(&self, path: &str) -> Value {
        self.ok("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        self.ok("POST", path, Some(body)).await
    }
}

struct FakePoly {
    base: String,
    chat_calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl FakePoly {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let chat_calls = Arc::new(AtomicUsize::new(0));
        let counter = chat_calls.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let counter = counter.clone();
                tokio::spawn(async move {
                    let (path, body) = read_request(&mut stream).await;
                    let call_no = counter.fetch_add(1, Ordering::SeqCst);
                    let (content_type, payload) = if path.ends_with("/models") {
                        (
                            "application/json",
                            json!({ "data": [{ "id": "stand-model" }] }).to_string(),
                        )
                    } else if path.ends_with("/health") || path.ends_with("/status") {
                        (
                            "application/json",
                            json!({ "ok": true, "version": "stand" }).to_string(),
                        )
                    } else if path.ends_with("/chat/completions") {
                        let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                        let stream_flag = request["stream"].as_bool().unwrap_or(false);
                        let prompt = request["messages"]
                            .as_array()
                            .and_then(|messages| messages.last())
                            .and_then(|message| message["content"].as_str())
                            .unwrap_or("")
                            .to_owned();
                        let has_tool_result = request["messages"]
                            .as_array()
                            .map(|messages| {
                                messages.iter().any(|message| message["role"] == "tool")
                            })
                            .unwrap_or(false);
                        if prompt.contains("cancel long") {
                            // Долгий ответ: Runtime должен прервать его по cancel.
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                        if stream_flag {
                            let has_tools = request["tools"]
                                .as_array()
                                .map(|tools| !tools.is_empty())
                                .unwrap_or(false);
                            if has_tools && !has_tool_result {
                                let arguments = serde_json::to_string(&json!({
                                    "path": "stand.txt",
                                    "content": "created by runtime stand",
                                    "overwrite": true
                                }))
                                .unwrap();
                                let delta = json!({
                                    "choices": [{
                                        "delta": {
                                            "tool_calls": [{
                                                "index": 0,
                                                "id": "call_stand_1",
                                                "type": "function",
                                                "function": {
                                                    "name": "fs_write",
                                                    "arguments": arguments
                                                }
                                            }]
                                        },
                                        "finish_reason": null
                                    }]
                                });
                                let done = json!({
                                    "choices": [{
                                        "delta": {},
                                        "finish_reason": "tool_calls"
                                    }],
                                    "usage": { "total_tokens": 15 }
                                });
                                let sse = format!(
                                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                                    delta, done
                                );
                                ("text/event-stream", sse)
                            } else if has_tools {
                                let sse = concat!(
                                    "data: {\"choices\":[{\"delta\":{\"content\":\"Файл записан и проверен.\"},\"finish_reason\":null}]}\n\n",
                                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"total_tokens\":20}}\n\n",
                                    "data: [DONE]\n\n",
                                );
                                ("text/event-stream", sse.to_owned())
                            } else {
                                let sse = concat!(
                                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
                                    "data: {\"choices\":[{\"delta\":{\"content\":\" from stand\"},\"finish_reason\":null}]}\n\n",
                                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"total_tokens\":7}}\n\n",
                                    "data: [DONE]\n\n",
                                );
                                ("text/event-stream", sse.to_owned())
                            }
                        } else if has_tool_result {
                            (
                                "application/json",
                                json!({
                                    "id": format!("chatcmpl-{call_no}"),
                                    "object": "chat.completion",
                                    "choices": [{
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": "Файл записан и проверен."
                                        },
                                        "finish_reason": "stop"
                                    }],
                                    "usage": { "total_tokens": 20 }
                                })
                                .to_string(),
                            )
                        } else {
                            (
                                "application/json",
                                json!({
                                    "id": format!("chatcmpl-{call_no}"),
                                    "object": "chat.completion",
                                    "choices": [{
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": null,
                                            "tool_calls": [{
                                                "id": "call_stand_1",
                                                "type": "function",
                                                "function": {
                                                    "name": "fs_write",
                                                    "arguments": serde_json::to_string(&json!({
                                                        "path": "stand.txt",
                                                        "content": "created by runtime stand",
                                                        "overwrite": true
                                                    })).unwrap()
                                                }
                                            }]
                                        },
                                        "finish_reason": "tool_calls"
                                    }],
                                    "usage": { "total_tokens": 15 }
                                })
                                .to_string(),
                            )
                        }
                    } else {
                        ("application/json", json!({ "ok": true }).to_string())
                    };
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self {
            base: format!("http://{address}/v1"),
            chat_calls,
            task,
        }
    }
}

impl Drop for FakePoly {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, String) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.unwrap();
        if read == 0 {
            return (String::new(), String::new());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_owned();
    let length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.unwrap();
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    (
        path,
        String::from_utf8_lossy(&buffer[header_end..]).to_string(),
    )
}

async fn start_server(env: &TestEnv) -> (String, tokio::task::JoinHandle<()>) {
    let app = nexus::build_router(env.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}"), handle)
}

fn runtime_binary() -> Option<std::path::PathBuf> {
    std::env::var_os("NEXUS_RUNTIME_BIN")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file())
}

fn spawn_runtime(
    bin: &std::path::Path,
    config: &std::path::Path,
    args: &[&str],
) -> std::process::Child {
    let mut command = std::process::Command::new(bin);
    command.arg("--config").arg(config);
    for arg in args {
        command.arg(arg);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("не удалось запустить nexus-ai-runtime")
}

/// Убивает дочерний Runtime даже при панике теста.
struct RuntimeProc(Option<std::process::Child>);

impl RuntimeProc {
    fn spawn(bin: &std::path::Path, config: &std::path::Path, args: &[&str]) -> Self {
        Self(Some(spawn_runtime(bin, config, args)))
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RuntimeProc {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn wait_node_online(api: &Api) -> Value {
    for _ in 0..120 {
        let nodes = api.get("/api/v1/ai/admin/runtime-nodes").await;
        if let Some(node) = nodes["items"]
            .as_array()
            .and_then(|items| items.iter().find(|node| node["name"] == "STAND-RUNTIME"))
        {
            if node["status"] == "online" {
                return node.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("Runtime не подключился к NEXUS за 30 секунд");
}

async fn wait_task(api: &Api, task_id: Uuid, status: &str) -> Value {
    for _ in 0..200 {
        let task = api.get(&format!("/api/v1/ai/tasks/{task_id}")).await;
        if task["status"] == status {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("задача {task_id} не перешла в {status}");
}

async fn wait_message(api: &Api, conversation_id: Uuid, status: &str) -> Value {
    for _ in 0..200 {
        let page = api
            .get(&format!(
                "/api/v1/ai/conversations/{conversation_id}/messages"
            ))
            .await;
        if let Some(message) = page["items"].as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["role"] == "assistant" && item["status"] == status)
        }) {
            return message.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("ассистент не перешёл в {status}");
}

#[tokio::test]
#[ignore = "требует собранный runtime (запускайте scripts/ai-runtime-stand.ps1)"]
async fn real_runtime_process_over_wss() {
    let Some(bin) = runtime_binary() else {
        eprintln!("NEXUS_RUNTIME_BIN не задан — стенд пропущен");
        return;
    };
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("standadmin", "SecretPass1!").await;
    api.login("standadmin", "SecretPass1!").await;

    let fake = FakePoly::start().await;
    let allowed = env.root.path().join("stand-roots");
    let workspace_root = allowed.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let state_dir = env.root.path().join("stand-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let host = base.replace("http://", "");
    let config_path = env.root.path().join("runtime.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[server]
url = "ws://{host}/api/v1/ai/runtime/connect"
enroll_url = "http://{host}"
name = "STAND-RUNTIME"
allow_insecure_ws = true
heartbeat_seconds = 5
reconnect_min_seconds = 1
reconnect_max_seconds = 2

[storage]
state_dir = {state_dir:?}
allowed_roots = [{allowed:?}]

[provider]
base_url = "{fake_base}"
timeout_seconds = 30
connect_timeout_seconds = 5

[logging]
level = "warn"
"#,
            fake_base = fake.base,
            state_dir = state_dir.to_string_lossy(),
            allowed = allowed.to_string_lossy(),
        ),
    )
    .unwrap();

    let enrollment = api
        .post(
            "/api/v1/ai/admin/enrollment-codes",
            json!({ "label": "stand", "ttlSeconds": 600 }),
        )
        .await;
    let code = enrollment["code"].as_str().unwrap().to_owned();
    let enroll = spawn_runtime(
        &bin,
        &config_path,
        &["enroll", "--code", &code, "--name", "STAND-RUNTIME"],
    );
    let status = tokio::task::spawn_blocking(move || enroll.wait_with_output().unwrap())
        .await
        .unwrap();
    assert!(status.status.success(), "enroll: {status:?}");

    let mut runtime = RuntimeProc::spawn(&bin, &config_path, &["run"]);
    let node = wait_node_online(&api).await;
    let node_id = Uuid::parse_str(node["id"].as_str().unwrap()).unwrap();

    let provider = api
        .post(
            "/api/v1/ai/admin/providers",
            json!({
                "name": "Stand PolyStrata",
                "baseUrl": fake.base,
                "defaultModel": "stand-model",
                "runtimeNodeId": node_id,
                "enabled": true
            }),
        )
        .await;
    let _provider_id = provider["id"].clone();

    let workspace = api
        .post(
            "/api/v1/ai/admin/workspaces",
            json!({
                "ownerId": admin_id,
                "runtimeNodeId": node_id,
                "name": "STAND WS",
                "rootPath": workspace_root.to_string_lossy(),
                "allowedTools": ["fs.list", "fs.read", "fs.write", "fs.mkdir", "skill.load"],
                "allowedPrograms": [],
                "maxParallelTasks": 1,
                "maxTaskSeconds": 120
            }),
        )
        .await;
    let workspace_id = workspace["id"].as_str().unwrap().to_owned();

    // 1. Потоковый Chat через реальный Runtime и PolyStrata stub.
    let chat = api
        .post(
            "/api/v1/ai/conversations",
            json!({ "mode": "chat", "model": "stand-model" }),
        )
        .await;
    let chat_id = Uuid::parse_str(chat["id"].as_str().unwrap()).unwrap();
    api.post(
        &format!("/api/v1/ai/conversations/{chat_id}/messages"),
        json!({ "content": "Скажи hello" }),
    )
    .await;
    let message = wait_message(&api, chat_id, "completed").await;
    assert_eq!(message["content"], "Hello from stand");

    // 2. Agent function calling + реальная запись файла на Runtime.
    let agent = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "stand-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let agent_id = Uuid::parse_str(agent["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{agent_id}/messages"),
            json!({ "content": "Запиши файл stand.txt" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();
    wait_task(&api, task_id, "completed").await;
    let file = workspace_root.join("stand.txt");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "created by runtime stand"
    );

    // 3. Реальный usage, измеренный Runtime на диске.
    let usage = api
        .post(
            &format!("/api/v1/ai/workspaces/{workspace_id}/usage/refresh"),
            json!({}),
        )
        .await;
    assert!(usage["usedBytes"].as_i64().unwrap() >= 24);
    assert_eq!(usage["usedFiles"], 1);

    // 4. Отмена длинной генерации с подтверждением Runtime.
    let cancel_conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "stand-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let cancel_id = Uuid::parse_str(cancel_conversation["id"].as_str().unwrap()).unwrap();
    let cancel_task = api
        .post(
            &format!("/api/v1/ai/conversations/{cancel_id}/messages"),
            json!({ "content": "cancel long operation" }),
        )
        .await;
    let cancel_task_id = Uuid::parse_str(cancel_task["task"]["id"].as_str().unwrap()).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let cancelled = api
        .post(
            &format!("/api/v1/ai/tasks/{cancel_task_id}/cancel"),
            json!({}),
        )
        .await;
    assert_eq!(cancelled["requested"], true);
    let task = wait_task(&api, cancel_task_id, "cancelled").await;
    assert_eq!(task["cancelStatus"], "confirmed");

    // 5. Перезапуск процесса Runtime и повторное подключение.
    runtime.kill();
    let mut restarted = RuntimeProc::spawn(&bin, &config_path, &["run"]);
    let node = wait_node_online(&api).await;
    assert_eq!(node["id"], json!(node_id));
    restarted.kill();
    let _ = fake.chat_calls.load(Ordering::SeqCst);

    // 6. Отзыв токена: узел становится revoked.
    api.post(
        &format!("/api/v1/ai/admin/runtime-nodes/{node_id}/revoke"),
        json!({}),
    )
    .await;
    let nodes = api.get("/api/v1/ai/admin/runtime-nodes").await;
    let node = nodes["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!(node_id))
        .unwrap();
    assert_eq!(node["status"], "revoked");
    server.abort();
}
