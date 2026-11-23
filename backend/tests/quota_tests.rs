//! Персональные квоты хранения (PLAN §8.1, §9.1).

mod common;

use axum::body::Body;
use common::TestEnv;
use nexus::cloud::{quota, uploads};
use uuid::Uuid;

async fn upload(
    env: &TestEnv,
    user: Uuid,
    name: &str,
    size: usize,
) -> Result<Uuid, nexus::core::error::ApiError> {
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: name.to_owned(),
            parent_id: None,
            size: size as i64,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await?;
    uploads::append_stream(&env.state, session.id, user, 0, Body::from(vec![7u8; size])).await?;
    let entry = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "auto".to_owned(),
            new_name: None,
        },
    )
    .await?;
    Ok(entry.id)
}

#[tokio::test]
async fn upload_over_quota_is_rejected_with_details() {
    let env = TestEnv::new().await;
    env.add_volume("quota-vol").await;
    let user = env.create_user("quotauser", "SecretPass1!").await;

    // Персональная квота 1 КиБ.
    quota::set_user_quota(&env.pool, user, Some(1024))
        .await
        .unwrap();

    let error = upload(&env, user, "big.bin", 4096)
        .await
        .expect_err("превышение квоты");
    assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error.code, "QUOTA_EXCEEDED");
    let details = error.details().cloned().unwrap();
    assert_eq!(details["limitBytes"], 1024);
    assert_eq!(details["usedBytes"], 0);
    assert_eq!(details["requestedBytes"], 4096);

    // В пределах квоты загрузка проходит.
    assert!(upload(&env, user, "small.bin", 512).await.is_ok());
}

#[tokio::test]
async fn quota_change_applies_without_restart() {
    let env = TestEnv::new().await;
    env.add_volume("quota2-vol").await;
    let user = env.create_user("quotachange", "SecretPass1!").await;
    quota::set_user_quota(&env.pool, user, Some(2048))
        .await
        .unwrap();

    assert!(upload(&env, user, "a.bin", 1024).await.is_ok());
    // Поднимаем квоту — вторая загрузка проходит.
    quota::set_user_quota(&env.pool, user, Some(8192))
        .await
        .unwrap();
    assert!(upload(&env, user, "b.bin", 2048).await.is_ok());
}

#[tokio::test]
async fn null_quota_uses_default() {
    let env = TestEnv::new().await;
    let user = env.create_user("quotanull", "SecretPass1!").await;
    let effective = quota::effective_quota(&env.pool, user, quota::Scope::Cloud)
        .await
        .unwrap();
    let default = quota::default_quota(&env.pool, quota::Scope::Cloud)
        .await
        .unwrap();
    assert_eq!(effective, default);
    assert!(effective > 0);
}

#[tokio::test]
async fn deleting_file_frees_quota() {
    let env = TestEnv::new().await;
    env.add_volume("quota3-vol").await;
    let user = env.create_user("quotafree", "SecretPass1!").await;
    quota::set_user_quota(&env.pool, user, Some(1024))
        .await
        .unwrap();

    let entry_id = upload(&env, user, "f.bin", 800).await.unwrap();
    let used = quota::used_bytes(&env.pool, user).await.unwrap();
    assert_eq!(used, 800);

    // Загрузка сверх оставшихся 224 байт отклоняется.
    assert!(upload(&env, user, "g.bin", 400).await.is_err());

    nexus::cloud::service::trash_entry(&env.pool, user, entry_id)
        .await
        .unwrap();
    nexus::cloud::service::purge_entry(&env.pool, user, entry_id)
        .await
        .unwrap();

    let used = quota::used_bytes(&env.pool, user).await.unwrap();
    assert_eq!(used, 0);
    assert!(upload(&env, user, "h.bin", 900).await.is_ok());
}

#[tokio::test]
async fn replacement_counts_only_delta() {
    let env = TestEnv::new().await;
    env.add_volume("quota4-vol").await;
    let user = env.create_user("quotadelta", "SecretPass1!").await;
    quota::set_user_quota(&env.pool, user, Some(1000))
        .await
        .unwrap();

    let _ = upload(&env, user, "same.bin", 900).await.unwrap();

    // Полный размер 950 не помещается (900 + 950 > 1000), но замена
    // учитывает только прирост 50 байт и проходит.
    let session = uploads::create_upload(
        &env.state,
        user,
        uploads::CreateUploadRequest {
            name: "same.bin".to_owned(),
            parent_id: None,
            size: 950,
            content_type: "application/octet-stream".to_owned(),
        },
    )
    .await
    .expect("дельта укладывается в квоту");
    uploads::append_stream(&env.state, session.id, user, 0, Body::from(vec![1u8; 950]))
        .await
        .unwrap();
    let entry = uploads::complete_upload(
        &env.state,
        session.id,
        user,
        uploads::CompleteUploadRequest {
            strategy: "replace".to_owned(),
            new_name: None,
        },
    )
    .await
    .expect("замена проходит");
    assert_eq!(entry.size_bytes, 950);
    assert_eq!(quota::used_bytes(&env.pool, user).await.unwrap(), 950);
}
