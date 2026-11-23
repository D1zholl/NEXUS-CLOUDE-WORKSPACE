import { canPreviewFile } from '../../lib/preview';
import type { CloudEntry } from '../../services/api';

export type View = 'folder' | 'all' | 'recent' | 'starred' | 'shares' | 'trash';
export type Layout = 'grid' | 'list';

export const VIEW_LABELS: Record<View, string> = {
  folder: 'Мои файлы',
  all: 'Все файлы',
  recent: 'Недавние',
  starred: 'Избранное',
  shares: 'Общий доступ',
  trash: 'Корзина',
};

export const SORT_LABELS: Array<{ value: string; label: string }> = [
  { value: 'name', label: 'По имени' },
  { value: 'size', label: 'По размеру' },
  { value: 'updated', label: 'По дате' },
  { value: 'type', label: 'По типу' },
];

/** E10: серверный фильтр по категории файла. */
export const TYPE_FILTERS: Array<{ value: string; label: string }> = [
  { value: 'all', label: 'Все типы' },
  { value: 'image', label: 'Изображения' },
  { value: 'video', label: 'Видео' },
  { value: 'audio', label: 'Аудио' },
  { value: 'document', label: 'Документы' },
  { value: 'archive', label: 'Архивы' },
  { value: 'other', label: 'Другое' },
];

/** E9: окно раздела «Недавние». */
export const RECENT_DAYS = 30;

export const EXPIRY_OPTIONS = [
  { value: 24, label: '24 часа' },
  { value: 24 * 7, label: '7 дней' },
  { value: 24 * 30, label: '30 дней' },
  { value: 0, label: 'Бессрочно' },
];

export function formatTtlHours(hours: number): string {
  if (hours > 0 && hours % 24 === 0) return `${hours / 24} дн.`;
  return `${hours} ч`;
}

export function trashDeadline(trashedAt: string | null, retentionDays: number): string | null {
  if (!trashedAt) return null;
  const date = new Date(trashedAt);
  if (Number.isNaN(date.getTime())) return null;
  date.setDate(date.getDate() + retentionDays);
  return date.toISOString();
}

export function canPreview(entry: CloudEntry): boolean {
  return canPreviewFile(entry.entryType, entry.name);
}

export function copyName(name: string, attempt: number): string {
  const dot = name.lastIndexOf('.');
  const base = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  const suffix = attempt <= 1 ? ' (копия)' : ` (копия ${attempt})`;
  return `${base}${suffix}${ext}`;
}

export function triggerDownload(url: string): void {
  const link = document.createElement('a');
  link.href = url;
  link.rel = 'noopener';
  link.style.display = 'none';
  document.body.appendChild(link);
  link.click();
  link.remove();
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}
