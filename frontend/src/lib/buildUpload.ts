/**
 * Загрузка артефактов сборки Minecraft: сессия → чанки → фиксация.
 *
 * Файл читается через `File.slice`, в память целиком не попадает.
 */

import { ApiRequestError, api, newIdempotencyKey, type BuildFileRow } from '../services/api';

export type ArtifactPhase = 'preparing' | 'uploading' | 'finishing' | 'done' | 'failed';

export interface ArtifactProgress {
  key: string;
  path: string;
  fileName: string;
  size: number;
  uploaded: number;
  phase: ArtifactPhase;
  error: string | null;
  result: BuildFileRow | null;
}

export interface ArtifactOptions {
  buildId: string;
  path: string;
  category?: string;
  side?: string;
  required?: boolean;
  description?: string;
  chunkMaxBytes: number;
  /** `replace` — атомарная замена существующего файла по тому же пути. */
  mode?: 'replace';
  /** Активная серверная сессия для продолжения после паузы/обрыва. */
  sessionId?: string | null;
  signal?: AbortSignal;
  /** Сообщает компоненту id сессии, чтобы пауза могла продолжить с offset. */
  onSession?: (sessionId: string) => void;
  onProgress: (patch: Partial<ArtifactProgress>) => void;
}

export function isAbort(error: unknown): boolean {
  return (
    (error instanceof DOMException && error.name === 'AbortError') ||
    (error instanceof Error && error.name === 'AbortError')
  );
}

export async function uploadArtifact(file: File, options: ArtifactOptions): Promise<BuildFileRow> {
  const { onProgress, signal } = options;
  const chunkSize = Math.max(1, Math.min(options.chunkMaxBytes, 64 * 1024 * 1024));

  onProgress({ phase: 'preparing', uploaded: 0, error: null });

  let sessionId = options.sessionId ?? null;
  let offset = 0;

  // Пауза оставляет серверную сессию активной: после resume продолжаем с
  // фактического серверного offset, не отправляя файл заново.
  if (sessionId) {
    try {
      const status = await api.buildUploadStatus(sessionId);
      if (status.status === 'active') {
        offset = Math.max(0, Math.min(status.offset, file.size));
      } else {
        sessionId = null;
      }
    } catch {
      sessionId = null;
    }
  }

  const pauseRequested = (): boolean => signal?.reason === 'pause';

  const abortServer = async (): Promise<void> => {
    if (sessionId && !pauseRequested()) {
      await api.abortBuildUpload(sessionId).catch(() => undefined);
    }
  };

  if (!sessionId) {
    const session = (await api.startBuildUpload({
      buildId: options.buildId,
      path: options.path,
      size: file.size,
      mode: options.mode,
      category: options.category,
      side: options.side,
      required: options.required,
      description: options.description,
      contentType: file.type || 'application/octet-stream',
    })) as { id: string };
    sessionId = session.id;
    options.onSession?.(sessionId);
    offset = 0;
  }

  const completeKey = newIdempotencyKey();
  onProgress({ phase: 'uploading', uploaded: offset });

  while (offset < file.size) {
    if (signal?.aborted) {
      await abortServer();
      throw new DOMException('Загрузка приостановлена', 'AbortError');
    }
    const end = Math.min(offset + chunkSize, file.size);
    const chunk = file.slice(offset, end);
    try {
      const result = await api.uploadBuildChunk(sessionId, offset, chunk);
      offset = result.offset;
      onProgress({ uploaded: offset });
    } catch (error) {
      if (error instanceof ApiRequestError && error.code === 'UPLOAD_OFFSET_MISMATCH') {
        const details = error.details as { offset?: number } | null;
        const serverOffset = details?.offset;
        if (typeof serverOffset === 'number' && serverOffset > offset) {
          offset = Math.min(serverOffset, file.size);
          onProgress({ uploaded: offset });
          continue;
        }
      }
      await abortServer();
      throw error;
    }
  }

  onProgress({ phase: 'finishing' });
  try {
    const result = await api.completeBuildUpload(sessionId, completeKey);
    onProgress({ phase: 'done', uploaded: file.size, result });
    return result;
  } catch (error) {
    await abortServer();
    throw error;
  }
}

/** Разбирает относительный путь выбранного файла (для drag&drop каталогов). */
export function relativePath(file: File, folderPrefix = ''): string {
  const relative = (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name;
  const normalized = relative.replace(/\\/g, '/').replace(/^\/+/, '');
  const prefix = folderPrefix
    .trim()
    .replace(/\\/g, '/')
    .replace(/^\/+|\/+$/g, '');
  return prefix ? `${prefix}/${normalized}` : normalized;
}

/** Подбирает категорию по расширению и первому сегменту пути. */
export function guessCategory(path: string): string {
  const lower = path.toLowerCase();
  const first = lower.split('/')[0];
  if (
    ['mods', 'config', 'resourcepacks', 'shaderpacks', 'scripts', 'overrides', 'other'].includes(
      first,
    )
  ) {
    return first;
  }
  if (lower.endsWith('.jar')) return 'mods';
  if (lower.endsWith('.zip')) return 'resourcepacks';
  if (lower.includes('shader')) return 'shaderpacks';
  if (lower.includes('script') || lower.endsWith('.zs')) return 'scripts';
  if (
    lower.endsWith('.toml') ||
    lower.endsWith('.json') ||
    lower.endsWith('.json5') ||
    lower.endsWith('.cfg') ||
    lower.endsWith('.properties') ||
    lower.endsWith('.txt')
  ) {
    return 'config';
  }
  return 'other';
}
