import { create } from 'zustand';

import { ApiRequestError, api, type CloudEntry } from '../services/api';
import {
  UploadQueue,
  type QueueStats,
  type UploadItem,
  type UploadItemInput,
} from '../lib/uploadQueue';

/**
 * Глобальная очередь передач.
 *
 * Живёт вне страницы Cloud Storage: виджет остаётся доступным при переходе
 * между разделами, а сама загрузка не прерывается размонтированием страницы.
 */

export interface EnqueueOptions {
  parentId: string | null;
  concurrency: number;
  chunkMaxBytes: number;
}

export interface RevealRequest {
  id: number;
  parentId: string | null;
}

interface TransfersState {
  items: UploadItem[];
  stats: QueueStats | null;
  collapsed: boolean;
  hidden: boolean;
  revealRequest: RevealRequest | null;
  enqueue: (inputs: UploadItemInput[], options: EnqueueOptions) => void;
  pause: (key: string) => void;
  resume: (key: string) => void;
  retry: (key: string) => void;
  cancel: (key: string) => void;
  cancelAll: () => void;
  retryFailed: () => void;
  remove: (key: string) => void;
  clearFinished: () => void;
  resolveConflict: (key: string, strategy: 'replace' | 'rename') => Promise<CloudEntry | null>;
  applyServerProgress: (sessionId: string, uploaded: number) => void;
  markServerFailure: (sessionId: string, message: string) => void;
  setCollapsed: (collapsed: boolean) => void;
  hide: () => void;
  requestReveal: (parentId: string | null) => void;
  clearReveal: () => void;
  reset: () => void;
}

let queue: UploadQueue | null = null;
let publishTimer: number | null = null;
let revealCounter = 0;
const folderCache = new Map<string, string | null>();

/** Создаёт/находит вложенные папки, переиспользуя уже созданные. */
async function ensureFolderPath(
  segments: string[],
  baseParentId: string | null,
): Promise<string | null> {
  let current = baseParentId;
  for (let index = 0; index < segments.length; index += 1) {
    const cacheKey = `${baseParentId ?? 'root'}/${current ?? 'root'}/${segments
      .slice(0, index + 1)
      .join('/')}`;
    const cached = folderCache.get(cacheKey);
    if (cached !== undefined) {
      current = cached;
      continue;
    }
    const name = segments[index];
    try {
      const created = await api.createCloudFolder({ name, parentId: current });
      current = created.id;
    } catch (error) {
      if (error instanceof ApiRequestError && error.code === 'FILE_NAME_CONFLICT') {
        const existing = await api.listCloud({
          view: 'folder',
          parentId: current ?? undefined,
          q: name,
          perPage: 100,
        });
        const folder = existing.items.find(
          (entry) =>
            entry.entryType === 'folder' && entry.name.toLowerCase() === name.toLowerCase(),
        );
        if (!folder) throw error;
        current = folder.id;
      } else {
        throw error;
      }
    }
    folderCache.set(cacheKey, current);
  }
  return current;
}

function publishImmediately(): void {
  if (!queue) {
    useTransfers.setState({ items: [], stats: null });
    return;
  }
  const items = queue.getItems();
  useTransfers.setState({
    items,
    stats: items.length > 0 ? queue.stats() : null,
  });
}

function schedulePublish(): void {
  if (publishTimer !== null) return;
  publishTimer = window.setTimeout(() => {
    publishTimer = null;
    publishImmediately();
  }, 200);
}

function ensureQueue(options: EnqueueOptions): UploadQueue {
  if (!queue) {
    queue = new UploadQueue({
      concurrency: options.concurrency,
      retries: 3,
      backoffMs: [1000, 3000, 9000],
      parentId: options.parentId,
      chunkMaxBytes: options.chunkMaxBytes,
      ensureFolder: ensureFolderPath,
      onUpdate: schedulePublish,
    });
  }
  return queue;
}

export const useTransfers = create<TransfersState>((set) => ({
  items: [],
  stats: null,
  collapsed: false,
  hidden: false,
  revealRequest: null,

  enqueue: (inputs, options) => {
    if (inputs.length === 0) return;
    const activeQueue = ensureQueue(options);
    activeQueue.setOptions({
      concurrency: options.concurrency,
      parentId: options.parentId,
      chunkMaxBytes: options.chunkMaxBytes,
    });
    activeQueue.add(
      inputs.map((input) => ({
        ...input,
        parentId: input.parentId !== undefined ? input.parentId : options.parentId,
      })),
    );
    activeQueue.resumeAll();
    // Новая передача всегда должна быть заметной, даже после ручного закрытия.
    set({ hidden: false, collapsed: false });
    publishImmediately();
  },

  pause: (key) => {
    queue?.pause(key);
    publishImmediately();
  },

  resume: (key) => {
    queue?.resume(key);
    publishImmediately();
  },

  retry: (key) => {
    queue?.retry(key);
    publishImmediately();
  },

  cancel: (key) => {
    void queue?.cancel(key);
  },

  cancelAll: () => {
    void queue?.cancelAll();
  },

  retryFailed: () => {
    queue?.retryFailed();
    publishImmediately();
  },

  remove: (key) => {
    void queue?.remove(key);
  },

  clearFinished: () => {
    queue?.clearFinished();
    publishImmediately();
  },

  resolveConflict: async (key, strategy) => {
    const entry = (await queue?.resolveConflict(key, strategy)) ?? null;
    publishImmediately();
    return entry;
  },

  applyServerProgress: (sessionId, uploaded) => {
    queue?.applyServerProgress(sessionId, uploaded);
  },

  markServerFailure: (sessionId, message) => {
    queue?.markServerFailure(sessionId, message);
  },

  setCollapsed: (collapsed) => set({ collapsed }),

  hide: () => set({ hidden: true }),

  requestReveal: (parentId) =>
    set({
      revealRequest: { id: (revealCounter += 1), parentId },
    }),

  clearReveal: () => set({ revealRequest: null }),

  reset: () => {
    const previous = queue;
    queue = null;
    if (publishTimer !== null) {
      window.clearTimeout(publishTimer);
      publishTimer = null;
    }
    folderCache.clear();
    void previous?.cancelAll();
    set({
      items: [],
      stats: null,
      collapsed: false,
      hidden: false,
      revealRequest: null,
    });
  },
}));
