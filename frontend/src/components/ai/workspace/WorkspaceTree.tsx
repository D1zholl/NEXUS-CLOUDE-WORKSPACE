/** Виртуализированное дерево файлов workspace. */
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ChevronRight,
  File,
  FileCode2,
  FileJson,
  FileText,
  Folder,
  FolderOpen,
  Image as ImageIcon,
  Loader2,
} from 'lucide-react';

import { extensionOf } from '../../../lib/fileLanguage';
import type { WorkspaceEntry } from '../../../services/aiWorkspace';

export const ROW_HEIGHT = 24;

export interface TreeRow {
  entry: WorkspaceEntry;
  depth: number;
}

interface WorkspaceTreeProps {
  rows: TreeRow[];
  activePath: string | null;
  expanded: Set<string>;
  loading: Set<string>;
  errors: Record<string, string>;
  changed: Set<string>;
  busy: Set<string>;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  onContextMenu: (event: React.MouseEvent, entry: WorkspaceEntry | null) => void;
  onDropEntry: (targetDir: string, sourcePath: string) => void;
  onDropFiles: (targetDir: string, files: FileList) => void;
  onRetry: (path: string) => void;
}

function EntryIcon({ entry, open }: { entry: WorkspaceEntry; open: boolean }) {
  if (entry.type === 'dir') {
    return open ? <FolderOpen aria-hidden="true" /> : <Folder aria-hidden="true" />;
  }
  const extension = extensionOf(entry.name);
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg', 'ico', 'avif'].includes(extension)) {
    return <ImageIcon aria-hidden="true" />;
  }
  if (extension === 'json') return <FileJson aria-hidden="true" />;
  if (['md', 'txt', 'log', 'lock'].includes(extension)) return <FileText aria-hidden="true" />;
  if (
    [
      'ts', 'tsx', 'js', 'jsx', 'rs', 'py', 'go', 'java', 'c', 'h', 'cpp', 'cs', 'php', 'rb',
      'html', 'css', 'scss', 'sql', 'sh', 'ps1', 'toml', 'yaml', 'yml', 'xml',
    ].includes(extension)
  ) {
    return <FileCode2 aria-hidden="true" />;
  }
  return <File aria-hidden="true" />;
}

export function WorkspaceTree({
  rows,
  activePath,
  expanded,
  loading,
  errors,
  changed,
  busy,
  onToggle,
  onOpen,
  onContextMenu,
  onDropEntry,
  onDropFiles,
  onRetry,
}: WorkspaceTreeProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(600);
  const [dragOver, setDragOver] = useState<string | null>(null);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setHeight(element.clientHeight));
    observer.observe(element);
    setHeight(element.clientHeight);
    return () => observer.disconnect();
  }, []);

  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - 4);
  const visible = Math.ceil(height / ROW_HEIGHT) + 8;
  const slice = useMemo(() => rows.slice(first, first + visible), [rows, first, visible]);

  return (
    <div
      className="ws-tree"
      ref={containerRef}
      onScroll={() => {
        const element = containerRef.current;
        if (element) setScrollTop(element.scrollTop);
      }}
      onContextMenu={(event) => {
        if (event.target !== event.currentTarget) return;
        event.preventDefault();
        onContextMenu(event, null);
      }}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault();
          event.dataTransfer.dropEffect = 'copy';
        }
      }}
      onDrop={(event) => {
        if (event.dataTransfer.files.length === 0) return;
        event.preventDefault();
        onDropFiles('', event.dataTransfer.files);
      }}
      role="tree"
      aria-label="Файлы workspace"
    >
      <div style={{ height: rows.length * ROW_HEIGHT, position: 'relative' }}>
        {slice.map((row, index) => {
          const position = (first + index) * ROW_HEIGHT;
          const isDir = row.entry.type === 'dir';
          const isOpen = expanded.has(row.entry.path);
          const isActive = activePath === row.entry.path;
          return (
            <div
              key={row.entry.path}
              role="treeitem"
              aria-selected={isActive}
              aria-expanded={isDir ? isOpen : undefined}
              className={[
                'ws-node',
                isActive ? 'is-active' : '',
                dragOver === row.entry.path ? 'is-drop' : '',
              ]
                .filter(Boolean)
                .join(' ')}
              style={{
                position: 'absolute',
                top: position,
                right: 0,
                left: 0,
                height: ROW_HEIGHT,
                paddingLeft: 6 + row.depth * 14,
              }}
              title={row.entry.path}
              draggable
              onDragStart={(event) => {
                event.dataTransfer.setData('application/x-nexus-path', row.entry.path);
                event.dataTransfer.effectAllowed = 'move';
              }}
              onDragOver={(event) => {
                if (!isDir) return;
                event.preventDefault();
                event.stopPropagation();
                setDragOver(row.entry.path);
              }}
              onDragLeave={() => setDragOver((current) => (current === row.entry.path ? null : current))}
              onDrop={(event) => {
                event.preventDefault();
                event.stopPropagation();
                setDragOver(null);
                if (!isDir) return;
                if (event.dataTransfer.files.length > 0) {
                  onDropFiles(row.entry.path, event.dataTransfer.files);
                  return;
                }
                const source = event.dataTransfer.getData('application/x-nexus-path');
                if (source && source !== row.entry.path) onDropEntry(row.entry.path, source);
              }}
              onClick={(event) => {
                event.stopPropagation();
                if (isDir) onToggle(row.entry.path);
                else onOpen(row.entry.path);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onContextMenu(event, row.entry);
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  if (isDir) onToggle(row.entry.path);
                  else onOpen(row.entry.path);
                }
              }}
              tabIndex={0}
            >
              <span className={`ws-node__chevron${isDir && isOpen ? ' is-open' : ''}`}>
                {isDir ? <ChevronRight aria-hidden="true" /> : null}
              </span>
              <span className="ws-node__icon">
                <EntryIcon entry={row.entry} open={isOpen} />
              </span>
              <span className="ws-node__name">{row.entry.name}</span>
              {loading.has(row.entry.path) ? (
                <Loader2 className="ws-node__spinner spin" aria-hidden="true" />
              ) : null}
              {changed.has(row.entry.path) ? (
                <span className="ws-node__changed" title="Изменён агентом" />
              ) : null}
              {busy.has(row.entry.path) ? (
                <span className="ws-node__busy" title="Файл изменяется AI">
                  AI
                </span>
              ) : null}
            </div>
          );
        })}
      </div>
      {errors[''] ? (
        <div className="ws-tree__error">
          <span>{errors['']}</span>
          <button type="button" className="btn btn--sm btn--ghost" onClick={() => onRetry('')}>
            Повторить
          </button>
        </div>
      ) : null}
    </div>
  );
}
