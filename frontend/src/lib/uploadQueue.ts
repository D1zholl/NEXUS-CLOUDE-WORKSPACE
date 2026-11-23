/**
 * Очередь загрузок Cloud Storage (PLAN §8.6).
 *
 * Требования §8.6: ограничение параллельности, пауза и продолжение с серверного
 * смещения, повтор только для сетевых ошибок и `5xx`, прогресс каждого файла,
 * общий прогресс, скорость и ETA (только при достаточной истории наблюдений).
 */

import { ApiRequestError, api, newIdempotencyKey, type CloudEntry } from '../services/api';
import { isAbort } from './cloudUpload';

export type ItemState =
  | 'queued'
  | 'preparing'
  | 'uploading'
  | 'paused'
  | 'retrying'
  | 'finishing'
  | 'done'
  | 'failed'
  | 'conflict'
  | 'cancelled';

export interface UploadItemInput {
  file: File;
  /** Сегменты относительного пути для загрузки папок. */
  segments?: string[];
  /** Имя корневой папки выбора — для прогресса по папке. */
  groupName?: string | null;
  /**
   * Каталог назначения на момент постановки в очередь. Позволяет очереди жить
   * вне страницы Cloud и не зависеть от текущей открытой папки.
   */
  parentId?: string | null;
}

export interface UploadItem {
  key: string;
  file: File;
  segments: string[];
  groupName: string | null;
  parentId: string | null;
  size: number;
  uploaded: number;
  state: ItemState;
  attempts: number;
  error: string | null;
  entry: CloudEntry | null;
  sessionId: string | null;
  /**
   * Ключ идемпотентности фиксации. Живёт в рамках одной сессии загрузки:
   * retry того же запроса переиспользует ключ, новая сессия — новый ключ.
   */
  completeKey: string | null;
  completeKeySessionId: string | null;
  /** Метки (время, байты) для расчёта скорости. */
  samples: Array<{ at: number; bytes: number }>;
}

export interface QueueOptions {
  concurrency: number;
  retries: number;
  backoffMs: number[];
  parentId: string | null;
  chunkMaxBytes: number;
  /** Создаёт/находит папки по сегментам и возвращает id конечной папки. */
  ensureFolder: (segments: string[], parentId: string | null) => Promise<string | null>;
  onUpdate: () => void;
}

export interface QueueStats {
  totalBytes: number;
  uploadedBytes: number;
  progress: number;
  speedBytesPerSec: number;
  etaSeconds: number | null;
  active: number;
  queued: number;
  failed: number;
  /** Конфликты имён, ожидающие решения пользователя. */
  conflicts: number;
  done: number;
  /** E5: прогресс по каждой выбранной папке. */
  groups: QueueGroupStats[];
}

export interface QueueGroupStats {
  name: string;
  totalBytes: number;
  uploadedBytes: number;
  progress: number;
  total: number;
  done: number;
  failed: number;
  active: number;
  queued: number;
}

const ETA_MIN_HISTORY_MS = 10_000;

let counter = 0;

export class UploadQueue {
  private items = new Map<string, UploadItem>();
  private running = new Set<string>();
  private paused = new Set<string>();
  private aborters = new Map<string, AbortController>();
  private cancelled = new Set<string>();
  private options: QueueOptions;
  private started = false;

  constructor(options: QueueOptions) {
    this.options = { ...options };
  }

  setOptions(patch: Partial<QueueOptions>): void {
    this.options = { ...this.options, ...patch };
  }

  add(inputs: UploadItemInput[]): void {
    for (const input of inputs) {
      const key = `u-${Date.now()}-${counter++}`;
      this.items.set(key, {
        key,
        file: input.file,
        segments: input.segments ?? [],
        groupName: input.groupName ?? null,
        parentId: input.parentId !== undefined ? input.parentId : this.options.parentId,
        size: input.file.size,
        uploaded: 0,
        state: 'queued',
        attempts: 0,
        error: null,
        entry: null,
        sessionId: null,
        completeKey: null,
        completeKeySessionId: null,
        samples: [],
      });
    }
    this.options.onUpdate();
    void this.pump();
  }

  resumeAll(): void {
    this.started = true;
    void this.pump();
  }

  pause(key: string): void {
    this.paused.add(key);
    this.aborters.get(key)?.abort();
    const item = this.items.get(key);
    if (item && item.state === 'uploading') {
      item.state = 'paused';
      this.options.onUpdate();
    }
  }

  resume(key: string): void {
    this.paused.delete(key);
    const item = this.items.get(key);
    if (item && (item.state === 'paused' || item.state === 'failed')) {
      item.state = 'queued';
      item.error = null;
      this.options.onUpdate();
      void this.pump();
    }
  }

  retry(key: string): void {
    this.cancelled.delete(key);
    this.paused.delete(key);
    const item = this.items.get(key);
    if (!item) return;
    item.state = 'queued';
    item.error = null;
    item.attempts = 0;
    this.options.onUpdate();
    void this.pump();
  }

  async cancel(key: string): Promise<void> {
    this.cancelled.add(key);
    this.paused.add(key);
    this.aborters.get(key)?.abort();
    const item = this.items.get(key);
    if (item?.sessionId) {
      await api.abortCloudUpload(item.sessionId).catch(() => undefined);
    }
    if (item) {
      item.state = 'cancelled';
      this.options.onUpdate();
    }
  }

  /** Отменяет все активные и ожидающие файлы, завершённые не трогает. */
  async cancelAll(): Promise<void> {
    const keys = [...this.items.values()]
      .filter(
        (item) =>
          item.state !== 'done' &&
          item.state !== 'cancelled' &&
          item.state !== 'failed' &&
          item.state !== 'conflict',
      )
      .map((item) => item.key);
    await Promise.all(keys.map((key) => this.cancel(key)));
  }

  /** Повторяет все файлы, завершившиеся сетевой ошибкой. */
  retryFailed(): void {
    for (const item of this.items.values()) {
      if (item.state === 'failed') this.retry(item.key);
    }
  }

  /**
   * Убирает запись из списка. Активную загрузку сначала корректно отменяет,
   * чтобы staging-сессия на сервере не осталась висеть.
   */
  async remove(key: string): Promise<void> {
    const item = this.items.get(key);
    if (!item) return;
    if (item.state !== 'done' && item.state !== 'cancelled') {
      await this.cancel(key);
    }
    if (this.items.delete(key)) {
      this.cancelled.delete(key);
      this.paused.delete(key);
      this.options.onUpdate();
    }
  }

  /**
   * Разрешает конфликт имени после `FILE_NAME_CONFLICT`.
   *
   * Staging-сессия и ключ идемпотентности сохраняются, поэтому «Заменить» не
   * отправляет файл повторно, а «Копия» подбирает свободное имя.
   */
  async resolveConflict(key: string, strategy: 'replace' | 'rename'): Promise<CloudEntry | null> {
    const item = this.items.get(key);
    if (!item?.sessionId) return null;
    item.state = 'finishing';
    item.error = null;
    this.options.onUpdate();
    try {
      let entry: CloudEntry | null = null;
      if (strategy === 'replace') {
        entry = await api.completeCloudUpload(
          item.sessionId,
          { strategy: 'replace' },
          item.completeKey ?? undefined,
        );
      } else {
        for (let attempt = 1; attempt <= 20; attempt += 1) {
          try {
            entry = await api.completeCloudUpload(
              item.sessionId,
              { strategy: 'rename', newName: copyName(item.file.name, attempt) },
              // Каждая попытка подобрать имя — отдельная операция фиксации.
              newIdempotencyKey(),
            );
            break;
          } catch (error) {
            if (error instanceof ApiRequestError && error.code === 'FILE_NAME_CONFLICT') {
              continue;
            }
            throw error;
          }
        }
      }
      if (!entry) throw new Error('Не удалось подобрать свободное имя файла.');
      item.entry = entry;
      item.state = 'done';
      item.uploaded = item.size;
      item.error = null;
      this.sample(item, item.size);
      this.options.onUpdate();
      return entry;
    } catch (error) {
      item.state = 'conflict';
      item.error = describe(error);
      this.options.onUpdate();
      return null;
    }
  }

  clearFinished(): boolean {
    let changed = false;
    for (const [key, item] of this.items) {
      if (item.state === 'done' || item.state === 'cancelled') {
        this.items.delete(key);
        changed = true;
      }
    }
    if (changed) this.options.onUpdate();
    return changed;
  }

  getItems(): UploadItem[] {
    return [...this.items.values()];
  }

  getItem(key: string): UploadItem | undefined {
    return this.items.get(key);
  }

  /**
   * Применяет `upload.progress` из realtime-канала.
   *
   * HTTP-ответ на чанк остаётся основным источником смещения; событие нужно,
   * чтобы прогресс не отставал при медленном ответе и был виден в других
   * вкладках. Возвращает `true`, если сессия принадлежит этой очереди.
   */
  applyServerProgress(sessionId: string, uploaded: number): boolean {
    for (const item of this.items.values()) {
      if (item.sessionId !== sessionId) continue;
      if (item.state === 'done' || item.state === 'cancelled' || item.state === 'failed') {
        return true;
      }
      const next = Math.max(item.uploaded, Math.min(uploaded, item.size));
      if (next !== item.uploaded) {
        item.uploaded = next;
        this.sample(item, next);
        this.options.onUpdate();
      }
      return true;
    }
    return false;
  }

  /** Применяет `upload.failed` из realtime-канала к активной сессии. */
  markServerFailure(sessionId: string, message: string): boolean {
    for (const item of this.items.values()) {
      if (item.sessionId !== sessionId) continue;
      if (item.state === 'cancelled' || item.state === 'done' || item.state === 'failed') {
        return true;
      }
      item.state = 'failed';
      item.error = message;
      this.options.onUpdate();
      return true;
    }
    return false;
  }

  stats(): QueueStats {
    let totalBytes = 0;
    let uploadedBytes = 0;
    let active = 0;
    let queued = 0;
    let failed = 0;
    let conflicts = 0;
    let done = 0;
    const now = Date.now();
    let speedTotal = 0;
    let speedSamples = 0;
    const groups = new Map<string, QueueGroupStats>();

    for (const item of this.items.values()) {
      totalBytes += item.size;
      uploadedBytes += item.uploaded;
      if (item.state === 'uploading' || item.state === 'preparing' || item.state === 'finishing')
        active += 1;
      if (item.state === 'queued') queued += 1;
      if (item.state === 'failed') failed += 1;
      if (item.state === 'conflict') conflicts += 1;
      if (item.state === 'done') done += 1;

      if (item.groupName) {
        const group = groups.get(item.groupName) ?? {
          name: item.groupName,
          totalBytes: 0,
          uploadedBytes: 0,
          progress: 0,
          total: 0,
          done: 0,
          failed: 0,
          active: 0,
          queued: 0,
        };
        group.totalBytes += item.size;
        group.uploadedBytes += item.uploaded;
        group.total += 1;
        if (item.state === 'done') group.done += 1;
        if (item.state === 'failed' || item.state === 'conflict') group.failed += 1;
        if (
          item.state === 'uploading' ||
          item.state === 'preparing' ||
          item.state === 'finishing'
        ) {
          group.active += 1;
        }
        if (item.state === 'queued') group.queued += 1;
        groups.set(item.groupName, group);
      }

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
      }
    }

    const speedBytesPerSec = speedSamples > 0 ? speedTotal / speedSamples : 0;
    const remaining = Math.max(totalBytes - uploadedBytes, 0);
    let etaSeconds: number | null = null;
    const enoughHistory = [...this.items.values()].some((item) => {
      if (item.samples.length < 2) return false;
      return (
        item.samples[0].at > 0 &&
        item.samples[item.samples.length - 1].at - item.samples[0].at >= ETA_MIN_HISTORY_MS
      );
    });
    if (enoughHistory && speedBytesPerSec > 1024 && remaining > 0) {
      etaSeconds = Math.round(remaining / speedBytesPerSec);
    }

    return {
      totalBytes,
      uploadedBytes,
      progress: totalBytes > 0 ? Math.min(uploadedBytes / totalBytes, 1) : 0,
      speedBytesPerSec,
      etaSeconds,
      active,
      queued,
      failed,
      conflicts,
      done,
      groups: [...groups.values()].map((group) => ({
        ...group,
        progress: group.totalBytes > 0 ? Math.min(group.uploadedBytes / group.totalBytes, 1) : 0,
      })),
    };
  }

  private nextQueued(): UploadItem | undefined {
    for (const item of this.items.values()) {
      if (
        item.state === 'queued' &&
        !this.paused.has(item.key) &&
        !this.cancelled.has(item.key) &&
        // resume() во время ещё не завершившегося run не должен запускать
        // второй параллельный run того же файла: иначе две сессии используют
        // общий completeKey и сервер отвечает IDEMPOTENCY_KEY_REUSED.
        !this.running.has(item.key)
      ) {
        return item;
      }
    }
    return undefined;
  }

  private async pump(): Promise<void> {
    // Не запускаем загрузки, пока пользователь не нажал «Загрузить».
    if (!this.started) return;
    while (this.running.size < Math.max(1, this.options.concurrency)) {
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

  private sample(item: UploadItem, bytes: number): void {
    const now = Date.now();
    item.samples.push({ at: now, bytes });
    const cutoff = now - 5000;
    while (item.samples.length > 2 && item.samples[0].at < cutoff) {
      item.samples.shift();
    }
  }

  private async run(item: UploadItem): Promise<void> {
    if (this.paused.has(item.key) || this.cancelled.has(item.key)) return;
    const aborter = new AbortController();
    this.aborters.set(item.key, aborter);

    try {
      item.state = 'preparing';
      this.options.onUpdate();

      const parentId = item.segments.length
        ? await this.options.ensureFolder(item.segments, item.parentId)
        : item.parentId;

      // Возобновление после паузы/обрыва: если сессия ещё активна, продолжаем
      // с фактического серверного смещения, а не отправляем файл заново.
      let sessionId = item.sessionId;
      let offset = item.uploaded;
      if (sessionId) {
        try {
          const status = await api.cloudUploadStatus(sessionId);
          if (status.status === 'active') {
            offset = Math.max(0, Math.min(status.offset, item.file.size));
            item.uploaded = offset;
          } else {
            sessionId = null;
          }
        } catch {
          // Сессия истекла или удалена сервером — начинаем новую.
          sessionId = null;
        }
      }
      if (!sessionId) {
        const session = await api.startCloudUpload({
          name: item.file.name,
          parentId,
          size: item.file.size,
          contentType: item.file.type || 'application/octet-stream',
        });
        sessionId = session.id;
        item.sessionId = sessionId;
        offset = 0;
        item.uploaded = 0;
      }

      item.state = 'uploading';
      this.sample(item, offset);
      this.options.onUpdate();

      let completeKey = item.completeKey;
      if (item.completeKeySessionId !== sessionId) {
        // Новая сессия — новая операция фиксации. Ключ читаем в локальную
        // переменную, чтобы параллельный run не перезаписал его между
        // отправкой complete и ответом сервера.
        completeKey = newIdempotencyKey();
        item.completeKey = completeKey;
        item.completeKeySessionId = sessionId;
      }

      const chunkSize = Math.max(1, Math.min(this.options.chunkMaxBytes, 64 * 1024 * 1024));
      while (offset < item.file.size) {
        if (this.cancelled.has(item.key)) {
          await api.abortCloudUpload(sessionId).catch(() => undefined);
          item.state = 'cancelled';
          return;
        }
        if (this.paused.has(item.key)) {
          item.state = 'paused';
          return;
        }
        const end = Math.min(offset + chunkSize, item.file.size);
        const chunk = item.file.slice(offset, end);
        try {
          const result = await api.uploadCloudChunk(sessionId, offset, chunk);
          offset = result.offset;
        } catch (error) {
          // Сервер уже принял часть данных (например, запрос оборвался после
          // записи). Продолжаем с его смещения вместо повторной отправки.
          if (error instanceof ApiRequestError && error.code === 'UPLOAD_OFFSET_MISMATCH') {
            const details = error.details as { offset?: number } | null;
            const serverOffset = details?.offset;
            if (typeof serverOffset === 'number' && serverOffset > offset) {
              offset = Math.min(serverOffset, item.file.size);
              item.uploaded = offset;
              this.sample(item, offset);
              this.options.onUpdate();
              continue;
            }
          }
          throw error;
        }
        item.uploaded = offset;
        this.sample(item, offset);
        this.options.onUpdate();
      }

      item.state = 'finishing';
      this.options.onUpdate();
      const entry = await api.completeCloudUpload(
        sessionId,
        { strategy: 'auto' },
        completeKey ?? undefined,
      );
      item.entry = entry;
      item.state = 'done';
      item.uploaded = item.size;
      this.options.onUpdate();
    } catch (error) {
      if (isAbort(error) || this.cancelled.has(item.key)) {
        item.state = this.cancelled.has(item.key) ? 'cancelled' : 'paused';
        return;
      }
      // Конфликт имени не сетевой: staging-сессия и completeKey сохраняются,
      // пользователь выбирает «Заменить» или «Копия» в виджете.
      if (error instanceof ApiRequestError && error.code === 'FILE_NAME_CONFLICT') {
        item.state = 'conflict';
        item.error = error.message;
        this.options.onUpdate();
        return;
      }
      if (await this.maybeRetry(item, error)) return;
      item.state = 'failed';
      item.error = describe(error);
      this.options.onUpdate();
    } finally {
      this.aborters.delete(item.key);
    }
  }

  /** Повтор только для сетевых ошибок и 5xx; число попыток ограничено. */
  private async maybeRetry(item: UploadItem, error: unknown): Promise<boolean> {
    const retryable = isRetryable(error);
    if (!retryable || item.attempts >= this.options.retries) return false;

    const delay =
      this.options.backoffMs[Math.min(item.attempts, this.options.backoffMs.length - 1)] ?? 1000;
    item.attempts += 1;
    item.state = 'retrying';
    this.options.onUpdate();

    // Продолжаем с фактического серверного смещения.
    if (item.sessionId) {
      try {
        const status = await api.cloudUploadStatus(item.sessionId);
        item.uploaded = status.offset;
        item.samples = [];
        this.sample(item, status.offset);
      } catch {
        item.sessionId = null;
        item.uploaded = 0;
      }
    }

    await new Promise((resolve) => setTimeout(resolve, delay));
    if (this.cancelled.has(item.key)) {
      item.state = 'cancelled';
      return true;
    }
    item.state = 'queued';
    item.error = null;
    // Возвращаем в очередь: pump подхватит с текущим смещением.
    void this.pump();
    return true;
  }
}

function isRetryable(error: unknown): boolean {
  if (error instanceof ApiRequestError) {
    if (error.code === 'NETWORK_ERROR') return true;
    return error.status >= 500;
  }
  return false;
}

function describe(error: unknown): string {
  if (error instanceof ApiRequestError) return error.message;
  if (error instanceof Error) return error.message;
  return 'Не удалось загрузить файл.';
}

/** «Копия» для конфликта имени: сохраняет расширение и не плодит суффиксы. */
function copyName(name: string, attempt: number): string {
  const dot = name.lastIndexOf('.');
  const base = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  const suffix = attempt <= 1 ? ' (копия)' : ` (копия ${attempt})`;
  return `${base}${suffix}${ext}`;
}
