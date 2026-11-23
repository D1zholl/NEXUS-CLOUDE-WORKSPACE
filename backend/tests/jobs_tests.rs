//! C1/C2/C3/C4/C5: планировщик, лимит, пауза/возобновление, отмена и события.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::realtime::Event;
use nexus::storage::engine::{ObjectLocation, build_object_key};
use nexus::storage::hashing::hash_hex;
use nexus::system::scheduler;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn send(env: &TestEnv, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = request;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("тело ответа читается");
    (status, headers, bytes.to_vec())
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("ответ — JSON")
}

async fn api_credentials(env: &TestEnv, user: Uuid) -> (String, String) {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .expect("сессия создаётся");
    let cookie = format!("{SESSION_COOKIE}={token}");
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    (cookie, csrf)
}

async fn post_job_action(
    env: &TestEnv,
    cookie: &str,
    csrf: &str,
    id: Uuid,
    action: &str,
) -> (StatusCode, Value) {
    let (status, _, bytes) = send(
        env,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/jobs/{id}/{action}"))
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    (status, parse(&bytes))
}

async fn insert_job(env: &TestEnv, job_type: &str, payload: Value) -> Uuid {
    sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable)
         values ($1, 'pending', $2, true)
         returning id",
    )
    .bind(job_type)
    .bind(payload)
    .fetch_one(&env.pool)
    .await
    .expect("задача создаётся")
}

async fn job_state(env: &TestEnv, id: Uuid) -> (String, Option<String>, Option<Value>) {
    sqlx::query_as("select status, error, result from background_jobs where id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .expect("задача читается")
}

async fn job_status(env: &TestEnv, id: Uuid) -> String {
    sqlx::query_scalar("select status from background_jobs where id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .expect("статус читается")
}

async fn wait_status(env: &TestEnv, id: Uuid, expected: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = job_status(env, id).await;
        if status == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "задача не перешла в {expected} за {timeout:?}, статус {status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_terminal(env: &TestEnv, id: Uuid, timeout: Duration) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let (status, _, _) = job_state(env, id).await;
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "задача не завершилась за {timeout:?}, статус {status}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Собирает события realtime до паузы в поступлении.
async fn collect_events(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await {
        events.push(event);
    }
    events
}

fn has_event(events: &[Event], kind: &str) -> bool {
    events.iter().any(|event| event.kind == kind)
}

/// C1: на чистой схеме появляются поля паузы и частичный индекс очереди.
#[tokio::test]
async fn migration_adds_pause_fields_and_pending_index() {
    let env = TestEnv::new().await;

    let columns: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_schema = 'public' and table_name = 'background_jobs'",
    )
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert!(columns.iter().any(|name| name == "pause_requested"));
    assert!(columns.iter().any(|name| name == "paused_at"));

    let definition: Option<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes
         where schemaname = 'public' and tablename = 'background_jobs'
           and indexname = 'background_jobs_pending_idx'",
    )
    .fetch_optional(&env.pool)
    .await
    .unwrap();
    let definition = definition.expect("частичный индекс очереди существует");
    assert!(definition.contains("status = 'pending'"));
}

/// C2/C5: задача из `POST /jobs/maintenance` больше не остаётся вечно pending.
#[tokio::test]
async fn maintenance_job_is_picked_up_by_scheduler() {
    let env = TestEnv::new().await;
    let user = env.create_user("jobscheduler", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let scheduler = scheduler::spawn(env.state.clone());

    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/jobs/maintenance")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "kind": "metrics_prune" }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = Uuid::parse_str(parse(&bytes)["data"]["id"].as_str().unwrap()).unwrap();

    let final_status = wait_terminal(&env, id, Duration::from_secs(15)).await;
    assert_eq!(final_status, "completed", "планировщик выполняет задачу");

    let (_, error, result) = job_state(&env, id).await;
    assert!(error.is_none());
    assert!(result.is_some(), "result записан");

    scheduler.shutdown(&env.pool).await;
    assert!(scheduler.wait_idle(Duration::from_secs(5)).await);
}

/// E8: планировщик сам ставит очистку ключей идемпотентности в очередь,
/// повторный вызов не создаёт дубль.
#[tokio::test]
async fn scheduler_enqueues_idempotency_cleanup_once() {
    let env = TestEnv::new().await;

    let first = scheduler::enqueue_scheduled_jobs(&env.pool).await;
    assert!(first >= 1, "первый вызов создаёт задачи обслуживания");
    let second = scheduler::enqueue_scheduled_jobs(&env.pool).await;
    assert_eq!(second, 0, "повторный вызов не создаёт дубликаты");

    let pending: i64 = sqlx::query_scalar(
        "select count(*) from background_jobs
         where job_type = 'system.idempotency.cleanup' and status = 'pending'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "ровно одна задача очистки ключей");

    // Задача реально выполняется и удаляет ключ старше 24 часов.
    let user = env.create_user("idem-scheduler", "SecretPass1!").await;
    let hash = nexus::core::idempotency::hash_request(&json!({ "x": 1 }));
    nexus::core::idempotency::begin(&env.pool, user, "test", "scheduled-key", &hash)
        .await
        .unwrap();
    sqlx::query(
        "update idempotency_keys set created_at = now() - interval '2 days'
         where key = 'scheduled-key'",
    )
    .execute(&env.pool)
    .await
    .unwrap();

    let job_id: Uuid = sqlx::query_scalar(
        "select id from background_jobs
         where job_type = 'system.idempotency.cleanup' and status = 'pending'
         order by created_at limit 1",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    let left: i64 = sqlx::query_scalar("select count(*) from idempotency_keys where key = $1")
        .bind("scheduled-key")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "задача очистки удалила устаревший ключ");
}

/// C5: при лимите 1 три задачи выполняются строго по очереди.
#[tokio::test]
async fn scheduler_honours_concurrency_limit() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("limit-vol").await;
    let root = env.root.path().join("limit-vol");

    // Достаточно крупный объект, чтобы задачи не завершались между опросами.
    let content = vec![7u8; 8 * 1024 * 1024];
    insert_object(
        &env,
        volume_id,
        &root,
        &content,
        &hash_hex(&content),
        "limit.bin",
    )
    .await;

    nexus::system::settings::apply_system_setting(
        &env.state,
        "jobs.max_concurrent",
        json!(1),
        None,
        "test",
        "127.0.0.1",
    )
    .await
    .expect("лимит сохраняется");
    assert_eq!(env.state.jobs_max_concurrent().await, 1);

    let ids: Vec<Uuid> = vec![
        insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await,
        insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await,
        insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await,
    ];

    let scheduler = scheduler::spawn(env.state.clone());
    let mut max_active = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        max_active = max_active.max(scheduler.active());
        let remaining: i64 = sqlx::query_scalar(
            "select count(*) from background_jobs where status in ('pending', 'running')",
        )
        .fetch_one(&env.pool)
        .await
        .unwrap();
        if remaining == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "очередь не разошлась за 30 с, осталось {remaining}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert_eq!(
        max_active, 1,
        "при лимите 1 одновременно активна одна задача"
    );
    for id in ids {
        let (status, _, result) = job_state(&env, id).await;
        assert_eq!(status, "completed");
        assert_eq!(result.expect("result записан")["checked"], 1);
    }

    scheduler.shutdown(&env.pool).await;
    assert!(scheduler.wait_idle(Duration::from_secs(5)).await);
}

/// C2/C5: при старте прерванные `running` возвращаются в `pending` с причиной.
#[tokio::test]
async fn restart_returns_running_job_to_pending() {
    let env = TestEnv::new().await;
    let id = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set status = 'running', started_at = now() where id = $1")
        .bind(id)
        .execute(&env.pool)
        .await
        .unwrap();

    let recovered = scheduler::recover_interrupted(&env.pool).await;
    assert_eq!(recovered, 1);

    let (status, error, _) = job_state(&env, id).await;
    assert_eq!(status, "pending");
    assert!(
        error.as_deref().unwrap_or_default().contains("перезапуск"),
        "причина восстановления сохраняется в error"
    );
}

/// Остановка службы: планировщик не теряет running-задачу, а только просит
/// отмену. Если обработчик не успел завершиться, следующий старт вернёт её в
/// `pending` — это и есть C2 для Stop-Service/жёсткого kill.
#[tokio::test]
async fn shutdown_marks_running_job_and_recovery_requeues_it() {
    let env = TestEnv::new().await;
    let scheduler = scheduler::spawn(env.state.clone());
    // Вставляем сразу running: планировщик не должен успеть захватить задачу
    // между insert и update.
    let id: Uuid = sqlx::query_scalar(
        "insert into background_jobs (job_type, status, payload, cancellable, started_at)
         values ('system.metrics.prune', 'running', $1::jsonb, true, now())
         returning id",
    )
    .bind(json!({ "kind": "system.metrics.prune" }))
    .fetch_one(&env.pool)
    .await
    .unwrap();

    scheduler.shutdown(&env.pool).await;

    let (status, cancel_requested): (String, bool) =
        sqlx::query_as("select status, cancel_requested from background_jobs where id = $1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(status, "running", "graceful stop не подменяет статус");
    assert!(
        cancel_requested,
        "текущей задаче выставлен cancel_requested"
    );

    let recovered = scheduler::recover_interrupted(&env.pool).await;
    assert!(
        recovered >= 1,
        "следующий старт возвращает задачу в очередь"
    );
    let (status, error, _) = job_state(&env, id).await;
    assert_eq!(status, "pending");
    assert!(
        error.as_deref().unwrap_or_default().contains("перезапуск"),
        "причина восстановления сохраняется в error"
    );
}

/// C4: незавершённая задача не может быть захвачена дважды.
#[tokio::test]
async fn pending_job_is_claimed_once() {
    let env = TestEnv::new().await;
    insert_job(&env, "system.metrics.prune", json!({})).await;

    let (first, second) = tokio::join!(
        scheduler::claim_next(&env.pool),
        scheduler::claim_next(&env.pool)
    );
    let claimed: Vec<Uuid> = [first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(claimed.len(), 1, "SKIP LOCKED не отдаёт одну задачу дважды");
}

/// C3/C5: пауза останавливает после текущей единицы, resume продолжает,
/// слот планировщика освобождается и события приходят в realtime.
#[tokio::test]
async fn pause_after_current_unit_and_resume_completes_job() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("pause-vol").await;
    let root = env.root.path().join("pause-vol");

    let first = b"first";
    let second = b"second";
    insert_object(&env, volume_id, &root, first, &hash_hex(first), "a.bin").await;
    insert_object(&env, volume_id, &root, second, &hash_hex(second), "b.bin").await;

    let user = env.create_user("jobpauser", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let mut rx = env.state.realtime.subscribe();

    let id = insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await;

    // 1. Пауза ожидающей задачи применяется сразу.
    let (status, body) = post_job_action(&env, &cookie, &csrf, id, "pause").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "paused");
    assert!(body["data"]["pausedAt"].is_string(), "paused_at заполнен");

    // 2. Возобновление возвращает задачу в очередь.
    let (status, body) = post_job_action(&env, &cookie, &csrf, id, "resume").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "pending");
    assert!(body["data"]["pausedAt"].is_null());

    // 3. Запрос паузы во время выполнения: задача должна закончить текущую
    // единицу и только затем встать на паузу.
    sqlx::query("update background_jobs set pause_requested = true where id = $1")
        .bind(id)
        .execute(&env.pool)
        .await
        .unwrap();

    let scheduler = scheduler::spawn(env.state.clone());
    wait_status(&env, id, "paused", Duration::from_secs(15)).await;

    let (status, error, _) = job_state(&env, id).await;
    assert_eq!(status, "paused");
    assert!(error.is_none());
    let (pause_requested, paused_at, done, total): (
        bool,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<i64>,
        Option<i64>,
    ) = sqlx::query_as(
        "select pause_requested, paused_at, done_units, total_units
           from background_jobs where id = $1",
    )
    .bind(id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(!pause_requested, "флаг сбрасывается при переходе в paused");
    assert!(paused_at.is_some(), "paused_at заполняется исполнителем");
    assert_eq!(done, Some(1), "текущая единица работы доведена до конца");
    assert_eq!(total, Some(2));

    // 4. Пауза освобождает слот: следующая задача выполняется без resume.
    let other = insert_job(&env, "system.metrics.prune", json!({})).await;
    assert_eq!(
        wait_terminal(&env, other, Duration::from_secs(15)).await,
        "completed",
        "слот освобождён, очередь продвигается"
    );

    // 5. Resume: задача возвращается в очередь и завершается.
    let (status, body) = post_job_action(&env, &cookie, &csrf, id, "resume").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "pending");

    assert_eq!(
        wait_terminal(&env, id, Duration::from_secs(15)).await,
        "completed"
    );
    let progress: f64 = sqlx::query_scalar("select progress from background_jobs where id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(progress, 100.0);

    let events = collect_events(&mut rx).await;
    assert!(has_event(&events, "job.paused"), "job.paused опубликован");
    assert!(has_event(&events, "job.resumed"), "job.resumed опубликован");
    assert!(
        has_event(&events, "job.completed"),
        "job.completed опубликован"
    );

    scheduler.shutdown(&env.pool).await;
    assert!(scheduler.wait_idle(Duration::from_secs(5)).await);
}

/// C3/C5: отмена из `pending`, `running` и `paused` даёт `cancelled`.
#[tokio::test]
async fn cancel_works_for_pending_running_and_paused_jobs() {
    let env = TestEnv::new().await;
    let user = env.create_user("jobcanceller", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    // pending → cancelled сразу.
    let pending = insert_job(&env, "system.metrics.prune", json!({})).await;
    let (status, body) = post_job_action(&env, &cookie, &csrf, pending, "cancel").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "cancelled");
    assert_eq!(job_status(&env, pending).await, "cancelled");

    // paused → cancelled сразу.
    let paused = insert_job(&env, "system.metrics.prune", json!({})).await;
    let (status, _) = post_job_action(&env, &cookie, &csrf, paused, "pause").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(job_status(&env, paused).await, "paused");
    let (status, body) = post_job_action(&env, &cookie, &csrf, paused, "cancel").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "cancelled");

    // running → обработчик завершается и выставляет cancelled.
    let running = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set status = 'running', started_at = now() where id = $1")
        .bind(running)
        .execute(&env.pool)
        .await
        .unwrap();
    let (status, body) = post_job_action(&env, &cookie, &csrf, running, "cancel").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["status"], "running",
        "исполнитель ещё работает"
    );

    nexus::system::jobs::execute(&env.state, running).await;
    assert_eq!(job_status(&env, running).await, "cancelled");
}

/// C5: прогресс доходит до 100 и публикуется `job.completed`.
#[tokio::test]
async fn progress_reaches_100_and_completed_event_is_published() {
    let env = TestEnv::new().await;
    let id = insert_job(&env, "system.metrics.prune", json!({})).await;
    let mut rx = env.state.realtime.subscribe();

    nexus::system::jobs::run_job(env.state.clone(), id).await;

    let (status, error, result) = job_state(&env, id).await;
    assert_eq!(status, "completed");
    assert!(error.is_none());
    assert!(result.is_some());

    let progress: f64 = sqlx::query_scalar("select progress from background_jobs where id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(progress, 100.0);

    let events = collect_events(&mut rx).await;
    let reached = events.iter().any(|event| {
        event.kind == "job.progress" && event.payload["progress"].as_f64() == Some(100.0)
    });
    assert!(reached, "job.progress доходит до 100");
    let completed = events
        .iter()
        .find(|event| event.kind == "job.completed")
        .expect("job.completed опубликован");
    assert_eq!(completed.payload["jobId"], id.to_string());
    assert_eq!(completed.payload["progress"], 100.0);
}

/// C4: `system.objects.verify` сверяет SHA-256 и пишет итог в result.
#[tokio::test]
async fn verify_objects_reports_sha_mismatch() {
    let env = TestEnv::new().await;
    let volume_id = env.add_volume("verify-sha").await;
    let root = env.root.path().join("verify-sha");

    let good = b"good!";
    let bad = b"evil!";
    assert_eq!(good.len(), bad.len());

    let good_id = insert_object(&env, volume_id, &root, good, &hash_hex(good), "good.bin").await;
    let bad_id = insert_object(&env, volume_id, &root, bad, &hash_hex(good), "bad.bin").await;

    let job_id = insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await;
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    let (status, error, result) = job_state(&env, job_id).await;
    assert_eq!(status, "completed");
    assert!(error.is_none());
    let result = result.expect("result записан");
    assert_eq!(result["mode"], "full");
    assert_eq!(result["checked"], 2);
    assert_eq!(result["verified"], 1);
    assert_eq!(result["shaMismatch"], 1);
    assert_eq!(result["sizeMismatch"], 0);
    assert_eq!(result["total"], 2);
    assert_eq!(result["mismatched"][0], json!(bad_id));
    assert!(
        result["mismatched"]
            .as_array()
            .unwrap()
            .iter()
            .all(|id| id.as_str() != Some(&good_id.to_string()))
    );

    let verified: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select verified_at from storage_objects where id = $1")
            .bind(good_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert!(verified.is_some(), "успешная сверка отмечает verified_at");

    // Выборочный режим: 100 % выборки проходит через LIMIT-параметр.
    let sample_job = insert_job(
        &env,
        "system.objects.verify",
        json!({ "mode": "sample", "samplePercent": 100 }),
    )
    .await;
    nexus::system::jobs::run_job(env.state.clone(), sample_job).await;
    let (status, _, result) = job_state(&env, sample_job).await;
    assert_eq!(status, "completed");
    let result = result.expect("result выборки записан");
    assert_eq!(result["mode"], "sample");
    assert_eq!(result["samplePercent"], 100);
    assert_eq!(result["checked"], 2);
    assert_eq!(result["shaMismatch"], 1);
}

/// C4: ошибка задачи пишется в error, дублируется в audit_events и событием.
#[tokio::test]
async fn failed_job_is_duplicated_in_audit() {
    let env = TestEnv::new().await;
    let job_id = insert_job(&env, "system.unknown", json!({})).await;
    let mut rx = env.state.realtime.subscribe();
    nexus::system::jobs::run_job(env.state.clone(), job_id).await;

    let (status, error, _) = job_state(&env, job_id).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap_or_default().contains("неизвестный тип"));

    let details: Option<Value> = sqlx::query_scalar(
        "select details from audit_events
         where event_type = 'job.failed' and target_type = 'job' and target_id = $1
         order by created_at desc limit 1",
    )
    .bind(job_id.to_string())
    .fetch_optional(&env.pool)
    .await
    .unwrap();
    let details = details.expect("job.failed в аудите");
    assert_eq!(details["jobType"], "system.unknown");

    let events = collect_events(&mut rx).await;
    assert!(has_event(&events, "job.failed"), "job.failed опубликован");
}

/* ------------------------------------------------------------------------- */
/* I4: HTTP-контракты списка, деталей и конфликтов действий над задачами.     */
/* ------------------------------------------------------------------------- */

async fn create_plain_user(env: &TestEnv, username: &str) -> Uuid {
    let hash = nexus::auth::password::hash_password("SecretPass1!".to_owned())
        .await
        .unwrap();
    sqlx::query_scalar(
        "insert into users (username, display_name, password_hash, status)
         values ($1, $1, $2, 'active') returning id",
    )
    .bind(username)
    .bind(hash)
    .fetch_one(&env.pool)
    .await
    .unwrap()
}

async fn get_jobs(env: &TestEnv, uri: &str, cookie: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = send(
        env,
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    (status, parse(&bytes))
}

#[tokio::test]
async fn list_jobs_filters_by_status_type_and_limit() {
    let env = TestEnv::new().await;
    let user = env.create_user("jobs-list", "SecretPass1!").await;
    let (cookie, _) = api_credentials(&env, user).await;

    let _pending_metrics = insert_job(&env, "system.metrics.prune", json!({})).await;
    let paused = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set status = 'paused', paused_at = now() where id = $1")
        .bind(paused)
        .execute(&env.pool)
        .await
        .unwrap();
    let verify = insert_job(&env, "system.objects.verify", json!({ "mode": "full" })).await;

    let (status, body) = get_jobs(&env, "/api/v1/jobs?status=paused", &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["items"][0]["id"], paused.to_string());
    assert_eq!(body["data"]["paused"], 1);
    assert_eq!(body["data"]["pending"], 2);

    let (status, body) = get_jobs(
        &env,
        "/api/v1/jobs?jobType=system.objects.verify&limit=1",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["items"][0]["id"], verify.to_string());

    // Некорректный limit зажимается, а не приводит к 500.
    let (status, body) = get_jobs(&env, "/api/v1/jobs?limit=500", &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["items"].as_array().unwrap().len() <= 200);
}

#[tokio::test]
async fn get_job_returns_details_and_404_for_unknown() {
    let env = TestEnv::new().await;
    let user = env.create_user("jobs-details", "SecretPass1!").await;
    let (cookie, _) = api_credentials(&env, user).await;
    let id = insert_job(
        &env,
        "system.objects.verify",
        json!({ "mode": "sample", "samplePercent": 25 }),
    )
    .await;

    let (status, body) = get_jobs(&env, &format!("/api/v1/jobs/{id}"), &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], id.to_string());
    assert_eq!(body["data"]["jobType"], "system.objects.verify");
    assert_eq!(body["data"]["payload"]["samplePercent"], 25);
    assert_eq!(body["data"]["cancellable"], true);

    let (status, body) = get_jobs(
        &env,
        "/api/v1/jobs/00000000-0000-0000-0000-000000000000",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn pause_rejects_finished_and_already_paused_jobs() {
    let env = TestEnv::new().await;
    let user = env.create_user("jobs-pause-conflict", "SecretPass1!").await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    let completed = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query(
        "update background_jobs set status = 'completed', finished_at = now() where id = $1",
    )
    .bind(completed)
    .execute(&env.pool)
    .await
    .unwrap();
    let (status, body) = post_job_action(&env, &cookie, &csrf, completed, "pause").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_FINISHED");

    let paused = insert_job(&env, "system.metrics.prune", json!({})).await;
    let (status, _) = post_job_action(&env, &cookie, &csrf, paused, "pause").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_job_action(&env, &cookie, &csrf, paused, "pause").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_ALREADY_PAUSED");

    let fixed = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set cancellable = false where id = $1")
        .bind(fixed)
        .execute(&env.pool)
        .await
        .unwrap();
    let (status, body) = post_job_action(&env, &cookie, &csrf, fixed, "pause").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_NOT_PAUSABLE");
}

#[tokio::test]
async fn resume_requires_paused_state() {
    let env = TestEnv::new().await;
    let user = env
        .create_user("jobs-resume-conflict", "SecretPass1!")
        .await;
    let (cookie, csrf) = api_credentials(&env, user).await;
    let pending = insert_job(&env, "system.metrics.prune", json!({})).await;

    let (status, body) = post_job_action(&env, &cookie, &csrf, pending, "resume").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_NOT_PAUSED");
    assert_eq!(job_status(&env, pending).await, "pending");

    let missing = Uuid::new_v4();
    let (status, body) = post_job_action(&env, &cookie, &csrf, missing, "resume").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn cancel_rejects_finished_and_non_cancellable_jobs() {
    let env = TestEnv::new().await;
    let user = env
        .create_user("jobs-cancel-conflict", "SecretPass1!")
        .await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    let failed = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set status = 'failed', error = 'x' where id = $1")
        .bind(failed)
        .execute(&env.pool)
        .await
        .unwrap();
    let (status, body) = post_job_action(&env, &cookie, &csrf, failed, "cancel").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_FINISHED");

    let fixed = insert_job(&env, "system.metrics.prune", json!({})).await;
    sqlx::query("update background_jobs set cancellable = false where id = $1")
        .bind(fixed)
        .execute(&env.pool)
        .await
        .unwrap();
    let (status, body) = post_job_action(&env, &cookie, &csrf, fixed, "cancel").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "JOB_NOT_CANCELLABLE");
    assert_eq!(job_status(&env, fixed).await, "pending");
}

#[tokio::test]
async fn maintenance_rejects_unknown_kind_and_invalid_verify_parameters() {
    let env = TestEnv::new().await;
    let user = env
        .create_user("jobs-maintenance-validation", "SecretPass1!")
        .await;
    let (cookie, csrf) = api_credentials(&env, user).await;

    for body in [
        json!({ "kind": "unknown" }),
        json!({ "kind": "verify_objects", "mode": "quick" }),
        json!({ "kind": "verify_objects", "samplePercent": 0 }),
        json!({ "kind": "verify_objects", "samplePercent": 101 }),
    ] {
        let (status, _, bytes) = send(
            &env,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/jobs/maintenance")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "тело {body}");
        assert_eq!(parse(&bytes)["error"]["code"], "VALIDATION_ERROR");
    }

    let (status, _, bytes) = send(
        &env,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/jobs/maintenance")
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "kind": "verify_objects", "mode": "sample", "samplePercent": 100 })
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = parse(&bytes);
    assert_eq!(body["data"]["jobType"], "system.objects.verify");
    let payload: Value = sqlx::query_scalar("select payload from background_jobs where id = $1")
        .bind(Uuid::parse_str(body["data"]["id"].as_str().unwrap()).unwrap())
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(payload["mode"], "sample");
    assert_eq!(payload["samplePercent"], 100);
}

#[tokio::test]
async fn job_actions_require_manage_permission_and_write_audit() {
    let env = TestEnv::new().await;
    let plain = create_plain_user(&env, "jobs-plain").await;
    let admin = env.create_user("jobs-admin", "SecretPass1!").await;
    let (plain_cookie, plain_csrf) = api_credentials(&env, plain).await;
    let (admin_cookie, admin_csrf) = api_credentials(&env, admin).await;
    let id = insert_job(&env, "system.metrics.prune", json!({})).await;

    let (status, _) = get_jobs(&env, "/api/v1/jobs", &plain_cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "нужно system.view");
    let (status, _) = post_job_action(&env, &plain_cookie, &plain_csrf, id, "pause").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "нужно system.jobs.manage");

    let (status, body) = post_job_action(&env, &admin_cookie, &admin_csrf, id, "pause").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "paused");

    let details: Value = sqlx::query_scalar(
        "select details from audit_events
         where event_type = 'jobs.pause_requested' and target_id = $1
         order by created_at desc limit 1",
    )
    .bind(id.to_string())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(details["from"], "pending");
    assert_eq!(details["immediate"], true);
}

async fn insert_object(
    env: &TestEnv,
    volume_id: Uuid,
    root: &std::path::Path,
    content: &[u8],
    sha256: &str,
    name: &str,
) -> Uuid {
    let key = build_object_key(sha256, &Uuid::new_v4());
    let location = ObjectLocation {
        volume_id,
        root: root.to_path_buf(),
        key: key.clone(),
        absolute: ObjectLocation::objects_root(root).join(&key),
    };
    std::fs::create_dir_all(location.absolute.parent().unwrap()).unwrap();
    std::fs::write(&location.absolute, content).unwrap();

    sqlx::query_scalar(
        "insert into storage_objects
           (volume_id, object_key, size_bytes, sha256, content_type, state, ref_count)
         values ($1, $2, $3, $4, 'application/octet-stream', 'committed', 0)
         returning id",
    )
    .bind(volume_id)
    .bind(&key)
    .bind(content.len() as i64)
    .bind(sha256)
    .fetch_one(&env.pool)
    .await
    .unwrap_or_else(|error| panic!("объект {name} не создан: {error}"))
}
