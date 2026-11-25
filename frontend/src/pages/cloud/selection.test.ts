import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import {
  MAX_SCROLL_PX_PER_FRAME,
  SCROLL_EDGE_ZONE_PX,
  edgeScrollSpeed,
  useFileSelection,
} from './selection';

describe('useFileSelection', () => {
  it('начинает с пустого выделения в обычном режиме', () => {
    const { result } = renderHook(() => useFileSelection());
    expect(result.current.count).toBe(0);
    expect(result.current.isSelectionMode).toBe(false);
    expect(result.current.selectedIds.size).toBe(0);
  });

  it('включает режим при выборе и выключает при снятии последнего файла', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.toggleSelection('a'));
    expect(result.current.isSelected('a')).toBe(true);
    expect(result.current.isSelectionMode).toBe(true);
    expect(result.current.count).toBe(1);

    act(() => result.current.toggleSelection('b'));
    expect(result.current.count).toBe(2);

    act(() => result.current.toggleSelection('a'));
    expect(result.current.isSelected('a')).toBe(false);
    expect(result.current.count).toBe(1);
    expect(result.current.isSelectionMode).toBe(true);

    act(() => result.current.toggleSelection('b'));
    expect(result.current.count).toBe(0);
    expect(result.current.isSelectionMode).toBe(false);
  });

  it('selectAll выбирает все переданные идентификаторы', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.selectAll(['a', 'b', 'c']));
    expect(result.current.count).toBe(3);
    expect(result.current.isSelectionMode).toBe(true);
    expect(result.current.isSelected('b')).toBe(true);

    act(() => result.current.selectAll([]));
    expect(result.current.count).toBe(0);
    expect(result.current.isSelectionMode).toBe(false);
  });

  it('добавляет элементы к текущему выделению', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.toggleSelection('a'));
    act(() => result.current.addSelection(['b', 'c']));
    expect(Array.from(result.current.selectedIds).sort()).toEqual(['a', 'b', 'c']);
    expect(result.current.isSelectionMode).toBe(true);
  });

  it('disableSelectionMode очищает выделение и выключает режим', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.selectAll(['a', 'b']));
    act(() => result.current.disableSelectionMode());
    expect(result.current.count).toBe(0);
    expect(result.current.isSelectionMode).toBe(false);
  });

  it('clearSelection очищает идентификаторы, но оставляет режим', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.selectAll(['a']));
    act(() => result.current.clearSelection());
    expect(result.current.count).toBe(0);
    expect(result.current.isSelectionMode).toBe(true);

    act(() => result.current.disableSelectionMode());
    expect(result.current.isSelectionMode).toBe(false);
  });

  it('setSelected заменяет выделение целиком', () => {
    const { result } = renderHook(() => useFileSelection());

    act(() => result.current.selectAll(['a', 'b']));
    act(() => result.current.setSelected(['c']));
    expect(Array.from(result.current.selectedIds)).toEqual(['c']);
    expect(result.current.isSelectionMode).toBe(true);
  });
});

describe('edgeScrollSpeed', () => {
  const size = 1000;

  it('не прокручивает в середине экрана', () => {
    expect(edgeScrollSpeed(size / 2, size)).toBe(0);
    expect(edgeScrollSpeed(SCROLL_EDGE_ZONE_PX, size)).toBe(0);
    expect(edgeScrollSpeed(size - SCROLL_EDGE_ZONE_PX, size)).toBe(0);
  });

  it('прокручивает вверх/влево у верхней и левой границы', () => {
    expect(edgeScrollSpeed(0, size)).toBe(-MAX_SCROLL_PX_PER_FRAME);
    expect(edgeScrollSpeed(40, size)).toBeCloseTo(-MAX_SCROLL_PX_PER_FRAME / 2, 5);
    expect(edgeScrollSpeed(SCROLL_EDGE_ZONE_PX - 1, size)).toBeLessThan(0);
  });

  it('прокручивает вниз/вправо у нижней и правой границы', () => {
    expect(edgeScrollSpeed(size, size)).toBe(MAX_SCROLL_PX_PER_FRAME);
    expect(edgeScrollSpeed(size - 40, size)).toBeCloseTo(MAX_SCROLL_PX_PER_FRAME / 2, 5);
    expect(edgeScrollSpeed(size - SCROLL_EDGE_ZONE_PX + 1, size)).toBeGreaterThan(0);
  });

  it('скорость растёт по мере приближения к краю', () => {
    const speeds = [SCROLL_EDGE_ZONE_PX - 1, 40, 20, 0].map((position) =>
      Math.abs(edgeScrollSpeed(position, size)),
    );
    expect(speeds[0]).toBeLessThan(speeds[1]);
    expect(speeds[1]).toBeLessThan(speeds[2]);
    expect(speeds[2]).toBeLessThan(speeds[3]);
  });

  it('держит максимальную скорость за пределами viewport', () => {
    expect(edgeScrollSpeed(-30, size)).toBe(-MAX_SCROLL_PX_PER_FRAME);
    expect(edgeScrollSpeed(size + 30, size)).toBe(MAX_SCROLL_PX_PER_FRAME);
  });
});
