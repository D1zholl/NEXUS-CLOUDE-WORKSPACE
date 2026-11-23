/**
 * Клиент realtime-канала (WebSocket).
 *
 * Канал **дополняет** опрос, а не заменяет его: при недоступности прокси без
 * Upgrade приложение продолжает работать через обычные запросы. Переподключение
 * выполняется с экспоненциальной задержкой, события фильтруются по типу.
 *
 * Полный каталог событий и их payload — `docs/API.md` §6.3.
 */

/** Payload-контракты событий сервера (docs/API.md §6.3). */
export interface RealtimePayloads {
  // Служебные кадры текущего соединения.
  connected: {
    userId: string;
    system: boolean;
    activeConnections: number;
  };
  lagged: { skipped: number };
  closed: { reason: string };

  // Cloud Storage: загрузка.
  'upload.progress': {
    uploadId: string;
    uploaded: number;
    size: number | null;
  };
  'upload.completed': {
    uploadId: string;
    entryId: string;
    name: string;
    size: number;
  };
  'upload.failed': {
    uploadId: string;
    entryName?: string;
    code: string;
    message: string;
    uploaded: number;
    size: number | null;
  };

  // Cloud Storage: скачивание.
  'download.progress': DownloadPayload;
  'download.completed': DownloadPayload;
  'download.failed': DownloadPayload & { code: string; message: string };

  // Фоновые задачи.
  'job.progress': {
    jobId: string;
    done: number;
    total: number;
    progress: number;
  };
  'job.paused': { jobId: string; pausedAt: string };
  'job.resumed': { jobId: string };
  'job.completed': { jobId: string; jobType: string; progress: number };
  'job.failed': { jobId: string; jobType: string; error: string };
  'job.cancelled': { jobId: string; jobType: string };

  // Хранилище, мониторинг, сборки.
  'storage.volume.updated': {
    volumeId: string;
    name: string;
    state: string;
    previousState?: string;
    totalBytes: number;
    availableBytes: number;
  };
  'system.metrics.updated': {
    cpu: number;
    memoryUsed: number;
    memoryTotal: number;
    rxRate: number;
    txRate: number;
    requestsPerSecond: number;
    activeRequests: number;
    activeDownloads: number;
    activeWebSockets: number;
  };
  'build.validation.completed': {
    buildId: string;
    valid: boolean;
    errors: number;
    warnings: number;
  };
  'build.published': {
    buildId: string;
    releaseId: string;
    version: string;
  };
  'build.publish.failed': {
    buildId: string;
    releaseId: string;
    code: string;
    message: string;
  };
  'build.unpublished': {
    buildId: string;
  };

  // NEXUS AI Workspace.
  'ai.message.started': { conversationId: string; messageId: string };
  'ai.message.delta': { conversationId: string; messageId: string; delta: string };
  'ai.message.completed': {
    conversationId: string;
    messageId: string;
    model?: string;
    finishReason?: string | null;
    usage?: Record<string, unknown>;
    cancelled?: boolean;
  };
  'ai.message.failed': {
    conversationId: string;
    messageId: string;
    code: string;
    message: string;
  };
  'ai.task.queued': { taskId: string; conversationId: string; status: string };
  'ai.task.started': { taskId: string; message?: string };
  'ai.task.progress': {
    taskId: string;
    message?: string;
    stage?: string;
    data?: Record<string, unknown>;
  };
  'ai.tool.started': {
    taskId: string;
    message?: string;
    data?: Record<string, unknown>;
  };
  'ai.tool.progress': {
    taskId: string;
    message?: string;
    data?: Record<string, unknown>;
  };
  'ai.tool.waiting_approval': {
    taskId: string;
    message?: string;
    data?: Record<string, unknown>;
  };
  'ai.tool.completed': {
    taskId: string;
    message?: string;
    data?: Record<string, unknown>;
  };
  'ai.tool.failed': {
    taskId: string;
    message?: string;
    data?: Record<string, unknown>;
  };
  'ai.task.completed': { taskId: string; message?: string };
  'ai.task.failed': { taskId: string; message?: string };
  'ai.task.cancelled': { taskId: string; message?: string };
  'ai.runtime.connected': { nodeId?: string };
  'ai.runtime.disconnected': { nodeId?: string };
}

export interface DownloadPayload extends Record<string, unknown> {
  entryId: string;
  name: string;
  transferred: number;
  size: number;
  percent: number;
}

export type RealtimeKind = keyof RealtimePayloads;

export interface RealtimeEvent<K extends string = RealtimeKind> {
  kind: K;
  userId?: string | null;
  payload: Record<string, unknown>;
  at?: string;
}

/** Событие с payload, типизированным по `kind`. */
export type RealtimeEventOf<K extends RealtimeKind> = Omit<RealtimeEvent<K>, 'payload'> & {
  payload: RealtimePayloads[K];
};

type Listener = (event: RealtimeEvent) => void;
type TypedListener<K extends RealtimeKind> = (event: RealtimeEventOf<K>) => void;

const RECONNECT_DELAYS = [1000, 2000, 4000, 8000, 15000, 30000];

/** Служебные кадры не передаются прикладным подписчикам. */
const INTERNAL_KINDS = new Set<string>(['connected', 'lagged']);

class RealtimeClient {
  private socket: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private attempt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;
  private connected = false;
  private statusListeners = new Set<(connected: boolean) => void>();

  /** Есть ли в окружении браузерный WebSocket. */
  isSupported(): boolean {
    return typeof window !== 'undefined' && typeof WebSocket !== 'undefined';
  }

  /**
   * Открывает соединение. Никогда не бросает исключение: при отказе прокси в
   * Upgrade интерфейс продолжает работать через HTTP-опрос.
   */
  connect(): void {
    if (!this.isSupported()) {
      this.setConnected(false);
      return;
    }
    if (
      this.socket &&
      (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING)
    ) {
      return;
    }
    this.stopped = false;
    const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
    const url = `${scheme}://${window.location.host}/api/v1/ws`;

    let socket: WebSocket;
    try {
      socket = new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;

    socket.onopen = () => {
      this.attempt = 0;
      this.setConnected(true);
    };
    socket.onmessage = (message) => {
      try {
        const parsed = JSON.parse(String(message.data)) as RealtimeEvent;
        if (!parsed || typeof parsed.kind !== 'string') return;
        if (INTERNAL_KINDS.has(parsed.kind)) return;
        if (parsed.kind === 'closed') {
          socket.close();
          return;
        }
        for (const listener of this.listeners) {
          try {
            listener(parsed);
          } catch {
            // Ошибка обработчика не должна ломать канал.
          }
        }
      } catch {
        // Некорректный кадр игнорируется.
      }
    };
    socket.onclose = () => {
      this.setConnected(false);
      this.socket = null;
      if (!this.stopped) this.scheduleReconnect();
    };
    socket.onerror = () => {
      // onclose выполнит переподключение.
      socket.close();
    };
  }

  disconnect(): void {
    this.stopped = true;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.setConnected(false);
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onclose = null;
      socket.close();
    }
  }

  isConnected(): boolean {
    return this.connected;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Подписка на конкретный тип события с типизированным payload. */
  on<K extends RealtimeKind>(kind: K, listener: TypedListener<K>): () => void {
    return this.subscribe((event) => {
      if (event.kind === kind) listener(event as RealtimeEventOf<K>);
    });
  }

  /** Подписка на любой тип из набора; удобно для инвалидации кэша. */
  onAny<K extends RealtimeKind>(kinds: readonly K[], listener: TypedListener<K>): () => void {
    const allowed = new Set<string>(kinds);
    return this.subscribe((event) => {
      if (allowed.has(event.kind)) listener(event as RealtimeEventOf<K>);
    });
  }

  onStatus(listener: (connected: boolean) => void): () => void {
    this.statusListeners.add(listener);
    return () => {
      this.statusListeners.delete(listener);
    };
  }

  private setConnected(value: boolean): void {
    if (this.connected === value) return;
    this.connected = value;
    for (const listener of this.statusListeners) {
      try {
        listener(value);
      } catch {
        // ignore
      }
    }
  }

  private scheduleReconnect(): void {
    if (this.stopped || this.timer) return;
    const delay = RECONNECT_DELAYS[Math.min(this.attempt, RECONNECT_DELAYS.length - 1)];
    this.attempt += 1;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.connect();
    }, delay);
  }
}

export const realtime = new RealtimeClient();

/** Хук-хелпер: подписка на события realtime с автоматической отпиской. */
export function subscribeRealtime<K extends RealtimeKind>(
  kind: K,
  listener: TypedListener<K>,
): () => void {
  return realtime.on(kind, listener);
}

/** Проверяет, что событие относится к указанной сборке. */
export function isBuildEvent(event: RealtimeEvent, buildId: string): boolean {
  return event.kind.startsWith('build.') && String(event.payload.buildId ?? '') === buildId;
}
