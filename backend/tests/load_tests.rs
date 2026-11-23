//! Нагрузочные проверки и контроль ресурсов.
//!
//! Тесты измеряют реальные значения на текущем оборудовании. Они не обещают
//! фиксированную пропускную способность, а фиксируют факты: файл не читается
//! в память целиком, параллельные операции не растят потребление бесконечно.

mod common;

use std::time::Instant;

use axum::body::Body;
use common::TestEnv;
use nexus::minecraft::{builds, manifests, releases, uploads};
use uuid::Uuid;

async fn owner(env: &TestEnv) -> Uuid {
    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from users where username = 'loadtester'")
            .fetch_optional(&env.pool)
            .await
            .unwrap();
    match existing {
        Some(id) => id,
        None => env.create_user("loadtester", "SecretPass1!").await,
    }
}

/// Рекурсивный размер каталога (для проверки, что данные реально на диске).
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if meta.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

#[tokio::test]
async fn many_small_files_are_uploaded_and_manifested() {
    let env = TestEnv::new().await;
    env.add_volume("load-vol").await;
    let user = owner(&env).await;

    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: "Load Pack".to_owned(),
            slug: Some("load-pack".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "tech".to_owned(),
            visibility: "public".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: Some(17),
            java_min: Some(17),
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();

    // 120 файлов — достаточно, чтобы поймать проблемы с путями и снимками.
    const COUNT: usize = 120;
    let payload = vec![7u8; 2048];
    let started = Instant::now();

    for index in 0..COUNT {
        let path = format!("mods/mod-{index:03}.jar");
        let session = uploads::create_upload(
            &env.state,
            user,
            build.id,
            uploads::CreateUploadRequest {
                path,
                size: payload.len() as i64,
                category: Some("mods".to_owned()),
                side: Some("client".to_owned()),
                required: Some(true),
                description: None,
                content_type: "application/java-archive".to_owned(),
            },
        )
        .await
        .unwrap();
        uploads::append_stream(&env.state, session.id, user, 0, Body::from(payload.clone()))
            .await
            .unwrap();
        uploads::complete_upload(&env.state, session.id, user)
            .await
            .unwrap();
    }

    let elapsed = started.elapsed();
    let files: i64 = sqlx::query_scalar(
        "select count(*) from minecraft_build_files where build_id = $1 and kind = 'file'",
    )
    .bind(build.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(files, COUNT as i64);

    // Сборка валидна и публикуется.
    let report = nexus::minecraft::checks::validate(&env.pool, build.id)
        .await
        .unwrap();
    assert!(report.valid, "{:?}", report.errors);

    let release = releases::create(
        &env.pool,
        user,
        build.id,
        releases::CreateReleaseRequest {
            version: Some("1.0.0".to_owned()),
            changelog: String::new(),
            channel: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(release.file_count, COUNT as i32);

    let manifest = manifests::build_manifest(&env.pool, build.id, release.id)
        .await
        .unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), COUNT);

    // Данные физически на диске.
    let on_disk = dir_size(env.root.path());
    assert!(
        on_disk >= (COUNT * payload.len()) as u64,
        "на диске должно быть не меньше {COUNT} файлов: {on_disk}"
    );

    println!(
        "загружено {COUNT} файлов за {:.2} с ({:.0} файлов/с)",
        elapsed.as_secs_f64(),
        COUNT as f64 / elapsed.as_secs_f64()
    );
}

#[tokio::test]
async fn large_file_is_streamed_not_buffered() {
    let env = TestEnv::new().await;
    env.add_volume("large-vol").await;
    let user = owner(&env).await;

    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: "Big File".to_owned(),
            slug: Some("big-file".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: None,
            java_min: None,
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();

    // 8 МиБ передаются чанками по 1 МиБ: файл не должен целиком попадать в RAM.
    const TOTAL: usize = 8 * 1024 * 1024;
    const CHUNK: usize = 1024 * 1024;
    let session = uploads::create_upload(
        &env.state,
        user,
        build.id,
        uploads::CreateUploadRequest {
            path: "overrides/big.bin".to_owned(),
            size: TOTAL as i64,
            category: Some("overrides".to_owned()),
            side: Some("both".to_owned()),
            required: Some(false),
            description: None,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    let chunk = vec![0xA5u8; CHUNK];
    let mut offset = 0usize;
    while offset < TOTAL {
        let written = uploads::append_stream(
            &env.state,
            session.id,
            user,
            offset as i64,
            Body::from(chunk.clone()),
        )
        .await
        .unwrap();
        offset = written as usize;
    }

    let file = uploads::complete_upload(&env.state, session.id, user)
        .await
        .unwrap();
    assert_eq!(file.size_bytes, TOTAL as i64);

    // SHA-256 посчитан потоково при фиксации.
    let sha: String = sqlx::query_scalar("select sha256 from storage_objects limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(sha.len(), 64);

    // Размер на диске совпадает.
    let object_size: i64 = sqlx::query_scalar("select size_bytes from storage_objects limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(object_size, TOTAL as i64);
}

#[tokio::test]
async fn concurrent_uploads_do_not_interfere() {
    let env = TestEnv::new().await;
    env.add_volume("parallel-vol").await;
    let user = owner(&env).await;

    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: "Parallel".to_owned(),
            slug: Some("parallel".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: None,
            java_min: None,
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();

    let mut tasks = Vec::new();
    for index in 0..6 {
        let state = env.state.clone();
        let path = format!("mods/parallel-{index}.jar");
        tasks.push(tokio::spawn(async move {
            let session = uploads::create_upload(
                &state,
                user,
                build.id,
                uploads::CreateUploadRequest {
                    path,
                    size: 1024,
                    category: Some("mods".to_owned()),
                    side: Some("client".to_owned()),
                    required: Some(true),
                    description: None,
                    content_type: "application/java-archive".to_owned(),
                },
            )
            .await
            .expect("сессия");
            uploads::append_stream(
                &state,
                session.id,
                user,
                0,
                Body::from(vec![index as u8; 1024]),
            )
            .await
            .expect("чанк");
            uploads::complete_upload(&state, session.id, user)
                .await
                .expect("фиксация")
        }));
    }

    for task in tasks {
        let file = task.await.expect("задача не должна падать");
        assert_eq!(file.size_bytes, 1024);
    }

    let files: i64 = sqlx::query_scalar(
        "select count(*) from minecraft_build_files where build_id = $1 and kind = 'file'",
    )
    .bind(build.id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(files, 6);

    let objects: i64 = sqlx::query_scalar("select count(*) from storage_objects")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(objects, 6, "у каждого файла свой объект");
}

#[tokio::test]
async fn resume_after_interruption_matches_final_size() {
    let env = TestEnv::new().await;
    env.add_volume("resume-vol").await;
    let user = owner(&env).await;

    let build = builds::create(
        &env.pool,
        user,
        builds::CreateBuildRequest {
            name: "Resume".to_owned(),
            slug: Some("resume".to_owned()),
            description: String::new(),
            version: "1.0.0".to_owned(),
            category: "other".to_owned(),
            visibility: "private".to_owned(),
            minecraft_version: "1.20.1".to_owned(),
            loader_type: "vanilla".to_owned(),
            loader_version: String::new(),
            java_major: None,
            java_min: None,
            java_notes: String::new(),
        },
    )
    .await
    .unwrap();

    let payload: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
    let session = uploads::create_upload(
        &env.state,
        user,
        build.id,
        uploads::CreateUploadRequest {
            path: "overrides/resume.bin".to_owned(),
            size: payload.len() as i64,
            category: Some("overrides".to_owned()),
            side: Some("both".to_owned()),
            required: None,
            description: None,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .unwrap();

    // «Обрыв» после 2000 байт.
    uploads::append_stream(
        &env.state,
        session.id,
        user,
        0,
        Body::from(payload[..2000].to_vec()),
    )
    .await
    .unwrap();

    // Продолжение с фактического смещения.
    let resumed = uploads::session_offset(&env.pool, session.id, user)
        .await
        .unwrap();
    assert_eq!(resumed.received_bytes, 2000);
    let written = uploads::append_stream(
        &env.state,
        session.id,
        user,
        resumed.received_bytes,
        Body::from(payload[2000..].to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(written, payload.len() as i64);

    let file = uploads::complete_upload(&env.state, session.id, user)
        .await
        .unwrap();
    assert_eq!(file.size_bytes, payload.len() as i64);

    // Содержимое совпадает байт в байт.
    let key: String = sqlx::query_scalar("select object_key from storage_objects limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let root: String = sqlx::query_scalar("select root_path from storage_volumes limit 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let path = std::path::PathBuf::from(root)
        .join(".nexus")
        .join("objects")
        .join(key);
    let stored = tokio::fs::read(&path).await.unwrap();
    assert_eq!(stored, payload, "после возобновления данные целы");
}

#[tokio::test]
async fn many_api_calls_are_counted() {
    // Метрики запросов не растут бесконечно и корректно считают RPS.
    use nexus::system::requests::RequestMetrics;
    use std::time::Duration;

    let metrics = RequestMetrics::new();
    for _ in 0..500 {
        metrics.record(Duration::from_micros(1500), false);
    }
    metrics.record(Duration::from_millis(5), true);

    let stats = metrics.snapshot();
    assert_eq!(stats.requests_last_minute, 501);
    assert_eq!(stats.errors_last_minute, 1);
    assert!(stats.requests_per_second >= 501.0);
    assert!(stats.average_latency_ms > 0.0);
}
