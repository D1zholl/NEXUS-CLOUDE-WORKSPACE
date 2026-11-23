//! Системные настройки: реестр, валидация, применение (PLAN §9.1).

mod common;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use common::TestEnv;
use nexus::auth::session::{SESSION_COOKIE, create_session, csrf_token};
use nexus::cloud::{quota, shares, uploads};
use nexus::state::AppState;
use nexus::system::monitoring::MetricsService;
use nexus::system::{settings, settings_catalog};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn put_setting(env: &TestEnv, key: &str, value: Value) -> Result<Value, String> {
    settings::apply_system_setting(&env.state, key, value, None, "test", "127.0.0.1")
        .await
        .map_err(|error| error.message)
}

#[tokio::test]
async fn setting_limits_apply_without_restart() {
    let env = TestEnv::new().await;
    let _ = env.create_user("settings", "SecretPass1!").await;

    assert_eq!(env.state.max_upload_bytes().await, 64 * 1024 * 1024);
    put_setting(&env, "limits.max_upload_mb", json!(512))
        .await
        .unwrap();
    // B1: новое значение видно сразу после записи, без ожидания TTL.
    assert_eq!(env.state.max_upload_bytes().await, 512 * 1024 * 1024);

    put_setting(&env, "limits.chunk_max_mb", json!(8))
        .await
        .unwrap();
    assert_eq!(env.state.chunk_max_bytes().await, 8 * 1024 * 1024);

    put_setting(&env, "limits.upload_ttl_hours", json!(48))
        .await
        .unwrap();
    assert_eq!(env.state.upload_ttl_hours().await, 48);

    put_setting(&env, "cloud.upload.concurrency", json!(5))
        .await
        .unwrap();
    assert_eq!(env.state.upload_concurrency().await, 5);

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["maxUploadBytes"], 512 * 1024 * 1024);
    assert_eq!(config["chunkMaxBytes"], 8 * 1024 * 1024);
    assert_eq!(config["uploadTtlHours"], 48);
    assert_eq!(config["uploadConcurrency"], 5);
}

#[tokio::test]
async fn audit_records_old_and_new_value() {
    let env = TestEnv::new().await;
    let _ = env.create_user("settings-audit", "SecretPass1!").await;

    put_setting(&env, "limits.max_upload_mb", json!(512))
        .await
        .unwrap();
    put_setting(&env, "limits.max_upload_mb", json!(1024))
        .await
        .unwrap();

    let details: Value = sqlx::query_scalar(
        "select details from audit_events
         where event_type = 'settings.system.updated'
         order by created_at desc, id desc limit 1",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();

    assert_eq!(details["key"], "limits.max_upload_mb");
    assert_eq!(details["old"], 512);
    assert_eq!(details["new"], 1024);
    // Секретов в diff нет: только ключ и значения настройки.
    assert!(details.get("password").is_none());
}

#[tokio::test]
async fn each_kind_is_validated() {
    // Int границы.
    assert!(settings_catalog::validate("limits.max_upload_mb", json!(0)).is_err());
    assert!(settings_catalog::validate("limits.max_upload_mb", json!(999_999)).is_err());
    // Bool.
    assert!(settings_catalog::validate("network.trust_proxy", json!(true)).is_ok());
    assert!(settings_catalog::validate("network.trust_proxy", json!(1)).is_err());
    // Enum.
    assert!(settings_catalog::validate("appearance.background.quality", json!("eco")).is_ok());
    assert!(settings_catalog::validate("appearance.background.quality", json!("ultra")).is_err());
    // Json.
    assert!(settings_catalog::validate("network.allowed_origins", json!([])).is_ok());
    assert!(settings_catalog::validate("network.allowed_origins", json!("x")).is_err());
}

#[test]
fn unknown_key_lists_allowed() {
    let error = settings_catalog::validate("unknown.key", json!(1)).unwrap_err();
    assert!(error.contains("Неизвестный ключ"));
    assert!(error.contains("limits.max_upload_mb"));
}

#[test]
fn requires_restart_is_marked() {
    let catalog = settings_catalog::catalog_json();
    let entries = catalog.as_array().unwrap();
    let origins = entries
        .iter()
        .find(|entry| entry["key"] == "network.allowed_origins")
        .expect("ключ в каталоге");
    assert_eq!(origins["requiresRestart"], true);

    let limit = entries
        .iter()
        .find(|entry| entry["key"] == "limits.max_upload_mb")
        .expect("ключ в каталоге");
    assert_eq!(limit["requiresRestart"], false);
    assert_eq!(limit["kind"], "int");
}

#[tokio::test]
async fn quota_setting_updates_default_and_rejects_upload_without_restart() {
    let env = TestEnv::new().await;
    let user = env.create_user("settings2", "SecretPass1!").await;

    put_setting(&env, "limits.quota_cloud_gb", json!(200))
        .await
        .unwrap();
    let expected = 200_i64 * 1024 * 1024 * 1024;
    let after = quota::default_quota(&env.pool, quota::Scope::Cloud)
        .await
        .unwrap();
    assert_eq!(after, expected);

    // Персональная квота приоритетнее значения по умолчанию.
    quota::set_user_quota(&env.pool, user, Some(1024))
        .await
        .unwrap();
    let effective = quota::effective_quota(&env.pool, user, quota::Scope::Cloud)
        .await
        .unwrap();
    assert_eq!(effective, 1024);
    quota::set_user_quota(&env.pool, user, None).await.unwrap();

    // Заполняем хранилище до новой квоты и пробуем загрузить ещё один байт.
    sqlx::query(
        "insert into storage_usage (user_id, used_bytes) values ($1, $2)
         on conflict (user_id) do update set used_bytes = excluded.used_bytes",
    )
    .bind(user)
    .bind(expected)
    .execute(&env.pool)
    .await
    .unwrap();

    let error = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "over-quota.bin".to_owned(),
            parent_id: None,
            size: 1,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect_err("загрузка сверх квоты");

    assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error.code, "QUOTA_EXCEEDED");
    assert_eq!(error.details().cloned().unwrap()["limitBytes"], expected);
}

#[tokio::test]
async fn chunk_setting_applies_to_upload_handler() {
    let env = TestEnv::new().await;
    env.add_volume("settings-chunk").await;
    let user = env.create_user("settings3", "SecretPass1!").await;

    put_setting(&env, "limits.chunk_max_mb", json!(2))
        .await
        .unwrap();

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["chunkMaxBytes"], 2 * 1024 * 1024);

    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "chunk.bin".to_owned(),
            parent_id: None,
            size: 3 * 1024 * 1024,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    // 1,5 МиБ проходят только потому, что обработчик читает настройку (2 МиБ),
    // а не тестовую конфигурацию (1 МиБ).
    let offset = uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(vec![1u8; 1536 * 1024]),
    )
    .await
    .unwrap();
    assert_eq!(offset, 1536 * 1024);
}

#[tokio::test]
async fn share_ttl_setting_is_default_for_new_links() {
    let env = TestEnv::new().await;
    let user = env.create_user("settings-share", "SecretPass1!").await;
    let entry_id: Uuid = sqlx::query_scalar(
        "insert into cloud_entries (owner_id, name, entry_type, size_bytes)
         values ($1, 'shared.txt', 'file', 5) returning id",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();

    put_setting(&env, "limits.share_ttl_hours", json!(5))
        .await
        .unwrap();
    let (row, _) = shares::create_share(
        &env.state,
        user,
        entry_id,
        shares::CreateShareRequest {
            expires_in_hours: None,
            password: None,
            max_downloads: None,
            auth_only: false,
        },
    )
    .await
    .unwrap();

    let expires_at = row.expires_at.expect("срок задан настройкой");
    let hours = (expires_at - Utc::now()).num_minutes() as f64 / 60.0;
    assert!(hours > 4.9 && hours <= 5.0, "получено {hours} ч");
}

#[tokio::test]
async fn remaining_settings_are_readable_without_restart() {
    let env = TestEnv::new().await;
    let _ = env.create_user("settings-other", "SecretPass1!").await;

    put_setting(&env, "limits.export_ttl_hours", json!(72))
        .await
        .unwrap();
    assert_eq!(env.state.export_ttl_hours().await, 72);

    put_setting(&env, "security.session_ttl_hours", json!(1))
        .await
        .unwrap();
    assert_eq!(env.state.session_ttl_hours().await, 1);

    put_setting(&env, "security.login_max_failures", json!(3))
        .await
        .unwrap();
    assert_eq!(env.state.login_max_failures().await, 3);

    put_setting(&env, "security.password_min_length", json!(8))
        .await
        .unwrap();
    assert_eq!(env.state.password_min_length().await, 8);
    assert!(
        nexus::auth::password::validate_password("Secret1!", 8).is_ok(),
        "8 символов достаточно при настройке 8"
    );
    assert!(
        nexus::auth::password::validate_password("Secret1!", 12).is_err(),
        "при пороге 12 тот же пароль отклоняется"
    );

    // B2: лимит фоновых задач только читается; исполнение — фаза C.
    put_setting(&env, "jobs.max_concurrent", json!(5))
        .await
        .unwrap();
    assert_eq!(env.state.jobs_max_concurrent().await, 5);
}

#[tokio::test]
async fn appearance_defaults_merge_and_user_fallback_resets() {
    let env = TestEnv::new().await;
    let user = env.create_user("settings-appearance", "SecretPass1!").await;

    let initial = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(initial["count"], 260, "встроенные значения на месте");

    // Пользователь сохранил ровно системный fallback.
    sqlx::query("insert into user_settings (user_id, key, value) values ($1, $2, $3)")
        .bind(user)
        .bind(settings::BACKGROUND_KEY)
        .bind(&initial)
        .execute(&env.pool)
        .await
        .unwrap();

    put_setting(&env, "appearance.background.quality", json!("eco"))
        .await
        .unwrap();
    put_setting(
        &env,
        "appearance.background.defaults",
        json!({ "intensity": 0.4 }),
    )
    .await
    .unwrap();

    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["quality"], "eco");
    assert_eq!(effective["intensity"], 0.4);
    assert_eq!(effective["count"], 260, "недостающие поля дополнены");

    // Копия старого fallback сброшена — пользователь получит новый дефолт.
    let copies: i64 = sqlx::query_scalar("select count(*) from user_settings where user_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(copies, 0);

    // Индивидуальная настройка, отличающаяся от дефолта, не удаляется.
    let mut custom = effective.clone();
    custom["count"] = json!(500);
    sqlx::query("insert into user_settings (user_id, key, value) values ($1, $2, $3)")
        .bind(user)
        .bind(settings::BACKGROUND_KEY)
        .bind(&custom)
        .execute(&env.pool)
        .await
        .unwrap();

    put_setting(
        &env,
        "appearance.background.defaults",
        json!({ "intensity": 0.8 }),
    )
    .await
    .unwrap();

    let saved: Value = sqlx::query_scalar("select value from user_settings where user_id = $1")
        .bind(user)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(saved["count"], 500);
}

/* ------------------------------------------------------------------------- */
/* B8: по тесту на каждый из 16 ключей реестра.                              */
/* ------------------------------------------------------------------------- */

#[tokio::test]
async fn key_limits_max_upload_mb_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.max_upload_bytes().await, 64 * 1024 * 1024);

    put_setting(&env, "limits.max_upload_mb", json!(512))
        .await
        .unwrap();
    assert_eq!(env.state.max_upload_bytes().await, 512 * 1024 * 1024);

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["maxUploadBytes"], 512 * 1024 * 1024);
}

#[tokio::test]
async fn key_limits_chunk_max_mb_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.chunk_max_bytes().await, 1024 * 1024);

    put_setting(&env, "limits.chunk_max_mb", json!(8))
        .await
        .unwrap();
    assert_eq!(env.state.chunk_max_bytes().await, 8 * 1024 * 1024);

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["chunkMaxBytes"], 8 * 1024 * 1024);
}

#[tokio::test]
async fn key_limits_upload_ttl_hours_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.upload_ttl_hours().await, 24);

    put_setting(&env, "limits.upload_ttl_hours", json!(48))
        .await
        .unwrap();
    assert_eq!(env.state.upload_ttl_hours().await, 48);

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["uploadTtlHours"], 48);
}

#[tokio::test]
async fn key_limits_share_ttl_hours_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.share_ttl_hours().await, 24);

    put_setting(&env, "limits.share_ttl_hours", json!(5))
        .await
        .unwrap();
    assert_eq!(env.state.share_ttl_hours().await, 5);

    let user = env.create_user("settings-key-share", "SecretPass1!").await;
    let entry_id: Uuid = sqlx::query_scalar(
        "insert into cloud_entries (owner_id, name, entry_type, size_bytes)
         values ($1, 'shared.txt', 'file', 5) returning id",
    )
    .bind(user)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    let (row, _) = shares::create_share(
        &env.state,
        user,
        entry_id,
        shares::CreateShareRequest {
            expires_in_hours: None,
            password: None,
            max_downloads: None,
            auth_only: false,
        },
    )
    .await
    .unwrap();
    let hours = (row.expires_at.expect("срок задан") - Utc::now()).num_minutes() as f64 / 60.0;
    assert!(hours > 4.9 && hours <= 5.0, "получено {hours} ч");
}

#[tokio::test]
async fn key_limits_export_ttl_hours_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.export_ttl_hours().await, 1);

    put_setting(&env, "limits.export_ttl_hours", json!(72))
        .await
        .unwrap();
    assert_eq!(env.state.export_ttl_hours().await, 72);
}

#[tokio::test]
async fn key_limits_trash_retention_days_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.trash_retention_days().await, 30);

    put_setting(&env, "limits.trash_retention_days", json!(90))
        .await
        .unwrap();
    assert_eq!(env.state.trash_retention_days().await, 90);

    assert!(
        put_setting(&env, "limits.trash_retention_days", json!(0))
            .await
            .is_err()
    );
    assert!(
        put_setting(&env, "limits.trash_retention_days", json!(3651))
            .await
            .is_err()
    );

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["trashRetentionDays"], 90);
}

#[tokio::test]
async fn key_limits_quota_cloud_gb_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(
        quota::default_quota(&env.pool, quota::Scope::Cloud)
            .await
            .unwrap(),
        50 * 1024 * 1024 * 1024
    );

    put_setting(&env, "limits.quota_cloud_gb", json!(200))
        .await
        .unwrap();
    assert_eq!(
        quota::default_quota(&env.pool, quota::Scope::Cloud)
            .await
            .unwrap(),
        200_i64 * 1024 * 1024 * 1024
    );
}

#[tokio::test]
async fn key_limits_quota_minecraft_gb_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(
        quota::default_quota(&env.pool, quota::Scope::Minecraft)
            .await
            .unwrap(),
        100 * 1024 * 1024 * 1024
    );

    put_setting(&env, "limits.quota_minecraft_gb", json!(300))
        .await
        .unwrap();
    assert_eq!(
        quota::default_quota(&env.pool, quota::Scope::Minecraft)
            .await
            .unwrap(),
        300_i64 * 1024 * 1024 * 1024
    );
    // Облачная квота не затронута.
    assert_eq!(
        quota::default_quota(&env.pool, quota::Scope::Cloud)
            .await
            .unwrap(),
        50 * 1024 * 1024 * 1024
    );
}

#[tokio::test]
async fn key_jobs_max_concurrent_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.jobs_max_concurrent().await, 2);

    put_setting(&env, "jobs.max_concurrent", json!(5))
        .await
        .unwrap();
    assert_eq!(env.state.jobs_max_concurrent().await, 5);
}

#[tokio::test]
async fn key_cloud_upload_concurrency_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.upload_concurrency().await, 3);

    put_setting(&env, "cloud.upload.concurrency", json!(6))
        .await
        .unwrap();
    assert_eq!(env.state.upload_concurrency().await, 6);

    let config = uploads::session_config(&env.state).await;
    assert_eq!(config["uploadConcurrency"], 6);
}

#[tokio::test]
async fn key_security_session_ttl_hours_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.session_ttl_hours().await, 24);

    put_setting(&env, "security.session_ttl_hours", json!(1))
        .await
        .unwrap();
    assert_eq!(env.state.session_ttl_hours().await, 1);
}

#[tokio::test]
async fn key_security_login_max_failures_applies_without_restart() {
    let env = TestEnv::new().await;

    put_setting(&env, "security.login_max_failures", json!(3))
        .await
        .unwrap();
    assert_eq!(env.state.login_max_failures().await, 3);

    put_setting(&env, "security.login_max_failures", json!(7))
        .await
        .unwrap();
    assert_eq!(env.state.login_max_failures().await, 7);
}

#[tokio::test]
async fn key_security_password_min_length_applies_without_restart() {
    let env = TestEnv::new().await;
    assert_eq!(
        env.state.password_min_length().await,
        nexus::auth::password::MIN_PASSWORD_LEN
    );

    put_setting(&env, "security.password_min_length", json!(8))
        .await
        .unwrap();
    assert_eq!(env.state.password_min_length().await, 8);
    assert!(nexus::auth::password::validate_password("Secret1!", 8).is_ok());
    assert!(nexus::auth::password::validate_password("Short1!", 8).is_err());
}

#[tokio::test]
async fn key_network_allowed_origins_applies_only_after_restart() {
    let env = TestEnv::new().await;
    let mut config = env.state.config.clone();
    config.security.allowed_origins = vec!["https://start.example".to_owned()];

    put_setting(
        &env,
        "network.allowed_origins",
        json!(["https://later.example", " https://second.example "]),
    )
    .await
    .unwrap();

    // До перезапуска действует конфигурация, собранная при старте.
    assert_eq!(
        config.security.allowed_origins,
        vec!["https://start.example".to_owned()]
    );
    assert_eq!(
        env.state.config.security.allowed_origins,
        Vec::<String>::new()
    );

    settings::apply_startup_overrides(&env.pool, &mut config).await;
    assert_eq!(
        config.security.allowed_origins,
        vec![
            "https://later.example".to_owned(),
            "https://second.example".to_owned()
        ]
    );
}

#[tokio::test]
async fn key_network_trust_proxy_applies_only_after_restart() {
    let env = TestEnv::new().await;
    let mut config = env.state.config.clone();
    assert!(!config.server.trust_proxy);

    put_setting(&env, "network.trust_proxy", json!(true))
        .await
        .unwrap();

    // До перезапуска клиентский IP по-прежнему берётся из соединения.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.9".parse().expect("заголовок"));
    let peer: Option<std::net::IpAddr> = Some("10.0.0.1".parse().expect("адрес"));
    assert_eq!(
        nexus::middleware::client_ip(&headers, peer, config.server.trust_proxy),
        "10.0.0.1"
    );

    settings::apply_startup_overrides(&env.pool, &mut config).await;
    assert!(config.server.trust_proxy);
    assert_eq!(
        nexus::middleware::client_ip(&headers, peer, config.server.trust_proxy),
        "203.0.113.9"
    );
}

#[tokio::test]
async fn key_appearance_background_quality_applies_without_restart() {
    let env = TestEnv::new().await;
    let initial = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(initial["quality"], "auto");

    put_setting(&env, "appearance.background.quality", json!("eco"))
        .await
        .unwrap();
    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["quality"], "eco");
}

#[tokio::test]
async fn key_appearance_background_defaults_applies_without_restart() {
    let env = TestEnv::new().await;

    put_setting(
        &env,
        "appearance.background.defaults",
        json!({ "intensity": 0.4, "stars": 900 }),
    )
    .await
    .unwrap();
    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["intensity"], 0.4);
    assert_eq!(effective["stars"], 900);
    // Недостающие поля дополнены встроенными значениями.
    assert_eq!(effective["count"], 260);
}

/* ------------------------------------------------------------------------- */
/* B8: границы, реестр, кеш, эффективные значения и CORS после перезапуска.  */
/* ------------------------------------------------------------------------- */

#[test]
fn int_boundaries_are_enforced_for_every_key() {
    for spec in settings_catalog::SETTINGS {
        if !matches!(spec.kind, settings_catalog::Kind::Int) {
            continue;
        }
        let min = spec.min.expect("min объявлен у int");
        let max = spec.max.expect("max объявлен у int");
        assert!(
            settings_catalog::validate(spec.key, json!(min)).is_ok(),
            "{}: нижняя граница {min} должна приниматься",
            spec.key
        );
        assert!(
            settings_catalog::validate(spec.key, json!(max)).is_ok(),
            "{}: верхняя граница {max} должна приниматься",
            spec.key
        );
        assert!(
            settings_catalog::validate(spec.key, json!(min - 1)).is_err(),
            "{}: значение ниже {min} должно отклоняться",
            spec.key
        );
        assert!(
            settings_catalog::validate(spec.key, json!(max + 1)).is_err(),
            "{}: значение выше {max} должно отклоняться",
            spec.key
        );
    }
}

#[test]
fn restart_only_keys_are_exactly_the_two_network_flags() {
    let catalog = settings_catalog::catalog_json();
    let entries = catalog.as_array().expect("каталог — массив");
    let restart: Vec<&str> = entries
        .iter()
        .filter(|entry| entry["requiresRestart"] == true)
        .map(|entry| entry["key"].as_str().expect("ключ"))
        .collect();
    assert_eq!(
        restart,
        vec!["network.allowed_origins", "network.trust_proxy"]
    );
}

#[tokio::test]
async fn cache_is_invalidated_immediately_after_write() {
    let env = TestEnv::new().await;
    // Прогреваем кэш старым значением: без инвалидации он жил бы до 10 секунд.
    assert_eq!(env.state.max_upload_bytes().await, 64 * 1024 * 1024);

    put_setting(&env, "limits.max_upload_mb", json!(128))
        .await
        .unwrap();
    assert_eq!(env.state.max_upload_bytes().await, 128 * 1024 * 1024);
}

#[tokio::test]
async fn effective_settings_cover_every_catalog_key() {
    let env = TestEnv::new().await;
    let effective = settings::effective_settings(&env.state).await.unwrap();
    let map = effective.as_object().expect("объект");

    for spec in settings_catalog::SETTINGS {
        assert!(
            map.contains_key(spec.key),
            "ключ {} отсутствует в effective",
            spec.key
        );
    }
    assert_eq!(map.len(), settings_catalog::SETTINGS.len());
    // Независимые значения по умолчанию, а не минимумы реестра.
    assert_eq!(map["limits.max_upload_mb"], 64);
    assert_eq!(map["limits.trash_retention_days"], 30);
    assert_eq!(map["jobs.max_concurrent"], 2);
    assert_eq!(map["cloud.upload.concurrency"], 3);
    assert_eq!(map["security.session_ttl_hours"], 24);
    assert_eq!(map["network.trust_proxy"], false);
    assert_eq!(map["network.allowed_origins"], json!([]));
}

async fn cors_origin(app: &Router, origin: &str) -> Option<String> {
    let request = Request::builder()
        .uri("/api/v1/health")
        .header(header::ORIGIN, origin)
        .body(Body::empty())
        .expect("запрос собирается");
    let response = app.clone().oneshot(request).await.expect("роутер отвечает");
    response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

/// B5/приёмка: до перезапуска CORS отдаёт старый origin, после пересборки
/// конфигурации из `system_settings` — новый.
#[tokio::test]
async fn cors_uses_saved_origins_only_after_config_rebuild() {
    let env = TestEnv::new().await;

    let mut applied = env.state.config.clone();
    applied.security.allowed_origins = vec!["https://old.example".to_owned()];
    let old_app = nexus::build_router(AppState::new(
        env.pool.clone(),
        applied.clone(),
        MetricsService::new(),
    ));

    assert_eq!(
        cors_origin(&old_app, "https://old.example")
            .await
            .as_deref(),
        Some("https://old.example")
    );
    assert_eq!(cors_origin(&old_app, "https://new.example").await, None);

    // Сохраняем новый origin: работающий сервер продолжает отдавать старый.
    put_setting(
        &env,
        "network.allowed_origins",
        json!(["https://new.example"]),
    )
    .await
    .unwrap();

    assert_eq!(cors_origin(&old_app, "https://new.example").await, None);
    assert_eq!(
        cors_origin(&old_app, "https://old.example")
            .await
            .as_deref(),
        Some("https://old.example")
    );

    // Пересборка конфигурации (аналог перезапуска службы) применяет новый список.
    settings::apply_startup_overrides(&env.pool, &mut applied).await;
    assert_eq!(
        applied.security.allowed_origins,
        vec!["https://new.example".to_owned()]
    );

    let new_app = nexus::build_router(AppState::new(
        env.pool.clone(),
        applied,
        MetricsService::new(),
    ));
    assert_eq!(
        cors_origin(&new_app, "https://new.example")
            .await
            .as_deref(),
        Some("https://new.example")
    );
    assert_eq!(cors_origin(&new_app, "https://old.example").await, None);
}

/* ------------------------------------------------------------------------- */
/* I4: дополнительные контракты HTTP, пользовательские настройки и границы.   */
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

async fn credentials(env: &TestEnv, user: Uuid) -> (String, String) {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "test",
    )
    .await
    .unwrap();
    let cookie = format!("{SESSION_COOKIE}={token}");
    let csrf = csrf_token(&env.state.config.security.secret, &token);
    (cookie, csrf)
}

async fn http(
    env: &TestEnv,
    method: axum::http::Method,
    uri: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = builder
        .body(Body::from(body.unwrap_or_default().to_owned()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Заголовки ответа нужны для проверки кеширования публичного фона.
async fn response_headers(
    env: &TestEnv,
    method: axum::http::Method,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))));
    let response = nexus::build_router(env.state.clone())
        .oneshot(request)
        .await
        .expect("роутер отвечает");
    (response.status(), response.headers().clone())
}

#[tokio::test]
async fn unknown_setting_returns_unknown_setting_code() {
    let env = TestEnv::new().await;
    let admin = env.create_user("settings-unknown", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, admin).await;

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/does.not.exist",
        Some(&cookie),
        Some(&csrf),
        Some("{\"value\":1}"),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "UNKNOWN_SETTING");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("limits.max_upload_mb")
    );
}

#[tokio::test]
async fn malformed_json_body_is_rejected_with_422() {
    let env = TestEnv::new().await;
    let admin = env.create_user("settings-malformed", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, admin).await;

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/limits.max_upload_mb",
        Some(&cookie),
        Some(&csrf),
        Some("{not-json"),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
    let saved: i64 = sqlx::query_scalar("select count(*) from system_settings")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(saved, 0, "некорректный запрос ничего не сохраняет");
}

#[tokio::test]
async fn settings_routes_require_manage_and_view_permissions() {
    let env = TestEnv::new().await;
    let plain = create_plain_user(&env, "settings-plain").await;
    let admin = env.create_user("settings-admin", "SecretPass1!").await;
    let (plain_cookie, plain_csrf) = credentials(&env, plain).await;
    let (admin_cookie, admin_csrf) = credentials(&env, admin).await;

    for uri in ["/api/v1/system/settings", "/api/v1/system/settings/catalog"] {
        let (status, _) = http(
            &env,
            axum::http::Method::GET,
            uri,
            Some(&plain_cookie),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "GET {uri} без прав");
    }
    let (status, _) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/limits.max_upload_mb",
        Some(&plain_cookie),
        Some(&plain_csrf),
        Some("512"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Системный фон по умолчанию — тоже только для администратора.
    let (status, _) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/appearance.background.defaults",
        Some(&plain_cookie),
        Some(&plain_csrf),
        Some(r#"{"palette":"magma"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/system/settings/catalog",
        Some(&admin_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        settings_catalog::SETTINGS.len()
    );

    let (status, _) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/limits.max_upload_mb",
        Some(&admin_cookie),
        Some(&admin_csrf),
        Some("512"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn system_settings_http_roundtrip_exposes_value_meta_and_effective() {
    let env = TestEnv::new().await;
    let admin = env.create_user("settings-roundtrip", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, admin).await;

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/system/settings/limits.upload_ttl_hours",
        Some(&cookie),
        Some(&csrf),
        Some("48"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"], 48);

    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/system/settings",
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(data["values"]["limits.upload_ttl_hours"], 48);
    assert_eq!(data["effective"]["limits.upload_ttl_hours"], 48);
    assert_eq!(data["meta"]["limits.upload_ttl_hours"]["kind"], "int");
    assert_eq!(
        data["meta"]["limits.upload_ttl_hours"]["requiresRestart"],
        false
    );
    assert!(data["catalog"].as_array().unwrap().len() >= settings_catalog::SETTINGS.len());
}

#[tokio::test]
async fn user_background_settings_are_sanitized_and_other_keys_rejected() {
    let env = TestEnv::new().await;
    let user = create_plain_user(&env, "settings-me").await;
    let (cookie, csrf) = credentials(&env, user).await;

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/appearance.background",
        Some(&cookie),
        Some(&csrf),
        Some(
            r#"{"count":9999,"sizeMin":5,"sizeMax":1,"intensity":9,
                "quality":"ultra","fpsLimit":45,"preset":"nope","palette":"nope"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let saved = &body["data"];
    assert_eq!(saved["count"], 700);
    assert_eq!(saved["sizeMin"], 1.0);
    assert_eq!(saved["sizeMax"], 2.0);
    assert_eq!(saved["intensity"], 2.0);
    assert_eq!(saved["quality"], "auto");
    assert_eq!(saved["fpsLimit"], 60);
    assert_eq!(saved["preset"], "flow");
    assert_eq!(saved["palette"], "neon");

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/limits.max_upload_mb",
        Some(&cookie),
        Some(&csrf),
        Some("1"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn user_interface_appearance_is_sanitized_and_returned_as_fallback() {
    let env = TestEnv::new().await;
    let user = create_plain_user(&env, "settings-interface").await;
    let (cookie, csrf) = credentials(&env, user).await;

    // Неизвестный акцент нормализуется к безопасному значению по умолчанию.
    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/appearance.interface",
        Some(&cookie),
        Some(&csrf),
        Some(r#"{"accent":"ultraviolet"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["accent"], "cyan");

    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/appearance.interface",
        Some(&cookie),
        Some(&csrf),
        Some(r#"{"accent":"magenta"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["accent"], "magenta");

    // GET /me/settings отдаёт обе настройки внешнего вида.
    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/me/settings",
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["appearance.interface"]["accent"], "magenta");
    assert!(body["data"]["appearance.background"].is_object());

    // Фон по-прежнему нельзя подменить произвольным ключом.
    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/appearance.interface.extra",
        Some(&cookie),
        Some(&csrf),
        Some("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn fresh_user_receives_system_background_defaults() {
    let env = TestEnv::new().await;
    let user = create_plain_user(&env, "settings-background-fresh").await;
    let (cookie, csrf) = credentials(&env, user).await;

    // Администратор задаёт системный фон: новые пользователи должны получать
    // именно его, а не заводские значения.
    put_setting(
        &env,
        "appearance.background.defaults",
        json!({
            "count": 123,
            "stars": 456,
            "palette": "ice",
            "speed": 0.55,
            "preset": "custom",
            "enabled": true,
        }),
    )
    .await
    .unwrap();
    put_setting(&env, "appearance.background.quality", json!("eco"))
        .await
        .unwrap();

    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/me/settings",
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let background = &body["data"]["appearance.background"];
    assert_eq!(background["count"], 123);
    assert_eq!(background["stars"], 456);
    assert_eq!(background["palette"], "ice");
    assert_eq!(background["speed"], 0.55);
    assert_eq!(background["preset"], "custom");
    assert_eq!(background["quality"], "eco");

    // Собственная настройка пользователя имеет приоритет над системной.
    let (status, body) = http(
        &env,
        axum::http::Method::PUT,
        "/api/v1/me/settings/appearance.background",
        Some(&cookie),
        Some(&csrf),
        Some(r#"{"count":321,"palette":"magma","preset":"magma"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    put_setting(
        &env,
        "appearance.background.defaults",
        json!({"count": 777}),
    )
    .await
    .unwrap();

    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/me/settings",
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["appearance.background"]["count"], 321);
}

#[tokio::test]
async fn public_background_is_available_without_session() {
    let env = TestEnv::new().await;

    put_setting(
        &env,
        "appearance.background.defaults",
        json!({
            "count": 143,
            "stars": 777,
            "palette": "ice",
            "speed": 0.4,
            "preset": "custom",
            "enabled": true,
        }),
    )
    .await
    .unwrap();
    put_setting(&env, "appearance.background.quality", json!("eco"))
        .await
        .unwrap();

    // Экран входа открывается с чистыми cookie: сессии и CSRF-токена нет.
    let (status, body) = http(
        &env,
        axum::http::Method::GET,
        "/api/v1/public/appearance/background",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let background = &body["data"];
    assert_eq!(background["count"], 143);
    assert_eq!(background["stars"], 777);
    assert_eq!(background["palette"], "ice");
    assert_eq!(background["speed"], 0.4);
    assert_eq!(background["preset"], "custom");
    assert_eq!(background["quality"], "eco");
    // Публичный маршрут отдаёт только фон, не остальные настройки.
    assert!(body["data"].get("appearance.interface").is_none());
    assert!(body["data"].get("limits.max_upload_mb").is_none());

    // Новый системный default не должен задерживаться в кешах: иначе форма
    // входа с чистыми cookie покажет предыдущий фон.
    let (status, headers) = response_headers(
        &env,
        axum::http::Method::GET,
        "/api/v1/public/appearance/background",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn background_defaults_reject_scalar_json() {
    let env = TestEnv::new().await;
    let admin = env.create_user("settings-defaults", "SecretPass1!").await;
    let (cookie, csrf) = credentials(&env, admin).await;
    let uri = "/api/v1/system/settings/appearance.background.defaults";

    for body in ["\"not-an-object\"", "42", "true"] {
        let (status, payload) = http(
            &env,
            axum::http::Method::PUT,
            uri,
            Some(&cookie),
            Some(&csrf),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "тело {body}");
        assert_eq!(payload["error"]["code"], "VALIDATION_ERROR");
    }

    let (status, _) = http(
        &env,
        axum::http::Method::PUT,
        uri,
        Some(&cookie),
        Some(&csrf),
        Some(r#"{"stars":900}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn quality_key_wins_over_snapshot_quality() {
    let env = TestEnv::new().await;

    put_setting(
        &env,
        "appearance.background.defaults",
        json!({ "quality": "eco", "stars": 400 }),
    )
    .await
    .unwrap();
    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["quality"], "eco");
    assert_eq!(effective["stars"], 400);

    // Отдельный ключ качества применяется последним.
    put_setting(&env, "appearance.background.quality", json!("high"))
        .await
        .unwrap();
    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["quality"], "high");

    // Повторная запись снимка с другим quality не перекрывает отдельный ключ.
    put_setting(
        &env,
        "appearance.background.defaults",
        json!({ "quality": "eco" }),
    )
    .await
    .unwrap();
    let effective = settings::effective_background_defaults(&env.state)
        .await
        .unwrap();
    assert_eq!(effective["quality"], "high");
}

#[tokio::test]
async fn enum_values_are_trimmed_and_case_sensitive() {
    assert_eq!(
        settings_catalog::validate("appearance.background.quality", json!(" eco ")).unwrap(),
        json!("eco")
    );
    assert!(settings_catalog::validate("appearance.background.quality", json!("ECO")).is_err());
    assert!(settings_catalog::validate("appearance.background.quality", json!(" high\n")).is_ok());
    assert!(settings_catalog::validate("appearance.background.quality", json!(1)).is_err());
}

#[tokio::test]
async fn saved_setting_updates_actor_and_timestamp() {
    let env = TestEnv::new().await;
    let admin = env.create_user("settings-actor", "SecretPass1!").await;

    settings::apply_system_setting(
        &env.state,
        "limits.max_upload_mb",
        json!(512),
        Some(admin),
        "settings-actor",
        "203.0.113.7",
    )
    .await
    .unwrap();

    let (updated_by, updated_at): (Option<Uuid>, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("select updated_by, updated_at from system_settings where key = $1")
            .bind("limits.max_upload_mb")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(updated_by, Some(admin));
    assert!(updated_at.is_some());

    let (actor_username, ip): (String, String) = sqlx::query_as(
        "select actor_username, ip from audit_events
         where event_type = 'settings.system.updated'
         order by created_at desc, id desc limit 1",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(actor_username, "settings-actor");
    assert_eq!(ip, "203.0.113.7");
}

#[tokio::test]
async fn writing_same_value_twice_keeps_one_row_and_audits_both() {
    let env = TestEnv::new().await;

    put_setting(&env, "limits.max_upload_mb", json!(512))
        .await
        .unwrap();
    put_setting(&env, "limits.max_upload_mb", json!(512))
        .await
        .unwrap();

    let rows: i64 = sqlx::query_scalar("select count(*) from system_settings where key = $1")
        .bind("limits.max_upload_mb")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "upsert не создаёт дубликат ключа");

    let events: Vec<Value> = sqlx::query_scalar(
        "select details from audit_events
         where event_type = 'settings.system.updated' order by created_at, id",
    )
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["old"], 512);
    assert_eq!(events[1]["new"], 512);
}

#[tokio::test]
async fn effective_network_settings_stay_on_startup_values_until_restart() {
    let env = TestEnv::new().await;
    let _ = env.create_user("settings-effective", "SecretPass1!").await;

    put_setting(&env, "network.trust_proxy", json!(true))
        .await
        .unwrap();
    put_setting(
        &env,
        "network.allowed_origins",
        json!(["https://saved.example"]),
    )
    .await
    .unwrap();
    put_setting(&env, "limits.max_upload_mb", json!(128))
        .await
        .unwrap();

    let effective = settings::effective_settings(&env.state).await.unwrap();
    assert_eq!(effective["network.trust_proxy"], false);
    assert_eq!(effective["network.allowed_origins"], json!([]));
    assert_eq!(effective["limits.max_upload_mb"], 128);
}

#[tokio::test]
async fn cache_entries_are_independent_per_key() {
    let env = TestEnv::new().await;
    assert_eq!(env.state.max_upload_bytes().await, 64 * 1024 * 1024);
    assert_eq!(env.state.chunk_max_bytes().await, 1024 * 1024);

    put_setting(&env, "limits.max_upload_mb", json!(256))
        .await
        .unwrap();
    put_setting(&env, "limits.chunk_max_mb", json!(8))
        .await
        .unwrap();
    put_setting(&env, "limits.max_upload_mb", json!(128))
        .await
        .unwrap();

    assert_eq!(env.state.max_upload_bytes().await, 128 * 1024 * 1024);
    assert_eq!(
        env.state.chunk_max_bytes().await,
        8 * 1024 * 1024,
        "изменение другого ключа не сбрасывает чанк"
    );
}

#[test]
fn fractional_int_is_truncated_and_string_number_rejected() {
    assert_eq!(
        settings_catalog::validate("limits.chunk_max_mb", json!(2.9)).unwrap(),
        json!(2)
    );
    assert!(settings_catalog::validate("limits.chunk_max_mb", json!("5")).is_err());
    assert!(settings_catalog::validate("limits.chunk_max_mb", json!(true)).is_err());
    assert!(settings_catalog::validate("limits.chunk_max_mb", json!(-1)).is_err());
}
