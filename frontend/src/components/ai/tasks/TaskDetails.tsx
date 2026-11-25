/** Детали Agent-задачи: обзор, события, выполнения, изменения. */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  CalendarClock,
  ExternalLink,
  Loader2,
  MessageSquare,
  TerminalSquare,
} from 'lucide-react';

import { Badge, Button, Segmented } from '../../ui';
import { aiKeys } from '../../../lib/aiKeys';
import { formatDateTimeShort } from '../../../lib/format';
import { useIncrementalTaskEvents } from '../../../lib/taskEvents';
import { aiApi, errorMessage } from '../../../services/api';
import type { AiTask } from '../../../services/api';
import { ChangedFiles } from '../changes/ChangedFiles';
import { AgentActivity } from '../agent/AgentActivity';
import { ExecutionHistory } from './ExecutionHistory';

type Tab = 'overview' | 'events' | 'executions' | 'changes';

interface TaskDetailsProps {
  workspaceId: string;
  task: AiTask;
  canWrite: boolean;
  onOpenFile: (path: string) => void;
  onOpenInPanel: (task: AiTask) => void;
  onRestored: () => void;
}

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

export function TaskDetails({
  workspaceId,
  task,
  canWrite,
  onOpenFile,
  onOpenInPanel,
  onRestored,
}: TaskDetailsProps) {
  const [tab, setTab] = useState<Tab>('overview');
  const active = ['queued', 'running', 'waiting_approval'].includes(task.status);
  const events = useIncrementalTaskEvents(task.id, active);
  const executions = useQuery({
    queryKey: aiKeys.executions(task.id),
    queryFn: () => aiApi.executions(task.id),
    refetchInterval: active ? 4000 : false,
  });

  const duration =
    task.finishedAt && task.startedAt
      ? Math.max(
          0,
          Math.round(
            (new Date(task.finishedAt).getTime() - new Date(task.startedAt).getTime()) / 1000,
          ),
        )
      : null;

  return (
    <div className="task-details">
      <header className="task-details__head">
        <div>
          <h3>{task.humanInput?.trim() || `Задача ${task.id.slice(0, 8)}`}</h3>
          <div className="task-details__badges">
            <Badge tone={statusTone(task.status)}>{task.status}</Badge>
            {task.model ? <Badge>{task.model}</Badge> : null}
            <span className="muted">{task.stage}</span>
          </div>
        </div>
        <div className="task-details__actions">
          <Button
            size="sm"
            variant="ghost"
            icon={<ExternalLink aria-hidden="true" />}
            onClick={() => onOpenInPanel(task)}
          >
            Открыть в панели
          </Button>
        </div>
      </header>
      <div className="task-details__tabs">
        <Segmented
          value={tab}
          onChange={setTab}
          options={[
            { value: 'overview', label: 'Обзор' },
            { value: 'events', label: 'События' },
            { value: 'executions', label: 'Выполнения' },
            { value: 'changes', label: 'Изменения' },
          ]}
          label="Разделы задачи"
        />
      </div>

      <div className="task-details__body">
        {tab === 'overview' ? (
          <div className="task-overview">
            <dl className="task-overview__grid">
              <div>
                <dt>
                  <CalendarClock aria-hidden="true" /> Создана
                </dt>
                <dd>{formatDateTimeShort(task.createdAt)}</dd>
              </div>
              <div>
                <dt>Завершена</dt>
                <dd>{task.finishedAt ? formatDateTimeShort(task.finishedAt) : '—'}</dd>
              </div>
              <div>
                <dt>Длительность</dt>
                <dd>{duration !== null ? `${duration} с` : '—'}</dd>
              </div>
              <div>
                <dt>Итерации</dt>
                <dd>
                  {task.iterations}/{task.maxIterations}
                </dd>
              </div>
              <div>
                <dt>
                  <TerminalSquare aria-hidden="true" /> Инструментов
                </dt>
                <dd>{task.toolCallsCount}</dd>
              </div>
              <div>
                <dt>
                  <MessageSquare aria-hidden="true" /> Диалог
                </dt>
                <dd className="task-overview__mono">{task.conversationId.slice(0, 8)}</dd>
              </div>
            </dl>
            <div className="task-overview__description">
              <span className="task-overview__label">Запрос пользователя</span>
              <p>{task.humanInput?.trim() || '—'}</p>
            </div>
            {task.statusText ? (
              <div className="task-overview__description">
                <span className="task-overview__label">Последний этап</span>
                <p>{task.statusText}</p>
              </div>
            ) : null}
            {task.errorCode ? (
              <div className="task-overview__description task-overview__description--error">
                <span className="task-overview__label">
                  <AlertTriangle aria-hidden="true" /> Ошибка
                </span>
                <p>
                  {task.errorCode}: {task.errorMessage}
                </p>
              </div>
            ) : null}
            <p className="muted">
              Открытие старой задачи не запускает выполнение. Чтобы продолжить работу,
              отправьте новое сообщение в Диалог.
            </p>
          </div>
        ) : tab === 'events' ? (
          <AgentActivity
            events={events.items}
            loading={events.loading}
            error={events.error}
            stats={events.stats}
            onReload={events.reload}
          />
        ) : tab === 'executions' ? (
          executions.isLoading ? (
            <Loader2 className="spin" />
          ) : executions.error ? (
            <p className="field__error">{errorMessage(executions.error)}</p>
          ) : (
            <ExecutionHistory
              executions={executions.data?.items ?? []}
              loading={false}
            />
          )
        ) : (
          <ChangedFiles
            workspaceId={workspaceId}
            taskId={task.id}
            canWrite={canWrite}
            onOpenFile={onOpenFile}
            onRestored={onRestored}
            compact
          />
        )}
      </div>
    </div>
  );
}
