import type { ReactNode } from 'react';

/**
 * Плавающая колонка уведомлений и прогресса.
 *
 * Позиционируется поверх страницы и не участвует в её потоке: появление и
 * исчезновение карточек не сдвигает содержимое каталога.
 */
export function NoticeStack({ label, children }: { label: string; children: ReactNode }) {
  return (
    <aside className="notice-stack" aria-label={label}>
      {children}
    </aside>
  );
}