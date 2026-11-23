-- NEXUS Launcher core schema.
--
-- Conventions:
--   * identifiers are UUID v4 strings in lower-case hyphenated form;
--   * timestamps are Unix milliseconds (UTC);
--   * every table is STRICT;
--   * secrets (tokens/passwords) are NEVER stored here. Only `secret_ref`
--     values that point to the Windows Credential Manager are allowed.

CREATE TABLE app_meta (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

INSERT INTO app_meta (key, value, created_at, updated_at) VALUES
    ('schema.name',            'nexus-launcher', CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000),
    ('schema.settings',        '1',              CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000),
    ('schema.instance',        '1',              CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000),
    ('schema.server',          '1',              CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000),
    ('db.clean_shutdown',      '1',              CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000),
    ('db.created_by',          '0.1.0',          CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

CREATE TABLE java_runtimes (
    id          TEXT PRIMARY KEY,
    vendor      TEXT NOT NULL,
    major       INTEGER NOT NULL CHECK (major BETWEEN 8 AND 40),
    arch        TEXT NOT NULL CHECK (arch IN ('x64', 'x86', 'arm64')),
    path        TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 32767),
    source      TEXT NOT NULL CHECK (source IN ('managed', 'system', 'custom')),
    sha256      TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    size_bytes  INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
    verified_at INTEGER,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
) STRICT;

CREATE UNIQUE INDEX ux_java_runtimes_path_active
    ON java_runtimes(path COLLATE NOCASE)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_java_runtimes_major_arch
    ON java_runtimes(major, arch)
    WHERE deleted_at IS NULL;

CREATE TABLE instances (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 96),
    description     TEXT NOT NULL DEFAULT '',
    dir_name        TEXT NOT NULL UNIQUE,
    game_version    TEXT NOT NULL CHECK (length(game_version) BETWEEN 1 AND 64),
    loader_type     TEXT NOT NULL
                    CHECK (loader_type IN ('vanilla', 'fabric', 'quilt', 'forge', 'neoforge')),
    loader_version  TEXT,
    java_runtime_id TEXT REFERENCES java_runtimes(id) ON DELETE SET NULL,
    memory_min_mb   INTEGER NOT NULL CHECK (memory_min_mb BETWEEN 512 AND 32768),
    memory_max_mb   INTEGER NOT NULL CHECK (memory_max_mb BETWEEN 512 AND 32768),
    icon_path       TEXT,
    source_kind     TEXT NOT NULL DEFAULT 'local'
                    CHECK (source_kind IN ('local', 'nexus', 'modpack')),
    source_id       TEXT,
    pack_id         TEXT,
    release         TEXT,
    install_state   TEXT NOT NULL DEFAULT 'pending'
                    CHECK (install_state IN ('pending', 'installed', 'failed')),
    archived_at     INTEGER,
    deleted_at      INTEGER,
    last_played_at  INTEGER,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    CHECK (memory_max_mb >= memory_min_mb),
    CHECK ((loader_type = 'vanilla' AND loader_version IS NULL)
        OR (loader_type <> 'vanilla' AND loader_version IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_instances_name_active
    ON instances(name COLLATE NOCASE)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_instances_list
    ON instances(deleted_at, archived_at, last_played_at DESC);
CREATE INDEX idx_instances_source
    ON instances(source_kind, source_id);

CREATE TABLE accounts (
    id                  TEXT PRIMARY KEY,
    provider            TEXT NOT NULL
                        CHECK (provider IN ('offline', 'microsoft', 'elyby', 'azuriom')),
    provider_account_id TEXT NOT NULL,
    username            TEXT NOT NULL CHECK (length(trim(username)) BETWEEN 1 AND 64),
    uuid                TEXT CHECK (uuid IS NULL OR length(uuid) = 36),
    display_name        TEXT,
    -- Reference into Windows Credential Manager. Never an actual token.
    secret_ref          TEXT CHECK (secret_ref IS NULL OR length(secret_ref) BETWEEN 1 AND 256),
    status              TEXT NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active', 'expired', 'error')),
    last_used_at        INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    deleted_at          INTEGER,
    CHECK ((provider = 'offline' AND secret_ref IS NULL)
        OR (provider <> 'offline'))
) STRICT;

CREATE UNIQUE INDEX ux_accounts_provider_account_active
    ON accounts(provider, provider_account_id)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX ux_accounts_secret_ref
    ON accounts(secret_ref)
    WHERE secret_ref IS NOT NULL;
CREATE INDEX idx_accounts_status
    ON accounts(status, last_used_at DESC)
    WHERE deleted_at IS NULL;

CREATE TABLE nexus_sources (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 64),
    base_url          TEXT NOT NULL CHECK (length(base_url) BETWEEN 1 AND 2048),
    enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    channel_default   TEXT NOT NULL DEFAULT 'stable'
                      CHECK (channel_default IN ('stable', 'beta')),
    auth_mode         TEXT NOT NULL DEFAULT 'public'
                      CHECK (auth_mode IN ('public', 'bearer')),
    secret_ref        TEXT,
    contract_version  TEXT NOT NULL DEFAULT '1.0.0',
    last_checked_at   INTEGER,
    last_status       TEXT CHECK (last_status IN ('ok', 'unreachable', 'contract_mismatch')),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    deleted_at        INTEGER
) STRICT;

CREATE UNIQUE INDEX ux_nexus_sources_base_url_active
    ON nexus_sources(base_url)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_nexus_sources_enabled
    ON nexus_sources(enabled, name)
    WHERE deleted_at IS NULL;

CREATE TABLE installed_builds (
    id              TEXT PRIMARY KEY,
    instance_id     TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    source_id       TEXT NOT NULL REFERENCES nexus_sources(id) ON DELETE RESTRICT,
    pack_id         TEXT NOT NULL CHECK (length(pack_id) BETWEEN 1 AND 128),
    release         TEXT NOT NULL CHECK (length(release) BETWEEN 1 AND 128),
    channel         TEXT NOT NULL CHECK (channel IN ('stable', 'beta')),
    manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
    installed_at    INTEGER NOT NULL,
    last_checked_at INTEGER,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    deleted_at      INTEGER
) STRICT;

CREATE UNIQUE INDEX ux_installed_builds_instance_active
    ON installed_builds(instance_id)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_installed_builds_source
    ON installed_builds(source_id, pack_id)
    WHERE deleted_at IS NULL;

CREATE TABLE servers (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 96),
    dir_name          TEXT NOT NULL UNIQUE,
    game_version      TEXT NOT NULL CHECK (length(game_version) BETWEEN 1 AND 64),
    loader_type       TEXT NOT NULL
                      CHECK (loader_type IN ('vanilla', 'fabric', 'quilt', 'forge', 'neoforge')),
    loader_version    TEXT,
    java_runtime_id   TEXT REFERENCES java_runtimes(id) ON DELETE SET NULL,
    memory_mb         INTEGER NOT NULL CHECK (memory_mb BETWEEN 512 AND 65536),
    port              INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    auto_restart_json TEXT NOT NULL DEFAULT '{}'
                      CHECK (json_valid(auto_restart_json)),
    eula_accepted_at  INTEGER,
    install_state     TEXT NOT NULL DEFAULT 'pending'
                      CHECK (install_state IN ('pending', 'installed', 'failed')),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    deleted_at        INTEGER,
    CHECK ((loader_type = 'vanilla' AND loader_version IS NULL)
        OR (loader_type <> 'vanilla' AND loader_version IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_servers_name_active
    ON servers(name COLLATE NOCASE)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_servers_install_state
    ON servers(install_state, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY CHECK (length(key) BETWEEN 1 AND 128),
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_app_settings_updated
    ON app_settings(updated_at);
