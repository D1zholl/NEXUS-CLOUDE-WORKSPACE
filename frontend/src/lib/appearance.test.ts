import { describe, expect, it, beforeEach } from 'vitest';

import { ACCENTS, DEFAULT_INTERFACE, applyAccent, sanitizeInterface } from './appearance';

describe('sanitizeInterface', () => {
  it('возвращает безопасный акцент для мусора', () => {
    expect(sanitizeInterface(null)).toEqual(DEFAULT_INTERFACE);
    expect(sanitizeInterface('magenta')).toEqual(DEFAULT_INTERFACE);
    expect(sanitizeInterface({ accent: 'ultraviolet' })).toEqual(DEFAULT_INTERFACE);
    expect(sanitizeInterface({ accent: 42 })).toEqual(DEFAULT_INTERFACE);
  });

  it('принимает только известные акценты', () => {
    for (const accent of ACCENTS) {
      expect(sanitizeInterface({ accent: accent.key })).toEqual({ accent: accent.key });
    }
  });
});

describe('applyAccent', () => {
  beforeEach(() => {
    delete document.documentElement.dataset.accent;
  });

  it('для базового акцента убирает атрибут', () => {
    document.documentElement.dataset.accent = 'amber';
    applyAccent('cyan');
    expect(document.documentElement.dataset.accent).toBeUndefined();
  });

  it('для остальных акцентов выставляет data-accent', () => {
    applyAccent('magenta');
    expect(document.documentElement.dataset.accent).toBe('magenta');
  });
});
