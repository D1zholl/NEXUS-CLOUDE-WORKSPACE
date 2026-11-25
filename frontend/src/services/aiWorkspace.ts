/** Клиент файлового API AI Agent Workspace. */
import { newIdempotencyKey, queryString, request } from './http';

export interface WorkspaceEntry {
  path: string;
  name: string;
  type: 'dir' | 'file';
  size: number;
  modified: string | null;
}

export interface WorkspaceTree {
  path: string;
  entries: WorkspaceEntry[];
  truncated: boolean;
}

export interface WorkspaceFileContent {
  path: string;
  content: string;
  sha256: string;
  truncated: boolean;
  encoding: 'utf8' | 'base64' | string;
  maxEditableBytes?: number;
}

export interface SaveFileResult {
  path: string;
  sha256: string;
  bytes: number;
  savedAt: string;
}

export interface WorkspaceOverview {
  workspaceId: string;
  name: string;
  description: string;
  status: string;
  runtimeNodeId: string;
  runtimeOnline: boolean;
  createdAt: string;
  updatedAt: string;
  lastActivityAt: string | null;
  tasksTotal: number;
  tasksActive: number;
  files: number;
  bytes: number;
  quotaBytes: number;
  maxFiles: number;
  maxFileBytes: number;
  maxParallelTasks: number;
  allowedTools: string[];
  allowedPrograms: string[];
  snapshots: number;
  usageCheckedAt: string | null;
  usageError: string | null;
}

export interface FileSearchHit {
  path: string;
  line: number;
  preview: string;
}

export interface FileSearchResult {
  query: string;
  results: FileSearchHit[];
  visitedFiles: number;
  truncated: boolean;
}

export interface FileSnapshot {
  id: string;
  taskId: string | null;
  executionId: string | null;
  path: string;
  action: string;
  beforeSha256: string | null;
  afterSha256: string | null;
  truncated: boolean;
  additions: number;
  deletions: number;
  restoredAt: string | null;
  createdAt: string | null;
  hasBefore: boolean;
  hasAfter: boolean;
}

export interface SnapshotContent {
  content?: string;
  contentBase64?: string;
  encoding: 'utf8' | 'base64';
}

export interface FileSnapshotDetail extends FileSnapshot {
  before: SnapshotContent | null;
  after: SnapshotContent | null;
}

export interface RestoreConflict {
  path: string;
  reason: string;
  message?: string;
  currentSha256?: string | null;
  expectedSha256?: string | null;
}

export interface RestoreResult {
  taskId: string | null;
  restored: Array<{ path: string; action: string }>;
}

export interface DeletedWorkspace {
  workspaceId: string;
  originalName: string;
  state: string;
  requestedAt: string;
  purgeAfter: string;
  attempts: number;
  purgedAt: string | null;
  restoredAt: string | null;
}

const base = '/ai';

async function rawBlob(id: string, path: string, signal?: AbortSignal): Promise<Blob> {
  const response = await fetch(
    `/api/v1${base}/workspaces/${id}/files/raw${queryString({ path })}`,
    { credentials: 'same-origin', signal },
  );
  if (!response.ok) {
    let message = 'Не удалось получить файл.';
    let code = `HTTP_${response.status}`;
    try {
      const payload = (await response.json()) as {
        error?: { message?: string; code?: string };
      };
      message = payload.error?.message ?? message;
      code = payload.error?.code ?? code;
    } catch {
      // Не JSON — оставляем общий текст.
    }
    const error = new Error(message) as Error & { code?: string; status?: number };
    error.code = code;
    error.status = response.status;
    throw error;
  }
  return response.blob();
}

export const aiWorkspaceApi = {
  tree: (id: string, path = '', depth = 1, signal?: AbortSignal) =>
    request<WorkspaceTree>(
      `${base}/workspaces/${id}/files${queryString({ path, depth })}`,
      { signal },
    ),
  content: (id: string, path: string, signal?: AbortSignal) =>
    request<WorkspaceFileContent>(
      `${base}/workspaces/${id}/files/content${queryString({ path })}`,
      { signal },
    ),
  stat: (id: string, path: string) =>
    request<{
      path: string;
      type: 'file' | 'dir';
      size: number;
      modified: string | null;
      readonly: boolean;
    }>(`${base}/workspaces/${id}/files/stat${queryString({ path })}`),
  save: (id: string, body: { path: string; content: string; baseSha256?: string | null }) =>
    request<SaveFileResult>(`${base}/workspaces/${id}/files/content`, {
      method: 'PUT',
      body,
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  download: (id: string, path: string) => rawBlob(id, path),
  raw: (id: string, path: string, signal?: AbortSignal) => rawBlob(id, path, signal),
  mkdir: (id: string, path: string) =>
    request<{ path: string; created: boolean }>(`${base}/workspaces/${id}/files/directory`, {
      method: 'POST',
      body: { path },
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  move: (id: string, from: string, to: string, overwrite = false) =>
    request<{ from: string; to: string; moved: boolean }>(`${base}/workspaces/${id}/files/move`, {
      method: 'POST',
      body: { from, to, overwrite },
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  copy: (id: string, from: string, to: string, overwrite = false) =>
    request<{ from: string; to: string; copied: boolean }>(`${base}/workspaces/${id}/files/copy`, {
      method: 'POST',
      body: { from, to, overwrite },
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  remove: (id: string, path: string, recursive = false) =>
    request<{ path: string; deleted: boolean }>(
      `${base}/workspaces/${id}/files${queryString({ path, recursive: recursive ? true : undefined })}`,
      {
        method: 'DELETE',
        headers: { 'Idempotency-Key': newIdempotencyKey() },
      },
    ),
  search: (id: string, body: { query: string; path?: string; maxResults?: number }) =>
    request<FileSearchResult>(`${base}/workspaces/${id}/files/search`, {
      method: 'POST',
      body,
    }),
  upload: (id: string, path: string, file: File) => {
    const formData = new FormData();
    formData.append('path', path);
    formData.append('file', file);
    return request<{ path: string; bytes: number; sha256: string }>(
      `${base}/workspaces/${id}/files/upload`,
      { method: 'POST', formData },
    );
  },
  overview: (id: string) =>
    request<WorkspaceOverview>(`${base}/workspaces/${id}/overview`),
  changes: (
    id: string,
    params: { taskId?: string; path?: string; limit?: number } = {},
  ) =>
    request<{ items: FileSnapshot[] }>(
      `${base}/workspaces/${id}/changes${queryString({
        taskId: params.taskId,
        path: params.path,
        limit: params.limit,
      })}`,
    ),
  snapshot: (id: string, snapshotId: string) =>
    request<FileSnapshotDetail>(`${base}/workspaces/${id}/snapshots/${snapshotId}`),
  restoreSnapshot: (id: string, snapshotId: string) =>
    request<RestoreResult>(`${base}/workspaces/${id}/snapshots/${snapshotId}/restore`, {
      method: 'POST',
      body: {},
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  restoreTask: (id: string, taskId: string) =>
    request<RestoreResult>(`${base}/workspaces/${id}/tasks/${taskId}/restore`, {
      method: 'POST',
      body: {},
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  createWorkspace: (body: { name: string; description?: string; runtimeNodeId?: string }) =>
    request<Record<string, unknown>>(`${base}/workspaces`, {
      method: 'POST',
      body,
      headers: { 'Idempotency-Key': newIdempotencyKey() },
    }),
  updateWorkspace: (id: string, body: { name?: string; description?: string }) =>
    request<Record<string, unknown>>(`${base}/workspaces/${id}`, {
      method: 'PATCH',
      body,
    }),
  deleteWorkspace: (id: string) =>
    request<{ deleted: boolean; softDelete: boolean }>(`${base}/workspaces/${id}`, {
      method: 'DELETE',
    }),
  listDeleted: () =>
    request<{ items: DeletedWorkspace[]; retentionDays: number }>(
      `${base}/workspaces/deleted`,
    ),
  restoreWorkspace: (id: string) =>
    request<Record<string, unknown>>(`${base}/workspaces/${id}/restore`, {
      method: 'POST',
      body: {},
    }),
};

/** Извлекает детали конфликта сохранения из ошибки API. */
export function saveConflictDetails(error: unknown): {
  currentContent: string | null;
  currentSha256: string | null;
} | null {
  if (!error || typeof error !== 'object') return null;
  const details = (error as { details?: unknown }).details;
  if (!details || typeof details !== 'object') return null;
  const value = details as Record<string, unknown>;
  if (typeof value.currentContent !== 'string' && typeof value.currentSha256 !== 'string') {
    return null;
  }
  return {
    currentContent:
      typeof value.currentContent === 'string' ? value.currentContent : null,
    currentSha256:
      typeof value.currentSha256 === 'string' ? value.currentSha256 : null,
  };
}

/** Скачивает blob под исходным именем файла. */
export function saveBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 2000);
}
