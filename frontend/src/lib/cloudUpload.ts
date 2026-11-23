/**
 * Загрузка файлов в Cloud Storage: сессия → чанки → фиксация.
 *
 * Размер чанка берётся из конфигурации сервера. Файл читается через `File.slice`
 * и отправляется по частям, поэтому в памяти не держится весь файл, а прогресс
 * отражает реально принятые сервером байты.
 */

import { ApiRequestError, api, newIdempotencyKey, type CloudEntry } from '../services/api';

/** E5: пределы загрузки одной папки. */
export const FOLDER_UPLOAD_MAX_FILES = 2000;
export const FOLDER_UPLOAD_MAX_DEPTH = 100;

export type UploadSelectionErrorCode = 'TOO_MANY_FILES' | 'TOO_DEEP';

export interface RelativeFile {
  file: File;
  /** Сегменты `webkitRelativePath`, включая имя файла. */
  segments: string[];
  /** Только каталоги — то, что нужно создать перед загрузкой файла. */
  folderSegments: string[];
  /** Имя корневой папки выбора или `null` для одиночных файлов. */
  rootName: string | null;
}

export type FolderSelectionResult =
  | {
      ok: true;
      items: RelativeFile[];
      skippedEmpty: number;
      skippedOversized: number;
      rootName: string | null;
    }
  | {
      ok: false;
      code: UploadSelectionErrorCode;
      message: string;
    };

/** Разбивает относительный путь браузера на безопасные сегменты. */
export function relativePathSegments(file: File): string[] {
  const relative = (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name;
  return relative
    .replace(/\\/g, '/')
    .split('/')
    .filter((segment) => segment && segment !== '.' && segment !== '..');
}

/**
 * E5: проверка выбора папки до постановки в очередь.
 *
 * Возвращает ошибку целиком для всего выбора — ни один файл не должен начать
 * загрузку, если превышен лимит количества или глубины. Пустые и слишком
 * большие файлы пропускаются по отдельности (как и раньше).
 */
export function validateFolderSelection(
  files: File[],
  options: { maxUploadBytes?: number } = {},
): FolderSelectionResult {
  if (files.length > FOLDER_UPLOAD_MAX_FILES) {
    return {
      ok: false,
      code: 'TOO_MANY_FILES',
      message: `За один раз можно загрузить не более ${FOLDER_UPLOAD_MAX_FILES} файлов (выбрано ${files.length}). Загрузка не начата.`,
    };
  }

  const maxBytes = options.maxUploadBytes ?? 0;
  const items: RelativeFile[] = [];
  let skippedEmpty = 0;
  let skippedOversized = 0;
  let rootName: string | null = null;

  for (const file of files) {
    const segments = relativePathSegments(file);
    if (segments.length > FOLDER_UPLOAD_MAX_DEPTH) {
      return {
        ok: false,
        code: 'TOO_DEEP',
        message: `Слишком глубокая вложенность папок: ${segments.length} уровней при максимуме ${FOLDER_UPLOAD_MAX_DEPTH}. Загрузка не начата.`,
      };
    }
    if (segments.length > 1 && rootName === null) {
      rootName = segments[0];
    }
    if (file.size === 0) {
      skippedEmpty += 1;
      continue;
    }
    if (maxBytes > 0 && file.size > maxBytes) {
      skippedOversized += 1;
      continue;
    }
    items.push({
      file,
      segments,
      folderSegments: segments.slice(0, -1),
      rootName: segments.length > 1 ? segments[0] : null,
    });
  }

  return { ok: true, items, skippedEmpty, skippedOversized, rootName };
}

export type UploadPhase = 'preparing' | 'uploading' | 'finishing' | 'done' | 'failed';

export interface UploadProgress {
  /** Стабильный локальный идентификатор задачи. */
  key: string;
  fileName: string;
  size: number;
  uploaded: number;
  phase: UploadPhase;
  error: string | null;
  entry: CloudEntry | null;
}

export interface UploadOptions {
  parentId: string | null;
  chunkMaxBytes: number;
  signal?: AbortSignal;
  onProgress: (patch: Partial<UploadProgress>) => void;
}

/** Конфликт имени: данные уже на сервере, нужно выбрать стратегию. */
export class UploadConflictError extends Error {
  readonly sessionId: string;
  readonly existingFileId: string;
  /** Ключ идемпотентности неудачной фиксации: повтор использует его же. */
  readonly idempotencyKey: string;

  constructor(sessionId: string, existingFileId: string, idempotencyKey: string) {
    super('Файл с таким именем уже существует в этой папке.');
    this.name = 'UploadConflictError';
    this.sessionId = sessionId;
    this.existingFileId = existingFileId;
    this.idempotencyKey = idempotencyKey;
  }
}

export function isAbort(error: unknown): boolean {
  return (
    (error instanceof DOMException && error.name === 'AbortError') ||
    (error instanceof Error && error.name === 'AbortError')
  );
}

/** Фиксирует загруженную сессию, разрешая конфликт имени выбранной стратегией. */
export async function finishUpload(
  sessionId: string,
  strategy: 'replace' | 'rename',
  newName?: string,
  idempotencyKey?: string,
): Promise<CloudEntry> {
  return api.completeCloudUpload(
    sessionId,
    { strategy, newName },
    idempotencyKey ?? newIdempotencyKey(),
  );
}

export async function uploadFile(file: File, options: UploadOptions): Promise<CloudEntry> {
  const { onProgress, signal } = options;
  const chunkSize = Math.max(1, Math.min(options.chunkMaxBytes, 64 * 1024 * 1024));

  onProgress({ phase: 'preparing', uploaded: 0, error: null });

  const session = await api.startCloudUpload({
    name: file.name,
    parentId: options.parentId,
    size: file.size,
    contentType: file.type || 'application/octet-stream',
  });
  // Одна операция фиксации — один ключ; повтор использует его же.
  const completeKey = newIdempotencyKey();

  let offset = 0;
  onProgress({ phase: 'uploading' });

  while (offset < file.size) {
    if (signal?.aborted) {
      await api.abortCloudUpload(session.id).catch(() => undefined);
      throw new DOMException('Загрузка отменена', 'AbortError');
    }
    const end = Math.min(offset + chunkSize, file.size);
    const chunk = file.slice(offset, end);
    try {
      const result = await api.uploadCloudChunk(session.id, offset, chunk);
      offset = result.offset;
      onProgress({ uploaded: offset });
    } catch (error) {
      // Смещение могло разойтись с сервером — уточняем и продолжаем с фактической позиции.
      if (error instanceof ApiRequestError && error.code === 'UPLOAD_OFFSET_MISMATCH') {
        const details = error.details as { offset?: number } | null;
        const serverOffset = details?.offset;
        if (typeof serverOffset === 'number' && serverOffset > offset) {
          offset = Math.min(serverOffset, file.size);
          onProgress({ uploaded: offset });
          continue;
        }
      }
      await api.abortCloudUpload(session.id).catch(() => undefined);
      throw error;
    }
  }

  onProgress({ phase: 'finishing' });

  try {
    const entry = await api.completeCloudUpload(session.id, { strategy: 'auto' }, completeKey);
    onProgress({ phase: 'done', uploaded: file.size, entry });
    return entry;
  } catch (error) {
    if (error instanceof ApiRequestError && error.code === 'FILE_NAME_CONFLICT') {
      const details = error.details as { existingFileId?: string } | null;
      const existingFileId = details?.existingFileId;
      if (existingFileId) {
        // Сессию не отменяем: staged-данные нужны для повторной фиксации.
        onProgress({ phase: 'done', uploaded: file.size, error: null });
        throw new UploadConflictError(session.id, existingFileId, completeKey);
      }
    }
    await api.abortCloudUpload(session.id).catch(() => undefined);
    throw error;
  }
}
