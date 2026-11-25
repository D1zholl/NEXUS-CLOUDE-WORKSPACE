/** Лента действий агента: чтение, поиск, изменения, запуск программ. */
import { useState } from 'react';
import {
  CheckCircle2,
  ChevronRight,
  FilePlus2,
  FileSearch,
  FileText,
  FolderSearch,
  Loader2,
  Pencil,
  Play,
  RefreshCw,
  ShieldAlert,
  Trash2,
  XCircle,
} from 'lucide-react';

import { formatDateTimeShort } from '../../../lib/format';
import type { AiTaskEvent } from '../../../services/ai';
import type { TaskEventStats } from '../../../lib/taskEvents';

function actionIcon(event: AiTaskEvent) {
  const tool = String(event.data?.toolId ?? '');
  const kind = event.kind;
  if (kind === 'agent.file.changed') {
    const action = String(event.data?.action ?? '');
    if (action === 'delete') return <Trash2 aria-hidden="true" />;
    if (action === 'mkdir') return <FilePlus2 aria-hidden="true" />;
    return <Pencil aria-hidden="true" />;
  }
  if (kind.includes('waiting_approval')) return <ShieldAlert aria-hidden="true" />;
  if (kind.includes('failed')) return <XCircle aria-hidden="true" />;
  if (kind.includes('completed')) return <CheckCircle2 aria-hidden="true" />;
  if (tool === 'fs.read' || tool === 'fs.stat') return <FileText aria-hidden="true" />;
  if (tool === 'fs.list') return <FolderSearch aria-hidden="true" />;
  if (tool === 'fs.search') return <FileSearch aria-hidden="true" />;
  if (tool === 'program.run') return <Play aria-hidden="true" />;
  if (tool.startsWith('fs.')) return <Pencil aria-hidden="true" />;
  if (kind.startsWith('ai.task.')) return <ChevronRight aria-hidden="true" />;
  return <Loader2 aria-hidden="true" />;
}

function tone(event: AiTaskEvent): string {
  if (event.kind.includes('failed')) return 'is-error';
  if (event.kind.includes('waiting_approval')) return 'is-warn';
  if (event.kind === 'agent.file.changed') return 'is-file';
  if (event.kind.includes('completed')) return 'is-ok';
  return '';
}

interface AgentActivityProps {
  events: AiTaskEvent[];
  loading: boolean;
  error: string | null;
  stats: TaskEventStats;
  onReload: () => void;
  compact?: boolean;
}

export function AgentActivity({
  events,
  loading,
  error,
  stats,
  onReload,
  compact = false,
}: AgentActivityProps) {
  const [expanded, setExpanded] = useState<number | null>(null);
  const visible = compact ? events.slice(-40) : events;

  return (
    <div className={`agent-activity${compact ? ' agent-activity--compact' : ''}`}>
      <div className="agent-activity__head">
        <span>Действия агента</span>
        <span className="muted">
          {events.length} событий
          {stats.duplicates > 0 ? ` · дублей ${stats.duplicates}` : ''}
        </span>
        <button
          type="button"
          className="ai-icon-btn"
          onClick={onReload}
          title="Синхронизировать события"
          disabled={loading}
        >
          <RefreshCw aria-hidden="true" className={loading ? 'spin' : undefined} />
        </button>
      </div>
      {error ? <div className="agent-activity__error">{error}</div> : null}
      {visible.length === 0 ? (
        <div className="agent-activity__empty muted">Событий пока нет.</div>
      ) : (
        <div className="agent-activity__list">
          {visible.map((event) => (
            <div key={event.id} className={`agent-activity__item ${tone(event)}`}>
              <button
                type="button"
                className="agent-activity__row"
                onClick={() => setExpanded(expanded === event.id ? null : event.id)}
                aria-expanded={expanded === event.id}
              >
                <span className="agent-activity__icon">{actionIcon(event)}</span>
                <span className="agent-activity__message">{event.message || event.kind}</span>
                <span className="agent-activity__time">{formatDateTimeShort(event.createdAt)}</span>
              </button>
              {expanded === event.id ? (
                <div className="agent-activity__details">
                  <div className="agent-activity__kind">{event.kind}</div>
                  <pre>{JSON.stringify(event.data ?? {}, null, 2)}</pre>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
