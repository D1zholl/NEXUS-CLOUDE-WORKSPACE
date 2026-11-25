/** Explorer рабочего пространства: дерево, операции, поиск, загрузка. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Copy,
  Download,
  Edit3,
  FilePlus2,
  FolderPlus,
  Loader2,
  RefreshCw,
  Trash2,
  Upload,
} from 'lucide-react';

import { Button, Field, Modal } from '../../ui';
import { formatBytes, formatDate } from '../../../lib/format';
import { basename, dirname, joinPath } from '../../../lib/fileLanguage';
import type { WorkspaceEntry } from '../../../services/aiWorkspace';
import {
  aiWorkspaceApi,
  saveBlob,
} from '../../../services/aiWorkspace';
import { errorMessage } from '../../../services/api';
import { useAgentWorkspace } from '../../../stores/ai/agentWorkspace';
import { toast } from '../../../stores/ui';
import { WorkspaceToolbar } from './WorkspaceToolbar';
import type { ExplorerSearchMode, ExplorerSort } from './WorkspaceToolbar';
import { WorkspaceTree } from './WorkspaceTree';
import type { TreeRow } from './WorkspaceTree';

interface WorkspaceExplorerProps {
  workspaceId: string;
  canWrite: boolean;
  activePath: string | null;
  changedPaths: Set<string>;
  busyPaths: Set<string>;
  refreshToken: number;
  onOpenFile: (path: string) => void;
  onFilesChanged: () => void;
}

interface MenuState {
  x: number;
  y: number;
  entry: WorkspaceEntry | null;
}

function sortEntries(entries: WorkspaceEntry[], sort: ExplorerSort): WorkspaceEntry[] {
  return [...entries].sort((left, right) => {
    if (left.type !== right.type) return left.type === 'dir' ? -1 : 1;
    if (sort === 'modified') {
      const leftTime = left.modified ? Date.parse(left.modified) : 0;
      const rightTime = right.modified ? Date.parse(right.modified) : 0;
      if (leftTime !== rightTime) return rightTime - leftTime;
    }
    if (sort === 'type') {
      const leftExt = left.name.includes('.') ? left.name.split('.').pop()! : '';
      const rightExt = right.name.includes('.') ? right.name.split('.').pop()! : '';
      const byExt = leftExt.localeCompare(rightExt, undefined, { sensitivity: 'base' });
      if (byExt !== 0) return byExt;
    }
    return left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' });
  });
}

export function WorkspaceExplorer({
  workspaceId,
  canWrite,
  activePath,
  changedPaths,
  busyPaths,
  refreshToken,
  onOpenFile,
  onFilesChanged,
}: WorkspaceExplorerProps) {
  const layout = useAgentWorkspace((state) => state.layout);
  const setLayout = useAgentWorkspace((state) => state.setLayout);

  const [children, setChildren] = useState<Record<string, WorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState<Set<string>>(new Set());
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [truncated, setTruncated] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchMode, setSearchMode] = useState<ExplorerSearchMode>('name');
  const [sort, setSort] = useState<ExplorerSort>('name');
  const [query, setQuery] = useState('');
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [searchFiles, setSearchFiles] = useState<WorkspaceEntry[]>([]);
  const [searchHits, setSearchHits] = useState<
    Array<{ path: string; line: number; preview: string }>
  >([]);
  const [searchTruncated, setSearchTruncated] = useState(false);

  const [createDialog, setCreateDialog] = useState<{
    type: 'file' | 'folder';
    dir: string;
    name: string;
  } | null>(null);
  const [renameDialog, setRenameDialog] = useState<{ entry: WorkspaceEntry; name: string } | null>(
    null,
  );
  const [transferDialog, setTransferDialog] = useState<{
    mode: 'move' | 'copy';
    entry: WorkspaceEntry;
    target: string;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<WorkspaceEntry | null>(null);
  const [pending, setPending] = useState(false);

  const uploadInput = useRef<HTMLInputElement>(null);
  const uploadDir = useRef('');
  const expandedRef = useRef<Set<string>>(new Set());
  expandedRef.current = expanded;

  const loadDir = useCallback(
    async (path: string) => {
      setLoading((current) => new Set(current).add(path));
      try {
        const tree = await aiWorkspaceApi.tree(workspaceId, path, 1);
        setChildren((current) => ({ ...current, [path]: tree.entries }));
        setTruncated((current) => {
          const next = new Set(current);
          if (tree.truncated) next.add(path);
          else next.delete(path);
          return next;
        });
        setErrors((current) => {
          if (!current[path]) return current;
          const next = { ...current };
          delete next[path];
          return next;
        });
      } catch (error) {
        const message = errorMessage(error);
        setErrors((current) => ({ ...current, [path]: message }));
        if (path === '') toast(`Explorer: ${message}`, 'error');
      } finally {
        setLoading((current) => {
          const next = new Set(current);
          next.delete(path);
          return next;
        });
      }
    },
    [workspaceId],
  );

  const refreshAll = useCallback(async () => {
    setChildren({});
    setErrors({});
    await loadDir('');
    const dirs = [...expandedRef.current];
    await Promise.all(dirs.map((path) => loadDir(path)));
  }, [loadDir]);

  useEffect(() => {
    setChildren({});
    setExpanded(new Set());
    setErrors({});
    setSearchOpen(false);
    setQuery('');
    setSearchFiles([]);
    setSearchHits([]);
    void loadDir('');
  }, [workspaceId, loadDir]);

  useEffect(() => {
    if (refreshToken === 0) return;
    // Несколько файлов могут измениться подряд: обновляем дерево один раз.
    const timer = window.setTimeout(() => {
      void refreshAll();
    }, 250);
    return () => window.clearTimeout(timer);
  }, [refreshToken, refreshAll]);

  // Закрытие контекстного меню.
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenu(null);
    };
    window.addEventListener('mousedown', close);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('scroll', close, true);
      window.removeEventListener('resize', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [menu]);

  const rows = useMemo<TreeRow[]>(() => {
    const result: TreeRow[] = [];
    const walk = (parent: string, depth: number) => {
      const entries = sortEntries(children[parent] ?? [], sort);
      for (const entry of entries) {
        result.push({ entry, depth });
        if (entry.type === 'dir' && expanded.has(entry.path) && children[entry.path]) {
          walk(entry.path, depth + 1);
        }
      }
    };
    walk('', 0);
    return result;
  }, [children, expanded, sort]);

  const toggle = (path: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!children[path]) void loadDir(path);
      }
      return next;
    });
  };

  const openCreate = (type: 'file' | 'folder') => {
    const entry = menu?.entry ?? null;
    const dir = entry?.type === 'dir' ? entry.path : dirname(activePath ?? '');
    setCreateDialog({ type, dir, name: '' });
    setMenu(null);
  };

  const submitCreate = async () => {
    if (!createDialog) return;
    const name = createDialog.name.trim();
    if (!name) {
      toast('Укажите имя.', 'warning');
      return;
    }
    setPending(true);
    try {
      const target = joinPath(createDialog.dir, name);
      if (createDialog.type === 'folder') {
        await aiWorkspaceApi.mkdir(workspaceId, target);
      } else {
        await aiWorkspaceApi.save(workspaceId, {
          path: target,
          content: '',
          baseSha256: null,
        });
      }
      setCreateDialog(null);
      await loadDir(createDialog.dir);
      if (createDialog.type === 'file') {
        onOpenFile(target);
        toast(`Создан файл ${name}`, 'success');
      } else {
        setExpanded((current) => new Set(current).add(target));
        toast(`Создана папка ${name}`, 'success');
      }
      onFilesChanged();
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const submitRename = async () => {
    if (!renameDialog) return;
    const name = renameDialog.name.trim();
    if (!name) {
      toast('Укажите новое имя.', 'warning');
      return;
    }
    setPending(true);
    try {
      const target = joinPath(dirname(renameDialog.entry.path), name);
      await aiWorkspaceApi.move(workspaceId, renameDialog.entry.path, target);
      setRenameDialog(null);
      await refreshAll();
      toast('Переименовано.', 'success');
      onFilesChanged();
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const submitTransfer = async () => {
    if (!transferDialog) return;
    const target = transferDialog.target.trim();
    if (!target) {
      toast('Укажите целевой путь.', 'warning');
      return;
    }
    setPending(true);
    try {
      if (transferDialog.mode === 'move') {
        await aiWorkspaceApi.move(workspaceId, transferDialog.entry.path, target);
      } else {
        await aiWorkspaceApi.copy(workspaceId, transferDialog.entry.path, target);
      }
      setTransferDialog(null);
      await refreshAll();
      toast(transferDialog.mode === 'move' ? 'Перемещено.' : 'Скопировано.', 'success');
      onFilesChanged();
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const submitDelete = async () => {
    if (!deleteTarget) return;
    setPending(true);
    try {
      await aiWorkspaceApi.remove(workspaceId, deleteTarget.path, deleteTarget.type === 'dir');
      setDeleteTarget(null);
      await refreshAll();
      toast('Удалено.', 'success');
      onFilesChanged();
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const download = async (entry: WorkspaceEntry) => {
    try {
      const blob = await aiWorkspaceApi.download(workspaceId, entry.path);
      saveBlob(blob, entry.name);
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const uploadFiles = async (dir: string, files: FileList | File[]) => {
    if (!canWrite) return;
    const list = Array.from(files);
    if (list.length === 0) return;
    setPending(true);
    let uploaded = 0;
    for (const file of list) {
      try {
        await aiWorkspaceApi.upload(workspaceId, dir, file);
        uploaded += 1;
      } catch (error) {
        toast(`${file.name}: ${errorMessage(error)}`, 'error');
      }
    }
    setPending(false);
    if (uploaded > 0) {
      await loadDir(dir);
      toast(`Загружено файлов: ${uploaded}`, 'success');
      onFilesChanged();
    }
  };

  const runSearch = async () => {
    const value = query.trim();
    if (!value) {
      setSearchFiles([]);
      setSearchHits([]);
      return;
    }
    setSearching(true);
    setSearchError(null);
    try {
      if (searchMode === 'name') {
        const tree = await aiWorkspaceApi.tree(workspaceId, '', 6);
        const needle = value.toLowerCase();
        const flat: WorkspaceEntry[] = [];
        for (const entry of tree.entries) {
          if (entry.name.toLowerCase().includes(needle)) flat.push(entry);
          if (flat.length >= 300) break;
        }
        setSearchFiles(flat);
        setSearchTruncated(tree.truncated || flat.length >= 300);
        setSearchHits([]);
      } else {
        const result = await aiWorkspaceApi.search(workspaceId, {
          query: value,
          maxResults: 200,
        });
        setSearchHits(result.results);
        setSearchTruncated(result.truncated);
        setSearchFiles([]);
      }
    } catch (error) {
      setSearchError(errorMessage(error));
    } finally {
      setSearching(false);
    }
  };

  const showSearchResults = query.trim().length > 0;

  return (
    <section className="explorer-pane" aria-label="Workspace Explorer">
      <WorkspaceToolbar
        canWrite={canWrite}
        searchOpen={searchOpen}
        searchMode={searchMode}
        query={query}
        busy={loading.size > 0 || pending}
        onToggleSearch={() => {
          const next = !searchOpen;
          setSearchOpen(next);
          if (!next) {
            setQuery('');
            setSearchFiles([]);
            setSearchHits([]);
            setSearchError(null);
          }
        }}
        onSearchMode={(mode) => {
          setSearchMode(mode);
          setSearchFiles([]);
          setSearchHits([]);
        }}
        onQuery={setQuery}
        onSearch={() => void runSearch()}
        sort={sort}
        onSort={setSort}
        onCreateFile={() => openCreate('file')}
        onCreateFolder={() => openCreate('folder')}
        onUpload={() => {
          uploadDir.current = menu?.entry?.type === 'dir' ? menu.entry.path : '';
          uploadInput.current?.click();
        }}
        onRefresh={() => void refreshAll()}
        onCollapseAll={() => setExpanded(new Set())}
        bottomTab={layout.bottomTab}
        onBottomTab={(tab) => setLayout({ bottomTab: tab, bottomVisible: true })}
      />
      <input
        ref={uploadInput}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          const files = Array.from(event.target.files ?? []);
          event.target.value = '';
          if (files.length > 0) void uploadFiles(uploadDir.current, files);
        }}
      />

      {showSearchResults ? (
        <div className="ws-search-results">
          <div className="ws-search-results__head">
            <span>
              {searchMode === 'name' ? 'Файлы по имени' : 'Совпадения в содержимом'}
            </span>
            {searching ? <Loader2 className="spin" aria-hidden="true" /> : null}
          </div>
          {searchError ? (
            <div className="ws-search-results__error">
              <AlertTriangle aria-hidden="true" /> {searchError}
            </div>
          ) : null}
          {searchMode === 'name'
            ? searchFiles.map((entry) => (
                <button
                  key={entry.path}
                  type="button"
                  className="ws-search-hit"
                  onClick={() => {
                    if (entry.type === 'file') onOpenFile(entry.path);
                    else {
                      setQuery('');
                      setExpanded((current) => new Set(current).add(entry.path));
                      void loadDir(entry.path);
                    }
                  }}
                  title={entry.path}
                >
                  <span className="ws-search-hit__path">{entry.path}</span>
                  <span className="muted">{entry.type === 'dir' ? 'папка' : formatBytes(entry.size)}</span>
                </button>
              ))
            : searchHits.map((hit, index) => (
                <button
                  key={`${hit.path}:${hit.line}:${index}`}
                  type="button"
                  className="ws-search-hit ws-search-hit--content"
                  onClick={() => onOpenFile(hit.path)}
                  title={hit.path}
                >
                  <span className="ws-search-hit__path">
                    {hit.path}
                    <span className="muted"> :{hit.line}</span>
                  </span>
                  <span className="ws-search-hit__preview">{hit.preview}</span>
                </button>
              ))}
          {!searching &&
          !searchError &&
          (searchMode === 'name' ? searchFiles.length : searchHits.length) === 0 ? (
            <div className="ws-search-results__empty">Ничего не найдено.</div>
          ) : null}
          {searchTruncated ? (
            <div className="ws-search-results__note">Показаны первые результаты.</div>
          ) : null}
        </div>
      ) : (
        <WorkspaceTree
          rows={rows}
          activePath={activePath}
          expanded={expanded}
          loading={loading}
          errors={errors}
          changed={changedPaths}
          busy={busyPaths}
          onToggle={toggle}
          onOpen={onOpenFile}
          onContextMenu={(event, entry) =>
            setMenu({ x: event.clientX, y: event.clientY, entry })
          }
          onDropEntry={(targetDir, source) => {
            if (!canWrite) return;
            const entry = rows.find((row) => row.entry.path === source)?.entry;
            if (!entry) return;
            setTransferDialog({
              mode: 'move',
              entry,
              target: joinPath(targetDir, entry.name),
            });
          }}
          onDropFiles={(targetDir, files) => void uploadFiles(targetDir, files)}
          onRetry={(path) => void loadDir(path)}
        />
      )}

      {truncated.size > 0 && !showSearchResults ? (
        <div className="ws-tree__note">Список усечён лимитом Runtime — откройте папку.</div>
      ) : null}
      {!showSearchResults && rows.length === 0 && !loading.has('') && !errors[''] ? (
        <div className="ws-tree__empty">
          <FolderPlus aria-hidden="true" />
          <span>Workspace пуст. Создайте файл или папку.</span>
        </div>
      ) : null}

      {menu ? (
        <div
          className="ws-menu"
          style={{ left: menu.x, top: menu.y }}
          onMouseDown={(event) => event.stopPropagation()}
          role="menu"
        >
          {menu.entry ? (
            <>
              {menu.entry.type === 'file' ? (
                <button type="button" onClick={() => { onOpenFile(menu.entry!.path); setMenu(null); }}>
                  Открыть
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => {
                    toggle(menu.entry!.path);
                    setMenu(null);
                  }}
                >
                  {expanded.has(menu.entry.path) ? 'Свернуть' : 'Раскрыть'}
                </button>
              )}
              <button
                type="button"
                disabled={!canWrite}
                onClick={() => {
                  setRenameDialog({ entry: menu.entry!, name: menu.entry!.name });
                  setMenu(null);
                }}
              >
                <Edit3 aria-hidden="true" /> Переименовать
              </button>
              <button
                type="button"
                disabled={!canWrite}
                onClick={() => {
                  const entry = menu.entry!;
                  setTransferDialog({
                    mode: 'move',
                    entry,
                    target: entry.path,
                  });
                  setMenu(null);
                }}
              >
                Переместить…
              </button>
              <button
                type="button"
                disabled={!canWrite || menu.entry.type === 'dir'}
                onClick={() => {
                  const entry = menu.entry!;
                  setTransferDialog({
                    mode: 'copy',
                    entry,
                    target: joinPath(dirname(entry.path), `${entry.name}.copy`),
                  });
                  setMenu(null);
                }}
              >
                <Copy aria-hidden="true" /> Копировать…
              </button>
              {menu.entry.type === 'dir' ? (
                <>
                  <button
                    type="button"
                    disabled={!canWrite}
                    onClick={() => {
                      setCreateDialog({ type: 'file', dir: menu.entry!.path, name: '' });
                      setMenu(null);
                    }}
                  >
                    <FilePlus2 aria-hidden="true" /> Новый файл
                  </button>
                  <button
                    type="button"
                    disabled={!canWrite}
                    onClick={() => {
                      setCreateDialog({ type: 'folder', dir: menu.entry!.path, name: '' });
                      setMenu(null);
                    }}
                  >
                    <FolderPlus aria-hidden="true" /> Новая папка
                  </button>
                  <button
                    type="button"
                    disabled={!canWrite}
                    onClick={() => {
                      uploadDir.current = menu.entry!.path;
                      uploadInput.current?.click();
                      setMenu(null);
                    }}
                  >
                    <Upload aria-hidden="true" /> Загрузить сюда
                  </button>
                </>
              ) : null}
              <div className="ws-menu__sep" />
              <button
                type="button"
                disabled={menu.entry.type === 'dir'}
                onClick={() => {
                  void download(menu.entry!);
                  setMenu(null);
                }}
              >
                <Download aria-hidden="true" /> Скачать
              </button>
              <button type="button" className="ws-menu__danger" onClick={() => { setMenu(null); void refreshAll(); }}>
                <RefreshCw aria-hidden="true" /> Обновить
              </button>
              <button
                type="button"
                className="ws-menu__danger"
                disabled={!canWrite}
                onClick={() => {
                  setDeleteTarget(menu.entry);
                  setMenu(null);
                }}
              >
                <Trash2 aria-hidden="true" /> Удалить
              </button>
              <div className="ws-menu__meta">
                {menu.entry.path}
                {menu.entry.type === 'file'
                  ? ` · ${formatBytes(menu.entry.size)} · ${formatDate(menu.entry.modified)}`
                  : ''}
              </div>
            </>
          ) : (
            <>
              <button type="button" disabled={!canWrite} onClick={() => openCreate('file')}>
                <FilePlus2 aria-hidden="true" /> Новый файл
              </button>
              <button type="button" disabled={!canWrite} onClick={() => openCreate('folder')}>
                <FolderPlus aria-hidden="true" /> Новая папка
              </button>
              <button
                type="button"
                disabled={!canWrite}
                onClick={() => {
                  uploadDir.current = '';
                  uploadInput.current?.click();
                  setMenu(null);
                }}
              >
                <Upload aria-hidden="true" /> Загрузить файлы
              </button>
              <div className="ws-menu__sep" />
              <button type="button" onClick={() => { void refreshAll(); setMenu(null); }}>
                <RefreshCw aria-hidden="true" /> Обновить
              </button>
            </>
          )}
        </div>
      ) : null}

      <Modal
        open={Boolean(createDialog)}
        onClose={() => setCreateDialog(null)}
        title={createDialog?.type === 'folder' ? 'Новая папка' : 'Новый файл'}
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreateDialog(null)}>
              Отмена
            </Button>
            <Button variant="primary" loading={pending} onClick={() => void submitCreate()}>
              Создать
            </Button>
          </>
        }
      >
        {createDialog ? (
          <>
            <Field label="Каталог" hint="Относительный путь внутри workspace">
              <input
                value={createDialog.dir}
                onChange={(event) =>
                  setCreateDialog({ ...createDialog, dir: event.target.value })
                }
                placeholder="(корень)"
              />
            </Field>
            <Field label="Имя">
              <input
                autoFocus
                value={createDialog.name}
                onChange={(event) =>
                  setCreateDialog({ ...createDialog, name: event.target.value })
                }
                onKeyDown={(event) => {
                  if (event.key === 'Enter') void submitCreate();
                }}
                placeholder={createDialog.type === 'folder' ? 'components' : 'App.tsx'}
              />
            </Field>
          </>
        ) : null}
      </Modal>

      <Modal
        open={Boolean(renameDialog)}
        onClose={() => setRenameDialog(null)}
        title="Переименовать"
        footer={
          <>
            <Button variant="ghost" onClick={() => setRenameDialog(null)}>
              Отмена
            </Button>
            <Button variant="primary" loading={pending} onClick={() => void submitRename()}>
              Переименовать
            </Button>
          </>
        }
      >
        {renameDialog ? (
          <Field label="Новое имя">
            <input
              autoFocus
              value={renameDialog.name}
              onChange={(event) =>
                setRenameDialog({ ...renameDialog, name: event.target.value })
              }
              onKeyDown={(event) => {
                if (event.key === 'Enter') void submitRename();
              }}
            />
          </Field>
        ) : null}
      </Modal>

      <Modal
        open={Boolean(transferDialog)}
        onClose={() => setTransferDialog(null)}
        title={transferDialog?.mode === 'copy' ? 'Копировать' : 'Переместить'}
        footer={
          <>
            <Button variant="ghost" onClick={() => setTransferDialog(null)}>
              Отмена
            </Button>
            <Button variant="primary" loading={pending} onClick={() => void submitTransfer()}>
              {transferDialog?.mode === 'copy' ? 'Копировать' : 'Переместить'}
            </Button>
          </>
        }
      >
        {transferDialog ? (
          <>
            <p className="muted">
              {basename(transferDialog.entry.path)} → новый относительный путь
            </p>
            <Field label="Целевой путь">
              <input
                autoFocus
                value={transferDialog.target}
                onChange={(event) =>
                  setTransferDialog({ ...transferDialog, target: event.target.value })
                }
                onKeyDown={(event) => {
                  if (event.key === 'Enter') void submitTransfer();
                }}
              />
            </Field>
          </>
        ) : null}
      </Modal>

      <Modal
        open={Boolean(deleteTarget)}
        onClose={() => setDeleteTarget(null)}
        title="Удалить безвозвратно?"
        description="Операция выполняется в workspace Runtime и не использует корзину. Изменения агента можно откатить только при наличии снимка."
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleteTarget(null)}>
              Отмена
            </Button>
            <Button variant="danger" loading={pending} onClick={() => void submitDelete()}>
              Удалить
            </Button>
          </>
        }
      >
        <p>
          {deleteTarget?.path}
          {deleteTarget?.type === 'dir' ? ' (каталог и всё содержимое)' : ''}
        </p>
      </Modal>
    </section>
  );
}
