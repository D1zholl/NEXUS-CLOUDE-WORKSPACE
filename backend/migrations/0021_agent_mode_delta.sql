-- NEXUS Agent Mode — аддитивные изменения схемы (delta).
--
-- Миграция НЕ применяется автоматически на рабочей БД в рамках аудита/разработки.
-- Перед применением администратор обязан проверить дубли (SQL ниже) и принять
-- решение об их устранении вручную: автоматическое удаление/слияние запрещено.
--
-- Проверка дублей root_path:
--   select runtime_node_id, lower(root_path), count(*)
--     from ai_workspaces group by 1, 2 having count(*) > 1;
-- Проверка нескольких активных задач в одном диалоге:
--   select conversation_id, count(*) from ai_tasks
--    where status in ('queued','running','waiting_approval')
--    group by 1 having count(*) > 1;

-- Промежуточные сообщения агента: привязка к запуску и номеру цикла модели.
alter table ai_messages
    add column if not exists task_id uuid references ai_tasks(id) on delete set null;
alter table ai_messages
    add column if not exists iteration integer;

-- Выбранные пользователем Skills конкретного запуска.
alter table ai_tasks
    add column if not exists skill_ids uuid[] not null default '{}';

-- Административный лимит циклов модели (вместо жёсткой константы).
alter table ai_user_quotas
    add column if not exists max_iterations integer not null default 10
        check (max_iterations between 1 and 50);

-- Хвост вывода длительного процесса и его идентификатор для восстановления UI.
alter table ai_tool_executions
    add column if not exists output_tail text;
alter table ai_tool_executions
    add column if not exists process_id text;

-- Один каталог не может быть назначен двум workspace на одном Runtime.
create unique index if not exists ai_workspaces_node_root_uq
    on ai_workspaces (runtime_node_id, lower(root_path));

-- В одном диалоге допускается только один активный запуск агента.
create unique index if not exists ai_tasks_active_conversation_uq
    on ai_tasks (conversation_id)
    where status in ('queued', 'running', 'waiting_approval');
