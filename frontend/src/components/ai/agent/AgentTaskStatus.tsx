/** Статус активной Agent-задачи: этап, прогресс, модель, отмена. */
import { AlertTriangle, Clock, Loader2, RotateCcw, Square, Wrench } from 'lucide-react';

import { Badge, Button } from '../../ui';
import { formatDateTimeShort } from '../../../lib/format';
import type { AiTask } from '../../../services/ai';

export function taskStatusTone(
  status: AiTask['status'],
): 'default' | 'ok' | 'warn' | 'danger' | 'info' {
  switch (status) {
    case 'completed':
      return 'ok';
    case 'queued':
    case 'running':
      return 'info';
    case 'waiting_approval':
      return 'warn';
    case 'failed':
      return 'danger';
    default:
      return 'default';
  }
}

export function taskStatusLabel(status: AiTask['status']): string {
  const map: Record<string, string> = {
    queued: 'В очереди',
    running: 'Выполняется',
    waiting_approval: 'Ждёт подтверждения',
    completed: 'Завершена',
    failed: 'Ошибка',
    cancelled: 'Отменена',
  };
  return map[status] ?? status;
}

interface AgentTaskStatusProps {
  task: AiTask | undefined;
  historical: boolean;
  canCancel: boolean;
  stopping: boolean;
  onCancel: () => void;
  onBackToLive: () => void;
}

export function AgentTaskStatus({
  task,
  historical,
  canCancel,
  stopping,
  onCancel,
  onBackToLive,
}: AgentTaskStatusProps) {
  if (!task) {
    return (
      <div className="agent-status agent-status--idle">
        <span className="muted">Активной задачи нет.</span>
      </div>
    );
  }
  const active = ['queued', 'running', 'waiting_approval'].includes(task.status);
  const progress = Math.max(0, Math.min(100, task.progress ?? 0));
  return (
    <div className={`agent-status agent-status--${task.status}`}>
      <div className="agent-status__row">
        <Badge tone={taskStatusTone(task.status)}>{taskStatusLabel(task.status)}</Badge>
        {historical ? <Badge tone="warn">историческая задача</Badge> : null}
        <span className="agent-status__stage" title={task.stage}>
          {task.statusText || task.stage}
        </span>
        <span className="agent-status__spacer" />
        {active ? (
          <Button
            size="sm"
            variant="danger"
            icon={stopping ? <Loader2 className="spin" aria-hidden="true" /> : <Square aria-hidden="true" />}
            disabled={!canCancel || stopping || task.cancelRequested}
            onClick={onCancel}
          >
            {task.cancelRequested ? 'Остановка…' : 'Остановить'}
          </Button>
        ) : historical ? (
          <Button size="sm" variant="ghost" icon={<RotateCcw aria-hidden="true" />} onClick={onBackToLive}>
            К активной
          </Button>
        ) : null}
      </div>
      <div className="agent-status__bar">
        <span style={{ width: `${progress}%` }} />
      </div>
      <div className="agent-status__meta">
        <span>
          <Wrench aria-hidden="true" /> {task.toolCallsCount} инструментов
        </span>
        <span>
          итерация {task.iterations}/{task.maxIterations}
        </span>
        <span>
          <Clock aria-hidden="true" /> {formatDateTimeShort(task.startedAt ?? task.createdAt)}
        </span>
        {task.model ? <span className="agent-status__model">{task.model}</span> : null}
        {task.cancelStatus === 'failed' ? (
          <span className="agent-status__warn">
            <AlertTriangle aria-hidden="true" /> отмена не подтверждена Runtime
          </span>
        ) : null}
      </div>
      {task.errorMessage ? (
        <div className="agent-status__error">
          <AlertTriangle aria-hidden="true" />
          <span>
            {task.errorCode}: {task.errorMessage}
          </span>
        </div>
      ) : null}
    </div>
  );
}
