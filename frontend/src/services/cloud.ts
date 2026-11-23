/**
 * Cloud Storage.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  CloudConfig,
  CloudEntry,
  CloudEntryDetails,
  CloudExportJob,
  CloudListResponse,
  CloudShare,
  CloudShareCreated,
  CloudUploadSession,
  ListCloudParams,
  UploadConflictStrategy,
} from './types';
import { request, queryString, idempotencyHeaders } from './http';

export const cloudApi = {
  cloudConfig: () => request<CloudConfig>('/cloud/config'),
  listCloud: (params: ListCloudParams = {}) =>
    request<CloudListResponse>(
      `/cloud/entries${queryString({
        page: params.page,
        perPage: params.perPage,
        parentId: params.parentId ?? undefined,
        q: params.q,
        sort: params.sort,
        direction: params.direction,
        view: params.view,
        entryType: params.entryType,
        category: params.category,
        sinceDays: params.sinceDays,
      })}`,
    ),
  cloudEntry: (id: string) =>
    request<CloudEntryDetails>(`/cloud/entries/${encodeURIComponent(id)}`),
  createCloudFolder: (input: { name: string; parentId: string | null }) =>
    request<CloudEntry>('/cloud/entries', { method: 'POST', body: input }),
  createCloudFoldersBatch: (input: { parentId: string | null; names: string[] }) =>
    request<CloudEntry[]>('/cloud/entries/batch', { method: 'POST', body: input }),
  updateCloudEntry: (
    id: string,
    input: {
      name?: string;
      parentId?: string | null;
      starred?: boolean;
      /** `replace` включает серверную проверку Idempotency-Key. */
      strategy?: 'replace';
    },
    idempotencyKey?: string,
  ) =>
    request<CloudEntry>(`/cloud/entries/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: input,
      headers: idempotencyHeaders(idempotencyKey),
    }),
  trashCloudEntry: (id: string) =>
    request<{ trashed: boolean }>(`/cloud/entries/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
  restoreCloudEntry: (id: string) =>
    request<CloudEntry>(`/cloud/entries/${encodeURIComponent(id)}/restore`, {
      method: 'POST',
      body: {},
    }),
  purgeCloudEntry: (id: string) =>
    request<{ deleted: boolean; files: number }>(`/cloud/entries/${encodeURIComponent(id)}/purge`, {
      method: 'DELETE',
    }),
  emptyCloudTrash: () =>
    request<{ deleted: boolean; files: number }>('/cloud/trash', { method: 'DELETE' }),

  cloudDownloadUrl: (id: string) => `/api/v1/cloud/entries/${encodeURIComponent(id)}/download`,
  cloudPreviewUrl: (id: string) => `/api/v1/cloud/entries/${encodeURIComponent(id)}/preview`,

  startCloudUpload: (input: {
    name: string;
    parentId: string | null;
    size: number;
    contentType: string;
  }) => request<CloudUploadSession>('/cloud/uploads', { method: 'POST', body: input }),
  cloudUploadStatus: (id: string) =>
    request<{
      id: string;
      offset: number;
      size: number | null;
      status: string;
      expiresAt: string;
      chunkMaxBytes: number;
    }>(`/cloud/uploads/${encodeURIComponent(id)}`),
  uploadCloudChunk: (id: string, offset: number, chunk: Blob) =>
    request<{ offset: number }>(`/cloud/uploads/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      raw: chunk,
      headers: {
        'Upload-Offset': String(offset),
        'Content-Type': 'application/offset+octet-stream',
      },
    }),
  completeCloudUpload: (
    id: string,
    input: { strategy: 'auto' | UploadConflictStrategy; newName?: string },
    idempotencyKey?: string,
  ) =>
    request<CloudEntry>(`/cloud/uploads/${encodeURIComponent(id)}/complete`, {
      method: 'POST',
      body: input,
      headers: idempotencyHeaders(idempotencyKey),
    }),
  abortCloudUpload: (id: string) =>
    request<{ aborted: boolean }>(`/cloud/uploads/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),

  listCloudShares: (entryId?: string) =>
    request<CloudShare[]>(`/cloud/shares${queryString({ entryId })}`),
  createCloudShare: (
    input: {
      entryId: string;
      expiresInHours?: number;
      password?: string;
      maxDownloads?: number | null;
      authOnly?: boolean;
    },
    idempotencyKey?: string,
  ) =>
    request<CloudShareCreated>('/cloud/shares', {
      method: 'POST',
      body: input,
      headers: idempotencyHeaders(idempotencyKey),
    }),
  revokeCloudShare: (id: string) =>
    request<{ revoked: boolean }>(`/cloud/shares/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),

  listCloudExports: () => request<CloudExportJob[]>('/cloud/exports'),
  createCloudExport: (entryIds: string[], idempotencyKey?: string) =>
    request<CloudExportJob>('/cloud/exports', {
      method: 'POST',
      body: { entryIds },
      headers: idempotencyHeaders(idempotencyKey),
    }),
  cloudExportStatus: (id: string) =>
    request<CloudExportJob>(`/cloud/exports/${encodeURIComponent(id)}`),
  deleteCloudExport: (id: string) =>
    request<{ deleted: boolean }>(`/cloud/exports/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
  cloudExportUrl: (id: string) => `/api/v1/cloud/exports/${encodeURIComponent(id)}/file`,
};
