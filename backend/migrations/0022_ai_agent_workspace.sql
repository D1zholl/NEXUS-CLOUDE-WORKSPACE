-- NEXUS AI Agent Workspace: пользовательские workspace, снимки файлов агента.
--
-- Миграция аддитивная: не изменяет существующие строки, кроме безопасного
-- добавления колонок со значениями по умолчанию.

alter table ai_workspaces add column if not exists description text not null default '';
alter table ai_workspaces add column if not exists deleted_at timestamptz;

create index if not exists ai_workspaces_owner_deleted_idx
    on ai_workspaces (owner_id, deleted_at);

-- Снимок файла до/после операции Runtime. Используется для diff и безопасного
-- отката изменений Agent Mode. Содержимое ограничено по размеру Runtime;
-- truncated=true означает, что откат этого файла недоступен.
create table if not exists ai_file_snapshots (
    id uuid primary key default gen_random_uuid(),
    workspace_id uuid not null references ai_workspaces(id) on delete cascade,
    owner_id uuid not null references users(id) on delete cascade,
    task_id uuid references ai_tasks(id) on delete cascade,
    execution_id uuid references ai_tool_executions(id) on delete cascade,
    path text not null,
    action text not null default 'modify',
    before_sha256 text,
    after_sha256 text,
    before_content text,
    after_content text,
    before_encoding text not null default 'utf8',
    after_encoding text not null default 'utf8',
    truncated boolean not null default false,
    additions integer not null default 0,
    deletions integer not null default 0,
    restored_at timestamptz,
    created_at timestamptz not null default now()
);

create index if not exists ai_file_snapshots_workspace_idx
    on ai_file_snapshots (workspace_id, created_at desc);
create index if not exists ai_file_snapshots_task_idx
    on ai_file_snapshots (task_id, created_at);
create index if not exists ai_file_snapshots_path_idx
    on ai_file_snapshots (workspace_id, path, created_at desc);
