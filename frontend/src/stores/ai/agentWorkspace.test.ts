import { beforeEach, describe, expect, it } from 'vitest';

import { useAgentWorkspace } from './agentWorkspace';

describe('agentWorkspace store', () => {
  beforeEach(() => {
    window.localStorage.clear();
    useAgentWorkspace.setState({
      tabs: [],
      activePath: null,
      drafts: {},
      mobilePane: 'editor',
    });
  });

  it('opens files once and activates the tab', () => {
    const store = useAgentWorkspace.getState();
    store.openFile('src/App.tsx');
    store.openFile('README.md');
    store.openFile('src/App.tsx');
    const state = useAgentWorkspace.getState();
    expect(state.tabs.map((tab) => tab.path)).toEqual(['src/App.tsx', 'README.md']);
    expect(state.activePath).toBe('src/App.tsx');
    expect(state.tabs[0].kind).toBe('text');
    expect(state.tabs[1].kind).toBe('markdown');
  });

  it('closes the active tab and drops its draft', () => {
    const store = useAgentWorkspace.getState();
    store.openFile('a.ts');
    store.openFile('b.ts');
    store.beginEdit('b.ts', 'content', 'sha');
    store.closeTab('b.ts');
    const state = useAgentWorkspace.getState();
    expect(state.tabs.map((tab) => tab.path)).toEqual(['a.ts']);
    expect(state.activePath).toBe('a.ts');
    expect(state.drafts['b.ts']).toBeUndefined();
  });

  it('tracks dirty state and clears it on save', () => {
    const store = useAgentWorkspace.getState();
    store.openFile('a.ts');
    store.beginEdit('a.ts', 'one', 'sha-1');
    expect(useAgentWorkspace.getState().drafts['a.ts'].dirty).toBe(false);
    store.setDraft('a.ts', { content: 'two', dirty: true });
    expect(useAgentWorkspace.getState().drafts['a.ts'].content).toBe('two');
    expect(useAgentWorkspace.getState().drafts['a.ts'].dirty).toBe(true);
    useAgentWorkspace.getState().markSaved('a.ts', 'two', 'sha-2');
    const draft = useAgentWorkspace.getState().drafts['a.ts'];
    expect(draft.dirty).toBe(false);
    expect(draft.baseSha256).toBe('sha-2');
    expect(draft.original).toBe('two');
  });

  it('persists panel layout in localStorage', () => {
    useAgentWorkspace.getState().setLayout({ explorerWidth: 300, bottomTab: 'usage' });
    const raw = window.localStorage.getItem('nexus.ai.agent.layout.v1');
    expect(raw).toContain('"explorerWidth":300');
    expect(raw).toContain('"bottomTab":"usage"');
  });

  it('resets all tabs on workspace switch', () => {
    const store = useAgentWorkspace.getState();
    store.openFile('a.ts');
    store.beginEdit('a.ts', 'x', 'sha');
    store.closeAll();
    const state = useAgentWorkspace.getState();
    expect(state.tabs).toEqual([]);
    expect(state.activePath).toBeNull();
    expect(state.drafts).toEqual({});
  });
});
