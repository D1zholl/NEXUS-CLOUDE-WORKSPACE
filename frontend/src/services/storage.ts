/**
 * Storage Engine и тома.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  BrowseResponse,
  CreateVolumeInput,
  DetectedDisk,
  StorageSummary,
  StorageVolume,
  UpdateVolumeInput,
  VolumeCheckOutcome,
  VolumeOwner,
  VolumeScanReport,
} from './types';
import { request, queryString } from './http';

export const storageApi = {
  listStorageDisks: () => request<DetectedDisk[]>('/storage/disks'),
  listStorageVolumes: () => request<StorageVolume[]>('/storage/volumes'),
  listStorageOwners: () => request<VolumeOwner[]>('/storage/owners'),
  storageSummary: () => request<StorageSummary>('/storage/summary'),
  createStorageVolume: (input: CreateVolumeInput) =>
    request<StorageVolume>('/storage/volumes', { method: 'POST', body: input }),
  scanStoragePath: (path: string) =>
    request<VolumeScanReport>('/storage/volumes/scan', { method: 'POST', body: { path } }),
  updateStorageVolume: (id: string, input: UpdateVolumeInput) =>
    request<StorageVolume>(`/storage/volumes/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: input,
    }),
  deleteStorageVolume: (id: string) =>
    request<{ deleted: boolean }>(`/storage/volumes/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
  checkStorageVolume: (id: string) =>
    request<VolumeCheckOutcome>(`/storage/volumes/${encodeURIComponent(id)}/check`, {
      method: 'POST',
      body: {},
    }),
  browseStorage: (path?: string) =>
    request<BrowseResponse>(`/storage/browse${queryString({ path })}`),
};
