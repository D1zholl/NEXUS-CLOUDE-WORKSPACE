/**
 * Состояние AI Agent Workspace: раскладка панелей, вкладки и буферы редактора.
 *
 * В localStorage сохраняется только расположение панелей и активной вкладки
 * (не токены и не секреты). Буферы живут в памяти: после перезагрузки страницы
 * несохранённые изменения не восстанавливаются автоматически, чтобы не
 * перезаписать более новую версию файла на Runtime.
 */
import { create } from 'zustand';

import type { FileKind } from '../../lib/fileLanguage';
import { basename, fileKind } from '../../lib/fileLanguage';

export type BottomTab = 'problems' | 'output' | 'executions' | 'changes' | 'usage';
export type MobilePane = 'explorer' | 'editor' | 'agent';

export interface EditorTab {
  path: string;
  name: string;
  kind: FileKind;
}

export interface EditorDraft {
  content: string;
  baseSha256: string;
  /** Содержимое на момент открытия — для diff «мои изменения». */
  original: string;
  dirty: boolean;
  saving: boolean;
  error: string | null;
  conflict: { content: string | null; sha256: string | null } | null;
}

export interface PanelLayout {
  explorerWidth: number;
  agentWidth: number;
  bottomHeight: number;
  bottomVisible: boolean;
  explorerVisible: boolean;
  agentVisible: boolean;
  editorMaximized: boolean;
  bottomTab: BottomTab;
}

const LAYOUT_KEY = 'nexus.ai.agent.layout.v1';

const DEFAULT_LAYOUT: PanelLayout = {
  explorerWidth: 260,
  agentWidth: 400,
  bottomHeight: 200,
  bottomVisible: true,
  explorerVisible: true,
  agentVisible: true,
  editorMaximized: false,
  bottomTab: 'changes',
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function loadLayout(): PanelLayout {
  try {
    const raw = window.localStorage.getItem(LAYOUT_KEY);
    if (!raw) return DEFAULT_LAYOUT;
    const parsed = JSON.parse(raw) as Partial<PanelLayout>;
    const tabs: BottomTab[] = ['problems', 'output', 'executions', 'changes', 'usage'];
    return {
      explorerWidth: clamp(Number(parsed.explorerWidth) || 260, 180, 520),
      agentWidth: clamp(Number(parsed.agentWidth) || 400, 300, 720),
      bottomHeight: clamp(Number(parsed.bottomHeight) || 200, 120, 560),
      bottomVisible: parsed.bottomVisible !== false,
      explorerVisible: parsed.explorerVisible !== false,
      agentVisible: parsed.agentVisible !== false,
      editorMaximized: parsed.editorMaximized === true,
      bottomTab: tabs.includes(parsed.bottomTab as BottomTab)
        ? (parsed.bottomTab as BottomTab)
        : 'changes',
    };
  } catch {
    return DEFAULT_LAYOUT;
  }
}

interface AgentWorkspaceState {
  layout: PanelLayout;
  tabs: EditorTab[];
  activePath: string | null;
  drafts: Record<string, EditorDraft>;
  mobilePane: MobilePane;
  setLayout: (patch: Partial<PanelLayout>) => void;
  resetLayout: () => void;
  openFile: (path: string) => void;
  closeTab: (path: string) => void;
  setActive: (path: string | null) => void;
  setDraft: (path: string, draft: Partial<EditorDraft>) => void;
  beginEdit: (path: string, content: string, baseSha256: string) => void;
  markSaved: (path: string, content: string, sha256: string) => void;
  setMobilePane: (pane: MobilePane) => void;
  closeAll: () => void;
}

function persistLayout(layout: PanelLayout): void {
  try {
    window.localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // Хранилище может быть недоступно — раскладка не критична.
  }
}

export const useAgentWorkspace = create<AgentWorkspaceState>()((set, get) => ({
  layout: typeof window === 'undefined' ? DEFAULT_LAYOUT : loadLayout(),
  tabs: [],
  activePath: null,
  drafts: {},
  mobilePane: 'editor',

  setLayout: (patch) => {
    const layout = { ...get().layout, ...patch };
    persistLayout(layout);
    set({ layout });
  },

  resetLayout: () => {
    persistLayout(DEFAULT_LAYOUT);
    set({ layout: DEFAULT_LAYOUT });
  },

  openFile: (path) => {
    const exists = get().tabs.some((tab) => tab.path === path);
    const tabs = exists
      ? get().tabs
      : [...get().tabs, { path, name: basename(path), kind: fileKind(path) }];
    set({ tabs, activePath: path, mobilePane: 'editor' });
  },

  closeTab: (path) => {
    const tabs = get().tabs.filter((tab) => tab.path !== path);
    const drafts = { ...get().drafts };
    delete drafts[path];
    let activePath = get().activePath;
    if (activePath === path) {
      activePath = tabs.length > 0 ? tabs[tabs.length - 1].path : null;
    }
    set({ tabs, activePath, drafts });
  },

  setActive: (path) => set({ activePath: path, mobilePane: path ? 'editor' : get().mobilePane }),

  setDraft: (path, draft) => {
    const current = get().drafts[path];
    if (!current) return;
    set({ drafts: { ...get().drafts, [path]: { ...current, ...draft } } });
  },

  beginEdit: (path, content, baseSha256) => {
    set({
      drafts: {
        ...get().drafts,
        [path]: {
          content,
          original: content,
          baseSha256,
          dirty: false,
          saving: false,
          error: null,
          conflict: null,
        },
      },
    });
  },

  markSaved: (path, content, sha256) => {
    const current = get().drafts[path];
    set({
      drafts: {
        ...get().drafts,
        [path]: {
          content,
          original: content,
          baseSha256: sha256,
          dirty: false,
          saving: false,
          error: null,
          conflict: null,
          ...(current ? {} : {}),
        },
      },
    });
  },

  setMobilePane: (pane) => set({ mobilePane: pane }),

  closeAll: () =>
    set({
      tabs: [],
      activePath: null,
      drafts: {},
      mobilePane: 'editor',
    }),
}));
