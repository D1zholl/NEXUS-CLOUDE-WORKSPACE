//! Интеграционные тесты Storage Engine: пути, объекты, замена, отказ диска.

mod common;

use common::TestEnv;
use nexus::core::error::ApiResult;
use nexus::storage::engine::{self, ObjectLocation, VolumeScope};
use nexus::storage::paths;

async fn put_object(
    env: &TestEnv,
    volume_root: &std::path::Path,
    payload: &[u8],
) -> ObjectLocation {
    let staging = volume_root.join("staging-test.bin");
    tokio::fs::write(&staging, payload).await.unwrap();
    let volume = engine::load_volume(&env.pool, volume_id_from_root(env, volume_root).await)
        .await
        .unwrap();
    engine::commit_from_staging(
        &env.pool,
        &volume,
        &staging,
        payload.len() as i64,
        "text/plain",
    )
    .await
    .unwrap()
}

async fn volume_id_from_root(env: &TestEnv, root: &std::path::Path) -> uuid::Uuid {
    sqlx::query_scalar("select id from storage_volumes where root_path = $1")
        .bind(root.to_string_lossy().to_string())
        .fetch_one(&env.pool)
        .await
        .unwrap()
}

fn volume_root(env: &TestEnv, name: &str) -> std::path::PathBuf {
    env.root.path().join(name)
}

#[test]
fn path_traversal_is_rejected() {
    assert!(paths::normalize_relative("../etc/passwd").is_err());
    assert!(paths::normalize_relative("a/../../b").is_err());
    assert!(paths::normalize_relative("/absolute").is_err());
    assert!(paths::normalize_relative("C:\\Windows").is_err());
    assert!(paths::normalize_relative("a/b/../../c").is_err());
}

#[test]
fn object_keys_are_content_addressed_and_unique() {
    let sha = "a".repeat(64);
    let first = engine::build_object_key(&sha, &uuid::Uuid::new_v4());
    let second = engine::build_object_key(&sha, &uuid::Uuid::new_v4());
    assert_ne!(first, second, "ключи должны различаться по id");
    // Формат: aa/bb/<фрагмент sha>-<фрагмент id>
    assert!(first.starts_with(&format!("{}/{}/", &sha[0..2], &sha[2..4])));
    assert!(
        first.contains(&sha[4..16]),
        "ключ должен содержать фрагмент SHA-256"
    );
    assert_eq!(first.split('-').count(), 2);
    // Ключ не содержит имени файла или абсолютного пути.
    assert!(!first.contains(':') && !first.contains('\\'));
}

#[tokio::test]
async fn object_roundtrip_and_delete() {
    let env = TestEnv::new().await;
    env.add_volume("vol-a").await;
    let root = volume_root(&env, "vol-a");
    let payload = b"storage engine payload";
    let location = put_object(&env, &root, payload).await;

    let stat = engine::stat_object(&location).unwrap();
    assert_eq!(stat, payload.len() as u64);

    let read = tokio::fs::read(&location.absolute).await.unwrap();
    assert_eq!(read, payload);

    let stored_sha: String = sqlx::query_scalar("select sha256 from storage_objects where id = $1")
        .bind(
            sqlx::query_scalar::<_, uuid::Uuid>(
                "select id from storage_objects where object_key = $1",
            )
            .bind(&location.key)
            .fetch_one(&env.pool)
            .await
            .unwrap(),
        )
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(stored_sha.len(), 64);
    assert_eq!(stored_sha, sha256_hex(payload));

    assert!(engine::delete_object(&location).await);
    assert!(!location.absolute.exists());
}

#[tokio::test]
async fn same_content_produces_same_sha_and_distinct_objects() {
    let env = TestEnv::new().await;
    env.add_volume("vol-dup").await;
    let root = volume_root(&env, "vol-dup");
    let a = put_object(&env, &root, b"same payload").await;
    let b = put_object(&env, &root, b"same payload").await;
    assert_ne!(a.key, b.key);
    let shas: Vec<String> = sqlx::query_scalar(
        "select sha256 from storage_objects where object_key in ($1, $2) order by object_key",
    )
    .bind(&a.key)
    .bind(&b.key)
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(shas.len(), 2);
    assert_eq!(
        shas[0], shas[1],
        "одинаковое содержимое → одинаковый SHA-256"
    );
}

#[tokio::test]
async fn missing_object_is_detected() {
    let env = TestEnv::new().await;
    env.add_volume("vol-miss").await;
    let root = volume_root(&env, "vol-miss");
    let location = put_object(&env, &root, b"temporary").await;
    tokio::fs::remove_file(&location.absolute).await.unwrap();
    assert!(engine::stat_object(&location).is_err());
    assert!(engine::open_object(&location).await.is_err());
}

#[tokio::test]
async fn volume_selection_respects_priority_and_space() {
    let env = TestEnv::new().await;
    let high = env.add_volume("vol-high").await;
    let low: uuid::Uuid = sqlx::query_scalar(
        "insert into storage_volumes (name, root_path, mode, priority)
         values ('vol-low', $1, 'auto', 10) returning id",
    )
    .bind(volume_root(&env, "vol-low").to_string_lossy().to_string())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    // Каталог второго тома тоже должен существовать.
    std::fs::create_dir_all(volume_root(&env, "vol-low")).unwrap();
    sqlx::query("update storage_volumes set available_bytes = 100000, total_bytes = 200000 where id = any($1)")
        .bind(&[high, low][..])
        .execute(&env.pool)
        .await
        .unwrap();

    let picked = engine::pick_volume(&env.pool, None, 1024, VolumeScope::System, None)
        .await
        .unwrap();
    assert_eq!(
        picked.id, high,
        "том с большим приоритетом выбирается первым"
    );

    // Отключённый том не участвует в выборе.
    sqlx::query("update storage_volumes set enabled = false where id = $1")
        .bind(high)
        .execute(&env.pool)
        .await
        .unwrap();
    let picked = engine::pick_volume(&env.pool, None, 1024, VolumeScope::System, None)
        .await
        .unwrap();
    assert_eq!(picked.id, low);
}

#[tokio::test]
async fn manual_volume_is_used_only_when_requested() {
    let env = TestEnv::new().await;
    env.add_volume("vol-auto").await;
    let manual: uuid::Uuid = sqlx::query_scalar(
        "insert into storage_volumes (name, root_path, mode, priority)
         values ('vol-manual', $1, 'manual', 100) returning id",
    )
    .bind(
        volume_root(&env, "vol-manual")
            .to_string_lossy()
            .to_string(),
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    std::fs::create_dir_all(volume_root(&env, "vol-manual")).unwrap();
    sqlx::query("update storage_volumes set available_bytes = 100000, total_bytes = 200000")
        .execute(&env.pool)
        .await
        .unwrap();

    let auto_pick = engine::pick_volume(&env.pool, None, 1024, VolumeScope::System, None)
        .await
        .unwrap();
    assert_ne!(
        auto_pick.id, manual,
        "manual-том не выбирается автоматически"
    );

    let manual_pick = engine::pick_volume(&env.pool, Some(manual), 1024, VolumeScope::System, None)
        .await
        .unwrap();
    assert_eq!(manual_pick.id, manual);
}

#[tokio::test]
async fn insufficient_space_is_reported() {
    let env = TestEnv::new().await;
    let id = env.add_volume("vol-small").await;
    sqlx::query(
        "update storage_volumes set total_bytes = 1000, available_bytes = 500 where id = $1",
    )
    .bind(id)
    .execute(&env.pool)
    .await
    .unwrap();
    let result: ApiResult<engine::VolumeRecord> =
        engine::pick_volume(&env.pool, None, 10_000, VolumeScope::System, None).await;
    assert!(result.is_err(), "недостаток места должен вернуть ошибку");
}

#[tokio::test]
async fn failed_commit_leaves_no_committed_object() {
    let env = TestEnv::new().await;
    let id = env.add_volume("vol-fail").await;
    let root = volume_root(&env, "vol-fail");

    // Файл staging отсутствует — фиксация обязана завершиться ошибкой.
    let missing = root.join("does-not-exist.bin");
    let volume = engine::load_volume(&env.pool, id).await.unwrap();
    let result = engine::commit_from_staging(&env.pool, &volume, &missing, 10, "text/plain").await;
    assert!(result.is_err());

    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 0, "при ошибке объект не должен фиксироваться");
}

async fn insert_volume(
    env: &TestEnv,
    name: &str,
    scope: &str,
    owner: Option<uuid::Uuid>,
    mode: &str,
    priority: i32,
) -> uuid::Uuid {
    let path = volume_root(env, name);
    std::fs::create_dir_all(&path).unwrap();
    let (total, available) = engine::disk_space(&path);
    sqlx::query_scalar(
        "insert into storage_volumes
           (name, root_path, scope, owner_user_id, mode, priority, state,
            total_bytes, available_bytes)
         values ($1, $2, $3, $4, $5, $6, 'online', $7, $8)
         returning id",
    )
    .bind(name)
    .bind(path.to_string_lossy().to_string())
    .bind(scope)
    .bind(owner)
    .bind(mode)
    .bind(priority)
    .bind(total as i64)
    .bind(available as i64)
    .fetch_one(&env.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn cloud_does_not_pick_minecraft_only_volume() {
    let env = TestEnv::new().await;
    let minecraft_only = insert_volume(&env, "vol-mc-only", "minecraft", None, "auto", 500).await;

    let cloud = engine::pick_volume(&env.pool, None, 1024, VolumeScope::Cloud, None).await;
    let error = cloud.expect_err("Cloud не должен выбирать minecraft-only том");
    assert_eq!(error.code, "NO_VOLUME_AVAILABLE");

    // Явный запрос чужой области тоже отклоняется, без раскрытия тома.
    let requested = engine::pick_volume(
        &env.pool,
        Some(minecraft_only),
        1024,
        VolumeScope::Cloud,
        None,
    )
    .await;
    assert_eq!(
        requested
            .expect_err("явный том другой области запрещён")
            .status,
        axum::http::StatusCode::NOT_FOUND
    );

    // Minecraft этот том видит.
    let picked = engine::pick_volume(&env.pool, None, 1024, VolumeScope::Minecraft, None)
        .await
        .unwrap();
    assert_eq!(picked.id, minecraft_only);
}

#[tokio::test]
async fn foreign_personal_volume_is_not_available() {
    let env = TestEnv::new().await;
    let alice = env.create_user("vol-alice", "SecretPass1!").await;
    let bob = env.create_user("vol-bob", "SecretPass1!").await;
    let personal = insert_volume(&env, "vol-alice", "cloud", Some(alice), "auto", 500).await;

    let bob_pick = engine::pick_volume(&env.pool, None, 1024, VolumeScope::Cloud, Some(bob)).await;
    assert_eq!(
        bob_pick
            .expect_err("чужой персональный том не выбирается")
            .code,
        "NO_VOLUME_AVAILABLE"
    );

    let bob_request = engine::pick_volume(
        &env.pool,
        Some(personal),
        1024,
        VolumeScope::Cloud,
        Some(bob),
    )
    .await;
    assert_eq!(
        bob_request
            .expect_err("чужой персональный том не выдаётся явно")
            .status,
        axum::http::StatusCode::NOT_FOUND
    );

    let alice_pick = engine::pick_volume(&env.pool, None, 1024, VolumeScope::Cloud, Some(alice))
        .await
        .unwrap();
    assert_eq!(alice_pick.id, personal);
}

#[tokio::test]
async fn existing_system_volume_is_shared_after_migration() {
    let env = TestEnv::new().await;
    // Запись без scope/owner эмулирует том, созданный до миграции K1.
    let id: uuid::Uuid = sqlx::query_scalar(
        "insert into storage_volumes (name, root_path, mode, priority, state)
         values ('vol-legacy', $1, 'auto', 100, 'online') returning id",
    )
    .bind(
        volume_root(&env, "vol-legacy")
            .to_string_lossy()
            .to_string(),
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    std::fs::create_dir_all(volume_root(&env, "vol-legacy")).unwrap();
    let (total, available) = engine::disk_space(&volume_root(&env, "vol-legacy"));
    sqlx::query("update storage_volumes set total_bytes = $2, available_bytes = $3 where id = $1")
        .bind(id)
        .bind(total as i64)
        .bind(available as i64)
        .execute(&env.pool)
        .await
        .unwrap();
    let (scope, owner): (String, Option<uuid::Uuid>) =
        sqlx::query_as("select scope, owner_user_id from storage_volumes where id = $1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(scope, "system");
    assert!(owner.is_none());

    for scope in [
        VolumeScope::Cloud,
        VolumeScope::Minecraft,
        VolumeScope::System,
    ] {
        let picked = engine::pick_volume(&env.pool, None, 1024, scope, None)
            .await
            .unwrap();
        assert_eq!(picked.id, id, "system-том доступен области {scope:?}");
    }
}

#[cfg(windows)]
#[tokio::test]
async fn duplicate_root_in_another_case_is_rejected() {
    use nexus::storage::service::{CreateVolumeRequest, create_volume};

    let env = TestEnv::new().await;
    let actor = env.create_user("vol-case", "SecretPass1!").await;
    let root = volume_root(&env, "vol-case-insensitive");
    std::fs::create_dir_all(&root).unwrap();

    let first = root.to_string_lossy().to_string();
    create_volume(
        &env.pool,
        actor,
        CreateVolumeRequest {
            name: "case-first".into(),
            path: first,
            mode: None,
            priority: None,
            reserved_bytes: None,
            max_bytes: None,
            enabled: None,
            scope: None,
            owner_user_id: None,
            adopt_orphans: false,
        },
    )
    .await
    .expect("первый том регистрируется");

    let mut alternate = root.to_string_lossy().to_string();
    if let Some(rest) = alternate.strip_prefix("C:") {
        alternate = format!("c:{rest}");
    }
    if !alternate.ends_with('\\') {
        alternate.push('\\');
    }
    let error = create_volume(
        &env.pool,
        actor,
        CreateVolumeRequest {
            name: "case-second".into(),
            path: alternate,
            mode: None,
            priority: None,
            reserved_bytes: None,
            max_bytes: None,
            enabled: None,
            scope: None,
            owner_user_id: None,
            adopt_orphans: false,
        },
    )
    .await
    .expect_err("тот же корень в другом регистре должен отклоняться");
    assert_eq!(error.code, "VOLUME_EXISTS");

    let stored: String = sqlx::query_scalar("select root_path from storage_volumes")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert!(
        stored.starts_with("C:"),
        "буква диска нормализована в верхний регистр: {stored}"
    );
    assert!(!stored.ends_with('\\'), "завершающий слэш убран: {stored}");
}

#[tokio::test]
async fn disconnected_volume_is_marked_offline() {
    use nexus::storage::service;

    let env = TestEnv::new().await;
    let id = env.add_volume("vol-offline").await;
    std::fs::remove_dir_all(volume_root(&env, "vol-offline")).unwrap();

    let outcome = service::check_volume(&env.pool, id).await.unwrap();
    assert_eq!(outcome.row.state, "offline");
    assert!(!outcome.report.reachable);
    assert!(
        outcome.report.message.contains("недоступ"),
        "{}",
        outcome.report.message
    );

    // Недоступный том не участвует в автоматическом выборе.
    let error = engine::pick_volume(&env.pool, None, 1, VolumeScope::Cloud, None)
        .await
        .expect_err("offline-том не выбирается");
    assert_eq!(error.code, "NO_VOLUME_AVAILABLE");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_paths_are_normalized_and_unc_is_rejected() {
    use nexus::storage::disks;
    use nexus::storage::service::{self, CreateVolumeRequest};

    let env = TestEnv::new().await;
    let actor = env.create_user("vol-windows", "SecretPass1!").await;

    // C:\Storage и c:\storage\ — один и тот же том.
    assert_eq!(
        paths::path_key(std::path::Path::new(r"C:\Storage")),
        paths::path_key(std::path::Path::new(r"c:\storage\"))
    );
    assert_eq!(
        paths::normalize_root(std::path::Path::new(r"c:\Storage\")),
        std::path::PathBuf::from(r"C:\Storage")
    );
    assert!(paths::is_unc_path(r"\\server\share\nexus"));
    assert!(paths::is_unc_path(r"\\?\UNC\server\share"));
    assert!(!paths::is_unc_path(r"\\?\C:\Storage"));

    // UNC отклоняется при регистрации с понятным сообщением.
    let error = service::create_volume(
        &env.pool,
        actor,
        CreateVolumeRequest {
            name: "unc".into(),
            path: r"\\server\share\nexus".into(),
            mode: None,
            priority: None,
            reserved_bytes: None,
            max_bytes: None,
            enabled: None,
            scope: None,
            owner_user_id: None,
            adopt_orphans: false,
        },
    )
    .await
    .expect_err("UNC-путь должен отклоняться");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        error.message.contains("UNC"),
        "сообщение должно объяснять причину: {}",
        error.message
    );

    // Свободное место запрашивается у конкретного тома.
    let (total, available) = disks::space_for_path(std::path::Path::new(r"C:\"))
        .expect("свободное место системного тома");
    assert!(total > 0);
    assert!(available <= total);
}

#[tokio::test]
async fn existing_objects_require_adoption_and_are_imported() {
    use nexus::storage::service::{self, CreateVolumeRequest};

    fn request(name: &str, path: String, adopt_orphans: bool) -> CreateVolumeRequest {
        CreateVolumeRequest {
            name: name.to_owned(),
            path,
            mode: None,
            priority: None,
            reserved_bytes: None,
            max_bytes: None,
            enabled: None,
            scope: None,
            owner_user_id: None,
            adopt_orphans,
        }
    }

    let env = TestEnv::new().await;
    let actor = env.create_user("vol-adopt", "SecretPass1!").await;
    let root = volume_root(&env, "vol-adopt");
    let object_key = "ab/cd/legacy-object";
    let payload = b"leftover from a previous volume";
    let object_path = engine::ObjectLocation::objects_root(&root).join(object_key);
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(&object_path, payload).unwrap();

    let error = service::create_volume(
        &env.pool,
        actor,
        request("adopt-first", root.to_string_lossy().to_string(), false),
    )
    .await
    .expect_err("непустой объектный каталог без подтверждения отклоняется");
    assert_eq!(error.code, "VOLUME_HAS_ORPHAN_OBJECTS");
    assert_eq!(error.details().unwrap()["objectsFound"], 1);

    let volumes: i64 = sqlx::query_scalar("select count(*) from storage_volumes")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(volumes, 0, "при отказе том не создаётся");
    assert!(object_path.exists(), "файл при отказе не изменяется");

    let volume = service::create_volume(
        &env.pool,
        actor,
        request("adopt-second", root.to_string_lossy().to_string(), true),
    )
    .await
    .expect("с подтверждением импорта том регистрируется");

    let (key, sha, size, ref_count): (String, String, i64, i32) = sqlx::query_as(
        "select object_key, sha256, size_bytes, ref_count
         from storage_objects where volume_id = $1",
    )
    .bind(volume.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(key, object_key);
    assert_eq!(size, payload.len() as i64);
    assert_eq!(sha, sha256_hex(payload));
    assert_eq!(ref_count, 1, "импортированный объект защищён от очистки");

    let refs: i64 = sqlx::query_scalar(
        "select count(*) from storage_object_refs
         where owner_module = 'system' and object_id = (
             select id from storage_objects where volume_id = $1
         )",
    )
    .bind(volume.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(refs, 1, "служебная ссылка фиксирует импорт");
    assert!(object_path.exists(), "импорт не перемещает файл");
    assert_eq!(tokio::fs::read(&object_path).await.unwrap(), payload);
}

#[tokio::test]
async fn deleting_volume_blocks_untracked_objects_on_disk() {
    use nexus::storage::service;

    let env = TestEnv::new().await;
    let id = env.add_volume("vol-untracked").await;
    let object_path = engine::ObjectLocation::objects_root(&volume_root(&env, "vol-untracked"))
        .join("aa/bb/untracked");
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(&object_path, b"not in database").unwrap();

    let error = service::delete_volume(&env.pool, id)
        .await
        .expect_err("нельзя удалить регистрацию при файлах без записей");
    assert_eq!(error.code, "VOLUME_HAS_UNTRACKED_OBJECTS");
    assert_eq!(error.details().unwrap()["objectsFound"], 1);

    let still_there: i64 = sqlx::query_scalar("select count(*) from storage_volumes where id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(still_there, 1);

    std::fs::remove_file(&object_path).unwrap();
    service::delete_volume(&env.pool, id)
        .await
        .expect("после очистки каталога регистрация удаляется");
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
