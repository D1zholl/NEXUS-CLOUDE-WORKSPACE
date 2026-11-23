use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use nexus_core::settings::keys;
use nexus_core::{
    AccountId, Clock, DataPaths, EffectiveSettings, FixedClock, InstanceId, SettingsGroup,
    SettingsPatch, SourceId, offline_player_uuid,
};
use nexus_platform::SettingsService;
use nexus_platform::db::{Db, DbError, DbOptions, compiled_max_version};
use nexus_platform::repositories::{accounts, activity, downloads, instances, mods, sources};
use nexus_platform::security::{MemorySecretStore, SecretStore, make_account_secret_ref};
use serde_json::json;
use tempfile::TempDir;

struct TestEnv {
    _dir: TempDir,
    root: PathBuf,
    clock: Arc<FixedClock>,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        Self {
            _dir: dir,
            root,
            clock: Arc::new(FixedClock::new(1_795_274_960_000)),
        }
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("launcher.db")
    }

    fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    async fn open(&self) -> (Db, nexus_platform::db::InitReport) {
        Db::open_initialized(&self.db_path(), DbOptions::default(), self.clock())
            .await
            .expect("database should initialize")
    }

    fn service(&self, db: &Db) -> SettingsService {
        SettingsService::new(db.pool().clone(), self.clock.clone())
    }
}

fn instance_request(name: &str) -> instances::NewInstance {
    instances::NewInstance {
        name: name.to_owned(),
        description: "Integration test instance".to_owned(),
        game_version: "1.21.1".to_owned(),
        loader_type: "fabric".to_owned(),
        loader_version: Some("0.16.9".to_owned()),
        java_runtime_id: None,
        memory_min_mb: 1024,
        memory_max_mb: 4096,
        source_kind: "local".to_owned(),
    }
}

fn offline_account(name: &str) -> accounts::NewAccount {
    accounts::NewAccount {
        provider: "offline".to_owned(),
        provider_account_id: name.to_lowercase(),
        username: name.to_owned(),
        uuid: Some(offline_player_uuid(name).to_string()),
        display_name: Some(name.to_owned()),
        secret_ref: None,
    }
}

fn download_task() -> downloads::NewTask {
    downloads::NewTask {
        kind: "minecraft".to_owned(),
        label: "Minecraft 26.2".to_owned(),
        job_id: None,
        priority: 100,
    }
}

fn download_file() -> downloads::NewFile {
    downloads::NewFile {
        url: "https://piston-data.mojang.com/v1/objects/abc/client.jar".to_owned(),
        root_kind: "shared_versions".to_owned(),
        rel_path: "26.2/client.jar".to_owned(),
        size_bytes: 2_048,
        sha1: None,
        sha256: None,
        required: true,
    }
}

#[tokio::test]
async fn fresh_database_has_required_tables_indexes_and_pragmas() {
    let env = TestEnv::new();
    let (db, report) = env.open().await;

    assert_eq!(report.migration.from_version, 0);
    assert_eq!(report.migration.to_version, compiled_max_version());
    assert!(!report.migration.applied.is_empty());
    assert!(report.migration.integrity_ok);
    assert!(report.health.ok);
    assert!(report.health.foreign_keys);
    assert_eq!(report.health.journal_mode.to_lowercase(), "wal");
    assert_eq!(
        report.health.schema_version,
        report.health.supported_schema_version
    );

    let objects: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY name",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();

    for required in [
        "instances",
        "instance_mods",
        "accounts",
        "java_runtimes",
        "download_tasks",
        "download_files",
        "servers",
        "backups",
        "nexus_sources",
        "installed_builds",
        "app_settings",
        "activity_logs",
        "app_meta",
        "_sqlx_migrations",
        "v_accounts_safe",
    ] {
        assert!(
            objects.iter().any(|name| name == required),
            "missing schema object {required}"
        );
    }

    let schema_name: String =
        sqlx::query_scalar("SELECT value FROM app_meta WHERE key = 'schema.name'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(schema_name, "nexus-launcher");

    let fk_violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert!(fk_violations.is_empty());

    db.close().await.unwrap();
}

#[tokio::test]
async fn settings_are_persisted_across_restart_and_unknown_keys_survive() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;
    let service = env.service(&db);

    sqlx::query(
        "INSERT INTO app_settings (key, value_json, created_at, updated_at)
         VALUES ('future.experimental', '{\"enabled\":true}', ?, ?)",
    )
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await
    .unwrap();

    let patch: SettingsPatch = serde_json::from_value(json!({
        "paths": {
            "gameDirectory": "C:\\Games\\NEXUS-Minecraft",
            "javaDirectory": "C:\\Java\\NEXUS"
        },
        "ui": {
            "language": "en",
            "theme": "light",
            "backgroundQuality": "high",
            "backgroundFps": 144
        },
        "downloads": {
            "maxConcurrent": 8,
            "maxSpeedKbps": 25000
        },
        "logging": {
            "enabled": false,
            "level": "debug"
        }
    }))
    .unwrap();

    let change = service.apply(&patch).await.unwrap();
    assert_eq!(change.changed_keys.len(), 10);
    assert_eq!(change.settings.ui.language, nexus_core::Language::En);
    assert_eq!(change.settings.downloads.max_concurrent, 8);

    db.close().await.unwrap();

    let (db, report) = env.open().await;
    assert!(!report.recovery.unclean_shutdown);
    let reopened = env.service(&db);
    let settings = reopened.get().await.unwrap();

    assert_eq!(settings, change.settings);
    assert_eq!(settings.ui.theme, nexus_core::Theme::Light);
    assert_eq!(settings.ui.background_fps, 144);
    assert_eq!(
        settings.paths.game_directory.as_deref(),
        Some("C:\\Games\\NEXUS-Minecraft")
    );
    assert_eq!(settings.downloads.max_speed_kbps, 25_000);
    assert!(!settings.logging.enabled);
    assert_eq!(settings.logging.level, nexus_core::LogLevel::Debug);

    // Unknown/future keys are preserved by partial updates.
    assert_eq!(
        reopened.raw_value("future.experimental").await.unwrap(),
        Some(json!({ "enabled": true }))
    );
    assert!(reopened.stored_key_count().await.unwrap() >= 11);

    db.close().await.unwrap();
}

#[tokio::test]
async fn restart_does_not_destroy_user_data_and_migrations_are_idempotent() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;

    let instance = instances::create(db.pool(), env.clock(), instance_request("Survival"))
        .await
        .unwrap();
    let _mod = mods::create(
        db.pool(),
        env.clock(),
        &instance.id,
        mods::NewInstanceMod {
            provider: "modrinth".to_owned(),
            project_id: "sodium".to_owned(),
            version_id: "mc1.21.1-0.6.5".to_owned(),
            project_name: "Sodium".to_owned(),
            version_name: "0.6.5".to_owned(),
            file_name: "sodium.jar".to_owned(),
            rel_path: "mods/sodium.jar".to_owned(),
            side: "client".to_owned(),
            enabled: true,
            managed: true,
            size_bytes: 1024,
            sha256: Some("a".repeat(64)),
        },
    )
    .await
    .unwrap();
    let account = accounts::create(db.pool(), env.clock(), offline_account("Player"))
        .await
        .unwrap();
    let task = downloads::create_task(db.pool(), env.clock(), download_task())
        .await
        .unwrap();
    downloads::add_file(db.pool(), env.clock(), &task.id, download_file())
        .await
        .unwrap();

    db.close().await.unwrap();

    let (db, report) = env.open().await;
    assert_eq!(report.migration.from_version, compiled_max_version());
    assert_eq!(report.migration.to_version, compiled_max_version());
    assert!(report.migration.applied.is_empty());
    assert!(report.migration.backup_path.is_none());
    assert!(!report.recovery.unclean_shutdown);

    let instances_after = instances::list(db.pool(), true).await.unwrap();
    assert_eq!(instances_after.len(), 1);
    assert_eq!(instances_after[0].name, "Survival");
    assert_eq!(instances_after[0].id, instance.id);

    let mods_after = mods::list_for_instance(db.pool(), &instance.id)
        .await
        .unwrap();
    assert_eq!(mods_after.len(), 1);
    assert_eq!(mods_after[0].rel_path, "mods/sodium.jar");

    let accounts_after = accounts::list(db.pool()).await.unwrap();
    assert_eq!(accounts_after.len(), 1);
    assert_eq!(accounts_after[0].id, account.id);

    let tasks_after = downloads::list_tasks(db.pool(), None).await.unwrap();
    assert_eq!(tasks_after.len(), 1);
    assert_eq!(tasks_after[0].total_files, 1);
    assert_eq!(tasks_after[0].total_bytes, 2_048);

    db.close().await.unwrap();
}

#[tokio::test]
async fn unique_constraints_soft_delete_and_cascade_integrity_work() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;

    let first = instances::create(db.pool(), env.clock(), instance_request("Same Name"))
        .await
        .unwrap();

    let duplicate = instances::create(db.pool(), env.clock(), instance_request("same name"))
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), nexus_core::ErrorCode::Conflict);

    // Soft delete hides the instance but keeps the row for diagnostics.
    assert!(
        instances::soft_delete(db.pool(), &first.id, env.clock())
            .await
            .unwrap()
    );
    assert!(instances::list(db.pool(), true).await.unwrap().is_empty());
    let deleted = instances::get_with_deleted(db.pool(), &first.id, true)
        .await
        .unwrap()
        .unwrap();
    assert!(deleted.deleted_at.is_some());

    // The name is free again after soft delete.
    let second = instances::create(db.pool(), env.clock(), instance_request("Same Name"))
        .await
        .unwrap();

    // Account uniqueness is scoped to active rows.
    let account = accounts::create(db.pool(), env.clock(), offline_account("Player"))
        .await
        .unwrap();
    let duplicate_account = accounts::create(db.pool(), env.clock(), offline_account("Player"))
        .await
        .unwrap_err();
    assert_eq!(duplicate_account.code(), nexus_core::ErrorCode::Conflict);
    let deleted_account = accounts::soft_delete(db.pool(), &account.id, env.clock())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted_account.secret_ref, None);
    accounts::create(db.pool(), env.clock(), offline_account("Player"))
        .await
        .unwrap();

    // Download file path uniqueness inside one task.
    let task = downloads::create_task(db.pool(), env.clock(), download_task())
        .await
        .unwrap();
    downloads::add_file(db.pool(), env.clock(), &task.id, download_file())
        .await
        .unwrap();
    let duplicate_file = downloads::add_file(db.pool(), env.clock(), &task.id, download_file())
        .await
        .unwrap_err();
    assert_eq!(duplicate_file.code(), nexus_core::ErrorCode::Conflict);

    // A source that is referenced by an installed build cannot be hard-deleted.
    let source = sources::create(
        db.pool(),
        env.clock(),
        sources::NewSource {
            name: "NEXUS".to_owned(),
            base_url: "https://nexus.example.com".to_owned(),
            channel_default: "stable".to_owned(),
            auth_mode: "public".to_owned(),
            secret_ref: None,
        },
    )
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO installed_builds
            (id, instance_id, source_id, pack_id, release, channel, manifest_sha256,
             installed_at, created_at, updated_at)
         VALUES (?, ?, ?, 'vanilla', '1.1', 'stable', ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&second.id)
    .bind(&source.id)
    .bind("b".repeat(64))
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await
    .unwrap();

    let restricted = sqlx::query("DELETE FROM nexus_sources WHERE id = ?")
        .bind(&source.id)
        .execute(db.pool())
        .await;
    assert!(
        restricted.is_err(),
        "ON DELETE RESTRICT must protect the source"
    );

    // Adding a mod and then hard-deleting the instance cascades children.
    mods::create(
        db.pool(),
        env.clock(),
        &second.id,
        mods::NewInstanceMod {
            provider: "local".to_owned(),
            project_id: "local".to_owned(),
            version_id: "1".to_owned(),
            project_name: String::new(),
            version_name: String::new(),
            file_name: "local.jar".to_owned(),
            rel_path: "mods/local.jar".to_owned(),
            side: "both".to_owned(),
            enabled: true,
            managed: false,
            size_bytes: 10,
            sha256: None,
        },
    )
    .await
    .unwrap();

    assert!(instances::hard_delete(db.pool(), &second.id).await.unwrap());
    let mods_left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM instance_mods WHERE instance_id = ?")
            .bind(&second.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(mods_left, 0);
    let builds_left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM installed_builds WHERE instance_id = ?")
            .bind(&second.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(builds_left, 0);

    db.close().await.unwrap();
}

#[tokio::test]
async fn newer_schema_is_refused_and_file_is_preserved() {
    let env = TestEnv::new();
    let db = Db::open(&env.db_path(), DbOptions::default())
        .await
        .unwrap();

    sqlx::query("CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO _sqlx_migrations (version) VALUES (999)")
        .execute(db.pool())
        .await
        .unwrap();

    let error = db.initialize(env.clock()).await.unwrap_err();
    match error {
        DbError::SchemaTooNew { found, supported } => {
            assert_eq!(found, 999);
            assert_eq!(supported, compiled_max_version());
        }
        other => panic!("unexpected error: {other}"),
    }

    assert!(env.db_path().is_file());
    let raw: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(raw, 999, "database must not be rewritten");
    db.pool().close().await;
}

#[tokio::test]
async fn unclean_shutdown_pauses_running_downloads_and_is_reported() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;

    let task = downloads::create_task(db.pool(), env.clock(), download_task())
        .await
        .unwrap();
    let file = downloads::add_file(db.pool(), env.clock(), &task.id, download_file())
        .await
        .unwrap();
    downloads::set_task_status(db.pool(), &task.id, "running", env.clock())
        .await
        .unwrap();
    downloads::set_file_status(db.pool(), &file.id, "running", 128, None, env.clock())
        .await
        .unwrap();

    // Simulate a killed process: close the pool without the clean-shutdown flag.
    db.pool().close().await;
    drop(db);

    let (db, report) = env.open().await;
    assert!(report.recovery.unclean_shutdown);
    assert_eq!(report.recovery.paused_tasks, 1);
    assert_eq!(report.recovery.paused_files, 1);

    let task_after = downloads::get_task(db.pool(), &task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task_after.status, "paused");
    let file_after = downloads::get_file(db.pool(), &file.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file_after.status, "paused");
    assert_eq!(file_after.bytes_done, 128);

    let logs = activity::list(db.pool(), 20).await.unwrap();
    assert!(logs.iter().any(|row| row.kind == "db.recovery"));

    db.close().await.unwrap();

    let (db, report) = env.open().await;
    assert!(!report.recovery.unclean_shutdown);
    let task_after = downloads::get_task(db.pool(), &task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task_after.status, "paused");
    db.close().await.unwrap();
}

#[tokio::test]
async fn pre_migration_backup_is_consistent_and_retained() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;
    instances::create(db.pool(), env.clock(), instance_request("Backup Me"))
        .await
        .unwrap();

    let first = db
        .create_pre_migration_backup(compiled_max_version())
        .await
        .unwrap();
    assert!(first.is_file());
    assert!(fs::metadata(&first).unwrap().len() > 0);

    let second = db
        .create_pre_migration_backup(compiled_max_version())
        .await
        .unwrap();
    assert!(second.is_file());
    assert!(fs::metadata(&second).unwrap().len() > 0);

    let third = db
        .create_pre_migration_backup(compiled_max_version())
        .await
        .unwrap();
    assert!(third.is_file());
    assert!(fs::metadata(&third).unwrap().len() > 0);

    let retained = db.list_pre_migration_backups().unwrap();
    assert_eq!(retained.len(), 2, "only the two newest backups are kept");
    assert!(retained.contains(&third));

    // The backup is a valid SQLite database and contains the user data.
    let backup_db = Db::open(&third, DbOptions::default()).await.unwrap();
    assert_eq!(
        backup_db.check_schema_version().await.unwrap(),
        compiled_max_version()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances WHERE deleted_at IS NULL")
        .fetch_one(backup_db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    backup_db.pool().close().await;

    db.close().await.unwrap();
}

#[tokio::test]
async fn tokens_never_reach_sqlite() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;
    let store = MemorySecretStore::new();

    let account_id = AccountId::new().to_string();
    let secret_ref = make_account_secret_ref(&account_id).unwrap();
    let token = "SUPER-SECRET-REFRESH-TOKEN-1234567890";
    let secret = nexus_platform::security::SecretValue::new(token).unwrap();
    store.put(&secret_ref, &secret).unwrap();
    assert_eq!(store.get(&secret_ref).unwrap().unwrap().expose(), token);

    accounts::create(
        db.pool(),
        env.clock(),
        accounts::NewAccount {
            provider: "microsoft".to_owned(),
            provider_account_id: account_id.clone(),
            username: "Player".to_owned(),
            uuid: None,
            display_name: Some("Player".to_owned()),
            secret_ref: Some(secret_ref.as_str().to_owned()),
        },
    )
    .await
    .unwrap();

    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(db.pool())
        .await
        .unwrap();

    let mut bytes = fs::read(env.db_path()).unwrap();
    let wal = env.root.join("launcher.db-wal");
    if wal.is_file() {
        bytes.extend(fs::read(wal).unwrap());
    }
    assert!(
        !bytes
            .windows(token.len())
            .any(|window| window == token.as_bytes()),
        "plaintext token must not be present in the database or WAL"
    );

    let columns: Vec<String> = sqlx::query("PRAGMA table_info(accounts)")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            use sqlx::Row;
            row.get::<String, _>("name")
        })
        .collect();
    for forbidden in ["access_token", "refresh_token", "password", "token"] {
        assert!(!columns.iter().any(|name| name == forbidden));
    }
    assert!(columns.iter().any(|name| name == "secret_ref"));

    let stored: String = sqlx::query_scalar("SELECT secret_ref FROM accounts LIMIT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, secret_ref.as_str());
    assert_ne!(stored, token);

    db.close().await.unwrap();
}

#[tokio::test]
async fn activity_details_are_redacted_and_damaged_settings_fall_back() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;

    activity::log(
        db.pool(),
        env.clock(),
        "auth.test",
        Some("account"),
        Some("abc"),
        "test activity",
        Some(json!({
            "refresh_token": "do-not-store-me",
            "nested": { "password": "also-secret", "safe": 1 }
        })),
    )
    .await
    .unwrap();

    let rows = activity::list(db.pool(), 10).await.unwrap();
    let details = rows[0].details_json.as_deref().unwrap();
    assert!(!details.contains("do-not-store-me"));
    assert!(!details.contains("also-secret"));
    assert!(details.contains("[redacted]"));

    // JSON validity is enforced by SQLite itself.
    let invalid_json = sqlx::query(
        "INSERT INTO app_settings (key, value_json, created_at, updated_at)
         VALUES ('ui.theme', '{not-json', ?, ?)",
    )
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await;
    assert!(invalid_json.is_err());

    // Type-invalid values are skipped by the service and defaults are used.
    sqlx::query(
        "INSERT INTO app_settings (key, value_json, created_at, updated_at)
         VALUES (?, '\"not-a-number\"', ?, ?)",
    )
    .bind(keys::UI_BACKGROUND_FPS)
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO app_settings (key, value_json, created_at, updated_at)
         VALUES (?, '\"light\"', ?, ?)",
    )
    .bind(keys::UI_THEME)
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await
    .unwrap();

    let settings = env.service(&db).get().await.unwrap();
    assert_eq!(settings.ui.background_fps, 60);
    assert_eq!(settings.ui.theme, nexus_core::Theme::Light);
    assert_eq!(settings, {
        let mut expected = EffectiveSettings::default();
        expected.ui.theme = nexus_core::Theme::Light;
        expected
    });

    db.close().await.unwrap();
}

#[tokio::test]
async fn reset_group_only_removes_that_group_and_keeps_unknown_keys() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;
    let service = env.service(&db);

    let patch: SettingsPatch = serde_json::from_value(json!({
        "ui": { "theme": "light", "backgroundFps": 30 },
        "downloads": { "maxConcurrent": 6 }
    }))
    .unwrap();
    service.apply(&patch).await.unwrap();
    sqlx::query(
        "INSERT INTO app_settings (key, value_json, created_at, updated_at)
         VALUES ('future.key', 'true', ?, ?)",
    )
    .bind(env.clock.now_ms())
    .bind(env.clock.now_ms())
    .execute(db.pool())
    .await
    .unwrap();

    let reset = service.reset_group(SettingsGroup::Ui).await.unwrap();
    assert_eq!(reset.ui.theme, nexus_core::Theme::Dark);
    assert_eq!(reset.ui.background_fps, 60);
    assert_eq!(reset.downloads.max_concurrent, 6);
    assert!(service.raw_value("future.key").await.unwrap().is_some());

    db.close().await.unwrap();
}

#[tokio::test]
async fn config_toml_is_the_base_layer_and_reset_reveals_it_again() {
    let env = TestEnv::new();
    let (db, _) = env.open().await;

    let base = EffectiveSettings {
        downloads: nexus_core::DownloadSettings {
            max_concurrent: 10,
            max_speed_kbps: 900,
        },
        logging: nexus_core::LoggingSettings {
            enabled: true,
            level: nexus_core::LogLevel::Debug,
        },
        ..EffectiveSettings::default()
    };
    let service = SettingsService::with_base(db.pool().clone(), env.clock.clone(), base);

    let initial = service.get().await.unwrap();
    assert_eq!(initial.downloads.max_concurrent, 10);
    assert_eq!(initial.logging.level, nexus_core::LogLevel::Debug);

    let patch: SettingsPatch =
        serde_json::from_value(json!({ "downloads": { "maxConcurrent": 3 } })).unwrap();
    service.apply(&patch).await.unwrap();
    assert_eq!(
        service.get().await.unwrap().downloads.max_concurrent,
        3,
        "app_settings must override config.toml"
    );

    let after_reset = service.reset_group(SettingsGroup::Downloads).await.unwrap();
    assert_eq!(
        after_reset.downloads.max_concurrent, 10,
        "reset returns to the config.toml layer"
    );
    assert_eq!(after_reset.downloads.max_speed_kbps, 900);

    db.close().await.unwrap();
}

#[tokio::test]
async fn data_paths_and_config_are_initialised_once_and_preserved() {
    let env = TestEnv::new();
    let paths = DataPaths::with_root(&env.root).unwrap();
    paths.ensure_layout().unwrap();

    let loaded = nexus_core::config::load_or_init(&paths.config_path()).unwrap();
    assert!(loaded.created);
    assert_eq!(loaded.config.downloads.max_concurrent, 4);

    let (db, _) = env.open().await;
    let service = env.service(&db);
    let patch: SettingsPatch =
        serde_json::from_value(json!({ "ui": { "theme": "light" } })).unwrap();
    service.apply(&patch).await.unwrap();
    db.close().await.unwrap();

    let (db, _) = env.open().await;
    assert_eq!(
        env.service(&db).get().await.unwrap().ui.theme,
        nexus_core::Theme::Light
    );
    let reloaded = nexus_core::config::load_or_init(&paths.config_path()).unwrap();
    assert!(!reloaded.created);
    assert_eq!(reloaded.config.schema_version, 1);
    db.close().await.unwrap();
}

#[test]
fn identifiers_used_by_schema_are_uuid_v4() {
    for id in [
        InstanceId::new().to_string(),
        AccountId::new().to_string(),
        SourceId::new().to_string(),
    ] {
        assert_eq!(id.len(), 36);
        let parsed = uuid::Uuid::parse_str(&id).unwrap();
        assert_eq!(parsed.get_version_num(), 4);
    }
}
