/**
 * Инкрементальная загрузка событий Agent-задачи.
 *
 * Backend отдаёт `GET /ai/tasks/{id}/events?after=<id>` в возрастающем порядке.
 * Клиент хранит максимальный полученный id и запрашивает только новые события.
 * WebSocket-события содержат тот же `eventId`/`sequence`, поэтому HTTP и WS
 * можно безопасно смешивать: дубликаты отбрасываются, порядок восстанавливается
 * по id. Полная страница (без `after`) запрашивается только при открытии задачи
 * и после reconnect — это позволяет добрать события, пропущенные при обрыве.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import type { AiTaskEvent } from '../services/ai';
import { aiApi } from '../services/ai';

export interface TaskEventStats {
  /** Сколько событий пришло из HTTP и WS (с повторами). */
  received: number;
  /** Повторно доставленные события (тот же eventId). */
  duplicates: number;
  /** Событие пришло позже уже известных с большим id; вставлено по порядку. */
  outOfOrder: number;
  /** Полных синхронизаций после reconnect/открытия задачи. */
  fullSyncs: number;
}

export const EMPTY_TASK_EVENT_STATS: TaskEventStats = {
  received: 0,
  duplicates: 0,
  outOfOrder: 0,
  fullSyncs: 0,
};

function validId(event: AiTaskEvent): boolean {
  return Number.isFinite(event.id) && event.id > 0;
}

/**
 * Сливает события по `id`: дубликаты схлопываются, порядок — возрастающий.
 * Более новое представление события (например, пришедшее из HTTP после WS)
 * дополняет уже сохранённое.
 */
export function mergeTaskEvents(
  existing: readonly AiTaskEvent[],
  incoming: readonly AiTaskEvent[],
): AiTaskEvent[] {
  if (incoming.length === 0) return existing as AiTaskEvent[];
  const byId = new Map<number, AiTaskEvent>();
  for (const event of existing) {
    if (validId(event)) byId.set(event.id, event);
  }
  for (const event of incoming) {
    if (!validId(event)) continue;
    const previous = byId.get(event.id);
    byId.set(event.id, previous ? { ...previous, ...event } : event);
  }
  return [...byId.values()].sort((left, right) => left.id - right.id);
}

export function maxTaskEventId(events: readonly AiTaskEvent[]): number {
  let max = 0;
  for (const event of events) {
    if (validId(event) && event.id > max) max = event.id;
  }
  return max;
}

function numberFrom(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

/**
 * Преобразует WS-payload в событие задачи. Возвращает null для старых
 * сообщений без `eventId` — вызывающая сторона выполнит HTTP-синхронизацию.
 */
export function taskEventFromRealtime(
  payload: Record<string, unknown>,
  fallbackKind: string,
  at?: string,
): AiTaskEvent | null {
  const id = numberFrom(payload.eventId ?? payload.sequence);
  if (id === null || id <= 0) return null;
  const kind = typeof payload.kind === 'string' && payload.kind ? payload.kind : fallbackKind;
  return {
    id,
    kind,
    stage: typeof payload.stage === 'string' ? payload.stage : '',
    message: typeof payload.message === 'string' ? payload.message : '',
    data:
      payload.data && typeof payload.data === 'object'
        ? (payload.data as Record<string, unknown>)
        : {},
    createdAt: at ?? new Date().toISOString(),
  };
}

export interface IncrementalTaskEvents {
  items: AiTaskEvent[];
  loading: boolean;
  error: string | null;
  stats: TaskEventStats;
  /** Применяет событие из WebSocket; false — payload без eventId. */
  ingestRealtime: (payload: Record<string, unknown>, kind: string) => boolean;
  /** Полная синхронизация последней страницы (reconnect, ручное обновление). */
  reload: () => void;
}

export function useIncrementalTaskEvents(
  taskId: string | undefined,
  pollWhileActive: boolean,
): IncrementalTaskEvents {
  const [items, setItems] = useState<AiTaskEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState<TaskEventStats>(EMPTY_TASK_EVENT_STATS);

  const itemsRef = useRef<AiTaskEvent[]>([]);
  const lastIdRef = useRef(0);
  const taskRef = useRef<string | undefined>(undefined);
  const busyRef = useRef(false);
  const pendingFullRef = useRef(false);
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const apply = useCallback((incoming: AiTaskEvent[]) => {
    const previousItems = itemsRef.current;
    const previousLast = lastIdRef.current;
    let duplicates = 0;
    let outOfOrder = 0;
    const known = new Set(previousItems.map((event) => event.id));
    for (const event of incoming) {
      if (!validId(event)) continue;
      if (known.has(event.id)) {
        duplicates += 1;
      } else if (event.id < previousLast) {
        outOfOrder += 1;
      }
    }
    const merged = mergeTaskEvents(previousItems, incoming);
    itemsRef.current = merged;
    lastIdRef.current = Math.max(previousLast, maxTaskEventId(merged));
    setItems(merged);
    if (incoming.length > 0) {
      setStats((current) => ({
        ...current,
        received: current.received + incoming.length,
        duplicates: current.duplicates + duplicates,
        outOfOrder: current.outOfOrder + outOfOrder,
      }));
    }
  }, []);

  const load = useCallback(
    async (full: boolean) => {
      const id = taskRef.current;
      if (!id) return;
      if (busyRef.current) {
        // Не теряем запрос полной синхронизации, если уже идёт инкрементальный.
        if (full) pendingFullRef.current = true;
        return;
      }
      busyRef.current = true;
      if (full) pendingFullRef.current = false;
      setLoading(true);
      try {
        const after = full ? undefined : lastIdRef.current || undefined;
        const page = await aiApi.taskEvents(id, { after, perPage: 200 });
        if (!aliveRef.current || taskRef.current !== id) return;
        apply(page.items);
        setError(null);
        if (full) {
          setStats((current) => ({ ...current, fullSyncs: current.fullSyncs + 1 }));
        }
      } catch (cause) {
        if (aliveRef.current && taskRef.current === id) {
          setError(cause instanceof Error ? cause.message : 'Не удалось загрузить события задачи.');
        }
      } finally {
        busyRef.current = false;
        if (aliveRef.current) setLoading(false);
        if (pendingFullRef.current && taskRef.current === id) {
          pendingFullRef.current = false;
          void load(true);
        }
      }
    },
    [apply],
  );

  // Открытие другой задачи: сбрасываем состояние и читаем последнюю страницу.
  useEffect(() => {
    taskRef.current = taskId;
    itemsRef.current = [];
    lastIdRef.current = 0;
    pendingFullRef.current = false;
    setItems([]);
    setError(null);
    setStats(EMPTY_TASK_EVENT_STATS);
    if (!taskId) {
      setLoading(false);
      return;
    }
    void load(true);
  }, [taskId, load]);

  // Инкрементальный опрос активной задачи.
  useEffect(() => {
    if (!taskId || !pollWhileActive) return;
    const timer = window.setInterval(() => {
      void load(false);
    }, 2500);
    return () => window.clearInterval(timer);
  }, [taskId, pollWhileActive, load]);

  // Задача завершилась: добираем последние события один раз, чтобы не
  // зависеть от доставки финального WS-кадра.
  const wasActiveRef = useRef(false);
  useEffect(() => {
    if (wasActiveRef.current && !pollWhileActive && taskId) {
      void load(false);
    }
    wasActiveRef.current = pollWhileActive;
  }, [pollWhileActive, taskId, load]);

  const ingestRealtime = useCallback(
    (payload: Record<string, unknown>, kind: string): boolean => {
      if (!taskRef.current) return false;
      const event = taskEventFromRealtime(payload, kind);
      if (!event) return false;
      apply([event]);
      return true;
    },
    [apply],
  );

  const reload = useCallback(() => {
    void load(true);
  }, [load]);

  return { items, loading, error, stats, ingestRealtime, reload };
}
