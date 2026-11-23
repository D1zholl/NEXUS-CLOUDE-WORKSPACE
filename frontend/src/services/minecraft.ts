/**
 * Minecraft Build Manager.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  BuildFileRow,
  BuildRow,
  BuildStats,
  BuildValidationReport,
  CloudEntryDetailResponse,
  CreateBuildInput,
  ReleaseHistoryRow,
  ReleaseRow,
  UpdateBuildInput,
} from './types';
import { request, queryString, idempotencyHeaders } from './http';

export const minecraftApi = {
  minecraftStats: () => request<BuildStats>('/minecraft/stats'),
  listBuilds: (params: Record<string, string | number | undefined> = {}) =>
    request<{ items: BuildRow[]; total: number; page: number; perPage: number }>(
      `/minecraft/builds${queryString(params)}`,
    ),
  getBuild: (id: string) =>
    request<CloudEntryDetailResponse>(`/minecraft/builds/${encodeURIComponent(id)}`),
  createBuild: (input: CreateBuildInput) =>
    request<BuildRow>('/minecraft/builds', { method: 'POST', body: input }),
  updateBuild: (id: string, input: UpdateBuildInput) =>
    request<BuildRow>(`/minecraft/builds/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: input,
    }),
  deleteBuild: (id: string) =>
    request<{ deleted: boolean }>(`/minecraft/builds/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
  cloneBuild: (id: string) =>
    request<BuildRow>(`/minecraft/builds/${encodeURIComponent(id)}/clone`, {
      method: 'POST',
      body: {},
    }),
  archiveBuild: (id: string) =>
    request<BuildRow>(`/minecraft/builds/${encodeURIComponent(id)}/archive`, {
      method: 'POST',
      body: {},
    }),
  unpublishBuild: (id: string) =>
    request<BuildRow>(`/minecraft/builds/${encodeURIComponent(id)}/unpublish`, {
      method: 'POST',
      body: {},
    }),
  restoreBuild: (id: string) =>
    request<BuildRow>(`/minecraft/builds/${encodeURIComponent(id)}/restore`, {
      method: 'POST',
      body: {},
    }),
  validateBuild: (id: string) =>
    request<BuildValidationReport>(`/minecraft/builds/${encodeURIComponent(id)}/validate`, {
      method: 'POST',
      body: {},
    }),

  listBuildFiles: (id: string, params: Record<string, string | undefined> = {}) =>
    request<{
      items: BuildFileRow[];
      stats: { files: number; folders: number; totalBytes: number };
    }>(`/minecraft/builds/${encodeURIComponent(id)}/files${queryString(params)}`),
  createBuildFolder: (id: string, path: string) =>
    request<BuildFileRow>(`/minecraft/builds/${encodeURIComponent(id)}/files/folder`, {
      method: 'POST',
      body: { path },
    }),
  updateBuildFile: (
    id: string,
    fileId: string,
    input: {
      path?: string;
      category?: string;
      side?: string;
      required?: boolean;
      enabled?: boolean;
      description?: string;
    },
  ) =>
    request<BuildFileRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/files/${encodeURIComponent(fileId)}`,
      { method: 'PATCH', body: input },
    ),
  deleteBuildFile: (id: string, fileId: string) =>
    request<{ removed: number }>(
      `/minecraft/builds/${encodeURIComponent(id)}/files/${encodeURIComponent(fileId)}`,
      { method: 'DELETE' },
    ),

  listReleases: (id: string) =>
    request<ReleaseRow[]>(`/minecraft/builds/${encodeURIComponent(id)}/releases`),
  createRelease: (
    id: string,
    input: { version?: string; changelog?: string; channel?: 'stable' | 'beta' },
  ) =>
    request<ReleaseRow>(`/minecraft/builds/${encodeURIComponent(id)}/releases`, {
      method: 'POST',
      body: input,
    }),
  publishRelease: (id: string, releaseId: string, acceptWarnings = false) =>
    request<ReleaseRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/publish`,
      { method: 'POST', body: { acceptWarnings } },
    ),
  activateRelease: (id: string, releaseId: string) =>
    request<ReleaseRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/activate`,
      { method: 'POST', body: {} },
    ),
  deprecateRelease: (id: string, releaseId: string) =>
    request<ReleaseRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/deprecate`,
      { method: 'POST', body: {} },
    ),
  archiveRelease: (id: string, releaseId: string) =>
    request<ReleaseRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/archive`,
      { method: 'POST', body: {} },
    ),
  releaseManifest: (id: string, releaseId: string) =>
    request<Record<string, unknown>>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/manifest`,
    ),
  launcherReleaseManifest: (id: string, releaseId: string) =>
    request<Record<string, unknown>>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/${encodeURIComponent(releaseId)}/launcher-manifest`,
    ),
  compareReleases: (id: string, left: string, right: string) =>
    request<{
      left: { id: string; version: string };
      right: { id: string; version: string };
      added: Array<{ path: string; size: number; sha256: string }>;
      removed: Array<{ path: string; size: number; sha256: string }>;
      changed: Array<{ path: string; size: number; sha256: string; previousSha256: string }>;
    }>(
      `/minecraft/builds/${encodeURIComponent(id)}/releases/compare${queryString({ left, right })}`,
    ),
  buildHistory: (id: string) =>
    request<ReleaseHistoryRow[]>(`/minecraft/builds/${encodeURIComponent(id)}/history`),
  uploadBuildIcon: (id: string, blob: Blob) =>
    request<{ iconUrl: string }>(`/minecraft/builds/${encodeURIComponent(id)}/icon`, {
      method: 'PUT',
      raw: blob,
      headers: { 'Content-Type': blob.type || 'application/octet-stream' },
    }),
  deleteBuildIcon: (id: string) =>
    request<{ deleted: boolean }>(`/minecraft/builds/${encodeURIComponent(id)}/icon`, {
      method: 'DELETE',
    }),
  buildIconUrl: (id: string) => `/api/v1/minecraft/builds/${encodeURIComponent(id)}/icon`,
  replaceBuildFile: (id: string, fileId: string, uploadId: string) =>
    request<BuildFileRow>(
      `/minecraft/builds/${encodeURIComponent(id)}/files/${encodeURIComponent(fileId)}/replace`,
      { method: 'POST', body: { uploadId } },
    ),

  minecraftUploadConfig: () =>
    request<{
      chunkMaxBytes: number;
      maxUploadBytes: number;
      uploadTtlHours: number;
      serverTime: string;
    }>('/minecraft/uploads/config'),
  startBuildUpload: (input: {
    buildId: string;
    path: string;
    size: number;
    /** `replace` — заменить содержимое существующего файла с тем же путём. */
    mode?: 'replace';
    category?: string;
    side?: string;
    required?: boolean;
    description?: string;
    contentType?: string;
  }) => request<Record<string, unknown>>('/minecraft/uploads', { method: 'POST', body: input }),
  uploadBuildChunk: (id: string, offset: number, chunk: Blob) =>
    request<{ offset: number }>(`/minecraft/uploads/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      raw: chunk,
      headers: {
        'Upload-Offset': String(offset),
        'Content-Type': 'application/offset+octet-stream',
      },
    }),
  buildUploadStatus: (id: string) =>
    request<{
      id: string;
      offset: number;
      size: number;
      status: string;
      expiresAt: string;
      chunkMaxBytes: number;
    }>(`/minecraft/uploads/${encodeURIComponent(id)}`),
  completeBuildUpload: (id: string, idempotencyKey?: string) =>
    request<BuildFileRow>(`/minecraft/uploads/${encodeURIComponent(id)}/complete`, {
      method: 'POST',
      body: {},
      headers: idempotencyHeaders(idempotencyKey),
    }),
  abortBuildUpload: (id: string) =>
    request<{ aborted: boolean }>(`/minecraft/uploads/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),

  minecraftVersions: () =>
    request<{
      latest: { release: string; snapshot: string } | null;
      versions: Array<{ id: string; releasedAt: string | null }>;
    }>('/minecraft/metadata/minecraft'),
  fabricVersions: () =>
    request<{ minecraft: string[]; loader: string[] }>('/minecraft/metadata/fabric'),
  forgeVersions: () =>
    request<{ entries: Array<{ minecraft: string; loader: string }> }>('/minecraft/metadata/forge'),
  neoforgeVersions: () =>
    request<{ entries: Array<{ minecraft: string; loader: string }> }>(
      '/minecraft/metadata/neoforge',
    ),
  quiltVersions: () => request<{ loader: string[] }>('/minecraft/metadata/quilt'),
  loaderVersions: (loader: string, minecraft: string) =>
    request<{ loader: string; minecraft: string; versions: string[] }>(
      `/minecraft/metadata/loaders${queryString({ loader, minecraft })}`,
    ),
};
