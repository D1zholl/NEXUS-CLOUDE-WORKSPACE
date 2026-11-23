-- Operational indexes for startup recovery and list queries.
--
-- SQLx records applied migration versions in `_sqlx_migrations`. This file is
-- intentionally additive: it never drops or rewrites user data.

CREATE INDEX idx_instances_updated
    ON instances(updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX idx_instances_archived
    ON instances(archived_at, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX idx_servers_updated
    ON servers(updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX idx_accounts_provider
    ON accounts(provider, status)
    WHERE deleted_at IS NULL;

CREATE INDEX idx_java_runtimes_source
    ON java_runtimes(source, major)
    WHERE deleted_at IS NULL;

CREATE INDEX idx_download_tasks_updated
    ON download_tasks(updated_at DESC);

CREATE INDEX idx_activity_logs_kind
    ON activity_logs(kind, at DESC);

-- A helper view for diagnostics. The view is read-only and never exposes
-- credentials: `secret_ref` is a reference, not a token.
CREATE VIEW v_accounts_safe AS
SELECT
    id,
    provider,
    provider_account_id,
    username,
    uuid,
    display_name,
    status,
    (secret_ref IS NOT NULL) AS has_secret,
    last_used_at,
    created_at,
    updated_at,
    deleted_at
FROM accounts;
