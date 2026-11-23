import type { InterfaceSettings } from '../services/api';

export const DEFAULT_INTERFACE: InterfaceSettings = { accent: 'cyan' };

export const ACCENTS: Array<{ key: string; name: string; description: string; gradient: string }> =
  [
    {
      key: 'cyan',
      name: 'Неоновый циан',
      description: 'Стандартный акцент NEXUS',
      gradient: 'linear-gradient(120deg, #58d4ff, #86e1ff)',
    },
    {
      key: 'magenta',
      name: 'Магента',
      description: 'Тёплый контрастный акцент',
      gradient: 'linear-gradient(120deg, #ff4fd8, #ff8fe6)',
    },
    {
      key: 'green',
      name: 'Изумруд',
      description: 'Спокойный зелёный акцент',
      gradient: 'linear-gradient(120deg, #3dffb0, #8affd0)',
    },
    {
      key: 'amber',
      name: 'Янтарь',
      description: 'Высококонтрастный акцент',
      gradient: 'linear-gradient(120deg, #ffb347, #ffd28a)',
    },
  ];

const ACCENT_KEYS = new Set(ACCENTS.map((item) => item.key));

export function sanitizeInterface(value: unknown): InterfaceSettings {
  if (typeof value !== 'object' || value === null) return { ...DEFAULT_INTERFACE };
  const accent = (value as Record<string, unknown>).accent;
  if (typeof accent === 'string' && ACCENT_KEYS.has(accent)) {
    return { accent };
  }
  return { ...DEFAULT_INTERFACE };
}

/** CSS-токены читают акцент с корневого элемента (`[data-accent]`). */
export function applyAccent(accent: string): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  if (accent === DEFAULT_INTERFACE.accent) {
    delete root.dataset.accent;
  } else {
    root.dataset.accent = accent;
  }
}