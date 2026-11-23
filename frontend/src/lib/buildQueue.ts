/**
 * Очередь загрузки артефактов сборки.
 *
 * Использует существующий движок `uploadArtifact` (чанки, серверный offset,
 * идемпотентная фиксация). Добавляет только оркестрацию: параллельность,
 * пауза/продолжение с серверного offset, повтор сетевых ошибок и отмена.
 */

import { ApiRequestError, api, type BuildFileRow } from '../services/api';
import { isAbort, uploadArtifact } from './buildUpload';

export type BuildUploadState =
  | 'queued'
  | 'preparing'
  | 'uploading'
  | 'paused'
  | 'retrying'
  | 'finishing'
  | 'done'
  | 'failed'
  | 'cancelled';

export interface BuildUploadItem {
  key: string;
  file: File;
  path: string;
  size: number;
  uploaded: number;
  state: BuildUploadState;
  attempts: number;
  error: string | null;
  sessionId: string | null;
  result: BuildFileRow | null;
  mode: 'auto' | 'replace';
  category?: string;
  side?: string;
  required?: boolean;
  description?: string;
  samples: Array<{ at: number; bytes: number }>;
}

export interface BuildQueueStats {
  totalBytes: number;
  uploadedBytes: number;
  progress: number;
  speedBytesPerSec: number;
  etaSeconds: number | null;
  active: number;
  queued: number;
  failed: number;
  done: number;
}

export interface BuildUploadInput {
  file: File;
  path: string;
  mode?: 'auto' | 'replace';
  category?: string;
  side?: string;
  required?: boolean;
  description?: string;
}

export interface BuildQueueOptions {
  buildId: string;
  chunkMaxBytes: number;
  concurrency?: number;
  categoryForPath: (path: string) => string;
  onUpdate: () => void;
  onCompleted?: (item: BuildUploadItem) => void;
  onFailed?: (item: BuildUploadItem) => void;
}

let counter = 0;

export class BuildUploadQueue {
  private items = new Map<string, BuildUploadItem>();
  private running = new Set<string>();
  private controllers = new Map<string, AbortController>();
  private options: BuildQueueOptions;

  constructor(options: BuildQueueOptions) {
    this.options = options;
  }

  setOptions(patch: Partial<BuildQueueOptions>): void {
    this.options = { ...this.options, ...patch };
  }

  add(inputs: BuildUploadInput[]): void {
    for (const input of inputs) {
      const key = `b-${Date.now()}-${counter++}`;
      this.items.set(key, {
        key,
        file: input.file,
        path: input.path,
        size: input.file.size,
        uploaded: 0,
        state: 'queued',
        attempts: 0,
        error: null,
        sessionId: null,
        result: null,
        mode: input.mode ?? 'auto',
        category: input.category,
        side: input.side,
        required: input.required,
        description: input.description,
        samples: [],
      });
    }
    this.options.onUpdate();
    void this.pump();
  }

  getItems(): BuildUploadItem[] {
    return [...this.items.values()].reverse();
  }

  getItem(key: string): BuildUploadItem | undefined {
    return this.items.get(key);
  }

  stats(): BuildQueueStats {
    let totalBytes = 0;
    let uploadedBytes = 0;
    let active = 0;
    let queued = 0;
    let failed = 0;
    let done = 0;
    let speedTotal = 0;
    let speedSamples = 0;
    const now = Date.now();
    let hasHistory = false;

    for (const item of this.items.values()) {
      totalBytes += item.size;
      uploadedBytes += item.uploaded;
      if (item.state === 'preparing' || item.state === 'uploading' || item.state === 'finishing') {
        active += 1;
      }
      if (item.state === 'queued' || item.state === 'retrying') queued += 1;
      if (item.state === 'failed') failed += 1;
      if (item.state === 'done') done += 1;
      if (item.state === 'uploading' && item.samples.length >= 2) {
        const first = item.samples[0];
        const last = item.samples[item.samples.length - 1];
        const windowMs = last.at - first.at;
        if (windowMs >= 1000 && now - first.at <= 15_000) {
          const bytes = last.bytes - first.bytes;
          if (bytes > 0) {
            speedTotal += (bytes / windowMs) * 1000;
            speedSamples += 1;
          }
        }
        if (windowMs >= 10_000) hasHistory = true;
      }
    }

    const speedBytesPerSec = speedSamples > 0 ? speedTotal / speedSamples : 0;
    const remaining = Math.max(totalBytes - uploadedBytes, 0);
    return {
      totalBytes,
      uploadedBytes,
      progress: totalBytes > 0 ? Math.min(uploadedBytes / totalBytes, 1) : 0,
      speedBytesPerSec,
      etaSeconds:
        hasHistory && speedBytesPerSec > 1024 && remaining > 0
          ? Math.round(remaining / speedBytesPerSec)
          : null,
      active,
      queued,
      failed,
      done,
    };
  }

  private sample(item: BuildUploadItem, bytes: number): void {
    const now = Date.now();
    item.samples.push({ at: now, bytes });
    const cutoff = now - 5000;
    while (item.samples.length > 2 && item.samples[0].at < cutoff) {
      item.samples.shift();
    }
  }

  /** Применяет `upload.progress` из WebSocket по id серверной сессии. */
  applyServerProgress(sessionId: string, uploaded: number): void {
    for (const item of this.items.values()) {
      if (item.sessionId !== sessionId) continue;
      if (item.state === 'done' || item.state === 'cancelled' || item.state === 'failed') return;
      const next = Math.max(item.uploaded, Math.min(uploaded, item.size));
      if (next !== item.uploaded) {
        item.uploaded = next;
        this.options.onUpdate();
      }
      return;
    }
  }

  /** Применяет `upload.failed` из WebSocket к активной сессии. */
  markServerFailure(sessionId: string, message: string): void {
    for (const item of this.items.values()) {
      if (item.sessionId !== sessionId) continue;
      if (item.state === 'done' || item.state === 'cancelled' || item.state === 'failed') return;
      item.state = 'failed';
      item.error = message;
      this.options.onUpdate();
      return;
    }
  }

  pause(key: string): void {
    const item = this.items.get(key);
    if (!item || item.state === 'done' || item.state === 'cancelled') return;
    item.state = 'paused';
    this.controllers.get(key)?.abort('pause');
    this.options.onUpdate();
  }

  resume(key: string): void {
    const item = this.items.get(key);
    if (!item || (item.state !== 'paused' && item.state !== 'failed')) return;
    item.state = 'queued';
    item.error = null;
    this.options.onUpdate();
    void this.pump();
  }

  retry(key: string): void {
    const item = this.items.get(key);
    if (!item) return;
    item.state = 'queued';
    item.error = null;
    item.attempts = 0;
    this.options.onUpdate();
    void this.pump();
  }

  async cancel(key: string): Promise<void> {
    const item = this.items.get(key);
    if (!item) return;
    item.state = 'cancelled';
    this.controllers.get(key)?.abort('cancel');
    if (item.sessionId) {
      await api.abortBuildUpload(item.sessionId).catch(() => undefined);
    }
    this.options.onUpdate();
  }

  async cancelAll(): Promise<void> {
    const keys = this.getItems()
      .filter((item) => item.state !== 'done' && item.state !== 'cancelled')
      .map((item) => item.key);
    await Promise.all(keys.map((key) => this.cancel(key)));
  }

  async remove(key: string): Promise<void> {
    const item = this.items.get(key);
    if (!item) return;
    if (item.state !== 'done' && item.state !== 'cancelled') {
      await this.cancel(key);
    }
    this.items.delete(key);
    this.options.onUpdate();
  }

  clearFinished(): void {
    let changed = false;
    for (const [key, item] of this.items) {
      if (item.state === 'done' || item.state === 'cancelled') {
        this.items.delete(key);
        changed = true;
      }
    }
    if (changed) this.options.onUpdate();
  }

  private nextQueued(): BuildUploadItem | undefined {
    for (const item of this.items.values()) {
      if (item.state === 'queued' && !this.running.has(item.key)) return item;
    }
    return undefined;
  }

  private async pump(): Promise<void> {
    const concurrency = Math.max(1, this.options.concurrency ?? 2);
    while (this.running.size < concurrency) {
      const item = this.nextQueued();
      if (!item) break;
      this.running.add(item.key);
      void this.run(item).finally(() => {
        this.running.delete(item.key);
        this.options.onUpdate();
        void this.pump();
      });
    }
  }

  private async run(item: BuildUploadItem): Promise<void> {
    if (item.state !== 'queued') return;
    const controller = new AbortController();
    this.controllers.set(item.key, controller);
    item.state = 'preparing';
    item.error = null;
    this.options.onUpdate();

    try {
      const result = await uploadArtifact(item.file, {
        buildId: this.options.buildId,
        path: item.path,
        mode: item.mode === 'replace' ? 'replace' : undefined,
        sessionId: item.sessionId,
        chunkMaxBytes: this.options.chunkMaxBytes,
        category: item.category ?? this.options.categoryForPath(item.path),
        side: item.side,
        required: item.required,
        description: item.description,
        signal: controller.signal,
        onSession: (sessionId) => {
          item.sessionId = sessionId;
        },
        onProgress: (patch) => {
          if (patch.phase === 'preparing') item.state = 'preparing';
          if (patch.phase === 'uploading') item.state = 'uploading';
          if (patch.phase === 'finishing') item.state = 'finishing';
          if (typeof patch.uploaded === 'number') item.uploaded = patch.uploaded;
          if (patch.phase === 'uploading') this.sample(item, item.uploaded);
          if (patch.error) item.error = patch.error;
          this.options.onUpdate();
        },
      });
      item.result = result;
      item.state = 'done';
      item.uploaded = item.size;
      item.error = null;
      this.options.onCompleted?.(item);
    } catch (error) {
      if (isAbort(error)) {
        const state = item.state as BuildUploadState;
        if (controller.signal.reason === 'pause' || state === 'paused') {
          item.state = 'paused';
        } else if (state !== 'cancelled') {
          item.state = 'cancelled';
        }
        return;
      }
      if (await this.maybeRetry(item, error)) return;
      item.state = 'failed';
      item.error = describe(error);
      this.options.onFailed?.(item);
    } finally {
      this.controllers.delete(item.key);
      this.options.onUpdate();
    }
  }

  private async maybeRetry(item: BuildUploadItem, error: unknown): Promise<boolean> {
    if (!isRetryable(error) || item.attempts >= 2 || item.state === 'cancelled') return false;
    item.attempts += 1;
    item.state = 'retrying';
    item.error = describe(error);
    this.options.onUpdate();
    await new Promise((resolve) => window.setTimeout(resolve, 900 * item.attempts));
    if ((item.state as BuildUploadState) === 'cancelled') return true;
    item.state = 'queued';
    item.error = null;
    this.options.onUpdate();
    void this.pump();
    return true;
  }
}

function isRetryable(error: unknown): boolean {
  if (error instanceof ApiRequestError) {
    return error.code === 'NETWORK_ERROR' || error.status >= 500;
  }
  return false;
}

function describe(error: unknown): string {
  if (error instanceof ApiRequestError) return error.message;
  if (error instanceof Error) return error.message;
  return 'Не удалось загрузить файл.';
}
