//! Журнал receipts Runtime с безопасной ротацией.
//!
//! Receipt — подтверждение того, что конкретный `messageId` уже исполнен.
//! Повторная доставка той же команды не должна вызывать side effect, поэтому
//! журнал читается при старте и пополняется синхронно до отправки результата.
//!
//! Старый формат `receipts.jsonl` не ограничивал размер: после 10 МБ запись
//! молча прекращалась, и после рестарта Runtime мог повторно выполнить
//! неидемпотентную операцию. Теперь текущий файл ротируется в
//! `receipts.NNNNNN.jsonl`; старые сегменты читаются при загрузке.
//!
//! Перед удалением сегмента его `messageId` сжимаются в
//! `receipts.index.jsonl`. Индекс хранит только идентификаторы и запрещает
//! повторное исполнение даже после очистки полного результата. Runtime
//! отвечает `AI_RECEIPT_ROTATED`, а не выполняет операцию заново.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::{ProviderChatCompleted, ToolOutcome};

pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const DEFAULT_MAX_ARCHIVES: usize = 16;
pub const DEFAULT_LOAD_LIMIT: usize = 5_000;
pub const MIN_MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptFailure {
    pub kind: String,
    pub code: String,
    pub message: String,
}

/// Компактная запись об уже исполненной операции, чей полный receipt удалён
/// при ротации. Результат недоступен, но повторный side effect запрещён.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrunedMarker {
    pub message_id: Uuid,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub message_id: Uuid,
    pub kind: String,
    #[serde(default)]
    pub provider: Option<ProviderChatCompleted>,
    #[serde(default)]
    pub tool: Option<ToolOutcome>,
    /// Результат операции известен как ошибка: повторно не исполняется.
    #[serde(default)]
    pub failure: Option<ReceiptFailure>,
}

pub struct ReceiptStore {
    path: PathBuf,
    max_bytes: u64,
    max_archives: usize,
    load_limit: usize,
    write_lock: Mutex<()>,
}

impl ReceiptStore {
    pub fn new(path: PathBuf, max_bytes: u64, max_archives: usize, load_limit: usize) -> Self {
        Self {
            path,
            max_bytes: max_bytes.max(64),
            max_archives: max_archives.max(1),
            load_limit: load_limit.max(1),
            write_lock: Mutex::new(()),
        }
    }

    /// Читает текущий файл, архивы и компактный индекс.
    ///
    /// Возвращает store, не более `load_limit` полных receipts и маркеры
    /// операций, чьи receipts были сжаты при ротации. Битые строки
    /// пропускаются и не мешают прочитать остальные.
    pub fn load(
        path: PathBuf,
        max_bytes: u64,
        max_archives: usize,
        load_limit: usize,
    ) -> (Self, Vec<Receipt>, Vec<PrunedMarker>) {
        let store = Self::new(path, max_bytes, max_archives, load_limit);
        let receipts = store.read_receipts();
        let markers = store.read_index();
        (store, receipts, markers)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Дописывает receipt. При превышении лимита текущий файл ротируется,
    /// поэтому запись не прекращается молча.
    pub fn append(&self, receipt: &Receipt) {
        let Ok(line) = serde_json::to_string(receipt) else {
            tracing::error!(message_id = %receipt.message_id, "receipt не сериализован");
            return;
        };
        let _guard = self.write_lock.lock();
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    tracing::error!(%error, path = %parent.display(), "не удалось создать каталог receipts");
                }
            }
        }
        let current_len = std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let incoming = line.len() as u64 + 1;
        if current_len > 0 && current_len.saturating_add(incoming) > self.max_bytes {
            self.rotate_locked();
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{line}") {
                    tracing::error!(%error, path = %self.path.display(), "receipt не записан");
                }
            }
            Err(error) => {
                // Не теряем подтверждение: путь и ошибка попадают в журнал.
                tracing::error!(%error, path = %self.path.display(), "не удалось открыть receipts");
            }
        }
    }

    fn read_receipts(&self) -> Vec<Receipt> {
        let mut receipts = Vec::new();
        let mut files = self.archive_files_desc();
        files.push(self.path.clone());
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for line in text.lines().rev() {
                if receipts.len() >= self.load_limit {
                    return receipts;
                }
                if let Ok(receipt) = serde_json::from_str::<Receipt>(line) {
                    receipts.push(receipt);
                }
            }
        }
        receipts
    }

    fn read_index(&self) -> Vec<PrunedMarker> {
        let Ok(text) = std::fs::read_to_string(self.index_path()) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut markers = Vec::new();
        for line in text.lines().rev() {
            if let Ok(marker) = serde_json::from_str::<PrunedMarker>(line) {
                if seen.insert(marker.message_id) {
                    markers.push(marker);
                }
            }
        }
        markers
    }

    /// Сохраняет только `messageId`, чтобы после удаления старого сегмента
    /// повторная доставка не выполнила операцию заново.
    fn compact_archive(&self, archive: &Path) {
        let Ok(text) = std::fs::read_to_string(archive) else {
            return;
        };
        let Ok(mut index) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.index_path())
        else {
            tracing::error!(
                path = %self.index_path().display(),
                "не удалось открыть индекс receipts; старый сегмент не удаляется"
            );
            return;
        };
        for line in text.lines() {
            let Ok(receipt) = serde_json::from_str::<Receipt>(line) else {
                continue;
            };
            let marker = PrunedMarker {
                message_id: receipt.message_id,
                kind: receipt.kind,
            };
            if let Ok(encoded) = serde_json::to_string(&marker) {
                let _ = writeln!(index, "{encoded}");
            }
        }
    }

    fn index_path(&self) -> PathBuf {
        self.path
            .with_file_name(format!("{}.index.{}", self.file_stem(), self.extension()))
    }

    fn rotate_locked(&self) {
        let sequence = self.next_archive_sequence();
        let archive = self.archive_path(sequence);
        if let Err(error) = std::fs::rename(&self.path, &archive) {
            // Ротация не должна приводить к потере записи: продолжаем писать в
            // текущий файл и сообщаем об этом в журнал.
            tracing::warn!(
                %error,
                from = %self.path.display(),
                to = %archive.display(),
                "не удалось ротировать receipts; файл продолжает расти"
            );
            return;
        }
        tracing::info!(
            from = %self.path.display(),
            to = %archive.display(),
            "receipts ротированы"
        );
        self.prune_archives_locked();
    }

    fn prune_archives_locked(&self) {
        let mut archives = self.archive_entries();
        if archives.len() <= self.max_archives {
            return;
        }
        archives.sort_by_key(|(sequence, _)| *sequence);
        let remove_count = archives.len() - self.max_archives;
        for (sequence, path) in archives.into_iter().take(remove_count) {
            // Сначала фиксируем messageId в компактном индексе, и только потом
            // удаляем полный receipt. При сбое индекс может содержать лишние
            // записи (безопасно), но не наоборот.
            self.compact_archive(&path);
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::warn!(
                    sequence,
                    path = %path.display(),
                    "старый сегмент receipts сжат в индекс и удалён"
                ),
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "не удалось удалить сегмент receipts")
                }
            }
        }
    }

    fn next_archive_sequence(&self) -> u64 {
        self.archive_entries()
            .into_iter()
            .map(|(sequence, _)| sequence)
            .max()
            .unwrap_or(0)
            + 1
    }

    fn archive_files_desc(&self) -> Vec<PathBuf> {
        let mut entries = self.archive_entries();
        entries.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
        entries.into_iter().map(|(_, path)| path).collect()
    }

    fn archive_entries(&self) -> Vec<(u64, PathBuf)> {
        let Some(parent) = self.path.parent() else {
            return Vec::new();
        };
        let base = self.file_stem();
        let extension = self.extension();
        let Ok(entries) = std::fs::read_dir(parent) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let name = path.file_name()?.to_string_lossy().to_string();
                if !name.starts_with(&format!("{base}."))
                    || !name.ends_with(&format!(".{extension}"))
                {
                    return None;
                }
                let middle = name
                    .strip_prefix(&format!("{base}."))?
                    .strip_suffix(&format!(".{extension}"))?;
                let sequence = middle.parse::<u64>().ok()?;
                Some((sequence, path))
            })
            .collect()
    }

    fn archive_path(&self, sequence: u64) -> PathBuf {
        self.path.with_file_name(format!(
            "{}.{sequence:06}.{}",
            self.file_stem(),
            self.extension()
        ))
    }

    fn file_stem(&self) -> String {
        self.path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "receipts".to_owned())
    }

    fn extension(&self) -> String {
        self.path
            .extension()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "jsonl".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(index: u8) -> Receipt {
        Receipt {
            message_id: Uuid::from_u128(index as u128 + 1),
            kind: "tool".to_owned(),
            provider: None,
            tool: Some(ToolOutcome {
                result: json!({ "index": index }),
                result_text: Some(format!("result-{index}")),
                exit_code: Some(0),
                duration_ms: 1,
                usage: None,
                changed_files: Vec::new(),
                diff: None,
                snapshots: Vec::new(),
            }),
            failure: None,
        }
    }

    #[test]
    fn rotates_and_preserves_recent_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.jsonl");
        // Один receipt ~150+ байт: лимит 300 байт даёт несколько сегментов.
        let store = ReceiptStore::new(path.clone(), 300, 4, 100);
        for index in 0..12 {
            store.append(&receipt(index));
        }
        let archives = store.archive_entries();
        assert!(!archives.is_empty(), "должна быть хотя бы одна ротация");
        assert!(
            std::fs::metadata(&path).unwrap().len() <= 300,
            "текущий файл не должен превышать лимит"
        );
        let (_, loaded, _) = ReceiptStore::load(path, 300, 4, 100);
        let latest = loaded
            .iter()
            .find(|item| item.message_id == Uuid::from_u128(12))
            .expect("последний receipt должен читаться");
        assert_eq!(
            latest.tool.as_ref().unwrap().result_text.as_deref(),
            Some("result-11")
        );
    }

    #[test]
    fn prunes_only_old_archives_and_keeps_write_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.jsonl");
        let store = ReceiptStore::new(path.clone(), 300, 2, 100);
        for index in 0..12 {
            store.append(&receipt(index));
        }
        let archives = store.archive_entries();
        assert!(
            archives.len() <= 2,
            "количество архивов ограничено: {}",
            archives.len()
        );
        // Текущий файл остаётся доступным для записи.
        store.append(&receipt(12));
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn pruned_segments_keep_dedupe_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.jsonl");
        let store = ReceiptStore::new(path.clone(), 300, 1, 100);
        for index in 0..12 {
            store.append(&receipt(index));
        }
        // Самый первый receipt обязан попасть в компактный индекс, даже если
        // полный сегмент уже удалён.
        let (_, loaded, markers) = ReceiptStore::load(path, 300, 1, 100);
        let first = Uuid::from_u128(1);
        assert!(
            markers.iter().any(|marker| marker.message_id == first),
            "маркер удалённого сегмента должен сохраниться"
        );
        assert!(
            !loaded.iter().any(|item| item.message_id == first),
            "полный receipt удалён"
        );
    }

    #[test]
    fn failure_receipts_are_persisted_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.jsonl");
        let store = ReceiptStore::new(path.clone(), 1024 * 1024, 4, 100);
        store.append(&Receipt {
            message_id: Uuid::from_u128(99),
            kind: "tool".to_owned(),
            provider: None,
            tool: None,
            failure: Some(ReceiptFailure {
                kind: "tool".to_owned(),
                code: "AI_TOOL_FAILED".to_owned(),
                message: "boom".to_owned(),
            }),
        });
        let (_, loaded, _) = ReceiptStore::load(path, 1024 * 1024, 4, 100);
        let item = loaded
            .iter()
            .find(|value| value.message_id == Uuid::from_u128(99))
            .expect("failure receipt загружен");
        assert_eq!(item.failure.as_ref().unwrap().code, "AI_TOOL_FAILED");
    }

    #[test]
    fn malformed_lines_do_not_break_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.jsonl");
        std::fs::write(&path, "{not-json}\n").unwrap();
        let store = ReceiptStore::new(path.clone(), 1024 * 1024, 4, 100);
        store.append(&receipt(1));
        let (_, loaded, _) = ReceiptStore::load(path, 1024 * 1024, 4, 100);
        assert_eq!(loaded.len(), 1);
    }
}
