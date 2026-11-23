/**
 * Чистые преобразования realtime-событий в состояние страниц.
 *
 * Вынесены из компонентов, чтобы их можно было покрыть тестами без React:
 * события WS должны мгновенно обновлять прогресс, а обычный polling остаётся
 * источником истины при недоступности канала.
 */

import type { BackgroundJob, JobsResponse } from '../services/api';
import type { DownloadPayload, RealtimeEvent } from './realtime';

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function numberValue(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/**
 * Применяет событие задачи к уже загруженному списку.
 *
 * Счётчики (`pending`, `running`, …) не пересчитываются: они отражают всю
 * очередь, а список может быть отфильтрован. Их синхронизирует периодический
 * опрос; событие лишь убирает задержку в строке задачи.
 */
export function applyJobEvent(data: JobsResponse, event: RealtimeEvent): JobsResponse {
  const jobId = stringValue(event.payload.jobId);
  if (!jobId) return data;
  const index = data.items.findIndex((job) => job.id === jobId);
  if (index < 0) return data;

  const current = data.items[index];
  const next: BackgroundJob = { ...current };
  const finishedAt = event.at ?? new Date().toISOString();

  switch (event.kind) {
    case 'job.progress': {
      const done = numberValue(event.payload.done);
      const total = numberValue(event.payload.total);
      const progress = numberValue(event.payload.progress);
      if (done !== null) next.doneUnits = done;
      if (total !== null) next.totalUnits = total;
      if (progress !== null) next.progress = progress;
      next.status = 'running';
      break;
    }
    case 'job.paused': {
      next.status = 'paused';
      next.pauseRequested = false;
      next.pausedAt = stringValue(event.payload.pausedAt) ?? new Date().toISOString();
      break;
    }
    case 'job.resumed': {
      next.status = 'pending';
      next.pauseRequested = false;
      next.pausedAt = null;
      break;
    }
    case 'job.completed': {
      next.status = 'completed';
      next.progress = 100;
      next.doneUnits = next.totalUnits;
      next.finishedAt = finishedAt;
      next.pauseRequested = false;
      break;
    }
    case 'job.failed': {
      next.status = 'failed';
      next.error = stringValue(event.payload.error) ?? next.error;
      next.finishedAt = finishedAt;
      next.pauseRequested = false;
      break;
    }
    case 'job.cancelled': {
      next.status = 'cancelled';
      next.finishedAt = finishedAt;
      next.pauseRequested = false;
      break;
    }
    default:
      return data;
  }

  const items = data.items.slice();
  items[index] = next;
  return { ...data, items };
}

export type DownloadState = 'active' | 'done' | 'failed';

export interface DownloadItem extends DownloadPayload {
  state: DownloadState;
  code?: string;
  message?: string;
}

const MAX_TRACKED_DOWNLOADS = 8;

function downloadFromEvent(event: RealtimeEvent): DownloadItem | null {
  const entryId = stringValue(event.payload.entryId);
  if (!entryId) return null;
  const transferred = numberValue(event.payload.transferred) ?? 0;
  const size = numberValue(event.payload.size) ?? 0;
  const percent = numberValue(event.payload.percent) ?? (size > 0 ? (transferred / size) * 100 : 0);
  return {
    entryId,
    name: stringValue(event.payload.name) ?? 'Файл',
    transferred,
    size,
    percent,
    state: 'active',
  };
}

/**
 * Применяет `download.progress` / `download.completed` / `download.failed`
 * к панели скачиваний. Возвращает новый массив (стабильный порядок, не более
 * восьми записей).
 */
export function applyDownloadEvent(items: DownloadItem[], event: RealtimeEvent): DownloadItem[] {
  if (
    event.kind !== 'download.progress' &&
    event.kind !== 'download.completed' &&
    event.kind !== 'download.failed'
  ) {
    return items;
  }
  const incoming = downloadFromEvent(event);
  if (!incoming) return items;

  if (event.kind === 'download.completed') incoming.state = 'done';
  if (event.kind === 'download.failed') {
    incoming.state = 'failed';
    incoming.code = stringValue(event.payload.code) ?? 'DOWNLOAD_FAILED';
    incoming.message = stringValue(event.payload.message) ?? 'Скачивание прервано.';
  }
  if (incoming.state === 'done') incoming.percent = 100;

  const index = items.findIndex((item) => item.entryId === incoming.entryId);
  if (index >= 0) {
    const next = items.slice();
    next[index] = { ...items[index], ...incoming };
    return next;
  }
  return [incoming, ...items].slice(0, MAX_TRACKED_DOWNLOADS);
}

/** Убирает запись панели скачиваний. */
export function dismissDownload(items: DownloadItem[], entryId: string): DownloadItem[] {
  return items.filter((item) => item.entryId !== entryId);
}
