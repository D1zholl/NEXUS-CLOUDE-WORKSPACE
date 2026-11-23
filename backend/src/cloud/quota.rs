//! Совместимый реэкспорт: квоты перенесены в [`crate::storage::quota`],
//! чтобы `minecraft` не зависел от Cloud Storage.
pub use crate::storage::quota::*;
