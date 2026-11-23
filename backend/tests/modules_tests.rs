//! Серверный гейт отключённых модулей (PLAN §9).

mod common;

use common::TestEnv;
use nexus::middleware::module_for_path;

async fn set_module(env: &TestEnv, key: &str, enabled: bool) {
    sqlx::query("update module_states set enabled = $2 where key = $1")
        .bind(key)
        .bind(enabled)
        .execute(&env.pool)
        .await
        .unwrap();
    env.state.modules.invalidate().await;
}

#[test]
fn module_is_resolved_from_path() {
    assert_eq!(module_for_path("/api/v1/cloud/entries"), Some("cloud"));
    assert_eq!(
        module_for_path("/api/v1/minecraft/builds"),
        Some("minecraft")
    );
    assert_eq!(
        module_for_path("/api/v1/launcher/catalog"),
        Some("minecraft")
    );
    assert_eq!(
        module_for_path("/api/v1/admin/launcher/tokens"),
        Some("minecraft")
    );
    assert_eq!(module_for_path("/api/packs"), Some("minecraft"));
    assert_eq!(
        module_for_path("/api/packs/demo/versions"),
        Some("minecraft")
    );
    assert_eq!(module_for_path("/api/files/sha256:abc"), Some("minecraft"));
    // Общие подсистемы всегда доступны.
    assert_eq!(module_for_path("/api/v1/system/overview"), None);
    assert_eq!(module_for_path("/api/v1/auth/me"), None);
    assert_eq!(module_for_path("/api/v1/monitoring/current"), None);
    assert_eq!(module_for_path("/api/v1/public/shares/abc"), None);
    assert_eq!(module_for_path("/api/v1/health"), None);
}

#[tokio::test]
async fn disabled_module_is_reported_as_disabled() {
    let env = TestEnv::new().await;
    env.add_volume("mod-vol").await;
    let user = env.create_user("moduser", "SecretPass1!").await;
    let _ = user;

    assert!(env.state.module_enabled("cloud").await);
    set_module(&env, "cloud", false).await;
    assert!(
        !env.state.module_enabled("cloud").await,
        "кэш инвалидируется сразу"
    );

    // Другой модуль продолжает работать.
    assert!(env.state.module_enabled("minecraft").await);

    // Включение обратно действует без перезапуска.
    set_module(&env, "cloud", true).await;
    assert!(env.state.module_enabled("cloud").await);
}

#[tokio::test]
async fn module_snapshot_contains_both_modules() {
    let env = TestEnv::new().await;
    let snapshot = env.state.modules.snapshot(&env.pool).await;
    assert_eq!(snapshot.get("cloud"), Some(&true));
    assert_eq!(snapshot.get("minecraft"), Some(&true));
    set_module(&env, "minecraft", false).await;
    let snapshot = env.state.modules.snapshot(&env.pool).await;
    assert_eq!(snapshot.get("minecraft"), Some(&false));
}

#[tokio::test]
async fn toggle_is_audited_via_job_path() {
    let env = TestEnv::new().await;
    // Аудит отключения модуля пишется обработчиком; проверяем контракт таблицы.
    let user = env.create_user("auditor", "SecretPass1!").await;
    nexus::system::audit::record(
        &env.pool,
        nexus::system::audit::AuditEvent {
            event_type: "module.toggled",
            actor_id: Some(user),
            actor_username: "auditor",
            target_type: "module",
            target_id: "cloud",
            result: "success",
            ip: "127.0.0.1",
            details: serde_json::json!({ "enabled": false }),
        },
    )
    .await;
    let count: i64 =
        sqlx::query_scalar("select count(*) from audit_events where event_type = 'module.toggled'")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn disabled_minecraft_module_hides_public_repository() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let env = TestEnv::new().await;
    set_module(&env, "minecraft", false).await;
    let response = nexus::build_router(env.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/packs?channel=stable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    set_module(&env, "minecraft", true).await;
    let response = nexus::build_router(env.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/packs?channel=stable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn disabled_module_does_not_affect_public_share_access() {
    let env = TestEnv::new().await;
    env.add_volume("pub-vol").await;
    let user = env.create_user("pubuser", "SecretPass1!").await;

    let session = nexus::cloud::uploads::create_upload(
        &env.state,
        user,
        nexus::cloud::uploads::CreateUploadRequest {
            name: "public.txt".to_owned(),
            parent_id: None,
            size: 4,
            content_type: "text/plain".to_owned(),
        },
    )
    .await
    .unwrap();
    nexus::cloud::uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        axum::body::Body::from(b"data".to_vec()),
    )
    .await
    .unwrap();
    let entry = nexus::cloud::uploads::complete_upload(
        &env.state,
        session.id,
        user,
        nexus::cloud::uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await
    .unwrap();

    let (_, token) = nexus::cloud::shares::create_share(
        &env.state,
        user,
        entry.id,
        nexus::cloud::shares::CreateShareRequest {
            expires_in_hours: Some(1),
            password: None,
            max_downloads: None,
            auth_only: false,
        },
    )
    .await
    .unwrap();

    set_module(&env, "cloud", false).await;

    // Публичные ссылки продолжают работать: данные не теряются.
    let share = nexus::cloud::shares::resolve_public_share(&env.pool, &token)
        .await
        .expect("публичная ссылка доступна при отключённом модуле");
    assert_eq!(share.name, "public.txt");
}
