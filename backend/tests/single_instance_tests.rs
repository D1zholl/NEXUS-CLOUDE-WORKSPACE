//! Защита от второго экземпляра: PostgreSQL advisory lock.

mod common;

use common::TestEnv;

const LOCK_SQL: &str = "select pg_try_advisory_lock(hashtext('nexus.instance'))";
const UNLOCK_SQL: &str = "select pg_advisory_unlock(hashtext('nexus.instance'))";

#[tokio::test]
async fn advisory_lock_allows_only_one_instance() {
    let env = TestEnv::new().await;
    let mut first = env.pool.acquire().await.expect("первое соединение");
    let mut second = env.pool.acquire().await.expect("второе соединение");

    let acquired: bool = sqlx::query_scalar(LOCK_SQL)
        .fetch_one(&mut *first)
        .await
        .expect("первая блокировка");
    assert!(acquired, "первый экземпляр получает блокировку");

    let denied: bool = sqlx::query_scalar(LOCK_SQL)
        .fetch_one(&mut *second)
        .await
        .expect("вторая блокировка");
    assert!(!denied, "второй экземпляр блокировку не получает");

    let released: bool = sqlx::query_scalar(UNLOCK_SQL)
        .fetch_one(&mut *first)
        .await
        .expect("снятие блокировки");
    assert!(released);

    let after: bool = sqlx::query_scalar(LOCK_SQL)
        .fetch_one(&mut *second)
        .await
        .expect("блокировка после остановки первого");
    assert!(after, "после остановки первого блокировка освобождается");

    let _: bool = sqlx::query_scalar(UNLOCK_SQL)
        .fetch_one(&mut *second)
        .await
        .expect("финальное снятие");
}
