//! Live progress accounting.
//!
//! Progress is derived exclusively from bytes that were actually written to a
//! staging file. There is no timer-based or synthetic interpolation. Speed is
//! computed from a short sliding window of real byte samples, and ETA is only
//! reported when the total size is known.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use super::store::FileRow;
use super::types::{FileProgressEntry, FileStatus, TaskProgress};

const SAMPLE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
const SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
const IDLE_AFTER: std::time::Duration = std::time::Duration::from_millis(1_200);

#[derive(Debug)]
pub(crate) struct FileMeter {
    pub file_id: String,
    pub relative_path: String,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    total_known: AtomicBool,
    status: AtomicU8,
    samples: Mutex<VecDeque<(Instant, u64)>>,
}

impl FileMeter {
    pub fn from_row(row: &FileRow) -> Self {
        let status = FileStatus::parse_db(&row.status).unwrap_or(FileStatus::Queued);
        let size = row.size_bytes_u64();
        let known = size > 0 || status.is_done();
        Self {
            file_id: row.id.clone(),
            relative_path: row.rel_path.clone(),
            bytes_done: AtomicU64::new(row.bytes_done_u64()),
            bytes_total: AtomicU64::new(size),
            total_known: AtomicBool::new(known),
            status: AtomicU8::new(status_code(status)),
            samples: Mutex::new(VecDeque::new()),
        }
    }

    pub fn status(&self) -> FileStatus {
        status_from_code(self.status.load(Ordering::Acquire))
    }

    pub fn set_status(&self, status: FileStatus) {
        self.status.store(status_code(status), Ordering::Release);
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_done.load(Ordering::Acquire)
    }

    pub fn bytes_total(&self) -> u64 {
        self.bytes_total.load(Ordering::Acquire)
    }

    pub fn total_known(&self) -> bool {
        self.total_known.load(Ordering::Acquire)
    }

    pub fn set_total(&self, total: u64) {
        self.bytes_total.store(total, Ordering::Release);
        self.total_known.store(true, Ordering::Release);
    }

    pub fn set_done(&self, bytes: u64) {
        self.bytes_done.store(bytes, Ordering::Release);
    }

    /// Records bytes written by the stream.
    pub fn add(&self, bytes: u64) {
        let total = self.bytes_done.fetch_add(bytes, Ordering::AcqRel) + bytes;
        let mut samples = lock_unpoisoned(&self.samples);
        let now = Instant::now();
        let should_push = samples
            .back()
            .map(|(at, _)| now.saturating_duration_since(*at) >= SAMPLE_INTERVAL)
            .unwrap_or(true);
        if should_push {
            samples.push_back((now, total));
            prune(&mut samples, now);
        }
    }

    pub fn speed_bps(&self, now: Instant) -> u64 {
        let mut samples = lock_unpoisoned(&self.samples);
        prune(&mut samples, now);
        speed_from(&samples, now)
    }

    pub fn eta_seconds(&self, now: Instant) -> Option<u64> {
        if !self.total_known() || self.status() != FileStatus::Running {
            return None;
        }
        let done = self.bytes_done();
        let total = self.bytes_total();
        if total <= done {
            return None;
        }
        eta(total - done, self.speed_bps(now))
    }
}

#[derive(Debug)]
pub(crate) struct TaskMeter {
    files: Vec<std::sync::Arc<FileMeter>>,
    session_bytes: AtomicU64,
    samples: Mutex<VecDeque<(Instant, u64)>>,
}

impl TaskMeter {
    pub fn from_files(rows: &[FileRow]) -> (std::sync::Arc<Self>, Vec<std::sync::Arc<FileMeter>>) {
        let files: Vec<_> = rows
            .iter()
            .map(|row| std::sync::Arc::new(FileMeter::from_row(row)))
            .collect();
        let meter = std::sync::Arc::new(Self {
            files: files.clone(),
            session_bytes: AtomicU64::new(0),
            samples: Mutex::new(VecDeque::new()),
        });
        (meter, files)
    }

    pub fn files(&self) -> &[std::sync::Arc<FileMeter>] {
        &self.files
    }

    pub fn file(&self, file_id: &str) -> Option<&std::sync::Arc<FileMeter>> {
        self.files.iter().find(|file| file.file_id == file_id)
    }

    pub fn note_session_bytes(&self, bytes: u64) {
        self.session_bytes.fetch_add(bytes, Ordering::AcqRel);
    }

    pub fn session_bytes(&self) -> u64 {
        self.session_bytes.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> TaskProgress {
        self.snapshot_at(Instant::now())
    }

    pub fn snapshot_at(&self, now: Instant) -> TaskProgress {
        let bytes_done: u64 = self.files.iter().map(|file| file.bytes_done()).sum();
        {
            let mut samples = lock_unpoisoned(&self.samples);
            let should_push = samples
                .back()
                .map(|(at, _)| now.saturating_duration_since(*at) >= SAMPLE_INTERVAL)
                .unwrap_or(true);
            if should_push {
                samples.push_back((now, bytes_done));
                prune(&mut samples, now);
            }
        }
        let speed_bps = {
            let samples = lock_unpoisoned(&self.samples);
            speed_from(&samples, now)
        };

        let bytes_total: u64 = self.files.iter().map(|file| file.bytes_total()).sum();
        let bytes_total_known = self.files.iter().all(|file| file.total_known());
        let files_done = self
            .files
            .iter()
            .filter(|file| file.status().is_done())
            .count() as u64;
        let eta_seconds = if bytes_total_known {
            eta(bytes_total.saturating_sub(bytes_done), speed_bps)
        } else {
            None
        };

        let files = self
            .files
            .iter()
            .map(|file| FileProgressEntry {
                file_id: file.file_id.clone(),
                relative_path: file.relative_path.clone(),
                status: file.status(),
                bytes_done: file.bytes_done(),
                bytes_total: file.bytes_total(),
                speed_bps: file.speed_bps(now),
                eta_seconds: file.eta_seconds(now),
            })
            .collect();

        TaskProgress {
            files_done,
            files_total: self.files.len() as u64,
            bytes_done,
            bytes_total,
            bytes_total_known,
            speed_bps,
            eta_seconds,
            files,
        }
    }
}

fn speed_from(samples: &VecDeque<(Instant, u64)>, now: Instant) -> u64 {
    if samples.len() < 2 {
        return 0;
    }
    let Some((last_at, last_bytes)) = samples.back() else {
        return 0;
    };
    if now.saturating_duration_since(*last_at) > IDLE_AFTER {
        return 0;
    }
    let Some((first_at, first_bytes)) = samples.front() else {
        return 0;
    };
    let elapsed = last_at.saturating_duration_since(*first_at).as_secs_f64();
    if elapsed <= 0.0 || last_bytes <= first_bytes {
        return 0;
    }
    let speed = (*last_bytes - *first_bytes) as f64 / elapsed;
    speed.round() as u64
}

fn eta(remaining: u64, speed_bps: u64) -> Option<u64> {
    if remaining == 0 {
        return Some(0);
    }
    if speed_bps == 0 {
        return None;
    }
    Some(remaining.div_ceil(speed_bps))
}

fn prune(samples: &mut VecDeque<(Instant, u64)>, now: Instant) {
    while samples
        .front()
        .map(|(at, _)| now.saturating_duration_since(*at) > SAMPLE_WINDOW)
        .unwrap_or(false)
    {
        samples.pop_front();
    }
    while samples.len() > 64 {
        samples.pop_front();
    }
}

fn status_code(status: FileStatus) -> u8 {
    match status {
        FileStatus::Queued => 0,
        FileStatus::Running => 1,
        FileStatus::Paused => 2,
        FileStatus::Completed => 3,
        FileStatus::Failed => 4,
        FileStatus::Skipped => 5,
    }
}

fn status_from_code(code: u8) -> FileStatus {
    match code {
        1 => FileStatus::Running,
        2 => FileStatus::Paused,
        3 => FileStatus::Completed,
        4 => FileStatus::Failed,
        5 => FileStatus::Skipped,
        _ => FileStatus::Queued,
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_is_only_reported_with_real_speed() {
        assert_eq!(eta(0, 0), Some(0));
        assert_eq!(eta(100, 0), None);
        assert_eq!(eta(100, 10), Some(10));
    }
}
