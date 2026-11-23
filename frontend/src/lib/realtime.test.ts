import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';

/**
 * Тесты realtime-клиента: фильтрация событий и переподключение.
 * WebSocket подменяется фейковой реализацией.
 */

class FakeSocket {
  static instances: FakeSocket[] = [];
  url: string;
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];

  constructor(url: string) {
    this.url = url;
    FakeSocket.instances.push(this);
  }

  open(): void {
    this.readyState = 1;
    this.onopen?.();
  }

  emit(data: unknown): void {
    this.onmessage?.({ data: JSON.stringify(data) });
  }

  close(): void {
    this.readyState = 3;
    this.onclose?.();
  }

  send(value: string): void {
    this.sent.push(value);
  }
}

const OPEN = 1;
const CONNECTING = 0;

beforeEach(() => {
  FakeSocket.instances = [];
  vi.stubGlobal(
    'WebSocket',
    Object.assign(FakeSocket, { OPEN, CONNECTING }) as unknown as typeof WebSocket,
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('realtime client', () => {
  it('delivers only events of the subscribed kind', async () => {
    const { realtime } = await import('./realtime');
    realtime.disconnect();
    realtime.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();

    const received: string[] = [];
    const unsubscribe = realtime.on('upload.progress', (event) => {
      received.push(event.kind);
    });

    socket.emit({ kind: 'connected' });
    socket.emit({ kind: 'upload.progress', payload: { uploaded: 10 } });
    socket.emit({ kind: 'job.progress', payload: {} });
    socket.emit({ kind: 'upload.progress', payload: { uploaded: 20 } });

    expect(received).toEqual(['upload.progress', 'upload.progress']);
    unsubscribe();
    realtime.disconnect();
  });

  it('ignores malformed frames without breaking', async () => {
    const { realtime } = await import('./realtime');
    realtime.disconnect();
    realtime.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();

    let called = 0;
    realtime.subscribe(() => {
      called += 1;
    });

    socket.onmessage?.({ data: 'not-json' });
    socket.emit({ kind: 'upload.completed', payload: {} });
    expect(called).toBe(1);
    realtime.disconnect();
  });

  it('schedules reconnect after close', async () => {
    vi.useFakeTimers();
    const { realtime } = await import('./realtime');
    realtime.disconnect();
    realtime.connect();
    const first = FakeSocket.instances.at(-1)!;
    first.open();
    expect(realtime.isConnected()).toBe(true);

    first.close();
    expect(realtime.isConnected()).toBe(false);
    expect(FakeSocket.instances.length).toBe(1);

    // Экспоненциальная задержка: первая попытка через 1 секунду.
    await vi.advanceTimersByTimeAsync(1100);
    expect(FakeSocket.instances.length).toBe(2);

    realtime.disconnect();
  });

  it('disconnect stops reconnection', async () => {
    vi.useFakeTimers();
    const { realtime } = await import('./realtime');
    realtime.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();
    realtime.disconnect();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.instances.length).toBe(1);
  });

  it('delivers typed payload for a subscribed event', async () => {
    const { realtime } = await import('./realtime');
    realtime.disconnect();
    realtime.connect();
    const socket = FakeSocket.instances.at(-1)!;
    socket.open();

    const updates: Array<{ done: number; progress: number }> = [];
    const unsubscribe = realtime.on('job.progress', (event) => {
      updates.push({ done: event.payload.done, progress: event.payload.progress });
    });

    socket.emit({
      kind: 'job.progress',
      userId: null,
      payload: { jobId: 'job-1', done: 25, total: 100, progress: 25 },
    });

    expect(updates).toEqual([{ done: 25, progress: 25 }]);
    unsubscribe();
    realtime.disconnect();
  });

  it('continues without WebSocket (no Upgrade) and retries later', async () => {
    vi.useFakeTimers();
    class BrokenSocket {
      static OPEN = OPEN;
      static CONNECTING = CONNECTING;
      constructor() {
        throw new Error('WebSocket upgrade is not supported');
      }
    }
    vi.stubGlobal('WebSocket', BrokenSocket as unknown as typeof WebSocket);

    const { realtime } = await import('./realtime');
    realtime.disconnect();

    // Интерфейс не должен падать: HTTP-polling продолжает работать.
    expect(() => realtime.connect()).not.toThrow();
    expect(realtime.isConnected()).toBe(false);

    let events = 0;
    const unsubscribe = realtime.on('upload.completed', () => {
      events += 1;
    });

    await vi.advanceTimersByTimeAsync(1100);
    expect(realtime.isConnected()).toBe(false);
    expect(events).toBe(0);

    unsubscribe();
    realtime.disconnect();
  });

  it('reports unsupported environment without scheduling attempts', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', undefined);
    const { realtime } = await import('./realtime');
    realtime.disconnect();
    expect(realtime.isSupported()).toBe(false);
    expect(() => realtime.connect()).not.toThrow();
    expect(realtime.isConnected()).toBe(false);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(realtime.isConnected()).toBe(false);
  });
});
