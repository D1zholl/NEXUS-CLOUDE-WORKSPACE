-- Download engine state.
--
-- The queue was already described by `download_tasks` / `download_files`.
-- This migration adds the fields needed for crash-safe resume, remote-version
-- detection, verification and an error history. It is additive: no existing
-- data is rewritten or removed.

ALTER TABLE download_tasks ADD COLUMN error_message TEXT;

ALTER TABLE download_files ADD COLUMN last_modified TEXT;
ALTER TABLE download_files ADD COLUMN content_length INTEGER
    CHECK (content_length IS NULL OR content_length >= 0);
ALTER TABLE download_files ADD COLUMN staging_rel TEXT;
ALTER TABLE download_files ADD COLUMN resume_offset INTEGER NOT NULL DEFAULT 0
    CHECK (resume_offset >= 0);
ALTER TABLE download_files ADD COLUMN verified_at INTEGER;

CREATE TABLE download_errors (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL REFERENCES download_tasks(id) ON DELETE CASCADE,
    file_id     TEXT REFERENCES download_files(id) ON DELETE CASCADE,
    attempt     INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    code        TEXT NOT NULL CHECK (length(code) BETWEEN 1 AND 64),
    message     TEXT NOT NULL CHECK (length(message) BETWEEN 0 AND 1024),
    http_status INTEGER CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 599),
    retryable   INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1)),
    at          INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_download_errors_task
    ON download_errors(task_id, at DESC);

CREATE INDEX idx_download_files_staging
    ON download_files(task_id, status)
    WHERE staging_rel IS NOT NULL;
