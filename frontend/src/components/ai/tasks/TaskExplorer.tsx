/** Список Agent-задач с фильтрами, пагинацией и выбором истории. */
import { useMemo } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { AlertTriangle, Clock, Loader2, Wrench } from 'lucide-react';

import { Badge } from '../../ui';
import { aiKeys } from '../../../lib/aiKeys';
import { formatDateTimeShort, relativeTime } from '../../../lib/format';
import { aiApi } from '../../../services/api';
import type { AiTask } from '../../../services/api';
import { errorMessage } from '../../../services/api';

export type TaskFilter = 'all' | 'running' | 'waiting_approval' | 'completed' | 'failed' | 'cancelled';

export const TASK_FILTERS: Array<{ value: TaskFilter; label: string; statuses?: string }> = [
  { value: 'all', label: 'Все' },
  { value: 'running', label: 'Выполняются', statuses: 'queued,running' },
  { value: 'waiting_approval', label: 'Ждут подтверждения', statuses: 'waiting_approval' },
  { value: 'completed', label: 'Завершены', statuses: 'completed' },
  { value: 'failed', label: 'Ошибки', statuses: 'failed' },
  { value: 'cancelled', label: 'Отменены', statuses: 'cancelled' },
];

function statusTone(status: string): 'ok' | 'warn' | 'danger' | 'info' | 'default' {
  switch (status) {
    case 'completed':
      return 'ok';
    case 'running':
    case 'queued':
      return 'info';
    case 'waiting_approval':
      return 'warn';
    case 'failed':
      return 'danger';
    default:
      return 'default';
  }
}

function titleOf(task: AiTask): string {
  const input = (task.humanInput ?? '').trim().replace(/\s+/g, ' ');
  if (input) return input.length > 90 ? `${input.slice(0, 90)}…` : input;
  if (task.statusText) return task.statusText;
  return `Задача ${task.id.slice(0, 8)}`;
}

interface TaskExplorerProps {
  workspaceId: string;
  selectedId: string | null;
  filter: TaskFilter;
  onFilter: (filter: TaskFilter) => void;
  onSelect: (task: AiTask) => void;
}

export function TaskExplorer({
  workspaceId,
  selectedId,
  filter,
  onFilter,
  onSelect,
}: TaskExplorerProps) {
  const statuses = TASK_FILTERS.find((item) => item.value === filter)?.statuses;
  const query = useInfiniteQuery({
    queryKey: aiKeys.tasks(workspaceId, filter),
    queryFn: ({ pageParam }) =>
      aiApi.tasks({
        workspaceId,
        statuses,
        page: pageParam,
        perPage: 20,
      }),
    initialPageParam: 1,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((total, page) => total + page.items.length, 0);
      return loaded < last.total ? pages.length + 1 : undefined;
    },
    refetchInterval: 8000,
    enabled: Boolean(workspaceId),
  });

  const items = useMemo(
    () => (query.data?.pages ?? []).flatMap((page) => page.items),
    [query.data],
  );

  return (
    <div className="task-explorer">
      <div className="task-explorer__filters">
        {TASK_FILTERS.map((item) => (
          <button
            key={item.value}
            type="button"
            className={filter === item.value ? 'is-active' : ''}
            onClick={() => onFilter(item.value)}
          >
            {item.label}
          </button>
        ))}
      </div>
      <div className="task-explorer__list">
        {query.isLoading ? (
          <div className="task-explorer__state">
            <Loader2 className="spin" aria-hidden="true" /> Загрузка задач…
          </div>
        ) : query.error ? (
          <div className="task-explorer__state task-explorer__state--error">
            <AlertTriangle aria-hidden="true" /> {errorMessage(query.error)}
          </div>
        ) : items.length === 0 ? (
          <div className="task-explorer__state muted">Задач с таким фильтром нет.</div>
        ) : (
          items.map((task) => (
            <button
              key={task.id}
              type="button"
              className={`task-item${task.id === selectedId ? ' is-active' : ''}`}
              onClick={() => onSelect(task)}
            >
              <span className="task-item__head">
                <Badge tone={statusTone(task.status)}>{task.status}</Badge>
                <span className="task-item__time">{relativeTime(task.createdAt)}</span>
              </span>
              <span className="task-item__title">{titleOf(task)}</span>
              <span className="task-item__meta">
                <span>
                  <Clock aria-hidden="true" /> {formatDateTimeShort(task.createdAt)}
                </span>
                <span>
                  <Wrench aria-hidden="true" /> {task.toolCallsCount}
                </span>
                {task.model ? <span className="task-item__model">{task.model}</span> : null}
              </span>
              {task.errorMessage ? (
                <span className="task-item__error">{task.errorCode}</span>
              ) : null}
            </button>
          ))
        )}
        {query.hasNextPage ? (
          <button
            type="button"
            className="btn btn--sm btn--ghost task-explorer__more"
            onClick={() => void query.fetchNextPage()}
            disabled={query.isFetchingNextPage}
          >
            {query.isFetchingNextPage ? <Loader2 className="spin" aria-hidden="true" /> : null}
            Показать ещё
          </button>
        ) : null}
      </div>
    </div>
  );
}
