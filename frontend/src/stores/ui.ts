import { create } from 'zustand';

export type ToastType = 'success' | 'error' | 'warning' | 'info';

export interface Toast {
  id: number;
  type: ToastType;
  message: string;
  leaving: boolean;
}

interface UiState {
  sidebarOpen: boolean;
  toasts: Toast[];
  setSidebarOpen: (open: boolean) => void;
  toast: (message: string, type?: ToastType) => number;
  dismissToast: (id: number) => void;
  removeToast: (id: number) => void;
}

let nextToastId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function scheduleRemoval(id: number, dismiss: (id: number) => void): void {
  const existing = timers.get(id);
  if (existing) clearTimeout(existing);
  timers.set(
    id,
    setTimeout(() => dismiss(id), 3800),
  );
}

export const useUi = create<UiState>((set, get) => ({
  sidebarOpen: false,
  toasts: [],

  setSidebarOpen: (open) => set({ sidebarOpen: open }),

  toast: (message, type = 'info') => {
    const id = nextToastId++;
    set((state) => ({
      toasts: [...state.toasts.slice(-4), { id, type, message, leaving: false }],
    }));
    scheduleRemoval(id, get().dismissToast);
    return id;
  },

  dismissToast: (id) => {
    const timer = timers.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.delete(id);
    }
    set((state) => ({
      toasts: state.toasts.map((toast) => (toast.id === id ? { ...toast, leaving: true } : toast)),
    }));
    setTimeout(() => {
      set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) }));
    }, 240);
  },

  removeToast: (id) => {
    const timer = timers.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.delete(id);
    }
    set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) }));
  },
}));

/** Удобный вызов уведомления вне React-компонентов. */
export function toast(message: string, type: ToastType = 'info'): void {
  useUi.getState().toast(message, type);
}