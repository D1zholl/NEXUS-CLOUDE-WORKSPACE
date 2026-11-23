import { useEffect, useState } from 'react';

/**
 * Удерживает элемент в DOM на время проигрывания exit-анимации.
 *
 * `mounted` становится true сразу при открытии (без лишнего кадра), а после
 * закрытия остаётся true, пока идёт анимация. Таймер очищается при
 * размонтировании и при повторном открытии, поэтому быстрые клики не
 * оставляют «зависших» окон.
 */
export function usePresence(open: boolean, exitMs: number): { mounted: boolean; closing: boolean } {
  const [retained, setRetained] = useState(open);
  const closing = !open && retained;
  const mounted = open || retained;

  useEffect(() => {
    if (open) {
      setRetained(true);
      return;
    }
    if (!retained) return;

    const reduced =
      typeof window !== 'undefined' &&
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const timer = window.setTimeout(() => setRetained(false), reduced ? 0 : exitMs);
    return () => window.clearTimeout(timer);
  }, [open, retained, exitMs]);

  return { mounted, closing };
}
