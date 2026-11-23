import { Pause, Play, RotateCcw, X, CheckCircle2, AlertTriangle } from 'lucide-react';

import { Button } from '../components/ui';
import { formatBytes } from '../lib/format';
import type { QueueStats, UploadItem } from '../lib/uploadQueue';

/**
 * Панель очереди загрузок: общий прогресс, скорость, ETA и управление файлами.
 *
 * ETA показывается только при достаточной истории наблюдений (см. `uploadQueue`),
 * чтобы не имитировать значение.
 */
export function UploadQueuePanel({
  stats,
  items,
  onPause,
  onResume,
  onRetry,
  onCancel,
  onClear,
}: {
  stats: QueueStats;
  items: UploadItem[];
  onPause: (key: string) => void;
  onResume: (key: string) => void;
  onRetry: (key: string) => void;
  onCancel: (key: string) => void;
  onClear: () => void;
}) {
  const percent = Math.round(stats.progress * 100);
  const { visible, hidden } = visibleQueueItems(items);

  return (
    <div className="card" style={{ padding: 16 }} data-testid="upload-queue">
      <div className="row row--between" style={{ gap: 12, flexWrap: 'wrap' }}>
        <div>
          <strong style={{ fontSize: 13 }}>Очередь загрузок</strong>
          <div className="tiny muted">
            {stats.done} готово · {stats.active} в работе · {stats.queued} в очереди
            {stats.failed > 0 ? ` · ${stats.failed} с ошибкой` : ''}
          </div>
        </div>
        <div className="row" style={{ gap: 12 }}>
          <span className="tiny muted">
            {formatBytes(stats.uploadedBytes)} из {formatBytes(stats.totalBytes)}
          </span>
          <span className="tiny">
            {stats.speedBytesPerSec > 0
              ? `${formatBytes(Math.round(stats.speedBytesPerSec))}/с`
              : '—'}
          </span>
          <span className="tiny muted">
            {stats.etaSeconds !== null ? `≈ ${formatEta(stats.etaSeconds)}` : 'осталось: —'}
          </span>
          <Button size="sm" variant="ghost" onClick={onClear}>
            Очистить
          </Button>
        </div>
      </div>

      <div className="meter" aria-hidden="true" style={{ margin: '10px 0 14px' }}>
        <div className="meter__fill" style={{ width: `${percent}%` }} />
      </div>

      {stats.groups.length > 0 ? (
        <div className="stack" style={{ gap: 8, marginBottom: 12 }}>
          {stats.groups.map((group) => (
            <div key={group.name}>
              <div className="row row--between" style={{ gap: 10, fontSize: 12 }}>
                <span className="truncate">Папка «{group.name}»</span>
                <span className="tiny muted">
                  {group.done + group.failed} / {group.total} файлов ·{' '}
                  {Math.round(group.progress * 100)}%
                </span>
              </div>
              <div className="meter" aria-hidden="true">
                <div
                  className={`meter__fill${group.failed > 0 ? ' meter__fill--warn' : ''}`}
                  style={{ width: `${Math.round(group.progress * 100)}%` }}
                />
              </div>
            </div>
          ))}
        </div>
      ) : null}

      <div className="stack" style={{ gap: 8, maxHeight: 260, overflowY: 'auto' }}>
        {visible.map((item) => {
          const itemPercent = item.size > 0 ? Math.round((item.uploaded / item.size) * 100) : 0;
          const removable = item.state === 'done' || item.state === 'cancelled';
          return (
            <div
              key={item.key}
              className="row row--between"
              style={{ gap: 10 }}
              data-testid="upload-queue-item"
              data-file-name={item.file.name}
            >
              <div className="row" style={{ gap: 8, minWidth: 0, flex: 1 }}>
                <StateIcon state={item.state} />
                <div style={{ minWidth: 0, flex: 1 }}>
                  <div className="truncate" style={{ fontSize: 12.5 }}>
                    {item.segments.length > 0 ? `${item.segments.join('/')}/` : ''}
                    {item.file.name}
                  </div>
                  <div className="tiny muted">
                    {formatBytes(item.uploaded)} / {formatBytes(item.size)}
                    {item.state === 'uploading' ? ` · ${itemPercent}%` : ''}
                    {item.error ? ` · ${item.error}` : ''}
                    {item.attempts > 0 && item.state !== 'done'
                      ? ` · попытка ${item.attempts}`
                      : ''}
                  </div>
                </div>
              </div>
              <div className="row" style={{ gap: 4 }}>
                {item.state === 'uploading' || item.state === 'preparing' ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Пауза"
                    onClick={() => onPause(item.key)}
                  >
                    <Pause aria-hidden="true" />
                  </Button>
                ) : null}
                {item.state === 'paused' ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Продолжить"
                    onClick={() => onResume(item.key)}
                  >
                    <Play aria-hidden="true" />
                  </Button>
                ) : null}
                {item.state === 'failed' ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Повторить"
                    onClick={() => onRetry(item.key)}
                  >
                    <RotateCcw aria-hidden="true" />
                  </Button>
                ) : null}
                {!removable ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Отменить"
                    onClick={() => onCancel(item.key)}
                  >
                    <X aria-hidden="true" />
                  </Button>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
      {hidden > 0 ? (
        <div className="tiny muted" style={{ marginTop: 8 }}>
          Показаны активные и ближайшие файлы. Скрыто ещё {hidden}.
        </div>
      ) : null}
    </div>
  );
}

/**
 * Папка на 2000 файлов не должна превращаться в 2000 DOM-строк: при каждом
 * прогрессе React перерисовывал весь список и загрузка замедлялась на порядок.
 * Показываем активные/проблемные элементы, ограниченную очередь и последние
 * завершённые; статистика и прогресс по папке считаются по полному набору.
 */
function visibleQueueItems(items: UploadItem[]): { visible: UploadItem[]; hidden: number } {
  const active: UploadItem[] = [];
  const queued: UploadItem[] = [];
  const done: UploadItem[] = [];
  const other: UploadItem[] = [];
  for (const item of items) {
    if (
      item.state === 'uploading' ||
      item.state === 'preparing' ||
      item.state === 'retrying' ||
      item.state === 'finishing' ||
      item.state === 'paused' ||
      item.state === 'failed'
    ) {
      active.push(item);
    } else if (item.state === 'queued') {
      queued.push(item);
    } else if (item.state === 'done') {
      done.push(item);
    } else {
      other.push(item);
    }
  }
  const visible = [...active, ...queued.slice(0, 40), ...other.slice(0, 5), ...done.slice(-10)];
  return { visible, hidden: Math.max(0, items.length - visible.length) };
}

function StateIcon({ state }: { state: UploadItem['state'] }) {
  if (state === 'done') return <CheckCircle2 aria-hidden="true" size={15} color="var(--accent)" />;
  if (state === 'failed')
    return <AlertTriangle aria-hidden="true" size={15} color="var(--danger, #ff6b6b)" />;
  return (
    <span
      aria-hidden="true"
      style={{
        width: 8,
        height: 8,
        borderRadius: '50%',
        background: state === 'paused' ? 'var(--text-secondary)' : 'var(--accent)',
        flexShrink: 0,
      }}
    />
  );
}

function formatEta(seconds: number): string {
  if (seconds < 60) return `${seconds} с`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) return `${minutes} мин ${rest} с`;
  const hours = Math.floor(minutes / 60);
  return `${hours} ч ${minutes % 60} мин`;
}
