import { describe, expect, it, vi, beforeEach } from 'vitest';

import { UploadQueue } from './uploadQueue';

/**
 * Тесты очереди загрузок (PLAN §8.6): параллельность, пауза/продолжение,
 * повтор только для 5xx и отмена с освобождением staging.
 */

interface MockApi {
  startCloudUpload: ReturnType<typeof vi.fn>;
  uploadCloudChunk: ReturnType<typeof vi.fn>;
  completeCloudUpload: ReturnType<typeof vi.fn>;
  abortCloudUpload: ReturnType<typeof vi.fn>;
  cloudUploadStatus: ReturnType<typeof vi.fn>;
}

vi.mock('../services/api', () => {
  class ApiRequestError extends Error {
    code: string;
    status: number;
    details: unknown;
    constructor(status: number, code: string, message: string, details: unknown = null) {
      super(message);
      this.code = code;
      this.status = status;
      this.details = details;
    }
  }
  return {
    ApiRequestError,
    newIdempotencyKey: vi.fn(() => `key-${Math.random().toString(36).slice(2)}`),
    api: {
      startCloudUpload: vi.fn(),
      uploadCloudChunk: vi.fn(),
      completeCloudUpload: vi.fn(),
      abortCloudUpload: vi.fn(),
      cloudUploadStatus: vi.fn(),
    },
  };
});

vi.mock('./cloudUpload', () => ({
  isAbort: (error: unknown) =>
    (error instanceof Error && error.name === 'AbortError') ||
    (error as { name?: string })?.name === 'AbortError',
}));

const apiModule = await import('../services/api');
const mockApi = apiModule.api as unknown as MockApi;

function makeFile(name: string, size: number): File {
  return new File([new Uint8Array(size)], name, { type: 'application/octet-stream' });
}

function makeQueue(concurrency = 2, retries = 3) {
  const onUpdate = vi.fn();
  const queue = new UploadQueue({
    concurrency,
    retries,
    backoffMs: [1, 1, 1],
    parentId: null,
    chunkMaxBytes: 4,
    ensureFolder: async () => null,
    onUpdate,
  });
  return { queue, onUpdate };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.startCloudUpload.mockImplementation(async () => ({ id: 'session-1' }));
  mockApi.uploadCloudChunk.mockImplementation(async (_id: string, offset: number, chunk: Blob) => ({
    offset: offset + chunk.size,
  }));
  mockApi.completeCloudUpload.mockImplementation(async () => ({
    id: 'entry-1',
    name: 'file.bin',
    entryType: 'file',
    sizeBytes: 4,
  }));
  mockApi.abortCloudUpload.mockResolvedValue({ aborted: true });
  mockApi.cloudUploadStatus.mockResolvedValue({
    id: 'session-1',
    offset: 0,
    status: 'active',
  });
});

describe('UploadQueue', () => {
  it('respects the concurrency limit', async () => {
    const { queue } = makeQueue(2);
    let active = 0;
    let peak = 0;
    mockApi.uploadCloudChunk.mockImplementation(async (_id, offset, chunk) => {
      active += 1;
      peak = Math.max(peak, active);
      await new Promise((resolve) => setTimeout(resolve, 5));
      active -= 1;
      return { offset: offset + (chunk as Blob).size };
    });

    queue.add([
      { file: makeFile('a.bin', 8) },
      { file: makeFile('b.bin', 8) },
      { file: makeFile('c.bin', 8) },
      { file: makeFile('d.bin', 8) },
    ]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.stats().done).toBe(4), { timeout: 4000 });
    expect(peak).toBeLessThanOrEqual(2);
  });

  it('pause stops transfer and resume continues from server offset', async () => {
    const { queue } = makeQueue(1);
    const chunks: number[] = [];
    mockApi.uploadCloudChunk.mockImplementation(async (_id, offset, chunk) => {
      chunks.push(offset);
      await new Promise((resolve) => setTimeout(resolve, 10));
      return { offset: offset + (chunk as Blob).size };
    });

    queue.add([{ file: makeFile('p.bin', 16) }]);
    queue.resumeAll();

    await vi.waitFor(() => expect(chunks.length).toBeGreaterThan(0), { timeout: 2000 });
    const key = queue.getItems()[0].key;
    queue.pause(key);

    const pausedItem = queue.getItem(key);
    expect(['paused', 'uploading']).toContain(pausedItem?.state);

    // Сервер сообщает фактическое смещение после паузы.
    mockApi.cloudUploadStatus.mockResolvedValue({
      id: 'session-1',
      offset: 4,
      status: 'active',
    });
    await vi.waitFor(() => expect(queue.getItem(key)?.state).not.toBe('uploading'), {
      timeout: 2000,
    });

    queue.resume(key);
    await vi.waitFor(() => expect(queue.stats().done).toBe(1), { timeout: 4000 });

    // Продолжение использует ту же сессию и серверное смещение 4,
    // а не отправляет первые 4 байта повторно.
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(1);
    expect(chunks.filter((offset) => offset === 4).length).toBeGreaterThan(0);
  });

  it('resume during an in-flight chunk does not start a second run', async () => {
    const { queue } = makeQueue(1);
    let releaseFirst!: () => void;
    const firstChunk = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    let firstStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      firstStarted = resolve;
    });
    let first = true;
    mockApi.uploadCloudChunk.mockImplementation(async (_id, offset, chunk) => {
      if (first) {
        first = false;
        firstStarted();
        await firstChunk;
      }
      return { offset: offset + (chunk as Blob).size };
    });

    queue.add([{ file: makeFile('race.bin', 16) }]);
    queue.resumeAll();
    await started;

    const key = queue.getItems()[0].key;
    queue.pause(key);
    // Пользователь сразу нажимает «Продолжить», пока PATCH ещё в полёте.
    queue.resume(key);
    releaseFirst();

    await vi.waitFor(() => expect(queue.stats().done).toBe(1), { timeout: 4000 });
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(1);
    expect(mockApi.completeCloudUpload).toHaveBeenCalledTimes(1);
  });

  it('reuses one idempotency key when completion is retried', async () => {
    const { queue } = makeQueue(1, 2);
    const error = Object.assign(new Error('server error'), { status: 500, code: 'HTTP_500' });
    Object.setPrototypeOf(error, apiModule.ApiRequestError.prototype);
    let attempts = 0;
    mockApi.completeCloudUpload.mockImplementation(async () => {
      attempts += 1;
      if (attempts === 1) throw error;
      return { id: 'entry-1', name: 'idem.bin', entryType: 'file', sizeBytes: 4 };
    });

    queue.add([{ file: makeFile('idem.bin', 4) }]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.stats().done).toBe(1), { timeout: 6000 });
    const keys = mockApi.completeCloudUpload.mock.calls.map((call) => (call as unknown[])[2]);
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBe(keys[0]);
  });

  it('retries only on 5xx and is bounded', async () => {
    const { queue } = makeQueue(1, 2);
    const error = Object.assign(new Error('server error'), { status: 500, code: 'HTTP_500' });
    Object.setPrototypeOf(error, apiModule.ApiRequestError.prototype);
    mockApi.uploadCloudChunk.mockRejectedValue(error);

    queue.add([{ file: makeFile('r.bin', 8) }]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.getItem(queue.getItems()[0].key)?.state).toBe('failed'), {
      timeout: 6000,
    });
    // Начальная попытка + 2 повтора, но загрузка продолжается в той же сессии.
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(1);
    expect(mockApi.cloudUploadStatus).toHaveBeenCalled();
  });

  it('does not retry on 4xx', async () => {
    const { queue } = makeQueue(1, 3);
    const error = Object.assign(new Error('bad request'), { status: 400, code: 'HTTP_400' });
    Object.setPrototypeOf(error, apiModule.ApiRequestError.prototype);
    mockApi.uploadCloudChunk.mockRejectedValue(error);

    queue.add([{ file: makeFile('x.bin', 8) }]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.getItem(queue.getItems()[0].key)?.state).toBe('failed'), {
      timeout: 3000,
    });
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(1);
  });

  it('cancel aborts upload and releases staging', async () => {
    const { queue } = makeQueue(1);
    mockApi.uploadCloudChunk.mockImplementation(async (_id, offset, chunk) => {
      await new Promise((resolve) => setTimeout(resolve, 20));
      return { offset: offset + (chunk as Blob).size };
    });

    queue.add([{ file: makeFile('c.bin', 32) }]);
    queue.resumeAll();
    await vi.waitFor(() => expect(queue.getItems()[0].sessionId).not.toBeNull(), { timeout: 2000 });

    await queue.cancel(queue.getItems()[0].key);
    expect(mockApi.abortCloudUpload).toHaveBeenCalled();
    expect(queue.getItem(queue.getItems()[0].key)?.state).toBe('cancelled');
  });

  it('reports total progress and hides ETA without history', async () => {
    const { queue } = makeQueue(2);
    queue.add([{ file: makeFile('a.bin', 100) }, { file: makeFile('b.bin', 100) }]);
    const stats = queue.stats();
    expect(stats.totalBytes).toBe(200);
    expect(stats.etaSeconds).toBeNull();
  });

  it('reports progress per uploaded folder', async () => {
    const { queue } = makeQueue(2);
    queue.add([
      { file: makeFile('a.bin', 100), groupName: 'Docs' },
      { file: makeFile('b.bin', 50), groupName: 'Docs' },
      { file: makeFile('c.bin', 25), groupName: 'Photos' },
    ]);

    const stats = queue.stats();
    const docs = stats.groups.find((group) => group.name === 'Docs');
    const photos = stats.groups.find((group) => group.name === 'Photos');

    expect(docs).toMatchObject({ total: 2, totalBytes: 150, progress: 0 });
    expect(photos).toMatchObject({ total: 1, totalBytes: 25 });
  });

  it('keeps a name conflict for manual replace or copy', async () => {
    const { queue } = makeQueue(1);
    const error = Object.assign(new Error('conflict'), {
      status: 409,
      code: 'FILE_NAME_CONFLICT',
    });
    Object.setPrototypeOf(error, apiModule.ApiRequestError.prototype);
    mockApi.completeCloudUpload.mockRejectedValueOnce(error);

    queue.add([{ file: makeFile('same.bin', 4) }]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.getItems()[0].state).toBe('conflict'), {
      timeout: 3000,
    });
    expect(queue.stats().conflicts).toBe(1);
    const key = queue.getItems()[0].key;
    const originalSession = queue.getItem(key)?.sessionId;
    expect(originalSession).toBeTruthy();

    await queue.resolveConflict(key, 'rename');

    expect(queue.getItem(key)?.state).toBe('done');
    const calls = mockApi.completeCloudUpload.mock.calls;
    expect(calls[calls.length - 1][1]).toMatchObject({ strategy: 'rename' });
    expect(String(calls[calls.length - 1][1].newName)).toContain('(копия)');
    // Файл не загружается заново: используется та же staging-сессия.
    expect(queue.getItem(key)?.sessionId).toBe(originalSession);
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(1);
  });

  it('cancelAll cancels queued and running files only', async () => {
    const { queue } = makeQueue(1);
    mockApi.uploadCloudChunk.mockImplementation(async (_id, offset, chunk) => {
      await new Promise((resolve) => setTimeout(resolve, 20));
      return { offset: offset + (chunk as Blob).size };
    });

    queue.add([
      { file: makeFile('a.bin', 16) },
      { file: makeFile('b.bin', 16) },
      { file: makeFile('c.bin', 16) },
    ]);
    queue.resumeAll();
    await vi.waitFor(() => expect(queue.getItems()[0].sessionId).not.toBeNull(), { timeout: 2000 });

    await queue.cancelAll();
    expect(queue.getItems().every((item) => item.state === 'cancelled')).toBe(true);
    expect(mockApi.abortCloudUpload).toHaveBeenCalled();
  });

  it('uses the parent captured for each item, not the latest page folder', async () => {
    const calls: Array<{ segments: string[]; parentId: string | null }> = [];
    const queue = new UploadQueue({
      concurrency: 2,
      retries: 0,
      backoffMs: [1],
      parentId: null,
      chunkMaxBytes: 4,
      ensureFolder: async (segments, parentId) => {
        calls.push({ segments, parentId });
        return `folder-${parentId}`;
      },
      onUpdate: vi.fn(),
    });

    queue.add([
      { file: makeFile('a.bin', 4), segments: ['A', 'a.bin'], parentId: 'root-a' },
      { file: makeFile('b.bin', 4), segments: ['B', 'b.bin'], parentId: 'root-b' },
    ]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.stats().done).toBe(2), { timeout: 4000 });
    expect(calls).toContainEqual({ segments: ['A', 'a.bin'], parentId: 'root-a' });
    expect(calls).toContainEqual({ segments: ['B', 'b.bin'], parentId: 'root-b' });
  });

  it('passes the complete nested folder path to ensureFolder without truncation', async () => {
    const calls: string[][] = [];
    const onUpdate = vi.fn();
    const queue = new UploadQueue({
      concurrency: 1,
      retries: 0,
      backoffMs: [1],
      parentId: null,
      chunkMaxBytes: 4,
      ensureFolder: async (segments) => {
        calls.push(segments);
        return `folder-${segments.length}`;
      },
      onUpdate,
    });

    const deep = Array.from({ length: 100 }, (_, index) => `level-${index}`);
    const nested = [...deep, 'payload.bin'];

    queue.add([
      { file: makeFile('payload.bin', 4), segments: nested, groupName: 'Deep' },
      { file: makeFile('second.bin', 4), segments: ['root', 'second.bin'], groupName: 'Root' },
    ]);
    queue.resumeAll();

    await vi.waitFor(() => expect(queue.stats().done).toBe(2), { timeout: 4000 });
    expect(calls).toContainEqual(nested);
    expect(calls).toContainEqual(['root', 'second.bin']);
    expect(mockApi.startCloudUpload).toHaveBeenCalledTimes(2);
  });

  it('keeps folder groups separate when files from different roots interleave', async () => {
    const { queue } = makeQueue(2);
    queue.add([
      { file: makeFile('a.bin', 10), groupName: 'RootA' },
      { file: makeFile('b.bin', 20), groupName: 'RootB' },
      { file: makeFile('c.bin', 30), groupName: 'RootA' },
    ]);

    const groups = queue.stats().groups;
    expect(groups).toHaveLength(2);
    expect(groups.find((group) => group.name === 'RootA')).toMatchObject({
      total: 2,
      totalBytes: 40,
      progress: 0,
    });
    expect(groups.find((group) => group.name === 'RootB')).toMatchObject({
      total: 1,
      totalBytes: 20,
      progress: 0,
    });
  });
});
