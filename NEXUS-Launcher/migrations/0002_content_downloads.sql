-- Content inventory, download queue, backups and activity journal.

CREATE TABLE instance_mods (
    id            TEXT PRIMARY KEY,
    instance_id   TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL
                  CHECK (provider IN ('modrinth', 'curseforge', 'nexus', 'local')),
    project_id    TEXT NOT NULL CHECK (length(project_id) BETWEEN 1 AND 128),
    version_id    TEXT NOT NULL CHECK (length(version_id) BETWEEN 1 AND 128),
    project_name  TEXT NOT NULL DEFAULT '',
    version_name  TEXT NOT NULL DEFAULT '',
    file_name     TEXT NOT NULL CHECK (length(file_name) BETWEEN 1 AND 255),
    -- Path inside the instance `game/` directory, using '/' separators.
    rel_path      TEXT NOT NULL CHECK (length(rel_path) BETWEEN 1 AND 1024),
    path_key      TEXT NOT NULL CHECK (length(path_key) BETWEEN 1 AND 1024),
    side          TEXT NOT NULL DEFAULT 'client'
                  CHECK (side IN ('client', 'server', 'both')),
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    managed       INTEGER NOT NULL DEFAULT 1 CHECK (managed IN (0, 1)),
    size_bytes    INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256        TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    installed_at  INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    deleted_at    INTEGER
) STRICT;

CREATE UNIQUE INDEX ux_instance_mods_path_active
    ON instance_mods(instance_id, path_key)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_instance_mods_provider
    ON instance_mods(provider, project_id)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_instance_mods_instance
    ON instance_mods(instance_id, enabled, rel_path)
    WHERE deleted_at IS NULL;

CREATE TABLE download_tasks (
    id              TEXT PRIMARY KEY,
    job_id          TEXT,
    kind            TEXT NOT NULL
                    CHECK (kind IN ('minecraft', 'java', 'loader', 'mod', 'modpack',
                                    'nexus', 'server', 'custom')),
    label           TEXT NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 256),
    status          TEXT NOT NULL
                    CHECK (status IN ('queued', 'running', 'paused', 'completed',
                                      'failed', 'cancelled')),
    priority        INTEGER NOT NULL DEFAULT 100,
    total_files     INTEGER NOT NULL DEFAULT 0 CHECK (total_files >= 0),
    completed_files INTEGER NOT NULL DEFAULT 0 CHECK (completed_files >= 0),
    total_bytes     INTEGER NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    done_bytes      INTEGER NOT NULL DEFAULT 0 CHECK (done_bytes >= 0),
    error_code      TEXT,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    finished_at     INTEGER,
    updated_at      INTEGER NOT NULL,
    CHECK (completed_files <= total_files),
    CHECK (done_bytes <= total_bytes OR total_bytes = 0)
) STRICT;

CREATE INDEX idx_download_tasks_queue
    ON download_tasks(status, priority, created_at);
CREATE INDEX idx_download_tasks_job
    ON download_tasks(job_id);

CREATE TABLE download_files (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES download_tasks(id) ON DELETE CASCADE,
    url              TEXT NOT NULL CHECK (length(url) BETWEEN 1 AND 4096),
    root_kind        TEXT NOT NULL
                     CHECK (root_kind IN ('shared_assets', 'shared_libraries',
                                          'shared_versions', 'instance', 'server',
                                          'java', 'cache', 'downloads')),
    rel_path         TEXT NOT NULL CHECK (length(rel_path) BETWEEN 1 AND 4096),
    path_key         TEXT NOT NULL CHECK (length(path_key) BETWEEN 1 AND 4096),
    size_bytes       INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha1             TEXT CHECK (sha1 IS NULL OR length(sha1) = 40),
    sha256           TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    required         INTEGER NOT NULL DEFAULT 1 CHECK (required IN (0, 1)),
    status           TEXT NOT NULL
                     CHECK (status IN ('queued', 'running', 'paused', 'completed',
                                       'failed', 'skipped')),
    bytes_done       INTEGER NOT NULL DEFAULT 0 CHECK (bytes_done >= 0),
    attempts         INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    etag             TEXT,
    last_http_status INTEGER,
    error_code       TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    CHECK (bytes_done <= size_bytes)
) STRICT;

CREATE UNIQUE INDEX ux_download_files_task_path
    ON download_files(task_id, path_key);
CREATE INDEX idx_download_files_task
    ON download_files(task_id, status);

CREATE TABLE backups (
    id          TEXT PRIMARY KEY,
    target_kind TEXT NOT NULL
                CHECK (target_kind IN ('instance', 'server', 'world')),
    target_id   TEXT NOT NULL CHECK (length(target_id) BETWEEN 1 AND 128),
    target_name TEXT NOT NULL CHECK (length(target_name) BETWEEN 0 AND 256),
    kind        TEXT NOT NULL CHECK (kind IN ('full', 'selected')),
    file_path   TEXT NOT NULL UNIQUE CHECK (length(file_path) BETWEEN 1 AND 4096),
    size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256      TEXT NOT NULL CHECK (length(sha256) = 64),
    status      TEXT NOT NULL
                CHECK (status IN ('creating', 'ready', 'failed', 'restored')),
    verified_at INTEGER,
    note        TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
) STRICT;

CREATE INDEX idx_backups_target
    ON backups(target_kind, target_id, created_at DESC)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_backups_status
    ON backups(status, created_at)
    WHERE deleted_at IS NULL;

CREATE TABLE activity_logs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    at           INTEGER NOT NULL,
    kind         TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 64),
    entity_kind  TEXT,
    entity_id    TEXT,
    summary      TEXT NOT NULL CHECK (length(summary) BETWEEN 0 AND 512),
    details_json TEXT CHECK (details_json IS NULL OR json_valid(details_json)),
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_activity_logs_at
    ON activity_logs(at DESC);
CREATE INDEX idx_activity_logs_entity
    ON activity_logs(entity_kind, entity_id, at DESC);
