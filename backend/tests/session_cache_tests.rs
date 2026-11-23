//! Кэш проверки сессии: hit/miss, TTL, явная инвалидация.

mod common;

use std::time::{Duration, Instant};

use common::TestEnv;
use nexus::auth::session::{create_session, hash_token, load_user_for_token};

async fn session_for(env: &TestEnv, user: uuid::Uuid) -> String {
    let (token, _) = create_session(
        &env.pool,
        user,
        env.state.session_ttl_hours().await,
        "127.0.0.1",
        "session-cache-test",
    )
    .await
    .expect("сессия создаётся");
    token
}

#[tokio::test]
async fn second_request_is_served_from_cache() {
    let env = TestEnv::new().await;
    let user = env.create_user("cache-hit", "SecretPass1!").await;
    let token = session_for(&env, user).await;

    let first = load_user_for_token(&env.state, &token)
        .await
        .expect("первый запрос");
    let (hits, misses) = env.state.session_cache_stats();
    assert_eq!((hits, misses), (0, 1));

    let second = load_user_for_token(&env.state, &token)
        .await
        .expect("второй запрос");
    let (hits, misses) = env.state.session_cache_stats();
    assert_eq!((hits, misses), (1, 1));
    assert_eq!(first.session_id, second.session_id);
}

#[tokio::test]
async fn stale_entry_is_reloaded_and_logout_invalidates() {
    let env = TestEnv::new().await;
    let user = env.create_user("cache-stale", "SecretPass1!").await;
    let token = session_for(&env, user).await;

    let auth = load_user_for_token(&env.state, &token)
        .await
        .expect("запрос");
    let key = hash_token(&token);
    if let Some(mut entry) = env.state.session_cache.get_mut(&key) {
        entry.cached_at = Instant::now() - Duration::from_secs(60);
    }
    load_user_for_token(&env.state, &token)
        .await
        .expect("после TTL сессия ещё действительна");
    let (_, misses) = env.state.session_cache_stats();
    assert_eq!(misses, 2, "просроченная запись перечитывается из БД");

    env.state.invalidate_session(auth.session_id);
    assert!(
        env.state.session_cache.get(&key).is_none(),
        "logout убирает запись из кэша"
    );

    sqlx::query("update sessions set revoked_at = now() where id = $1")
        .bind(auth.session_id)
        .execute(&env.pool)
        .await
        .expect("сессия отозвана");
    let error = match load_user_for_token(&env.state, &token).await {
        Ok(_) => panic!("отозванная сессия не должна обслуживаться"),
        Err(error) => error,
    };
    assert_eq!(error.code, "SESSION_EXPIRED");
}

#[tokio::test]
async fn permission_change_applies_after_invalidation() {
    let env = TestEnv::new().await;
    let user = env.create_user("cache-roles", "SecretPass1!").await;
    // create_user выдаёт administrator; для проверки изменения ролей начинаем
    // с пользователя без ролей.
    sqlx::query("delete from user_roles where user_id = $1")
        .bind(user)
        .execute(&env.pool)
        .await
        .expect("роли очищены");
    let token = session_for(&env, user).await;

    let before = load_user_for_token(&env.state, &token)
        .await
        .expect("запрос");
    let admin_role: uuid::Uuid =
        sqlx::query_scalar("select id from roles where key = 'administrator'")
            .fetch_one(&env.pool)
            .await
            .expect("роль administrator");
    sqlx::query("insert into user_roles (user_id, role_id) values ($1, $2) on conflict do nothing")
        .bind(user)
        .bind(admin_role)
        .execute(&env.pool)
        .await
        .expect("роль выдана");
    env.state.invalidate_all_sessions();

    let after = load_user_for_token(&env.state, &token)
        .await
        .expect("после инвалидации");
    assert!(after.roles.len() > before.roles.len());
    assert!(after.can("system.users.manage"), "права роли применились");
}
