import { create } from 'zustand';

import {
  BACKGROUND_PRESETS,
  DEFAULT_BACKGROUND,
  sanitizeBackground,
  settingsEqual,
  type PresetKey,
} from '../lib/background';
import { api, errorMessage, type BackgroundSettings } from '../services/api';
import { toast } from './ui';

const SETTINGS_KEY = 'appearance.background';

interface BackgroundState {
  settings: BackgroundSettings;
  saved: BackgroundSettings;
  loaded: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
  load: (force?: boolean) => Promise<void>;
  loadPublic: () => Promise<void>;
  update: (patch: Partial<BackgroundSettings>) => void;
  applyPreset: (preset: PresetKey) => void;
  reset: () => void;
  discard: () => void;
  save: () => Promise<boolean>;
}

let loadPromise: Promise<void> | null = null;

/**
 * Последний известный системный фон.
 *
 * Кешируется, чтобы после выхода из аккаунта экран входа не мигнул заводскими
 * параметрами, пока повторно загружается публичное значение.
 */
let publicDefaults: BackgroundSettings | null = null;

/**
 * Номер поколения запросов.
 *
 * Публичный и пользовательский запросы могут выполняться одновременно
 * (например, при входе во время загрузки фона для формы входа). Устаревший
 * ответ не должен перезаписывать настройки более нового запроса.
 */
let generation = 0;

function initialSettings(): BackgroundSettings {
  return { ...(publicDefaults ?? DEFAULT_BACKGROUND) };
}

export const useBackground = create<BackgroundState>((set, get) => ({
  settings: { ...DEFAULT_BACKGROUND },
  saved: { ...DEFAULT_BACKGROUND },
  loaded: false,
  loading: false,
  saving: false,
  error: null,

  load: async (force = false) => {
    if (get().loaded && !force) return;
    if (loadPromise) return loadPromise;
    const ticket = ++generation;
    loadPromise = (async () => {
      set({ loading: true, error: null });
      try {
        const values = await api.getMySettings();
        // Сервер всегда возвращает полный снимок: если у пользователя нет
        // собственной настройки, это эффективный системный default.
        const settings = sanitizeBackground(values[SETTINGS_KEY]);
        if (ticket !== generation) {
          set({ loading: false });
          return;
        }
        set({ settings, saved: settings, loaded: true, loading: false });
      } catch (error) {
        // Не помечаем настройки загруженными, чтобы повторный запрос после
        // восстановления сети применил системный default, а не заводской.
        if (ticket !== generation) {
          set({ loading: false });
          return;
        }
        set({ loading: false, error: errorMessage(error), loaded: false });
      }
    })().finally(() => {
      loadPromise = null;
    });
    return loadPromise;
  },

  /**
   * Загружает системный фон для неаутентифицированных страниц.
   *
   * Значение не помечается как `loaded`: после входа личные настройки
   * пользователя должны загрузиться отдельным запросом и иметь приоритет.
   */
  loadPublic: async () => {
    const ticket = ++generation;
    try {
      const value = await api.getPublicBackground();
      publicDefaults = sanitizeBackground(value);
      if (ticket !== generation) return;
      const settings = { ...publicDefaults };
      set({ settings, saved: settings, error: null });
    } catch (error) {
      // Экран входа не должен ломаться из-за недоступного сервера:
      // остаются заводские или последние известные параметры.
      if (ticket !== generation) return;
      set({ error: errorMessage(error) });
    }
  },

  update: (patch) => {
    const current = get().settings;
    const next: BackgroundSettings = { ...current, ...patch };
    const presetChanged = patch.preset !== undefined;
    set({
      settings: presetChanged ? next : { ...next, preset: 'custom' },
    });
  },

  applyPreset: (preset) => {
    const definition = BACKGROUND_PRESETS[preset];
    const next: BackgroundSettings = {
      ...get().settings,
      ...definition.patch,
      preset,
    };
    set({ settings: next });
  },

  reset: () => {
    set({ settings: { ...DEFAULT_BACKGROUND } });
  },

  discard: () => {
    set({ settings: { ...get().saved } });
  },

  save: async () => {
    const settings = get().settings;
    set({ saving: true });
    try {
      const result = await api.saveMySetting(SETTINGS_KEY, settings);
      const saved = sanitizeBackground(result);
      set({ settings: saved, saved, saving: false });
      toast('Настройки фона сохранены.', 'success');
      return true;
    } catch (error) {
      set({ saving: false });
      toast(errorMessage(error), 'error');
      return false;
    }
  },
}));

/**
 * Сбрасывает локальное состояние при выходе из аккаунта.
 *
 * Также очищает in-flight обещание: иначе вход под другим пользователем мог
 * получить ответ предыдущего запроса и применить чужие настройки фона.
 */
export function resetBackgroundStore(): void {
  generation += 1;
  loadPromise = null;
  const settings = initialSettings();
  useBackground.setState({
    settings,
    saved: { ...settings },
    loaded: false,
    loading: false,
    saving: false,
    error: null,
  });
}

export function isBackgroundDirty(state: BackgroundState): boolean {
  return !settingsEqual(state.settings, state.saved);
}