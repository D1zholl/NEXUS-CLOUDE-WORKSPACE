import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { DragEvent, ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  FileCode2,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  GripVertical,
  Image,
  Info,
  Menu,
  Package,
  Pause,
  Pencil,
  Play,
  RefreshCw,
  RotateCw,
  Search,
  Sparkles,
  Trash2,
  UploadCloud,
  X,
} from 'lucide-react';

import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  Field,
  LoadingInline,
  Modal,
  SectionTitle,
} from './ui';
import { api, errorMessage, type BuildFileRow } from '../services/api';
import { formatBytes, formatDate, formatUptime, pluralize } from '../lib/format';
import { realtime } from '../lib/realtime';
import { toast } from '../stores/ui';
import {
  ALLOWED_EXTENSIONS,
  CANONICAL_ROOT_FOLDERS,
  buildBuildTree,
  categoryForPath,
  directChildFolders,
  directFiles,
  isFixedRootPath,
  isOptionsPath,
  nextFreePath,
  pathKey,
  resolveDropTarget,
  validateBuildPath,
  type BuildPathKind,
  type BuildTreeNode,
} from '../lib/buildPaths';
import { collectDroppedFiles, type DroppedFile } from '../lib/buildDrop';
import {
  BuildUploadQueue,
  type BuildQueueStats,
  type BuildUploadInput,
  type BuildUploadItem,
} from '../lib/buildQueue';

const CATEGORY_LABELS: Record<string, string> = {
  mods: 'Mods',
  config: 'Config',
  resourcepacks: 'Resourcepacks',
  shaderpacks: 'Shaderpacks',
  scripts: 'Scripts',
  overrides: 'Overrides',
  other: 'Other',
};

const SIDE_LABELS: Record<string, string> = {
  client: 'клиент',
  server: 'сервер',
  both: 'обе',
};

const SIDE_FILTERS = [
  { value: '', label: 'Любая сторона' },
  { value: 'client', label: 'Client' },
  { value: 'server', label: 'Server' },
  { value: 'both', label: 'Both' },
];

type SortKey = 'path-asc' | 'path-desc' | 'size-desc' | 'date-desc';

type ConflictStrategy = 'keep' | 'rename' | 'replace' | 'skip';

interface PreviewRow {
  id: string;
  file: File;
  sourcePath: string;
  path: string;
  error: string | null;
  ambiguous: string[];
  strategy: ConflictStrategy;
}

interface PreviewState {
  rows: PreviewRow[];
  target: string;
}

interface RenameState {
  row: BuildFileRow;
  value: string;
}

interface MoveState {
  row: BuildFileRow;
  value: string;
}

interface BulkMoveState {
  rows: BuildFileRow[];
  value: string;
}

function fileIcon(row: BuildFileRow) {
  if (row.kind === 'dir') return <Folder aria-hidden="true" />;
  const lower = row.path.toLowerCase();
  if (lower.endsWith('.jar')) return <Package aria-hidden="true" />;
  if (lower.endsWith('.zip')) return <Sparkles aria-hidden="true" />;
  if (lower.endsWith('.png') || lower.endsWith('.jpg') || lower.endsWith('.jpeg')) {
    return <Image aria-hidden="true" />;
  }
  if (
    lower.endsWith('.json') ||
    lower.endsWith('.json5') ||
    lower.endsWith('.toml') ||
    lower.endsWith('.cfg') ||
    lower.endsWith('.properties')
  ) {
    return <FileCode2 aria-hidden="true" />;
  }
  return <FileText aria-hidden="true" />;
}

function folderIcon(path: string) {
  const root = path.split('/')[0];
  if (root === 'mods') return <Package aria-hidden="true" />;
  if (root === 'shaderpacks') return <Sparkles aria-hidden="true" />;
  if (root === 'resourcepacks') return <Image aria-hidden="true" />;
  if (root === 'config') return <FileCode2 aria-hidden="true" />;
  return <Folder aria-hidden="true" />;
}

async function copyToClipboard(value: string, message = 'Путь скопирован.'): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
    toast(message, 'success');
  } catch {
    toast(value, 'warning');
  }
}

function parentOf(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  const index = normalized.lastIndexOf('/');
  return index > 0 ? normalized.slice(0, index) : '';
}

function baseName(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  const index = normalized.lastIndexOf('/');
  return index >= 0 ? normalized.slice(index + 1) : normalized;
}

/**
 * Файловый менеджер сборки: каноническое дерево инстанса, таблица файлов,
 * drag & drop из проводника, внутренние перемещения и настройки файла.
 */
export function BuildFileManager({
  buildId,
  canEdit,
  canUpload,
  chunkMaxBytes,
  maxUploadBytes,
  onChanged,
}: {
  buildId: string;
  canEdit: boolean;
  canUpload: boolean;
  chunkMaxBytes?: number;
  maxUploadBytes?: number;
  onChanged: () => void;
}) {
  const filesQuery = useQuery({
    queryKey: ['mc-files', buildId],
    queryFn: () => api.listBuildFiles(buildId),
    enabled: Boolean(buildId),
  });

  const files = useMemo(() => filesQuery.data?.items ?? [], [filesQuery.data]);
  const tree = useMemo(() => buildBuildTree(files), [files]);
  const existingKeys = useMemo(() => new Set(files.map((row) => pathKey(row.path))), [files]);
  const optionsRow = useMemo(
    () => files.find((row) => row.kind === 'file' && isOptionsPath(row.path)),
    [files],
  );

  const [currentFolder, setCurrentFolder] = useState('');
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(['config']));
  const [treeOpen, setTreeOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('');
  const [sideFilter, setSideFilter] = useState('');
  const [sort, setSort] = useState<SortKey>('path-asc');
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const [editorRow, setEditorRow] = useState<BuildFileRow | null>(null);
  const [rename, setRename] = useState<RenameState | null>(null);
  const [move, setMove] = useState<MoveState | null>(null);
  const [bulkMove, setBulkMove] = useState<BulkMoveState | null>(null);
  const [inlineFolder, setInlineFolder] = useState<string | null>(null);
  const [deleteTargets, setDeleteTargets] = useState<BuildFileRow[] | null>(null);
  const [replaceTarget, setReplaceTarget] = useState<BuildFileRow | null>(null);
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [viewer, setViewer] = useState<{ path: string; text: string; error: string | null } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [uploads, setUploads] = useState<BuildUploadItem[]>([]);
  const [queueStats, setQueueStats] = useState<BuildQueueStats | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);
  const folderInputRef = useRef<HTMLInputElement>(null);
  const optionsInputRef = useRef<HTMLInputElement>(null);
  const replaceInputRef = useRef<HTMLInputElement>(null);
  const queueRef = useRef<BuildUploadQueue | null>(null);
  const chunkRef = useRef(chunkMaxBytes ?? 8 * 1024 * 1024);
  chunkRef.current = chunkMaxBytes ?? 8 * 1024 * 1024;

  const dragDepth = useRef(0);
  const [externalDrag, setExternalDrag] = useState(false);
  const [dropTarget, setDropTarget] = useState<string | null>(null);

  const refresh = useCallback((): void => {
    void filesQuery.refetch();
    onChanged();
  }, [filesQuery, onChanged]);

  useEffect(() => {
    if (currentFolder && !files.some((row) => row.kind === 'dir' && row.path === currentFolder)) {
      const isSkeleton = (CANONICAL_ROOT_FOLDERS as readonly string[]).includes(currentFolder);
      if (!isSkeleton) setCurrentFolder(parentOf(currentFolder));
    }
  }, [currentFolder, files]);

  useEffect(() => {
    if (!currentFolder) return;
    const parts = currentFolder.split('/');
    setExpanded((current) => {
      const next = new Set(current);
      next.add('');
      for (let index = 1; index < parts.length; index += 1) {
        next.add(parts.slice(0, index).join('/'));
      }
      return next;
    });
  }, [currentFolder]);

  useEffect(() => {
    setSelected((current) => {
      const alive = new Set(files.map((row) => row.id));
      const next = new Set([...current].filter((id) => alive.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [files]);

  /* ------------------------------ очередь ------------------------------ */

  const publishQueue = useCallback((): void => {
    const queue = queueRef.current;
    setUploads(queue?.getItems() ?? []);
    setQueueStats(queue?.stats() ?? null);
  }, []);

  const queueNow = useCallback((): BuildUploadQueue => {
    if (!queueRef.current) {
      queueRef.current = new BuildUploadQueue({
        buildId,
        chunkMaxBytes: chunkRef.current,
        concurrency: 2,
        categoryForPath: (path) => categoryForPath(path),
        onUpdate: publishQueue,
        onCompleted: () => {
          refresh();
        },
        onFailed: (item) => {
          if (item.error) toast(`«${item.path}»: ${item.error}`, 'error');
        },
      });
    } else {
      queueRef.current.setOptions({ chunkMaxBytes: chunkRef.current });
    }
    return queueRef.current;
  }, [buildId, publishQueue, refresh]);

  useEffect(() => {
    const offProgress = realtime.on('upload.progress', (event) => {
      queueRef.current?.applyServerProgress(event.payload.uploadId, event.payload.uploaded);
    });
    const offFailed = realtime.on('upload.failed', (event) => {
      if (!event.payload.entryName) return;
      queueRef.current?.markServerFailure(event.payload.uploadId, event.payload.message);
    });
    return () => {
      offProgress();
      offFailed();
    };
  }, []);

  // При переходе на другую сборку очередь не должна отправлять чанки в старую.
  useEffect(() => {
    return () => {
      void queueRef.current?.cancelAll();
      queueRef.current = null;
      setUploads([]);
      setQueueStats(null);
    };
  }, [buildId]);

  const enqueue = useCallback(
    (inputs: BuildUploadInput[]): void => {
      if (!canUpload) {
        toast('Недостаточно прав для загрузки файлов.', 'error');
        return;
      }
      const maxBytes = maxUploadBytes ?? 0;
      const accepted: BuildUploadInput[] = [];
      for (const input of inputs) {
        if (input.file.size === 0) {
          toast(`Файл «${input.file.name}» пустой.`, 'error');
          continue;
        }
        if (maxBytes > 0 && input.file.size > maxBytes) {
          toast(`Файл «${input.file.name}» больше допустимых ${formatBytes(maxBytes)}.`, 'error');
          continue;
        }
        const validation = validateBuildPath(input.path, 'file');
        if (!validation.ok) {
          toast(`«${input.path}»: ${validation.errors[0]}`, 'error');
          continue;
        }
        accepted.push({ ...input, path: validation.normalized });
      }
      if (accepted.length === 0) return;
      const queue = queueNow();
      queue.add(accepted);
      toast(
        `${accepted.length} ${pluralize(accepted.length, 'файл', 'файла', 'файлов')} в очереди загрузки.`,
        'info',
      );
    },
    [canUpload, maxUploadBytes, queueNow],
  );

  /* --------------------------- Drag & Drop --------------------------- */

  const openPreview = useCallback(
    (dropped: DroppedFile[], target: string): void => {
      if (dropped.length === 0) {
        toast('Не удалось прочитать перетаскиваемые файлы.', 'error');
        return;
      }
      const rows: PreviewRow[] = dropped.map((entry, index) => {
        const resolved = resolveDropTarget(entry.file.name, entry.relativePath, target);
        const conflict = existingKeys.has(pathKey(resolved.targetPath));
        const optionsConflict = isOptionsPath(resolved.targetPath);
        return {
          id: `${index}-${entry.relativePath}`,
          file: entry.file,
          sourcePath: resolved.sourcePath,
          path: resolved.targetPath,
          error: resolved.error,
          ambiguous: resolved.ambiguous,
          strategy: optionsConflict && conflict ? 'replace' : conflict ? 'rename' : 'keep',
        };
      });
      setPreview({ rows, target });
    },
    [existingKeys],
  );

  const resolutionForPreview = useCallback(
    (rows: PreviewRow[]): Array<{ row: PreviewRow; path: string; mode: 'auto' | 'replace' }> => {
      const used = new Set(existingKeys);
      const resolved: Array<{ row: PreviewRow; path: string; mode: 'auto' | 'replace' }> = [];
      for (const row of rows) {
        if (row.error || row.strategy === 'skip') continue;
        const validation = validateBuildPath(row.path, 'file');
        if (!validation.ok) continue;
        let path = validation.normalized;
        let mode: 'auto' | 'replace' = 'auto';
        const conflict = used.has(pathKey(path));
        if (conflict) {
          if (row.strategy === 'replace') {
            mode = 'replace';
          } else if (row.strategy === 'rename' || row.strategy === 'keep') {
            path = nextFreePath(path, used);
          } else {
            continue;
          }
        }
        used.add(pathKey(path));
        resolved.push({ row, path, mode });
      }
      return resolved;
    },
    [existingKeys],
  );

  const previewResolved = useMemo(
    () => (preview ? resolutionForPreview(preview.rows) : []),
    [preview, resolutionForPreview],
  );

  const dropFiles = useCallback(
    async (event: DragEvent, target: string): Promise<void> => {
      event.preventDefault();
      event.stopPropagation();
      dragDepth.current = 0;
      setExternalDrag(false);
      setDropTarget(null);
      const dropped = await collectDroppedFiles(event.dataTransfer);
      if (dropped.length === 0) {
        toast('Перетащите файлы или папки из проводника.', 'warning');
        return;
      }
      const resolved = dropped.map((entry) =>
        resolveDropTarget(entry.file.name, entry.relativePath, target),
      );
      const canEnqueueDirectly =
        target.length > 0 &&
        resolved.every(
          (entry) =>
            !entry.error &&
            entry.ambiguous.length === 0 &&
            !existingKeys.has(pathKey(entry.targetPath)),
        );
      if (canEnqueueDirectly) {
        enqueue(
          dropped.map((entry, index) => ({
            file: entry.file,
            path: resolved[index]?.targetPath ?? entry.relativePath,
          })),
        );
        return;
      }
      openPreview(dropped, target);
    },
    [enqueue, existingKeys, openPreview],
  );

  const onDragEnter = (event: DragEvent): void => {
    if (!event.dataTransfer.types.includes('Files')) return;
    event.preventDefault();
    dragDepth.current += 1;
    setExternalDrag(true);
  };

  const onDragLeave = (event: DragEvent): void => {
    if (!event.dataTransfer.types.includes('Files')) return;
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) setExternalDrag(false);
  };

  /* --------------------------- мутации путей --------------------------- */

  const performMove = useCallback(
    async (row: BuildFileRow, nextPath: string): Promise<boolean> => {
      const kind: BuildPathKind = row.kind === 'dir' ? 'dir' : 'file';
      const validation = validateBuildPath(nextPath, kind);
      if (!validation.ok) {
        toast(validation.errors[0] ?? 'Некорректный путь.', 'error');
        return false;
      }
      if (pathKey(validation.normalized) === pathKey(row.path)) return true;
      setBusy(true);
      try {
        await api.updateBuildFile(buildId, row.id, { path: validation.normalized });
        refresh();
        toast('Путь обновлён.', 'success');
        return true;
      } catch (error) {
        toast(errorMessage(error), 'error');
        return false;
      } finally {
        setBusy(false);
      }
    },
    [buildId, refresh],
  );

  const handleInternalDrop = useCallback(
    async (payload: string, targetFolder: string): Promise<void> => {
      let source: { id?: string; path?: string; kind?: string } = {};
      try {
        source = JSON.parse(payload) as typeof source;
      } catch {
        return;
      }
      if (!source.path) return;
      if (
        source.kind === 'dir' &&
        (targetFolder === source.path || targetFolder.startsWith(`${source.path}/`))
      ) {
        toast('Нельзя переместить папку внутрь самой себя.', 'error');
        return;
      }
      const row = files.find((item) => pathKey(item.path) === pathKey(source.path ?? ''));
      if (!row) {
        toast('Перемещение доступно только для сохранённых файлов и папок.', 'warning');
        return;
      }
      const nextPath = targetFolder ? `${targetFolder}/${baseName(row.path)}` : baseName(row.path);
      const kind: BuildPathKind = row.kind === 'dir' ? 'dir' : 'file';
      const validation = validateBuildPath(nextPath, kind);
      if (!validation.ok) {
        setMove({ row, value: nextPath });
        toast(validation.errors[0] ?? 'Путь не соответствует контракту.', 'error');
        return;
      }
      const success = await performMove(row, validation.normalized);
      if (!success) setMove({ row, value: nextPath });
    },
    [files, performMove],
  );

  const onTreeDrop = useCallback(
    async (event: DragEvent, targetFolder: string): Promise<void> => {
      const internal = event.dataTransfer.getData('application/x-nexus-build-file');
      if (internal) {
        event.preventDefault();
        event.stopPropagation();
        setDropTarget(null);
        await handleInternalDrop(internal, targetFolder);
        return;
      }
      await dropFiles(event, targetFolder);
    },
    [dropFiles, handleInternalDrop],
  );

  /* ----------------------------- действия ----------------------------- */

  const createFolder = async (): Promise<void> => {
    if (inlineFolder === null) return;
    const validation = validateBuildPath(inlineFolder, 'dir');
    if (!validation.ok) return;
    setBusy(true);
    try {
      await api.createBuildFolder(buildId, validation.normalized);
      setInlineFolder(null);
      setExpanded((current) => new Set([...current, parentOf(validation.normalized)]));
      setCurrentFolder(validation.normalized);
      refresh();
      toast('Папка создана.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const saveRename = async (): Promise<void> => {
    if (!rename) return;
    const kind: BuildPathKind = rename.row.kind === 'dir' ? 'dir' : 'file';
    const parent = parentOf(rename.row.path);
    const target = parent ? `${parent}/${rename.value.trim()}` : rename.value.trim();
    const validation = validateBuildPath(target, kind);
    if (!validation.ok) return;
    setBusy(true);
    try {
      await api.updateBuildFile(buildId, rename.row.id, { path: validation.normalized });
      setRename(null);
      refresh();
      toast('Переименовано.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const saveMove = async (): Promise<void> => {
    if (!move) return;
    const ok = await performMove(move.row, move.value);
    if (ok) setMove(null);
  };

  const deleteRows = async (): Promise<void> => {
    if (!deleteTargets) return;
    setBusy(true);
    try {
      for (const row of deleteTargets) {
        await api.deleteBuildFile(buildId, row.id);
      }
      setDeleteTargets(null);
      setSelected(new Set());
      refresh();
      toast('Удалено.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const applyBulk = async (
    rows: BuildFileRow[],
    patch: { side?: string; required?: boolean; enabled?: boolean },
  ): Promise<void> => {
    if (rows.length === 0) return;
    setBusy(true);
    try {
      for (const row of rows) {
        await api.updateBuildFile(buildId, row.id, patch);
      }
      refresh();
      toast('Настройки обновлены.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const saveBulkMove = async (): Promise<void> => {
    if (!bulkMove) return;
    const validation = validateBuildPath(bulkMove.value, 'dir');
    if (!validation.ok) return;
    setBusy(true);
    let moved = 0;
    try {
      for (const row of bulkMove.rows) {
        const target = `${validation.normalized}/${baseName(row.path)}`;
        try {
          await api.updateBuildFile(buildId, row.id, { path: target });
          moved += 1;
        } catch (error) {
          toast(`«${row.path}»: ${errorMessage(error)}`, 'error');
        }
      }
      setBulkMove(null);
      setSelected(new Set());
      refresh();
      toast(`Перемещено файлов: ${moved}.`, 'success');
    } finally {
      setBusy(false);
    }
  };

  const replaceFile = async (target: BuildFileRow, file: File): Promise<void> => {
    // `mode: replace` создаёт серверную сессию и атомарно подменяет object_id
    // при фиксации; отдельная сессия здесь не нужна.
    setReplaceTarget(null);
    enqueue([
      {
        file,
        path: target.path,
        mode: 'replace',
        category: target.category,
        side: target.side,
        required: target.required,
        description: target.description,
      },
    ]);
  };

  const openOptionsUpload = (): void => optionsInputRef.current?.click();

  const openFilePicker = (): void => fileInputRef.current?.click();
  const openFolderPicker = (): void => folderInputRef.current?.click();

  const handlePickedFiles = (picked: File[], target: string): void => {
    const existing = picked
      .map((file) => ({
        file,
        relativePath:
          (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name,
      }))
      .filter(Boolean);
    const unresolved = existing.some((entry) => {
      const resolved = resolveDropTarget(entry.file.name, entry.relativePath, target);
      return (
        resolved.error !== null ||
        resolved.ambiguous.length > 0 ||
        existingKeys.has(pathKey(resolved.targetPath))
      );
    });
    if (unresolved) {
      openPreview(existing, target);
    } else {
      enqueue(
        existing.map((entry) => ({
          file: entry.file,
          path: resolveDropTarget(entry.file.name, entry.relativePath, target).targetPath,
        })),
      );
    }
  };

  const openViewer = async (row: BuildFileRow): Promise<void> => {
    if (!row.sha256) {
      toast('Содержимое файла недоступно.', 'warning');
      return;
    }
    setViewer({ path: row.path, text: 'Загрузка…', error: null });
    try {
      const response = await fetch(`/api/files/sha256:${row.sha256}`, { credentials: 'include' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const text = await response.text();
      const limit = 256 * 1024;
      setViewer({
        path: row.path,
        text:
          text.length > limit
            ? `${text.slice(0, limit)}\n\n… показаны первые ${Math.round(limit / 1024)} КБ.`
            : text,
        error: null,
      });
    } catch (error) {
      setViewer({
        path: row.path,
        text: '',
        error: `Не удалось прочитать файл: ${errorMessage(error)}`,
      });
    }
  };

  /* --------------------------- производные --------------------------- */

  const searching = search.trim().length > 0 || categoryFilter.length > 0 || sideFilter.length > 0;

  const visibleFiles = useMemo(() => {
    const source = searching
      ? files.filter((row) => row.kind === 'file')
      : directFiles(tree, currentFolder);
    const query = search.trim().toLowerCase();
    const filtered = source.filter((row) => {
      if (query && !row.path.toLowerCase().includes(query)) return false;
      if (categoryFilter && row.category !== categoryFilter) return false;
      if (sideFilter && row.side !== sideFilter) return false;
      return true;
    });
    return filtered.sort((left, right) => {
      if (sort === 'path-desc') return right.path.localeCompare(left.path, 'ru');
      if (sort === 'size-desc') return right.sizeBytes - left.sizeBytes;
      if (sort === 'date-desc') {
        return new Date(right.createdAt).getTime() - new Date(left.createdAt).getTime();
      }
      return left.path.localeCompare(right.path, 'ru');
    });
  }, [categoryFilter, currentFolder, files, search, searching, sideFilter, sort, tree]);

  const childFolders = useMemo(
    () => directChildFolders(tree, currentFolder),
    [currentFolder, tree],
  );

  const currentFolderRow = useMemo(
    () => files.find((row) => row.kind === 'dir' && pathKey(row.path) === pathKey(currentFolder)),
    [currentFolder, files],
  );

  const selectedRows = useMemo(
    () => files.filter((row) => selected.has(row.id)),
    [files, selected],
  );

  const breadcrumbs = currentFolder ? currentFolder.split('/') : [];
  const currentFixed = currentFolder === '' || isFixedRootPath(currentFolder);

  const toggleSelected = (id: string): void => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAll = (): void => {
    setSelected((current) => {
      const ids = visibleFiles.map((row) => row.id);
      const all = ids.length > 0 && ids.every((id) => current.has(id));
      return all ? new Set() : new Set(ids);
    });
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'F2' || !canEdit) return;
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)
      ) {
        return;
      }
      const row =
        selectedRows.length === 1
          ? selectedRows[0]
          : visibleFiles.length === 1
            ? visibleFiles[0]
            : null;
      if (row) {
        event.preventDefault();
        setRename({ row, value: baseName(row.path) });
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [canEdit, selectedRows, visibleFiles]);

  const renderNode = (node: BuildTreeNode, level: number): ReactNode => {
    const isExpanded = node.path === '' || expanded.has(node.path);
    const hasChildren = node.children.length > 0;
    const active = pathKey(currentFolder) === pathKey(node.path);
    const draggable = canEdit && !node.fixed && node.path !== '';
    const canToggle = hasChildren && node.path !== '';
    return (
      <div key={node.path || 'root'}>
        <div
          className={`build-tree__row${active ? ' build-tree__row--active' : ''}${
            dropTarget === node.path ? ' build-tree__row--drop' : ''
          }`}
          style={{ paddingLeft: 8 + level * 14 }}
          onClick={() => {
            setCurrentFolder(node.path);
            setTreeOpen(false);
          }}
          onDragOver={(event) => {
            event.preventDefault();
            if (event.dataTransfer.types.includes('Files')) event.dataTransfer.dropEffect = 'copy';
            else event.dataTransfer.dropEffect = 'move';
            setDropTarget(node.path);
          }}
          onDragLeave={() => setDropTarget((current) => (current === node.path ? null : current))}
          onDrop={(event) => void onTreeDrop(event, node.path)}
          draggable={draggable}
          data-testid={`build-tree-${node.path || 'root'}`}
          onDragStart={(event) => {
            if (!draggable) return;
            const row = files.find((item) => pathKey(item.path) === pathKey(node.path));
            if (!row) return;
            event.dataTransfer.setData(
              'application/x-nexus-build-file',
              JSON.stringify({ id: row.id, path: row.path, kind: row.kind }),
            );
            event.dataTransfer.effectAllowed = 'move';
          }}
        >
          <button
            type="button"
            className="build-tree__toggle"
            aria-label={isExpanded ? 'Свернуть' : 'Развернуть'}
            onClick={(event) => {
              event.stopPropagation();
              setExpanded((current) => {
                const next = new Set(current);
                if (next.has(node.path)) next.delete(node.path);
                else next.add(node.path);
                return next;
              });
            }}
            disabled={!canToggle}
          >
            {canToggle ? (
              isExpanded ? (
                <ChevronDown aria-hidden="true" />
              ) : (
                <ChevronRight aria-hidden="true" />
              )
            ) : (
              <span className="build-tree__dot" />
            )}
          </button>
          <span className="build-tree__icon">{folderIcon(node.path)}</span>
          <span className="build-tree__label truncate" title={node.path}>
            {node.path === '' ? 'Корень версии' : node.label}
          </span>
          {node.fixed ? <span className="build-tree__fixed">fixed</span> : null}
          <span className="build-tree__count tiny muted">
            {node.totalFiles} · {formatBytes(node.totalBytes)}
          </span>
          {draggable ? <GripVertical aria-hidden="true" className="build-tree__grip" /> : null}
        </div>
        {node.path === '' ? (
          <button
            type="button"
            className={`build-tree__row build-tree__row--option${
              optionsRow ? '' : ' build-tree__row--empty'
            }${dropTarget === 'options.txt' ? ' build-tree__row--drop' : ''}`}
            style={{ paddingLeft: 22 }}
            data-testid="build-tree-options"
            onClick={() => {
              setCurrentFolder('');
              setTreeOpen(false);
              if (optionsRow) setEditorRow(optionsRow);
              else openOptionsUpload();
            }}
            onDragOver={(event) => {
              if (!event.dataTransfer.types.includes('Files')) return;
              event.preventDefault();
              setDropTarget('options.txt');
            }}
            onDrop={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void (async () => {
                const dropped = await collectDroppedFiles(event.dataTransfer);
                if (dropped.length !== 1 || !isOptionsPath(dropped[0]?.file.name ?? '')) {
                  toast('В слот options.txt можно перетащить только options.txt.', 'error');
                  return;
                }
                enqueue([
                  {
                    file: dropped[0].file,
                    path: 'options.txt',
                    mode: optionsRow ? 'replace' : 'auto',
                    category: 'config',
                    side: 'client',
                    required: true,
                  },
                ]);
              })();
            }}
          >
            <span className="build-tree__dot" />
            <span className="build-tree__icon">
              <FileText aria-hidden="true" />
            </span>
            <span className="build-tree__label">options.txt</span>
            <span className="build-tree__count tiny muted">
              {optionsRow ? formatBytes(optionsRow.sizeBytes) : 'слот'}
            </span>
            {optionsRow?.enabled ? <Badge tone="ok">on</Badge> : null}
          </button>
        ) : null}
                {isExpanded ? node.children.map((child) => renderNode(child, level + 1)) : null}
      </div>
    );
  };

  return (
    <div
      className="build-manager"
      onDragEnter={onDragEnter}
      onDragLeave={onDragLeave}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault();
          event.dataTransfer.dropEffect = 'copy';
        }
      }}
      onDrop={(event) => void dropFiles(event, currentFolder)}
      data-testid="build-file-manager"
    >
      {externalDrag ? (
        <div className="build-drop-overlay" aria-hidden="true">
          <UploadCloud />
          <strong>Отпустите файлы или папку</strong>
          <span>
            {dropTarget || currentFolder
              ? `Назначение: ${dropTarget || currentFolder}/`
              : 'Назначение будет предложено в предпросмотре'}
          </span>
        </div>
      ) : null}

      <div className={`build-manager__grid${treeOpen ? ' build-manager__grid--tree-open' : ''}`}>
        <aside className="build-tree card">
          <div className="build-tree__head">
            <SectionTitle
              title="Дерево инстанса"
              meta={`${files.length} ${pluralize(files.length, 'запись', 'записи', 'записей')}`}
            />
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              className="build-tree__close"
              aria-label="Скрыть дерево"
              onClick={() => setTreeOpen(false)}
            >
              <X aria-hidden="true" />
            </Button>
          </div>
          <div className="build-tree__scroll">{renderNode(tree, 0)}</div>
          <div className="build-tree__legend tiny muted">
            <span>
              <Badge plain>config</Badge> любая вложенность
            </span>
            <span>mods — только .jar</span>
          </div>
        </aside>

        <section className="build-files">
          <div className="build-files__bar">
            <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
              <Button
                size="sm"
                variant="ghost"
                icon={<Menu aria-hidden="true" />}
                className="build-files__tree-button"
                onClick={() => setTreeOpen(true)}
              >
                Дерево
              </Button>
              <nav className="build-breadcrumbs" aria-label="Полный путь">
                <button type="button" onClick={() => setCurrentFolder('')}>
                  Корень версии
                </button>
                {breadcrumbs.map((segment, index) => {
                  const path = breadcrumbs.slice(0, index + 1).join('/');
                  return (
                    <span key={path} className="build-breadcrumbs__item">
                      <ChevronRight aria-hidden="true" />
                      <button type="button" onClick={() => setCurrentFolder(path)}>
                        {segment}
                      </button>
                    </span>
                  );
                })}
              </nav>
              <Button
                size="sm"
                variant="ghost"
                iconOnly
                aria-label="Копировать полный путь"
                title="Копировать полный путь"
                onClick={() => void copyToClipboard(currentFolder || 'options.txt')}
              >
                <Copy aria-hidden="true" />
              </Button>
            </div>
            <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
              {canUpload ? (
                <>
                  <Button
                    size="sm"
                    variant="primary"
                    icon={<UploadCloud aria-hidden="true" />}
                    onClick={openFilePicker}
                  >
                    Загрузить файлы
                  </Button>
                  <Button size="sm" icon={<Folder aria-hidden="true" />} onClick={openFolderPicker}>
                    Загрузить папку
                  </Button>
                  {optionsRow ? null : (
                    <Button
                      size="sm"
                      icon={<FileText aria-hidden="true" />}
                      onClick={openOptionsUpload}
                    >
                      options.txt
                    </Button>
                  )}
                </>
              ) : null}
              {canEdit ? (
                <Button
                  size="sm"
                  icon={<FolderPlus aria-hidden="true" />}
                  onClick={() => setInlineFolder(currentFolder ? `${currentFolder}/` : 'config/')}
                >
                  Папка
                </Button>
              ) : null}
            </div>
          </div>

                    {inlineFolder !== null ? (
            <form
              className="build-inline-folder"
              onSubmit={(event) => {
                event.preventDefault();
                void createFolder();
              }}
            >
              <input
                className="input mono"
                aria-label="Путь новой папки"
                value={inlineFolder}
                autoFocus
                onChange={(event) => setInlineFolder(event.target.value)}
                placeholder="config/new"
              />
              <Button
                type="submit"
                size="sm"
                variant="primary"
                loading={busy}
                disabled={!validateBuildPath(inlineFolder, 'dir').ok}
              >
                Создать
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setInlineFolder(null)}>
                Отмена
              </Button>
              <span className="tiny muted">
                {validateBuildPath(inlineFolder, 'dir').errors[0] ??
                  `Итог: ${validateBuildPath(inlineFolder, 'dir').normalized}`}
              </span>
            </form>
          ) : null}

          <div className="build-files__filters">
            <form
              className="input-wrap"
              onSubmit={(event) => {
                event.preventDefault();
              }}
            >
              <Search aria-hidden="true" />
              <input
                className="input input--with-icon"
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="Поиск по имени и пути…"
                aria-label="Поиск файлов сборки"
              />
            </form>
            <select
              className="select"
              value={categoryFilter}
              onChange={(event) => setCategoryFilter(event.target.value)}
              aria-label="Фильтр по категории"
            >
              <option value="">Все категории</option>
              {Object.entries(CATEGORY_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <select
              className="select"
              value={sideFilter}
              onChange={(event) => setSideFilter(event.target.value)}
              aria-label="Фильтр по стороне"
            >
              {SIDE_FILTERS.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
            <select
              className="select"
              value={sort}
              onChange={(event) => setSort(event.target.value as SortKey)}
              aria-label="Сортировка"
            >
              <option value="path-asc">Путь А→Я</option>
              <option value="path-desc">Путь Я→А</option>
              <option value="size-desc">Сначала крупные</option>
              <option value="date-desc">Сначала новые</option>
            </select>
          </div>

                    {selected.size > 0 ? (
            <div className="build-bulk card">
              <strong>{selected.size} выбрано</strong>
              <Button
                size="sm"
                onClick={() =>
                  setBulkMove({ rows: selectedRows, value: currentFolder || 'config' })
                }
              >
                Переместить
              </Button>
              <select
                className="select select--sm"
                aria-label="Изменить сторону выбранных"
                value=""
                onChange={(event) => {
                  if (event.target.value)
                    void applyBulk(selectedRows, { side: event.target.value });
                }}
              >
                <option value="">Сторона…</option>
                <option value="client">Client</option>
                <option value="server">Server</option>
                <option value="both">Both</option>
              </select>
              <Button size="sm" onClick={() => void applyBulk(selectedRows, { required: true })}>
                Обязательные
              </Button>
              <Button size="sm" onClick={() => void applyBulk(selectedRows, { required: false })}>
                Необязательные
              </Button>
              <Button size="sm" onClick={() => void applyBulk(selectedRows, { enabled: true })}>
                Включить
              </Button>
              <Button size="sm" onClick={() => void applyBulk(selectedRows, { enabled: false })}>
                Выключить
              </Button>
              <Button
                size="sm"
                variant="danger"
                icon={<Trash2 aria-hidden="true" />}
                onClick={() => setDeleteTargets(selectedRows)}
              >
                Удалить
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
                Снять выбор
              </Button>
            </div>
          ) : null}

          {uploads.length > 0 ? (
            <div className="notice-card build-queue-card" aria-label="Очередь загрузки сборки">
            <div className="row row--between" style={{ gap: 10, marginBottom: 8 }}>
              <strong style={{ fontSize: 12 }}>Очередь загрузки сборки</strong>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  queueRef.current?.clearFinished();
                  publishQueue();
                }}
              >
                Очистить
              </Button>
            </div>
            {queueStats ? (
              <div className="tiny muted" style={{ marginBottom: 8 }}>
                {formatBytes(queueStats.uploadedBytes)} / {formatBytes(queueStats.totalBytes)} ·{' '}
                {queueStats.active} активно · {queueStats.queued} в очереди
                {queueStats.failed > 0 ? ` · ${queueStats.failed} ошибок` : ''}
                {queueStats.speedBytesPerSec > 0
                  ? ` · ${formatBytes(queueStats.speedBytesPerSec)}/с`
                  : ''}
                {queueStats.etaSeconds !== null
                  ? ` · ≈ ${formatUptime(queueStats.etaSeconds)}`
                  : ''}
              </div>
            ) : null}
            <div className="stack" style={{ gap: 8, maxHeight: 260, overflow: 'auto' }}>
              {uploads.map((item) => {
                const percent =
                  item.size > 0 ? Math.min(100, Math.round((item.uploaded / item.size) * 100)) : 0;
                return (
                  <div key={item.key} className="build-upload upload-item">
                    <div className="row row--between" style={{ gap: 8 }}>
                      <span className="mono tiny truncate" title={item.path}>
                        {item.path}
                      </span>
                      <span className="tiny muted">{percent}%</span>
                    </div>
                    <div className="meter" aria-hidden="true">
                      <div
                        className={`meter__fill${
                          item.state === 'failed' ? ' meter__fill--warn' : ''
                        }`}
                        style={{ width: `${percent}%` }}
                      />
                    </div>
                    <div className="row row--between" style={{ gap: 6 }}>
                      <span className="tiny muted">
                        {item.state === 'done'
                          ? 'Готово'
                          : item.state === 'failed'
                            ? (item.error ?? 'Ошибка')
                            : item.state === 'paused'
                              ? 'Пауза'
                              : item.state === 'retrying'
                                ? 'Повтор…'
                                : `${formatBytes(item.uploaded)} / ${formatBytes(item.size)}`}
                      </span>
                      <span className="row" style={{ gap: 2 }}>
                        {item.state === 'uploading' || item.state === 'queued' ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            aria-label="Пауза"
                            onClick={() => {
                              queueRef.current?.pause(item.key);
                              publishQueue();
                            }}
                          >
                            <Pause aria-hidden="true" />
                          </Button>
                        ) : null}
                        {item.state === 'paused' ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            aria-label="Продолжить"
                            onClick={() => {
                              queueRef.current?.resume(item.key);
                              publishQueue();
                            }}
                          >
                            <Play aria-hidden="true" />
                          </Button>
                        ) : null}
                        {item.state === 'failed' ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            aria-label="Повторить"
                            onClick={() => {
                              queueRef.current?.retry(item.key);
                              publishQueue();
                            }}
                          >
                            <RotateCw aria-hidden="true" />
                          </Button>
                        ) : null}
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          aria-label="Отменить"
                          onClick={() => {
                            void queueRef.current?.cancel(item.key);
                            publishQueue();
                          }}
                        >
                          <X aria-hidden="true" />
                        </Button>
                      </span>
                    </div>
                  </div>
                );
                              })}
              </div>
            </div>
          ) : null}

          <div className="build-files__current tiny muted">
            Полный путь: <span className="mono">{currentFolder || '/'}</span>
            {currentFolder && !currentFixed ? (
              <span className="row" style={{ gap: 4, display: 'inline-flex', marginLeft: 8 }}>
                <Button
                  size="sm"
                  variant="ghost"
                  iconOnly
                  aria-label="Переименовать папку"
                  onClick={() => {
                    if (currentFolderRow) {
                      setRename({ row: currentFolderRow, value: baseName(currentFolderRow.path) });
                    } else {
                      toast('Папка ещё не сохранена на сервере.', 'warning');
                    }
                  }}
                >
                  <Pencil aria-hidden="true" />
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  iconOnly
                  aria-label="Удалить папку"
                  onClick={() => {
                    if (currentFolderRow) setDeleteTargets([currentFolderRow]);
                    else toast('Папка ещё не сохранена на сервере.', 'warning');
                  }}
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </span>
            ) : null}
          </div>

                    {filesQuery.isLoading ? (
            <LoadingInline label="Загрузка файлов" />
          ) : filesQuery.isError ? (
              <Alert>{errorMessage(filesQuery.error)}</Alert>
            ) : visibleFiles.length === 0 ? (
              <div className="card">
                <EmptyState
                  icon={
                    currentFolder === 'mods' ? (
                      <Package aria-hidden="true" />
                    ) : currentFolder === 'config' ? (
                      <FileCode2 aria-hidden="true" />
                    ) : (
                      <UploadCloud aria-hidden="true" />
                    )
                  }
                  title={searching ? 'Ничего не найдено' : 'Папка пуста'}
                  action={
                    canUpload ? (
                      <Button
                        variant="primary"
                        icon={<UploadCloud aria-hidden="true" />}
                        onClick={
                          currentFolder.startsWith('config') ? openFolderPicker : openFilePicker
                        }
                      >
                        {currentFolder.startsWith('config')
                          ? 'Загрузить папку конфига'
                          : 'Загрузить файлы'}
                      </Button>
                    ) : undefined
                  }
                >
                  {searching
                    ? 'Измените запрос или сбросьте фильтры.'
                    : currentFolder === 'mods'
                      ? 'Перетащите .jar сюда — подпапки в mods запрещены.'
                      : currentFolder.startsWith('config')
                        ? 'Загрузите папку конфига целиком: структура подпапок сохранится.'
                        : currentFolder === ''
                          ? 'Перетащите options.txt или папку config, либо загрузите .jar в mods.'
                          : 'Перетащите файлы в эту папку.'}
                </EmptyState>
              </div>
            ) : (
              <>
                {!searching && childFolders.length > 0 ? (
                  <div className="build-subfolders">
                    {childFolders.map((node) => (
                      <button
                        key={node.path}
                        type="button"
                        className="build-subfolder"
                        onClick={() => setCurrentFolder(node.path)}
                      >
                        <FolderOpen aria-hidden="true" />
                        <span className="truncate">{node.label}</span>
                        <span className="tiny muted">
                          {node.totalFiles} · {formatBytes(node.totalBytes)}
                        </span>
                      </button>
                    ))}
                  </div>
                ) : null}
                <div className="table-card">
                  <div className="table-wrap">
                    <table className="table build-table">
                      <thead>
                        <tr>
                          <th style={{ width: 34 }}>
                            <input
                              type="checkbox"
                              aria-label="Выбрать все файлы"
                              checked={
                                visibleFiles.length > 0 &&
                                visibleFiles.every((row) => selected.has(row.id))
                              }
                              onChange={toggleAll}
                            />
                          </th>
                          <th>Имя / полный путь</th>
                          <th>Размер</th>
                          <th>SHA-256</th>
                          <th>Side</th>
                          <th>Req</th>
                          <th>On</th>
                          <th>Загружен</th>
                          <th style={{ textAlign: 'right' }}>Действия</th>
                        </tr>
                      </thead>
                      <tbody>
                        {visibleFiles.map((row) => (
                          <tr
                            key={row.id}
                            className={
                              selected.has(row.id) ? 'build-table__row--selected' : undefined
                            }
                            draggable={canEdit}
                            onDragStart={(event) => {
                              event.dataTransfer.setData(
                                'application/x-nexus-build-file',
                                JSON.stringify({ id: row.id, path: row.path, kind: row.kind }),
                              );
                              event.dataTransfer.effectAllowed = 'move';
                            }}
                          >
                            <td>
                              <input
                                type="checkbox"
                                aria-label={`Выбрать ${row.path}`}
                                checked={selected.has(row.id)}
                                onChange={() => toggleSelected(row.id)}
                              />
                            </td>
                            <td>
                              <div className="build-file">
                                {fileIcon(row)}
                                <div className="build-file__name">
                                  <span className="truncate">{baseName(row.path)}</span>
                                  <button
                                    type="button"
                                    className="build-file__path mono tiny"
                                    title={`${row.path} — копировать`}
                                    onClick={() => void copyToClipboard(row.path)}
                                  >
                                    {row.path}
                                    <Copy aria-hidden="true" />
                                  </button>
                                </div>
                                {!row.enabled ? <Badge tone="warn">выключен</Badge> : null}
                                {!row.required ? <Badge plain>необязательный</Badge> : null}
                              </div>
                            </td>
                            <td className="mono tiny">{formatBytes(row.sizeBytes)}</td>
                            <td>
                              {row.sha256 ? (
                                <button
                                  type="button"
                                  className="sha-chip mono tiny"
                                  title={`${row.sha256} — копировать`}
                                  onClick={() =>
                                    void copyToClipboard(row.sha256, 'SHA-256 скопирован.')
                                  }
                                >
                                  {row.sha256.slice(0, 12)}…
                                  <Copy aria-hidden="true" />
                                </button>
                              ) : (
                                <span className="tiny muted">—</span>
                              )}
                            </td>
                            <td className="tiny">{SIDE_LABELS[row.side] ?? row.side}</td>
                            <td className="tiny">{row.required ? 'да' : 'нет'}</td>
                            <td className="tiny">{row.enabled ? 'да' : 'нет'}</td>
                            <td className="tiny muted">{formatDate(row.createdAt)}</td>
                            <td>
                              <div className="table__actions">
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  iconOnly
                                  aria-label="Настроить"
                                  title="Настроить"
                                  onClick={() => setEditorRow(row)}
                                >
                                  <Eye aria-hidden="true" />
                                </Button>
                                {canEdit ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Переименовать"
                                    title="F2 — переименовать"
                                    onClick={() => setRename({ row, value: baseName(row.path) })}
                                  >
                                    <Pencil aria-hidden="true" />
                                  </Button>
                                ) : null}
                                {canEdit ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Переместить"
                                    onClick={() => setMove({ row, value: row.path })}
                                  >
                                    <ExternalLinkIcon />
                                  </Button>
                                ) : null}
                                {canUpload ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Заменить содержимое"
                                    onClick={() => {
                                      setReplaceTarget(row);
                                      replaceInputRef.current?.click();
                                    }}
                                  >
                                    <RefreshCw aria-hidden="true" />
                                  </Button>
                                ) : null}
                                {isOptionsPath(row.path) || row.category === 'config' ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Просмотреть текст"
                                    onClick={() => void openViewer(row)}
                                  >
                                    <FileText aria-hidden="true" />
                                  </Button>
                                ) : null}
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  iconOnly
                                  aria-label="Информация"
                                  onClick={() => setEditorRow(row)}
                                >
                                  <Info aria-hidden="true" />
                                </Button>
                                {canEdit && !isFixedRootPath(row.path) ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Удалить"
                                    onClick={() => setDeleteTargets([row])}
                                  >
                                    <Trash2 aria-hidden="true" />
                                  </Button>
                                ) : null}
                              </div>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
                          </>
          )}

          {!canUpload ? (
            <div className="tiny muted" style={{ marginTop: 8 }}>
              Загрузка недоступна: нет права minecraft.artifacts.upload.
            </div>
          ) : null}
        </section>
      </div>

            {editorRow ? (
        <FileEditorDrawer
          key={editorRow.id}
          row={editorRow}
          buildId={buildId}
          canEdit={canEdit}
          onClose={() => setEditorRow(null)}
          onSaved={() => {
            setEditorRow(null);
            refresh();
          }}
          onPreview={
            isOptionsPath(editorRow.path) || editorRow.category === 'config'
              ? () => void openViewer(editorRow)
              : undefined
          }
        />
      ) : null}

      {/* Скрытые input'ы: кнопки полностью дублируют Drag & Drop. */}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        className="hidden"
        data-testid="build-file-input"
        onChange={(event) => {
          const picked = Array.from(event.target.files ?? []);
          if (picked.length > 0) handlePickedFiles(picked, currentFolder);
          event.target.value = '';
        }}
      />
      <input
        ref={folderInputRef}
        type="file"
        multiple
        className="hidden"
        data-testid="build-folder-input"
        // @ts-expect-error нестандартный атрибут поддерживается Chromium
        webkitdirectory=""
        onChange={(event) => {
          const picked = Array.from(event.target.files ?? []);
          if (picked.length > 0) handlePickedFiles(picked, currentFolder || 'config');
          event.target.value = '';
        }}
      />
      <input
        ref={optionsInputRef}
        type="file"
        accept=".txt,text/plain"
        className="hidden"
        data-testid="build-options-input"
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) {
            if (file.name.toLowerCase() !== 'options.txt') {
              toast('Слот options.txt принимает только файл options.txt.', 'error');
            } else {
              enqueue([{ file, path: 'options.txt', mode: optionsRow ? 'replace' : 'auto' }]);
            }
          }
          event.target.value = '';
        }}
      />
      <input
        ref={replaceInputRef}
        type="file"
        className="hidden"
        data-testid="build-replace-input"
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file && replaceTarget) void replaceFile(replaceTarget, file);
          event.target.value = '';
        }}
      />

      {/* -------------------------- предпросмотр drop -------------------------- */}
      <Modal
        open={preview !== null}
        onClose={() => setPreview(null)}
        wide
        title="Предпросмотр загрузки"
        description="Проверьте назначение и разрешите конфликты. Путь можно изменить для каждой строки."
        footer={
          <>
            <Button onClick={() => setPreview(null)}>Отмена</Button>
            <Button
              variant="primary"
              disabled={previewResolved.length === 0}
              onClick={() => {
                if (!preview) return;
                enqueue(
                  previewResolved.map((entry) => {
                    const existing = files.find(
                      (row) => row.kind === 'file' && pathKey(row.path) === pathKey(entry.path),
                    );
                    return {
                      file: entry.row.file,
                      path: entry.path,
                      mode: entry.mode,
                      category: existing?.category,
                      side: existing?.side,
                      required: existing?.required,
                      description: existing?.description,
                    };
                  }),
                );
                setPreview(null);
              }}
            >
              Загрузить {previewResolved.length}
            </Button>
          </>
        }
      >
        {preview ? (
          <div className="stack" style={{ gap: 10 }}>
            {preview.rows.map((row, index) => {
              const effective = previewResolved.find((entry) => entry.row.id === row.id);
              const validation = validateBuildPath(row.path, 'file');
              return (
                <div className="drop-row" key={row.id}>
                  <div className="row row--between" style={{ gap: 8 }}>
                    <span className="mono tiny truncate" title={row.sourcePath}>
                      {row.sourcePath}
                    </span>
                    <span className="tiny muted">{formatBytes(row.file.size)}</span>
                  </div>
                  <input
                    className="input mono"
                    aria-label={`Назначение ${row.sourcePath}`}
                    value={row.path}
                    onChange={(event) => {
                      const value = event.target.value;
                      setPreview((current) =>
                        current
                          ? {
                              ...current,
                              rows: current.rows.map((item, itemIndex) =>
                                itemIndex === index ? { ...item, path: value } : item,
                              ),
                            }
                          : current,
                      );
                    }}
                  />
                  {row.ambiguous.length > 0 ? (
                    <div className="drop-row__hint">
                      Неоднозначно: <Badge tone="info">resourcepacks</Badge> или{' '}
                      <Badge tone="info">shaderpacks</Badge> — выберите путь вручную.
                    </div>
                  ) : null}
                  {validation.errors.length > 0 ? (
                    <div className="drop-row__error">{validation.errors[0]}</div>
                  ) : (
                    <div className="drop-row__hint tiny muted">
                      Итог: <span className="mono">{effective?.path ?? row.path}</span>
                      {effective?.mode === 'replace' ? ' · замена' : ''}
                    </div>
                  )}
                  <select
                    className="select"
                    aria-label={`Конфликт ${row.sourcePath}`}
                    value={row.strategy}
                    onChange={(event) => {
                      const value = event.target.value as ConflictStrategy;
                      setPreview((current) =>
                        current
                          ? {
                              ...current,
                              rows: current.rows.map((item, itemIndex) =>
                                itemIndex === index ? { ...item, strategy: value } : item,
                              ),
                            }
                          : current,
                      );
                    }}
                  >
                    <option value="keep">Без конфликта / как есть</option>
                    <option value="rename">Переименовать</option>
                    <option value="replace">Заменить</option>
                    <option value="skip">Пропустить</option>
                  </select>
                </div>
              );
            })}
            <div className="tiny muted">
              Разрешённые расширения: <span className="mono">{ALLOWED_EXTENSIONS.join(', ')}</span>
            </div>
          </div>
        ) : null}
      </Modal>

      {/* ---------------------------- переименование ---------------------------- */}
      <Modal
        open={rename !== null}
        onClose={() => setRename(null)}
        title="Переименовать"
        description={rename ? rename.row.path : ''}
        footer={
          <>
            <Button onClick={() => setRename(null)}>Отмена</Button>
            <Button
              variant="primary"
              loading={busy}
              disabled={
                !rename ||
                !validateBuildPath(
                  parentOf(rename.row.path)
                    ? `${parentOf(rename.row.path)}/${rename.value.trim()}`
                    : rename.value.trim(),
                  rename.row.kind === 'dir' ? 'dir' : 'file',
                ).ok
              }
              onClick={() => void saveRename()}
            >
              Сохранить
            </Button>
          </>
        }
      >
        {rename ? (
          <PathField
            id="build-rename"
            label="Новое имя"
            value={rename.value}
            kind={rename.row.kind === 'dir' ? 'dir' : 'file'}
            basePath={parentOf(rename.row.path)}
            onChange={(value) => setRename({ ...rename, value })}
          />
        ) : null}
      </Modal>

      {/* ------------------------------ перемещение ------------------------------ */}
      <Modal
        open={move !== null}
        onClose={() => setMove(null)}
        title="Переместить"
        description={move ? move.row.path : ''}
        footer={
          <>
            <Button onClick={() => setMove(null)}>Отмена</Button>
            <Button
              variant="primary"
              loading={busy}
              disabled={
                !move || !validateBuildPath(move.value, move.row.kind === 'dir' ? 'dir' : 'file').ok
              }
              onClick={() => void saveMove()}
            >
              Переместить
            </Button>
          </>
        }
      >
        {move ? (
          <PathField
            id="build-move"
            label="Новый путь"
            value={move.value}
            kind={move.row.kind === 'dir' ? 'dir' : 'file'}
            onChange={(value) => setMove({ ...move, value })}
          />
        ) : null}
      </Modal>

      <Modal
        open={bulkMove !== null}
        onClose={() => setBulkMove(null)}
        title="Переместить выбранные файлы"
        description={bulkMove ? `Файлов: ${bulkMove.rows.length}` : ''}
        footer={
          <>
            <Button onClick={() => setBulkMove(null)}>Отмена</Button>
            <Button
              variant="primary"
              loading={busy}
              disabled={!bulkMove || !validateBuildPath(bulkMove.value, 'dir').ok}
              onClick={() => void saveBulkMove()}
            >
              Переместить
            </Button>
          </>
        }
      >
        {bulkMove ? (
          <PathField
            id="build-bulk-move"
            label="Папка назначения"
            value={bulkMove.value}
            kind="dir"
            onChange={(value) => setBulkMove({ ...bulkMove, value })}
          />
        ) : null}
      </Modal>

      <Modal
        open={viewer !== null}
        onClose={() => setViewer(null)}
        wide
        title="Просмотр файла"
        description={viewer?.path ?? ''}
        footer={<Button onClick={() => setViewer(null)}>Закрыть</Button>}
      >
        {viewer ? (
          viewer.error ? (
            <Alert>{viewer.error}</Alert>
          ) : (
            <pre className="cloud-preview-text build-text-preview">{viewer.text}</pre>
          )
        ) : null}
      </Modal>

      <ConfirmDialog
        open={deleteTargets !== null}
        title="Удалить записи?"
        description={
          deleteTargets
            ? deleteTargets.length === 1
              ? `«${deleteTargets[0].path}» будет удалён${deleteTargets[0].kind === 'dir' ? ' вместе с содержимым' : ''}.`
              : `${deleteTargets.length} записей будут удалены вместе с содержимым папок.`
            : ''
        }
        confirmLabel="Удалить"
        busy={busy}
        onConfirm={() => void deleteRows()}
        onCancel={() => setDeleteTargets(null)}
      />
    </div>
  );
}

/* ------------------------------- поля пути ------------------------------- */

function PathField({
  id,
  label,
  value,
  kind,
  basePath = '',
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  kind: BuildPathKind;
  basePath?: string;
  onChange: (value: string) => void;
}) {
  const full = kind === 'dir' || !basePath ? value : `${basePath}/${value.trim()}`;
  const validation = validateBuildPath(full, kind);
  return (
    <Field
      label={label}
      htmlFor={id}
      error={validation.errors[0]}
      hint={`Итоговый путь: ${validation.normalized || '—'}`}
    >
      <input
        id={id}
        className="input mono"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        data-modal-autofocus
      />
    </Field>
  );
}

/* ---------------------------- редактор файла ---------------------------- */

function FileEditorDrawer({
  row,
  buildId,
  canEdit,
  onClose,
  onSaved,
  onPreview,
}: {
  row: BuildFileRow;
  buildId: string;
  canEdit: boolean;
  onClose: () => void;
  onSaved: () => void;
  onPreview?: () => void;
}) {
  const kind: BuildPathKind = row.kind === 'dir' ? 'dir' : 'file';
  const [form, setForm] = useState({
    path: row.path,
    side: row.side,
    required: row.required,
    enabled: row.enabled,
    description: row.description,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [closing, setClosing] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const validation = validateBuildPath(form.path, kind);

  useEffect(
    () => () => {
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    },
    [],
  );

  // Панель остаётся в DOM на время exit-анимации и сразу перестаёт
  // принимать ввод. Таймер очищается при размонтировании.
  const requestClose = (): void => {
    if (closing) return;
    setClosing(true);
    const reduced =
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    closeTimer.current = window.setTimeout(() => onClose(), reduced ? 0 : 180);
  };
  const pathChanged = pathKey(validation.normalized) !== pathKey(row.path);
  const canSave = !pathChanged || validation.ok;
  const category = categoryForPath(validation.normalized || form.path);

  const save = async (): Promise<void> => {
    if (!canSave) return;
    setBusy(true);
    setError(null);
    try {
      const payload: {
        path?: string;
        side: string;
        required: boolean;
        enabled: boolean;
        description: string;
      } = {
        side: form.side,
        required: form.required,
        enabled: form.enabled,
        description: form.description,
      };
      // Legacy-файлы с путями, которые новые правила уже не пропускают,
      // остаются редактируемыми: путь отправляется только при его изменении.
      if (pathChanged) {
        payload.path = validation.normalized;
      }
      await api.updateBuildFile(buildId, row.id, payload);
      onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside
      className={`build-drawer${closing ? ' build-drawer--closing' : ''}`}
      aria-label="Настройки файла"
      aria-hidden={closing || undefined}
      inert={closing || undefined}
    >
      <div className="build-drawer__head">
        <div className="build-drawer__title">
          <strong>{row.kind === 'dir' ? 'Свойства папки' : 'Настройки файла'}</strong>
          <span className="tiny muted mono break-all">{row.path}</span>
        </div>
        <Button size="sm" variant="ghost" iconOnly aria-label="Закрыть настройки" onClick={requestClose}>
          <X aria-hidden="true" />
        </Button>
      </div>
      <div className="build-drawer__body">
        {error ? <Alert>{error}</Alert> : null}
        {!canEdit ? (
          <Alert tone="info">Нет права minecraft.build.edit — доступен только просмотр.</Alert>
        ) : null}
        <Field
          label="Относительный путь"
          htmlFor="drawer-path"
          error={validation.errors[0]}
          hint={`Итог: ${validation.normalized || '—'}`}
        >
          <input
            id="drawer-path"
            className="input mono"
            value={form.path}
            autoFocus
            data-modal-autofocus
            onChange={(event) => setForm((current) => ({ ...current, path: event.target.value }))}
          />
        </Field>
        <div className="build-drawer__meta">
          <span>
            Категория: <strong>{CATEGORY_LABELS[category] ?? category}</strong>
          </span>
          <span>Определяется путём автоматически</span>
        </div>
        <Field label="Сторона установки" htmlFor="drawer-side">
          <select
            id="drawer-side"
            className="select"
            value={form.side}
            onChange={(event) => setForm((current) => ({ ...current, side: event.target.value }))}
          >
            <option value="client">Клиент</option>
            <option value="server">Сервер</option>
            <option value="both">Обе стороны</option>
          </select>
        </Field>
        <Field label="Описание" htmlFor="drawer-description">
          <input
            id="drawer-description"
            className="input"
            value={form.description}
            maxLength={300}
            onChange={(event) =>
              setForm((current) => ({ ...current, description: event.target.value }))
            }
          />
        </Field>
        <div className="row" style={{ gap: 16, flexWrap: 'wrap' }}>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={form.required}
              onChange={(event) =>
                setForm((current) => ({ ...current, required: event.target.checked }))
              }
            />
            Обязательный
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={form.enabled}
              onChange={(event) =>
                setForm((current) => ({ ...current, enabled: event.target.checked }))
              }
            />
            Включён в релиз
          </label>
        </div>
        <div className="info-list">
          <div className="info-list__row">
            <span>Размер</span>
            <span>{formatBytes(row.sizeBytes)}</span>
          </div>
          <div className="info-list__row">
            <span>SHA-256</span>
            <span className="mono tiny break-all">{row.sha256 || '—'}</span>
          </div>
          <div className="info-list__row">
            <span>Загружен</span>
            <span>{formatDate(row.createdAt)}</span>
          </div>
          <div className="info-list__row">
            <span>Обновлён</span>
            <span>{formatDate(row.updatedAt)}</span>
          </div>
        </div>
        {onPreview ? (
          <Button icon={<FileText aria-hidden="true" />} onClick={onPreview}>
            Просмотреть содержимое
          </Button>
        ) : null}
        <div className="row" style={{ justifyContent: 'flex-end', gap: 8 }}>
          <Button onClick={requestClose}>Закрыть</Button>
          {canEdit ? (
            <Button
              variant="primary"
              loading={busy}
              disabled={!canSave}
              onClick={() => void save()}
            >
              Сохранить
            </Button>
          ) : null}
        </div>
      </div>
    </aside>
  );
}

/* Внешняя ссылка как иконка без импорта конфликтующего имени. */
function ExternalLinkIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="15 3 21 3 21 9" />
      <line x1="10" y1="14" x2="21" y2="3" />
    </svg>
  );
}
