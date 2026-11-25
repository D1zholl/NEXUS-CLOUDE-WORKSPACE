/** История выполнений инструментов Agent-задачи. */
import { useState } from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  Loader2,
  ShieldAlert,
  Square,
} from 'lucide-react';

import { Badge } from '../../ui';
import { formatDateTimeShort } from '../../../lib/format';
import type { AiExecution } from '../../../services/ai';

function statusIcon(status: string) {
  switch (status) {
    case 'completed':
      return <CheckCircle2 aria-hidden="true" />;
    case 'failed':
      return <AlertTriangle aria-hidden="true" />;
    case 'cancelled':
      return <Square aria-hidden="true" />;
    case 'waiting_approval':
      return <ShieldAlert aria-hidden="true" />;
    default:
      return <Loader2 aria-hidden="true" className="spin" />;
  }
}

function tone(status: string): 'ok' | 'danger' | 'warn' | 'info' | 'default' {
  switch (status) {
    case 'completed':
      return 'ok';
    case 'failed':
      return 'danger';
    case 'cancelled':
      return 'warn';
    case 'running':
    case 'pending':
      return 'info';
    default:
      return 'default';
  }
}

interface ExecutionHistoryProps {
  executions: AiExecution[];
  loading: boolean;
  error?: string | null;
  emptyText?: string;
}

export function ExecutionHistory({
  executions,
  loading,
  error,
  emptyText = 'Выполнений пока нет.',
}: ExecutionHistoryProps) {
  const [expanded, setExpanded] = useState<string | null>(null);

  if (loading) {
    return (
      <div className="exec-history exec-history--empty">
        <Loader2 className="spin" aria-hidden="true" /> Загрузка выполнений…
      </div>
    );
  }
  if (error) {
    return <div className="exec-history exec-history--empty field__error">{error}</div>;
  }
  if (executions.length === 0) {
    return <div className="exec-history exec-history--empty muted">{emptyText}</div>;
  }

  return (
    <div className="exec-history">
      {executions.map((execution) => {
        const isOpen = expanded === execution.id;
        return (
          <div key={execution.id} className={`exec-item exec-item--${execution.status}`}>
            <button
              type="button"
              className="exec-item__row"
              onClick={() => setExpanded(isOpen ? null : execution.id)}
              aria-expanded={isOpen}
            >
              <ChevronRight aria-hidden="true" className={isOpen ? 'is-open' : undefined} />
              <span className="exec-item__status">{statusIcon(execution.status)}</span>
              <span className="exec-item__tool">{execution.toolId}</span>
              <Badge tone={tone(execution.status)}>{execution.status}</Badge>
              {execution.risk !== 'low' ? (
                <Badge tone={execution.risk === 'high' ? 'danger' : 'warn'}>{execution.risk}</Badge>
              ) : null}
              {execution.exitCode !== null ? (
                <span className="exec-item__exit">exit {execution.exitCode}</span>
              ) : null}
              {execution.durationMs !== null ? (
                <span className="exec-item__duration">{execution.durationMs} мс</span>
              ) : null}
              <span className="exec-item__time">{formatDateTimeShort(execution.createdAt)}</span>
            </button>
            {isOpen ? (
              <div className="exec-item__details">
                <div className="exec-item__section">
                  <span className="exec-item__label">Аргументы</span>
                  <pre>{JSON.stringify(execution.arguments ?? {}, null, 2)}</pre>
                </div>
                {execution.errorCode ? (
                  <div className="exec-item__section exec-item__section--error">
                    <span className="exec-item__label">Ошибка</span>
                    <pre>
                      {execution.errorCode}: {execution.errorMessage}
                    </pre>
                  </div>
                ) : null}
                {execution.resultText ? (
                  <div className="exec-item__section">
                    <span className="exec-item__label">Результат</span>
                    <pre>{execution.resultText}</pre>
                  </div>
                ) : null}
                {execution.diff ? (
                  <div className="exec-item__section">
                    <span className="exec-item__label">Diff</span>
                    <pre>{execution.diff}</pre>
                  </div>
                ) : null}
                {(execution.changedFiles ?? []).length > 0 ? (
                  <div className="exec-item__section">
                    <span className="exec-item__label">Изменённые файлы</span>
                    <ul className="exec-item__files">
                      {execution.changedFiles?.map((file) => (
                        <li key={file.path}>
                          <span>{file.path}</span>
                          <span className="changed-file__add">+{file.additions}</span>
                          <span className="changed-file__del">-{file.deletions}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
