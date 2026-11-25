/** Вкладки открытых файлов редактора. */
import { X } from 'lucide-react';

import type { EditorTab } from '../../../stores/ai/agentWorkspace';

interface EditorTabsProps {
  tabs: EditorTab[];
  activePath: string | null;
  dirty: Record<string, boolean>;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
}

export function EditorTabs({ tabs, activePath, dirty, onSelect, onClose }: EditorTabsProps) {
  if (tabs.length === 0) return null;
  return (
    <div className="editor-tabs" role="tablist" aria-label="Открытые файлы">
      {tabs.map((tab) => (
        <div
          key={tab.path}
          role="tab"
          aria-selected={tab.path === activePath}
          className={`editor-tab${tab.path === activePath ? ' is-active' : ''}`}
          title={tab.path}
          onClick={() => onSelect(tab.path)}
          onAuxClick={(event) => {
            if (event.button === 1) onClose(tab.path);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') onSelect(tab.path);
          }}
          tabIndex={0}
        >
          <span className={`editor-tab__dot editor-tab__dot--${tab.kind}`} aria-hidden="true" />
          <span className="editor-tab__name">{tab.name}</span>
          {dirty[tab.path] ? <span className="editor-tab__dirty" title="Не сохранено" /> : null}
          <button
            type="button"
            className="editor-tab__close"
            title="Закрыть"
            onClick={(event) => {
              event.stopPropagation();
              onClose(tab.path);
            }}
          >
            <X aria-hidden="true" />
          </button>
        </div>
      ))}
    </div>
  );
}
