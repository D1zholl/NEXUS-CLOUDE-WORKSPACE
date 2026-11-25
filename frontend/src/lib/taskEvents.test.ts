import { describe, expect, it } from 'vitest';

import type { AiTaskEvent } from '../services/ai';
import { maxTaskEventId, mergeTaskEvents, taskEventFromRealtime } from './taskEvents';

function event(id: number, kind = 'ai.task.progress'): AiTaskEvent {
  return { id, kind, stage: '', message: '', data: {}, createdAt: '2026-11-24T00:00:00Z' };
}

describe('mergeTaskEvents', () => {
  it('сохраняет возрастающий порядок и убирает дубликаты', () => {
    const merged = mergeTaskEvents([event(3), event(1)], [event(2), event(3), event(4)]);
    expect(merged.map((item) => item.id)).toEqual([1, 2, 3, 4]);
  });

  it('обновляет уже известное событие более полным представлением', () => {
    const merged = mergeTaskEvents(
      [{ ...event(5), message: '' }],
      [{ ...event(5), message: 'готово' }],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].message).toBe('готово');
  });

  it('игнорирует события с некорректным id', () => {
    const merged = mergeTaskEvents([event(1)], [{ ...event(0), id: Number.NaN }, event(2)]);
    expect(merged.map((item) => item.id)).toEqual([1, 2]);
  });

  it('позднее событие с меньшим id вставляется на своё место', () => {
    const merged = mergeTaskEvents([event(7), event(9)], [event(8)]);
    expect(merged.map((item) => item.id)).toEqual([7, 8, 9]);
  });
});

describe('maxTaskEventId', () => {
  it('возвращает 0 для пустого списка и максимум для заполненного', () => {
    expect(maxTaskEventId([])).toBe(0);
    expect(maxTaskEventId([event(2), event(11), event(5)])).toBe(11);
  });
});

describe('taskEventFromRealtime', () => {
  it('восстанавливает событие из WS payload', () => {
    const event = taskEventFromRealtime(
      {
        taskId: 't1',
        eventId: 42,
        sequence: 42,
        kind: 'ai.tool.completed',
        stage: 'tool',
        message: 'Инструмент выполнен',
        data: { toolId: 'fs.read' },
      },
      'ai.task.progress',
    );
    expect(event).toMatchObject({
      id: 42,
      kind: 'ai.tool.completed',
      stage: 'tool',
      message: 'Инструмент выполнен',
      data: { toolId: 'fs.read' },
    });
  });

  it('возвращает null без eventId (legacy payload)', () => {
    expect(taskEventFromRealtime({ taskId: 't1' }, 'ai.task.progress')).toBeNull();
  });

  it('принимает sequence как fallback и числовой строковый id', () => {
    const event = taskEventFromRealtime({ sequence: '7' }, 'ai.task.started');
    expect(event?.id).toBe(7);
    expect(event?.kind).toBe('ai.task.started');
  });
});
