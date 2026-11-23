import { useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  Ban,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  CloudUpload,
  FolderOpen,
  Pause,
  Play,
  RotateCcw,
  X,
} from 'lucide-react';

import { Button, ConfirmDialog } from './ui';
import { formatBytes } from '../lib/format';
import { realtime } from '../lib/realtime';
import type { UploadItem } from '../lib/uploadQueue';
import { useTransfers } from '../stores/transfers';

/**
 * Плавающая очередь передач в правом нижнем углу.
 *
 * Один контейнер на все файлы: общий прогресс, группы папок, скорость/ETA,
 * пауза, повтор, отмена и переход к загруженному файлу. Виджет не закрывается
 * автоматически: ошибки должны остаться видимыми, пока пользователь сам не
 * скроет окно.
 */
export function TransferDrawer() {
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const items = useTransfers((state) => state.items);
  const stats = useTransfers((state) => state.stats);
  const collapsed = useTransfers((state) => state.collapsed);
  const hidden = useTransfers((state) => state.hidden);
  const setCollapsed = useTransfers((state) => state.setCollapsed);
  const hide = useTransfers((state) => state.hide);
  const clearFinished = useTransfers((state) => state.clearFinished);

  const [confirmClose, setConfirmClose] = useState(false);
  const lastDoneRef = useRef(0);

  useEffect(() => {
    const unsubscribers = [
      realtime.on('upload.progress', (event) => {
        useTransfers.getState().applyServerProgress(event.payload.uploadId, event.payload.uploaded);
      }),
      realtime.on('upload.failed', (event) => {
        if (event.payload.code === 'UPLOAD_ABORTED') return;
        useTransfers.getState().markServerFailure(event.payload.uploadId, event.payload.message);
      }),
      realtime.on('upload.completed', () => {
        void queryClient.invalidateQueries({ queryKey: ['cloud-list'] });
      }),
    ];
    return () => {
      for (const unsubscribe of unsubscribers) unsubscribe();
    };
  }, [queryClient]);

  useEffect(() => {
    const done = stats?.done ?? 0;
    if (done <= lastDoneRef.current) return;
    lastDoneRef.current = done;
    void queryClient.invalidateQueries({ queryKey: ['cloud-list'] });
    void queryClient.invalidateQueries({ queryKey: ['my-usage'] });
  }, [queryClient, stats?.done]);

  const { running, active, failed, finished, hiddenQuiet } = useMemo(
    () => splitItems(items),
    [items],
  );

  if (hidden || !stats || items.length === 0) return null;

  const total = items.length;
  const percent = Math.min(100, Math.max(0, Math.round(stats.progress * 100)));
  const errors = failed.length;
  const allComplete = running.length === 0 && errors === 0;
  const title =
    running.length > 0
      ? `Загрузка: ${stats.done} из ${total}`
      : errors > 0
        ? `Загрузка: ${stats.done} из ${total} · есть ошибки`
        : `Загрузка завершена: ${stats.done} из ${total}`;

  const close = (): void => {
    if (running.length > 0 || errors > 0) {
      setConfirmClose(true);
      return;
    }
    clearFinished();
    hide();
  };

  const confirmAndClose = (): void => {
    setConfirmClose(false);
    hide();
  };

  const reveal = (item: UploadItem): void => {
    const parentId = item.entry?.parentId ?? item.parentId;
    useTransfers.getState().requestReveal(parentId);
    if (location.pathname !== '/workspace/storage') {
      navigate('/workspace/storage');
    }
  };

  return (
    <>
      <section
        className={`transfer-drawer${collapsed ? ' transfer-drawer--collapsed' : ''}`}
        aria-label="Очередь загрузок"
        data-testid="upload-queue"
      >
        <header className="transfer-drawer__head">
          <span className={`transfer-drawer__badge${allComplete ? ' is-done' : ''}`}>
            {allComplete ? (
              <CheckCircle2 aria-hidden="true" />
            ) : errors > 0 && running.length === 0 ? (
              <AlertTriangle aria-hidden="true" />
            ) : (
              <CloudUpload aria-hidden="true" />
            )}
          </span>
          <div className="transfer-drawer__title grow">
            <div className="truncate">{title}</div>
            <div className="transfer-drawer__meta">
              <span className="mono">{percent}%</span>
              <span>{stats.done} готово</span>
              {stats.active > 0 ? <span>{stats.active} активно</span> : null}
              {stats.queued > 0 ? <span>{stats.queued} в очереди</span> : null}
              {errors > 0 ? <span className="is-error">{errors} с ошибкой</span> : null}
              {stats.speedBytesPerSec > 0 && running.length > 0 ? (
                <span>{formatBytes(Math.round(stats.speedBytesPerSec))}/с</span>
              ) : null}
              {stats.etaSeconds !== null && running.length > 0 ? (
                <span>≈ {formatEta(stats.etaSeconds)}</span>
              ) : null}
            </div>
          </div>
          {running.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              title="Отменить все"
              aria-label="Отменить все загрузки"
              onClick={() => useTransfers.getState().cancelAll()}
            >
              <Ban aria-hidden="true" />
            </Button>
          ) : null}
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title={collapsed ? 'Развернуть' : 'Свернуть'}
            aria-label={collapsed ? 'Развернуть очередь' : 'Свернуть очередь'}
            onClick={() => setCollapsed(!collapsed)}
          >
            {collapsed ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title="Закрыть"
            aria-label="Закрыть очередь загрузок"
            onClick={close}
          >
            <X aria-hidden="true" />
          </Button>
        </header>

        <div className="transfer-drawer__meter">
          <div
            className={`meter__fill${errors > 0 ? ' meter__fill--warn' : allComplete ? ' is-ok' : ''}`}
            style={{ width: `${percent}%` }}
          />
        </div>

        {!collapsed ? (
          <>
            <div className="transfer-drawer__body">
              {stats.groups.length > 0 ? (
                <div className="transfer-groups">
                  {stats.groups.map((group) => (
                    <div key={group.name} className="transfer-group">
                      <div className="transfer-group__row">
                        <span className="truncate">Папка «{group.name}»</span>
                        <span className="mono">
                          {group.done + group.failed}/{group.total} ·{' '}
                          {Math.round(group.progress * 100)}%
                        </span>
                      </div>
                      <div className="meter">
                        <div
                          className={`meter__fill${group.failed > 0 ? ' meter__fill--warn' : ''}`}
                          style={{ width: `${Math.round(group.progress * 100)}%` }}
                        />
                      </div>
                    </div>
                  ))}
                </div>
              ) : null}

              {failed.length > 0 ? (
                <div className="transfer-alert" role="status">
                  <AlertTriangle aria-hidden="true" />
                  <span className="grow">
                    Не удалось загрузить {failed.length}{' '}
                    {failed.length === 1 ? 'файл' : failed.length < 5 ? 'файла' : 'файлов'}. Окно не
                    закроется автоматически.
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<RotateCcw aria-hidden="true" />}
                    onClick={() => useTransfers.getState().retryFailed()}
                  >
                    Повторить
                  </Button>
                </div>
              ) : null}

              {active.length > 0 ? (
                <>
                  <div className="transfer-section-label">
                    <span>Активные</span>
                    <span className="mono">{active.length}</span>
                  </div>
                  <div className="transfer-list">
                    {active.map((item) => (
                      <TransferRow key={item.key} item={item} onReveal={() => reveal(item)} />
                    ))}
                  </div>
                </>
              ) : null}

              {failed.length > 0 ? (
                <>
                  <div className="transfer-section-label is-error">
                    <span>С ошибкой</span>
                    <span className="mono">{failed.length}</span>
                  </div>
                  <div className="transfer-list">
                    {failed.map((item) => (
                      <TransferRow key={item.key} item={item} onReveal={() => reveal(item)} />
                    ))}
                  </div>
                </>
              ) : null}

              {finished.length > 0 ? (
                <>
                  <div className="transfer-section-label">
                    <span>Завершено</span>
                    <span className="mono">{finished.length}</span>
                  </div>
                  <div className="transfer-list">
                    {finished.slice(-20).map((item) => (
                      <TransferRow key={item.key} item={item} onReveal={() => reveal(item)} />
                    ))}
                  </div>
                </>
              ) : null}

              {hiddenQuiet > 0 ? (
                <div className="transfer-drawer__note">
                  Показаны активные, проблемные и последние завершённые файлы. Ещё скрыто:{' '}
                  {hiddenQuiet}.
                </div>
              ) : null}
            </div>

            <footer className="transfer-drawer__foot">
              <Button
                size="sm"
                variant="ghost"
                disabled={finished.length === 0}
                onClick={clearFinished}
              >
                Очистить завершённые
              </Button>
              <div className="grow" />
            </footer>
          </>
        ) : null}
      </section>

      <ConfirmDialog
        open={confirmClose}
        title={running.length > 0 ? 'Прервать загрузку?' : 'Скрыть окно с ошибками?'}
        description={
          running.length > 0
            ? `Активных файлов: ${running.length}. Если закрыть окно сейчас, загрузки будут отменены. Продолжить?`
            : `Есть файлы с ошибкой: ${errors}. Они останутся в очереди и появятся снова при следующей загрузке.`
        }
        confirmLabel={running.length > 0 ? 'Прервать и закрыть' : 'Скрыть'}
        cancelLabel="Оставить открытым"
        onCancel={() => setConfirmClose(false)}
        onConfirm={() => {
          if (running.length > 0) useTransfers.getState().cancelAll();
          confirmAndClose();
        }}
      />
    </>
  );
}

function TransferRow({ item, onReveal }: { item: UploadItem; onReveal: () => void }) {
  const percent = item.size > 0 ? Math.min(100, Math.round((item.uploaded / item.size) * 100)) : 0;
  const rate = item.state === 'uploading' ? itemRate(item) : 0;
  const eta = rate > 1024 && item.size > item.uploaded ? (item.size - item.uploaded) / rate : null;
  const path = item.segments.length > 0 ? `${item.segments.join('/')}/` : '';

  return (
    <div
      className={`transfer-item transfer-item--${item.state}`}
      data-testid="upload-queue-item"
      data-file-name={item.file.name}
    >
      <div className="transfer-item__row">
        <StateIcon state={item.state} />
        <div className="transfer-item__name grow">
          <button
            type="button"
            className="transfer-item__link"
            title={item.entry ? 'Показать в папке' : item.file.name}
            onClick={item.entry ? onReveal : undefined}
            disabled={!item.entry}
          >
            <span className="truncate">
              {path}
              {item.file.name}
            </span>
          </button>
          <div className="transfer-item__meta">
            <span className="mono">
              {formatBytes(item.uploaded)} / {formatBytes(item.size)}
            </span>
            <span>{statusText(item)}</span>
            {rate > 0 ? <span>{formatBytes(Math.round(rate))}/с</span> : null}
            {eta !== null ? <span>≈ {formatEta(Math.round(eta))}</span> : null}
            {item.attempts > 0 && item.state !== 'done' && item.state !== 'conflict' ? (
              <span>попытка {item.attempts}</span>
            ) : null}
          </div>
        </div>
        <div className="transfer-item__actions">
          {item.state === 'uploading' ||
          item.state === 'preparing' ||
          item.state === 'finishing' ? (
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              title="Пауза"
              aria-label="Поставить на паузу"
              onClick={() => useTransfers.getState().pause(item.key)}
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
              aria-label="Продолжить загрузку"
              onClick={() => useTransfers.getState().resume(item.key)}
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
              aria-label="Повторить загрузку"
              onClick={() => useTransfers.getState().retry(item.key)}
            >
              <RotateCcw aria-hidden="true" />
            </Button>
          ) : null}
          {item.state === 'conflict' ? (
            <>
              <Button
                size="sm"
                onClick={() => void useTransfers.getState().resolveConflict(item.key, 'replace')}
              >
                Заменить
              </Button>
              <Button
                size="sm"
                onClick={() => void useTransfers.getState().resolveConflict(item.key, 'rename')}
              >
                Копия
              </Button>
            </>
          ) : null}
          {item.state === 'done' && item.entry ? (
            <Button
              size="sm"
              variant="ghost"
              icon={<FolderOpen aria-hidden="true" />}
              onClick={onReveal}
            >
              В папке
            </Button>
          ) : null}
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title={
              item.state === 'done' || item.state === 'cancelled' ? 'Убрать из списка' : 'Отменить'
            }
            aria-label={
              item.state === 'done' || item.state === 'cancelled'
                ? 'Убрать из списка'
                : 'Отменить загрузку'
            }
            onClick={() => useTransfers.getState().remove(item.key)}
          >
            <X aria-hidden="true" />
          </Button>
        </div>
      </div>
      {item.state !== 'done' && item.state !== 'cancelled' ? (
        <div className="meter transfer-item__meter">
          <div
            className={`meter__fill${item.state === 'failed' || item.state === 'conflict' ? ' meter__fill--warn' : ''}`}
            style={{ width: `${percent}%` }}
          />
        </div>
      ) : null}
      {item.error ? <div className="transfer-item__error">{item.error}</div> : null}
    </div>
  );
}

function StateIcon({ state }: { state: UploadItem['state'] }) {
  if (state === 'done') return <CheckCircle2 aria-hidden="true" className="is-ok" size={16} />;
  if (state === 'failed' || state === 'conflict') {
    return <AlertTriangle aria-hidden="true" className="is-error" size={16} />;
  }
  return <span className="transfer-item__dot" aria-hidden="true" />;
}

function statusText(item: UploadItem): string {
  switch (item.state) {
    case 'queued':
      return 'В очереди';
    case 'preparing':
      return 'Подготовка…';
    case 'uploading':
      return `Загрузка ${item.size > 0 ? Math.round((item.uploaded / item.size) * 100) : 0}%`;
    case 'paused':
      return 'Пауза';
    case 'retrying':
      return 'Повтор после ошибки…';
    case 'finishing':
      return 'Фиксация на сервере…';
    case 'done':
      return 'Готово';
    case 'conflict':
      return 'Имя уже занято — выберите действие';
    case 'failed':
      return 'Ошибка';
    case 'cancelled':
      return 'Отменено';
    default:
      return '';
  }
}

function itemRate(item: UploadItem): number {
  if (item.samples.length < 2) return 0;
  const first = item.samples[0];
  const last = item.samples[item.samples.length - 1];
  const windowMs = last.at - first.at;
  if (windowMs < 500) return 0;
  const bytes = last.bytes - first.bytes;
  return bytes > 0 ? (bytes / windowMs) * 1000 : 0;
}

/** Активные, проблемные и ограниченное число ожидающих/завершённых строк. */
function splitItems(items: UploadItem[]): {
  running: UploadItem[];
  active: UploadItem[];
  failed: UploadItem[];
  finished: UploadItem[];
  hiddenQuiet: number;
} {
  const running: UploadItem[] = [];
  const queued: UploadItem[] = [];
  const failed: UploadItem[] = [];
  const finished: UploadItem[] = [];

  for (const item of items) {
    if (item.state === 'failed' || item.state === 'conflict') failed.push(item);
    else if (item.state === 'done' || item.state === 'cancelled') finished.push(item);
    else if (item.state === 'queued') queued.push(item);
    else running.push(item);
  }

  const active = [...running, ...queued.slice(0, 40)];
  const visibleCount = active.length + failed.length + Math.min(finished.length, 20);
  const hiddenQuiet = Math.max(0, items.length - visibleCount);
  return { running, active, failed, finished, hiddenQuiet };
}

function formatEta(seconds: number): string {
  if (seconds < 60) return `${seconds} с`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) return `${minutes} мин ${rest} с`;
  const hours = Math.floor(minutes / 60);
  return `${hours} ч ${minutes % 60} мин`;
}