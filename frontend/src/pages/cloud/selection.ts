import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from 'react';

/**
 * Состояние множественного выделения в Cloud Storage.
 *
 * Режим включается, как только появляется хотя бы один выбранный элемент, и
 * автоматически выключается, когда выделение опустело. Это позволяет клику по
 * карточке в обычном режиме открывать файл, а в режиме выделения — переключать
 * чекбокс.
 */
export interface FileSelectionState {
  selectedIds: Set<string>;
  isSelectionMode: boolean;
  count: number;
  isSelected: (id: string) => boolean;
  toggleSelection: (id: string) => void;
  selectAll: (ids: Iterable<string>) => void;
  setSelected: (ids: Iterable<string>) => void;
  addSelection: (ids: Iterable<string>) => void;
  clearSelection: () => void;
  enableSelectionMode: () => void;
  disableSelectionMode: () => void;
}

interface SelectionStore {
  ids: Set<string>;
  mode: boolean;
}

export function useFileSelection(initial?: Iterable<string>): FileSelectionState {
  const [store, setStore] = useState<SelectionStore>(() => ({
    ids: new Set(initial ?? []),
    mode: Boolean(initial && Array.from(initial).length > 0),
  }));

  const enableSelectionMode = useCallback(() => {
    setStore((current) => (current.mode ? current : { ...current, mode: true }));
  }, []);

  const clearSelection = useCallback(() => {
    setStore((current) => (current.ids.size === 0 ? current : { ...current, ids: new Set() }));
  }, []);

  const disableSelectionMode = useCallback(() => {
    setStore((current) =>
      !current.mode && current.ids.size === 0 ? current : { ids: new Set(), mode: false },
    );
  }, []);

  const toggleSelection = useCallback((id: string) => {
    setStore((current) => {
      const next = new Set(current.ids);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return { ids: next, mode: next.size > 0 };
    });
  }, []);

  const selectAll = useCallback((ids: Iterable<string>) => {
    const next = new Set(ids);
    setStore({ ids: next, mode: next.size > 0 });
  }, []);

  const setSelected = useCallback((ids: Iterable<string>) => {
    const next = new Set(ids);
    setStore({ ids: next, mode: next.size > 0 });
  }, []);

  const addSelection = useCallback((ids: Iterable<string>) => {
    const incoming = Array.from(ids);
    if (incoming.length === 0) return;
    setStore((current) => {
      const next = new Set(current.ids);
      for (const id of incoming) next.add(id);
      return { ids: next, mode: true };
    });
  }, []);

  const isSelected = useCallback((id: string) => store.ids.has(id), [store.ids]);

  return useMemo(
    () => ({
      selectedIds: store.ids,
      isSelectionMode: store.mode,
      count: store.ids.size,
      isSelected,
      toggleSelection,
      selectAll,
      setSelected,
      addSelection,
      clearSelection,
      enableSelectionMode,
      disableSelectionMode,
    }),
    [
      store.ids,
      store.mode,
      isSelected,
      toggleSelection,
      selectAll,
      setSelected,
      addSelection,
      clearSelection,
      enableSelectionMode,
      disableSelectionMode,
    ],
  );
}

/* ------------------------------ drag selection ----------------------------- */

const LONG_PRESS_MS = 500;
/** Порог в пикселях: до него жест считается кликом, а не выделением. */
const DRAG_THRESHOLD_PX = 6;
/** Сколько миллисекунд после drag/long-press подавлять клик по карточке. */
const CLICK_SUPPRESS_MS = 450;
/** Ширина зоны у края viewport, в которой включается auto-scroll. */
export const SCROLL_EDGE_ZONE_PX = 80;
/** Максимальная скорость прокрутки за кадр — у самого края экрана. */
export const MAX_SCROLL_PX_PER_FRAME = 18;
/** Минимальная скорость у внутренней границы зоны. */
const MIN_SCROLL_PX_PER_FRAME = 2;

const INTERACTIVE_SELECTOR =
  'button, a, input, select, textarea, label, [role="menuitem"], [data-no-drag]';

type ScrollTarget = HTMLElement | null;

/**
 * Скорость auto-scroll для координаты указателя.
 *
 * Отрицательное значение — вверх/влево, положительное — вниз/вправо.
 * Внутри зоны скорость линейно растёт от минимальной у границы до
 * максимальной у края.
 */
export function edgeScrollSpeed(position: number, size: number): number {
  if (position < 0) return -MAX_SCROLL_PX_PER_FRAME;
  if (position > size) return MAX_SCROLL_PX_PER_FRAME;
  if (position < SCROLL_EDGE_ZONE_PX) {
    return -Math.min(
      MAX_SCROLL_PX_PER_FRAME,
      Math.max(
        MIN_SCROLL_PX_PER_FRAME,
        MAX_SCROLL_PX_PER_FRAME * (1 - position / SCROLL_EDGE_ZONE_PX),
      ),
    );
  }
  if (position > size - SCROLL_EDGE_ZONE_PX) {
    const distance = size - position;
    return Math.min(
      MAX_SCROLL_PX_PER_FRAME,
      Math.max(
        MIN_SCROLL_PX_PER_FRAME,
        MAX_SCROLL_PX_PER_FRAME * (1 - distance / SCROLL_EDGE_ZONE_PX),
      ),
    );
  }
  return 0;
}

/**
 * Ищет ближайший прокручиваемый предок по нужной оси. `null` означает
 * прокрутку самого окна — это запасной вариант для обычной страницы.
 */
function findScrollTarget(element: HTMLElement, axis: 'x' | 'y'): ScrollTarget {
  let node: HTMLElement | null = element;
  while (node && node !== document.body && node !== document.documentElement) {
    const style = window.getComputedStyle(node);
    const overflow = axis === 'y' ? style.overflowY : style.overflowX;
    const scrollable = overflow === 'auto' || overflow === 'scroll' || overflow === 'overlay';
    const hasOverflow =
      axis === 'y'
        ? node.scrollHeight > node.clientHeight + 1
        : node.scrollWidth > node.clientWidth + 1;
    if (scrollable && hasOverflow) return node;
    node = node.parentElement;
  }
  return null;
}

function canScrollTarget(target: ScrollTarget, axis: 'x' | 'y'): boolean {
  if (target) {
    return axis === 'y'
      ? target.scrollHeight > target.clientHeight + 1
      : target.scrollWidth > target.clientWidth + 1;
  }
  const doc = document.scrollingElement ?? document.documentElement;
  return axis === 'y'
    ? doc.scrollHeight > window.innerHeight + 1
    : doc.scrollWidth > window.innerWidth + 1;
}

function scrollTargetBy(target: ScrollTarget, dx: number, dy: number): void {
  if (target) {
    if (dx !== 0) target.scrollLeft += dx;
    if (dy !== 0) target.scrollTop += dy;
    return;
  }
  // Окно прокручивается отдельными вызовами, чтобы не сбрасывать ось,
  // которая в данный момент не используется.
  if (dx !== 0) window.scrollBy(dx, 0);
  if (dy !== 0) window.scrollBy(0, dy);
}

export interface SelectionBand {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface SelectionGesturesOptions {
  /** Выделение доступно только когда есть элементы и не открыт раздел ссылок. */
  enabled: boolean;
  isSelectionMode: boolean;
  selectedIds: Set<string>;
  onEnableSelectionMode: () => void;
  onToggleSelection: (id: string) => void;
  onSetSelection: (ids: string[]) => void;
  onExitSelectionMode: () => void;
}

export interface SelectionGestures {
  containerRef: RefObject<HTMLDivElement | null>;
  band: SelectionBand | null;
  /** Прямо сейчас пользователь тянет рамку. */
  isDragging: boolean;
  containerProps: {
    onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
    onClickCapture: (event: ReactMouseEvent<HTMLDivElement>) => void;
    onClick: (event: ReactMouseEvent<HTMLDivElement>) => void;
    onContextMenu: (event: ReactMouseEvent<HTMLDivElement>) => void;
  };
  /** Клик после drag/long-press нужно игнорировать. */
  isClickSuppressed: () => boolean;
}

interface DragStart {
  pointerId: number;
  startX: number;
  startY: number;
  additive: boolean;
  baseIds: string[];
  cardId: string | null;
  dragging: boolean;
}

function elementFrom(target: EventTarget | null): Element | null {
  return target instanceof Element ? target : null;
}

function intersects(a: DOMRect, b: SelectionBand): boolean {
  return !(
    a.right < b.left ||
    a.left > b.left + b.width ||
    a.bottom < b.top ||
    a.top > b.top + b.height
  );
}

/**
 * Жесты выделения: rubber-band мышью и long-press на touch-устройствах.
 *
 * Рамка строится по координатам указателя, а карточки определяются по
 * `data-entry-id` внутри контейнера. Пересечение считается по реальным
 * bounding-box, поэтому работает и для сетки, и для таблицы.
 */
export function useSelectionGestures({
  enabled,
  isSelectionMode,
  selectedIds,
  onEnableSelectionMode,
  onToggleSelection,
  onSetSelection,
  onExitSelectionMode,
}: SelectionGesturesOptions): SelectionGestures {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [band, setBand] = useState<SelectionBand | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const suppressUntilRef = useRef(0);
  const cleanupRef = useRef<(() => void) | null>(null);
  const rafRef = useRef<number | null>(null);

  const isClickSuppressed = useCallback(
    () => typeof performance !== 'undefined' && performance.now() < suppressUntilRef.current,
    [],
  );

  useEffect(
    () => () => {
      if (rafRef.current !== null) {
        window.cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      cleanupRef.current?.();
      cleanupRef.current = null;
    },
    [],
  );

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!enabled || event.button !== 0) return;
      const target = elementFrom(event.target);
      const container = containerRef.current;
      if (!container) return;
      const card = target?.closest<HTMLElement>('[data-entry-id]');
      const cardId = card && container.contains(card) ? (card.dataset.entryId ?? null) : null;
      // Внутри карточки drag может начаться и с кнопки: порог в 6 px
      // отличит жест от обычного клика. Вне карточки интерактивные элементы
      // не трогаем.
      if (!cardId && target?.closest(INTERACTIVE_SELECTOR)) return;

      // Подавление от предыдущего жеста уже не нужно: его клик прошёл.
      suppressUntilRef.current = 0;

      const start: DragStart = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        additive: event.shiftKey,
        baseIds: Array.from(selectedIds),
        cardId,
        dragging: false,
      };

      let longPressTimer: number | null = null;
      if (cardId && event.pointerType !== 'mouse') {
        longPressTimer = window.setTimeout(() => {
          longPressTimer = null;
          if (start.dragging) return;
          suppressUntilRef.current = performance.now() + CLICK_SUPPRESS_MS;
          onEnableSelectionMode();
          if (!selectedIds.has(cardId)) onToggleSelection(cardId);
        }, LONG_PRESS_MS);
      }

      // Точка старта запоминается в координатах содержимого контейнера.
      // При прокрутке окна или родительского контейнера рамка остаётся
      // привязанной к тем карточкам, с которых началось выделение.
      const initialRect = container.getBoundingClientRect();
      const anchorLocalX = start.startX - initialRect.left + container.scrollLeft;
      const anchorLocalY = start.startY - initialRect.top + container.scrollTop;
      const verticalTarget = findScrollTarget(container, 'y');
      const horizontalTarget = findScrollTarget(container, 'x');
      const canScrollY = canScrollTarget(verticalTarget, 'y');
      const canScrollX = canScrollTarget(horizontalTarget, 'x');
      let pointerX = start.startX;
      let pointerY = start.startY;

      const stopAutoScroll = (): void => {
        if (rafRef.current !== null) {
          window.cancelAnimationFrame(rafRef.current);
          rafRef.current = null;
        }
      };

      const updateSelectionAt = (clientX: number, clientY: number): void => {
        const rect = container.getBoundingClientRect();
        const pointerLocalX = clientX - rect.left + container.scrollLeft;
        const pointerLocalY = clientY - rect.top + container.scrollTop;
        const width = Math.abs(pointerLocalX - anchorLocalX);
        const height = Math.abs(pointerLocalY - anchorLocalY);

        setBand({
          left: Math.min(anchorLocalX, pointerLocalX),
          top: Math.min(anchorLocalY, pointerLocalY),
          width,
          height,
        });

        // Пересечение считаем в клиентских координатах: карточки двигаются
        // вместе с прокруткой, а якорь рамки остаётся на своём месте.
        const anchorClientX = anchorLocalX + rect.left - container.scrollLeft;
        const anchorClientY = anchorLocalY + rect.top - container.scrollTop;
        const clientBand: SelectionBand = {
          left: Math.min(anchorClientX, clientX),
          top: Math.min(anchorClientY, clientY),
          width: Math.abs(clientX - anchorClientX),
          height: Math.abs(clientY - anchorClientY),
        };
        const hits: string[] = [];
        for (const node of container.querySelectorAll<HTMLElement>('[data-entry-id]')) {
          const id = node.dataset.entryId;
          if (!id) continue;
          const box = node.getBoundingClientRect();
          if (box.width === 0 && box.height === 0) continue;
          if (intersects(box, clientBand)) hits.push(id);
        }

        if (start.additive) {
          onSetSelection(Array.from(new Set([...start.baseIds, ...hits])));
        } else {
          onSetSelection(hits);
        }
      };

      /**
       * Кадр auto-scroll. Позиция указателя может не меняться, поэтому
       * скорость пересчитывается по последней известной точке каждый кадр.
       */
      const autoScrollTick = (): void => {
        rafRef.current = null;
        if (!start.dragging) return;
        const dy = canScrollY ? edgeScrollSpeed(pointerY, window.innerHeight) : 0;
        const dx = canScrollX ? edgeScrollSpeed(pointerX, window.innerWidth) : 0;
        if (dy === 0 && dx === 0) return;
        const beforeTop = verticalTarget ? verticalTarget.scrollTop : window.scrollY;
        const beforeLeft = horizontalTarget ? horizontalTarget.scrollLeft : window.scrollX;
        scrollTargetBy(verticalTarget, 0, dy);
        scrollTargetBy(horizontalTarget, dx, 0);
        const afterTop = verticalTarget ? verticalTarget.scrollTop : window.scrollY;
        const afterLeft = horizontalTarget ? horizontalTarget.scrollLeft : window.scrollX;
        // Дошли до границы прокрутки — цикл больше не нужен.
        if (afterTop === beforeTop && afterLeft === beforeLeft) return;
        // После прокрутки пересчитываем и рамку, и пересекающиеся карточки.
        updateSelectionAt(pointerX, pointerY);
        rafRef.current = window.requestAnimationFrame(autoScrollTick);
      };

      const ensureAutoScroll = (): void => {
        const dy = canScrollY ? edgeScrollSpeed(pointerY, window.innerHeight) : 0;
        const dx = canScrollX ? edgeScrollSpeed(pointerX, window.innerWidth) : 0;
        if (dy === 0 && dx === 0) {
          stopAutoScroll();
          return;
        }
        if (rafRef.current === null) {
          rafRef.current = window.requestAnimationFrame(autoScrollTick);
        }
      };

      const cleanup = (): void => {
        stopAutoScroll();
        if (longPressTimer !== null) {
          window.clearTimeout(longPressTimer);
          longPressTimer = null;
        }
        window.removeEventListener('pointermove', handleMove);
        window.removeEventListener('pointerup', handleUp);
        window.removeEventListener('pointercancel', handleUp);
        cleanupRef.current = null;
      };

      const handleMove = (moveEvent: PointerEvent): void => {
        if (moveEvent.pointerId !== start.pointerId) return;
        const dx = moveEvent.clientX - start.startX;
        const dy = moveEvent.clientY - start.startY;

        if (!start.dragging && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;

        if (!start.dragging) {
          start.dragging = true;
          if (longPressTimer !== null) {
            window.clearTimeout(longPressTimer);
            longPressTimer = null;
          }
          setIsDragging(true);
          // Выделение начинается даже из обычного режима.
          if (!isSelectionMode) onEnableSelectionMode();
          window.getSelection()?.removeAllRanges();
        }

        moveEvent.preventDefault();

        pointerX = moveEvent.clientX;
        pointerY = moveEvent.clientY;
        updateSelectionAt(pointerX, pointerY);
        ensureAutoScroll();
      };

      const handleUp = (upEvent: PointerEvent): void => {
        if (upEvent.pointerId !== start.pointerId) return;
        if (start.dragging) {
          // Клик, который браузер отправит после pointerup, не должен
          // открывать файл или переключать карточку повторно.
          suppressUntilRef.current = performance.now() + CLICK_SUPPRESS_MS;
        }
        setBand(null);
        setIsDragging(false);
        cleanup();
      };

      cleanupRef.current?.();
      cleanupRef.current = cleanup;
      window.addEventListener('pointermove', handleMove, { passive: false });
      window.addEventListener('pointerup', handleUp);
      window.addEventListener('pointercancel', handleUp);
    },
    [
      enabled,
      isSelectionMode,
      selectedIds,
      onEnableSelectionMode,
      onToggleSelection,
      onSetSelection,
    ],
  );

  const onClickCapture = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      if (!enabled || !isClickSuppressed()) return;
      // После drag или long-press клик не должен открывать файл и не должен
      // нажимать вложенные кнопки карточки.
      event.preventDefault();
      event.stopPropagation();
    },
    [enabled, isClickSuppressed],
  );

  const onClick = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      if (!enabled || isClickSuppressed()) return;
      const target = elementFrom(event.target);
      if (!target) return;
      // Клики по карточкам и интерактивным элементам обрабатываются отдельно.
      if (target.closest('[data-entry-id]')) return;
      if (target.closest(INTERACTIVE_SELECTOR)) return;
      if (isSelectionMode) onExitSelectionMode();
    },
    [enabled, isClickSuppressed, isSelectionMode, onExitSelectionMode],
  );

  const onContextMenu = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      if (!enabled) return;
      const target = elementFrom(event.target);
      const card = target?.closest<HTMLElement>('[data-entry-id]');
      const id = card?.dataset.entryId;
      if (!id) return;
      event.preventDefault();
      // Long-press на touch часто завершается contextmenu — не переключаем
      // карточку второй раз.
      if (isClickSuppressed()) return;
      onEnableSelectionMode();
      if (!selectedIds.has(id)) onToggleSelection(id);
    },
    [enabled, isClickSuppressed, selectedIds, onEnableSelectionMode, onToggleSelection],
  );

  const containerProps = useMemo(
    () => ({ onPointerDown, onClickCapture, onClick, onContextMenu }),
    [onPointerDown, onClickCapture, onClick, onContextMenu],
  );

  return { containerRef, band, isDragging, containerProps, isClickSuppressed };
}
