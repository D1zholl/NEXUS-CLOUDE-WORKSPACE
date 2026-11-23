import { create } from 'zustand';

import { api, errorMessage, setCsrfToken, type MeResponse } from '../services/api';

type AuthStatus = 'loading' | 'authenticated' | 'anonymous';

interface AuthState {
  status: AuthStatus;
  session: MeResponse | null;
  error: string | null;
  bootstrap: () => Promise<void>;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  clearSession: () => void;
  refresh: () => Promise<void>;
}

let bootstrapped = false;
let listenersAttached = false;

function attachListeners(): void {
  if (listenersAttached || typeof window === 'undefined') return;
  listenersAttached = true;
  window.addEventListener('nexus:session-expired', () => {
    setCsrfToken(null);
    useAuth.setState({ status: 'anonymous', session: null });
  });
}

export const useAuth = create<AuthState>((set, _get) => ({
  status: 'loading',
  session: null,
  error: null,

  bootstrap: async () => {
    if (bootstrapped) return;
    bootstrapped = true;
    attachListeners();
    try {
      const session = await api.me();
      setCsrfToken(session.csrfToken);
      set({ status: 'authenticated', session, error: null });
    } catch {
      setCsrfToken(null);
      set({ status: 'anonymous', session: null, error: null });
    }
  },

  login: async (username, password) => {
    const session = await api.login(username, password);
    setCsrfToken(session.csrfToken);
    set({ status: 'authenticated', session, error: null });
  },

  logout: async () => {
    try {
      await api.logout();
    } catch (error) {
      // Сессия могла уже истечь — выходим локально в любом случае.
      if (error instanceof Error) {
        // ничего не показываем: выход должен завершаться предсказуемо
      }
    }
    setCsrfToken(null);
    set({ status: 'anonymous', session: null });
  },

  clearSession: () => {
    setCsrfToken(null);
    set({ status: 'anonymous', session: null });
  },

  refresh: async () => {
    try {
      const session = await api.me();
      setCsrfToken(session.csrfToken);
      set({ status: 'authenticated', session, error: null });
    } catch (error) {
      setCsrfToken(null);
      set({ status: 'anonymous', session: null, error: errorMessage(error) });
    }
  },
}));

export function can(session: MeResponse | null, permission: string): boolean {
  if (!session) return false;
  return session.user.isSuperadmin || session.user.permissions.includes(permission);
}

export function useCan(permission: string): boolean {
  return useAuth((state) => can(state.session, permission));
}

export function useSession(): MeResponse | null {
  return useAuth((state) => state.session);
}

export function resetAuthBootstrap(): void {
  bootstrapped = false;
}