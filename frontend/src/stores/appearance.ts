import { create } from 'zustand';

import { DEFAULT_INTERFACE, applyAccent, sanitizeInterface } from '../lib/appearance';
import { api, errorMessage, type InterfaceSettings } from '../services/api';
import { toast } from './ui';

const SETTINGS_KEY = 'appearance.interface';

interface AppearanceState {
  settings: InterfaceSettings;
  saved: InterfaceSettings;
  loaded: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
  load: () => Promise<void>;
  update: (patch: Partial<InterfaceSettings>) => void;
  discard: () => void;
  save: () => Promise<boolean>;
}

let loadPromise: Promise<void> | null = null;

function apply(settings: InterfaceSettings): void {
  applyAccent(settings.accent);
}

export const useAppearance = create<AppearanceState>((set, get) => ({
  settings: { ...DEFAULT_INTERFACE },
  saved: { ...DEFAULT_INTERFACE },
  loaded: false,
  loading: false,
  saving: false,
  error: null,

  load: async () => {
    if (get().loaded) return;
    if (loadPromise) return loadPromise;
    loadPromise = (async () => {
      set({ loading: true, error: null });
      try {
        const values = await api.getMySettings();
        const settings = sanitizeInterface(values[SETTINGS_KEY]);
        apply(settings);
        set({ settings, saved: settings, loaded: true, loading: false });
      } catch (error) {
        set({ loading: false, error: errorMessage(error), loaded: true });
      }
    })().finally(() => {
      loadPromise = null;
    });
    return loadPromise;
  },

  update: (patch) => {
    const next: InterfaceSettings = { ...get().settings, ...patch };
    apply(next);
    set({ settings: next });
  },

  discard: () => {
    const saved = get().saved;
    apply(saved);
    set({ settings: { ...saved } });
  },

  save: async () => {
    const settings = get().settings;
    set({ saving: true });
    try {
      const result = await api.saveMySetting(SETTINGS_KEY, settings);
      const saved = sanitizeInterface(result);
      apply(saved);
      set({ settings: saved, saved, saving: false });
      toast('Внешний вид сохранён.', 'success');
      return true;
    } catch (error) {
      set({ saving: false });
      toast(errorMessage(error), 'error');
      return false;
    }
  },
}));

export function isAppearanceDirty(state: AppearanceState): boolean {
  return state.settings.accent !== state.saved.accent;
}