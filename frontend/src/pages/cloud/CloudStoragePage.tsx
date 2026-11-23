import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ArrowUpDown,
  Check,
  ChevronRight,
  Cloud,
  Copy,
  Download,
  FileArchive,
  FolderInput,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Home,
  LayoutGrid,
  Link2,
  List,
  Loader2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Search,
  Share2,
  Star,
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
  PageHead,
  SectionTitle,
  StatCard,
} from '../../components/ui';
import { NoticeStack } from '../../components/NoticeStack';
import {
  api,
  errorMessage,
  newIdempotencyKey,
  type CloudEntry,
  type CloudExportJob,
  type CloudShare,
  type ListCloudParams,
} from '../../services/api';
import { formatBytes, formatDate, pluralize, relativeTime } from '../../lib/format';
import { isImageFile } from '../../lib/preview';
import {
  FOLDER_UPLOAD_MAX_DEPTH,
  FOLDER_UPLOAD_MAX_FILES,
  validateFolderSelection,
} from '../../lib/cloudUpload';
import { realtime } from '../../lib/realtime';
import { applyDownloadEvent, dismissDownload, type DownloadItem } from '../../lib/realtimeEvents';
import { useTransfers } from '../../stores/transfers';
import { CloudPreviewModal } from '../../components/CloudPreview';
import { useCan } from '../../stores/auth';
import { toast } from '../../stores/ui';

import { FileGlyph } from './fileIcon';
import { SharesView } from './SharesView';
import { CloudThumb } from './thumbnail';
import {
  EXPIRY_OPTIONS,
  RECENT_DAYS,
  SORT_LABELS,
  TYPE_FILTERS,
  VIEW_LABELS,
  canPreview,
  formatTtlHours,
  sleep,
  trashDeadline,
  triggerDownload,
  type Layout,
  type View,
} from './helpers';

export function CloudStoragePage() {
  const queryClient = useQueryClient();
  const canUpload = useCan('cloud.file.upload');
  const canDownload = useCan('cloud.file.download');
  const canDelete = useCan('cloud.file.delete');
  const canShare = useCan('cloud.file.share');
  const canCreateFolder = useCan('cloud.folder.create');

  const [view, setView] = useState<View>('folder');
  const [parentId, setParentId] = useState<string | null>(null);
  const [searchInput, setSearchInput] = useState('');
  const [search, setSearch] = useState('');
  const [sort, setSort] = useState('name');
  const [direction, setDirection] = useState<'asc' | 'desc'>('asc');
  const [typeFilter, setTypeFilter] = useState('all');
  const [page, setPage] = useState(1);
  const [layout, setLayout] = useState<Layout>('grid');
  const [selection, setSelection] = useState<Set<string>>(new Set());

  const [downloads, setDownloads] = useState<DownloadItem[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [busyExport, setBusyExport] = useState<CloudExportJob | null>(null);

  const [folderModal, setFolderModal] = useState(false);
  const [folderName, setFolderName] = useState('');
  const [folderBusy, setFolderBusy] = useState(false);
  const [renameTarget, setRenameTarget] = useState<CloudEntry | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [moveTarget, setMoveTarget] = useState<CloudEntry | null>(null);
  const [moveParent, setMoveParent] = useState<string | null>(null);
  const [shareTarget, setShareTarget] = useState<CloudEntry | null>(null);
  const [previewTarget, setPreviewTarget] = useState<CloudEntry | null>(null);
  const [purgeTarget, setPurgeTarget] = useState<CloudEntry | null>(null);
  const [confirmEmptyTrash, setConfirmEmptyTrash] = useState(false);
  const [purgeBusy, setPurgeBusy] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
  const [shareForm, setShareForm] = useState({
    expiresInHours: 24 * 7,
    password: '',
    maxDownloads: '',
    authOnly: false,
  });
  const [shareBusy, setShareBusy] = useState(false);
  /** Один ключ на попытку создания ссылки: повтор после ошибки безопасен. */
  const shareKeyRef = useRef<string | null>(null);
  const [createdShare, setCreatedShare] = useState<{
    url: string;
    password: boolean;
    authOnly: boolean;
  } | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);
  const folderInputRef = useRef<HTMLInputElement>(null);

  const config = useQuery({ queryKey: ['cloud-config'], queryFn: () => api.cloudConfig() });
  const quota = useQuery({ queryKey: ['my-usage'], queryFn: () => api.myUsage() });
  const shareTtlHours = config.data?.shareTtlHours ?? 24 * 7;
  const trashRetentionDays = config.data?.trashRetentionDays ?? 30;
  const params: ListCloudParams = useMemo(
    () => ({
      view,
      page,
      perPage: 48,
      parentId: view === 'folder' ? parentId : undefined,
      q: search || undefined,
      sort,
      direction,
      category: typeFilter === 'all' ? undefined : typeFilter,
      sinceDays: view === 'recent' ? RECENT_DAYS : undefined,
    }),
    [view, page, parentId, search, sort, direction, typeFilter],
  );

  const list = useQuery({
    queryKey: ['cloud-list', params],
    queryFn: () => api.listCloud(params),
  });

  const shares = useQuery({
    queryKey: ['cloud-shares'],
    queryFn: () => api.listCloudShares(),
    enabled: canShare,
  });

  const exportJobs = useQuery({
    queryKey: ['cloud-exports'],
    queryFn: () => api.listCloudExports(),
    enabled: canDownload,
  });

  const sharesByEntry = useMemo(() => {
    const map = new Map<string, CloudShare[]>();
    for (const share of shares.data ?? []) {
      if (!share.active) continue;
      const bucket = map.get(share.entryId) ?? [];
      bucket.push(share);
      map.set(share.entryId, bucket);
    }
    return map;
  }, [shares.data]);

  const invalidate = useCallback((): void => {
    void queryClient.invalidateQueries({ queryKey: ['cloud-list'] });
    void queryClient.invalidateQueries({ queryKey: ['cloud-shares'] });
  }, [queryClient]);

  useEffect(() => {
    setSelection(new Set());
  }, [view, parentId, search, page, typeFilter]);

  // «Показать в папке» из плавающего виджета: открываем каталог, в который
  // сервер фактически сохранил файл, даже если пользователь ушёл со страницы.
  const revealRequest = useTransfers((state) => state.revealRequest);
  useEffect(() => {
    if (!revealRequest) return;
    setView('folder');
    setParentId(revealRequest.parentId);
    setSearch('');
    setSearchInput('');
    setPage(1);
    useTransfers.getState().clearReveal();
  }, [revealRequest]);

  // Realtime дополняет опрос: прогресс скачиваний приходит сразу, а при
  // недоступности WebSocket те же данные обновляет обычный polling.
  // Прогресс загрузок обрабатывает глобальный TransferDrawer.
  useEffect(() => {
    const unsubscribers = [
      realtime.on('download.progress', (event) => {
        setDownloads((current) => applyDownloadEvent(current, event));
      }),
      realtime.on('download.completed', (event) => {
        setDownloads((current) => applyDownloadEvent(current, event));
      }),
      realtime.on('download.failed', (event) => {
        setDownloads((current) => applyDownloadEvent(current, event));
      }),
    ];
    return () => {
      for (const unsubscribe of unsubscribers) unsubscribe();
    };
  }, [queryClient]);

  /* -------------------------------- операции -------------------------------- */

  const createFolder = async (): Promise<void> => {
    setFolderBusy(true);
    try {
      const entry = await api.createCloudFolder({ name: folderName.trim(), parentId });
      setFolderModal(false);
      setFolderName('');
      invalidate();
      toast(`Папка «${entry.name}» создана.`, 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setFolderBusy(false);
    }
  };

  const saveRename = async (): Promise<void> => {
    if (!renameTarget) return;
    try {
      await api.updateCloudEntry(renameTarget.id, { name: renameValue.trim() });
      setRenameTarget(null);
      invalidate();
      toast('Имя изменено.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const saveMove = async (): Promise<void> => {
    if (!moveTarget) return;
    try {
      await api.updateCloudEntry(moveTarget.id, { parentId: moveParent });
      setMoveTarget(null);
      invalidate();
      toast('Запись перемещена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const toggleStar = async (entry: CloudEntry): Promise<void> => {
    try {
      await api.updateCloudEntry(entry.id, { starred: !entry.starred });
      invalidate();
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const trashOne = async (id: string): Promise<void> => {
    try {
      await api.trashCloudEntry(id);
      invalidate();
      toast('Перемещено в корзину.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const trashSelected = async (): Promise<void> => {
    setTrashBusy(true);
    let failed = 0;
    for (const id of selection) {
      try {
        await api.trashCloudEntry(id);
      } catch {
        failed += 1;
      }
    }
    setTrashBusy(false);
    setSelection(new Set());
    invalidate();
    toast(
      failed === 0
        ? 'Выбранные записи перемещены в корзину.'
        : `Перемещено, но ${failed} записей не удалось обработать.`,
      failed === 0 ? 'success' : 'warning',
    );
  };

  const restoreOne = async (id: string): Promise<void> => {
    try {
      await api.restoreCloudEntry(id);
      invalidate();
      toast('Запись восстановлена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const purgeConfirmed = async (): Promise<void> => {
    if (!purgeTarget) return;
    setPurgeBusy(true);
    try {
      await api.purgeCloudEntry(purgeTarget.id);
      setPurgeTarget(null);
      invalidate();
      toast('Запись удалена безвозвратно.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPurgeBusy(false);
    }
  };

  const emptyTrashConfirmed = async (): Promise<void> => {
    setTrashBusy(true);
    try {
      const result = await api.emptyCloudTrash();
      setConfirmEmptyTrash(false);
      invalidate();
      toast(`Корзина очищена: удалено файлов — ${result.files}.`, 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setTrashBusy(false);
    }
  };

  const revoke = async (id: string): Promise<void> => {
    try {
      await api.revokeCloudShare(id);
      void queryClient.invalidateQueries({ queryKey: ['cloud-shares'] });
      toast('Ссылка отозвана.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  /* -------------------------------- загрузка -------------------------------- */

  /**
   * Ставит выбранные файлы и папки в глобальную очередь передач.
   *
   * Одна очередь на приложение: виджет доступен на любой странице, прогресс
   * не теряется при переходе между разделами, а файлы из разных корневых
   * папок группируются по корню выбора.
   */
  const enqueueSelection = useCallback(
    (files: FileList | File[]): void => {
      if (!canUpload) {
        toast('Недостаточно прав для загрузки файлов.', 'error');
        return;
      }
      if (view === 'trash') {
        toast('Загрузка недоступна в корзине.', 'error');
        return;
      }
      const incoming = Array.from(files);
      if (incoming.length === 0) return;
      const maxBytes = config.data?.maxUploadBytes ?? 0;

      // E5: проверяем весь выбор до постановки в очередь. При превышении
      // количества или глубины не загружается ни один файл.
      const checked = validateFolderSelection(incoming, { maxUploadBytes: maxBytes });
      if (!checked.ok) {
        toast(checked.message, 'error');
        return;
      }
      if (checked.skippedEmpty > 0) {
        toast(`Пропущено пустых файлов: ${checked.skippedEmpty}.`, 'warning');
      }
      if (checked.skippedOversized > 0) {
        toast(
          `Пропущено файлов больше ${formatBytes(maxBytes)}: ${checked.skippedOversized}.`,
          'warning',
        );
      }

      const items = checked.items.map((item) => ({
        file: item.file,
        segments: item.folderSegments,
        groupName: item.rootName,
      }));
      if (items.length === 0) return;
      useTransfers.getState().enqueue(items, {
        parentId: view === 'folder' ? parentId : null,
        concurrency: config.data?.uploadConcurrency ?? 3,
        chunkMaxBytes: config.data?.chunkMaxBytes ?? 8 * 1024 * 1024,
      });
    },
    [
      canUpload,
      config.data?.chunkMaxBytes,
      config.data?.maxUploadBytes,
      config.data?.uploadConcurrency,
      parentId,
      view,
    ],
  );

  // Файлы и папки проходят один и тот же путь: различает их только наличие
  // сегментов относительного пути (webkitRelativePath).
  const enqueueFiles = enqueueSelection;
  const enqueueFolder = enqueueSelection;

  /* --------------------------------- экспорт -------------------------------- */

  const runExport = useCallback(
    async (ids: string[]): Promise<void> => {
      if (ids.length === 0) return;
      try {
        const job = await api.createCloudExport(ids, newIdempotencyKey());
        setBusyExport(job);
        void queryClient.invalidateQueries({ queryKey: ['cloud-exports'] });
        for (let tick = 0; tick < 900; tick += 1) {
          const status = await api.cloudExportStatus(job.id);
          setBusyExport(status);
          if (status.status === 'completed') {
            triggerDownload(api.cloudExportUrl(job.id));
            void queryClient.invalidateQueries({ queryKey: ['cloud-exports'] });
            toast('Архив готов, скачивание началось.', 'success');
            setBusyExport(null);
            return;
          }
          if (status.status === 'failed') {
            void queryClient.invalidateQueries({ queryKey: ['cloud-exports'] });
            toast(status.error ?? 'Не удалось сформировать архив.', 'error');
            setBusyExport(null);
            return;
          }
          await sleep(1000);
        }
        toast('Формирование архива занимает больше времени, чем ожидалось.', 'warning');
        setBusyExport(null);
      } catch (error) {
        setBusyExport(null);
        toast(errorMessage(error), 'error');
      }
    },
    [queryClient],
  );

  /* -------------------------------- действия -------------------------------- */

  /**
   * Начинает скачивание и заводит запись в панели прогресса. Дальнейшие
   * байты приходят из `download.progress`; без WebSocket запись остаётся
   * нейтральной, а файл всё равно скачивается браузером.
   */
  const startDownload = useCallback((entry: CloudEntry): void => {
    setDownloads((current) =>
      applyDownloadEvent(current, {
        kind: 'download.progress',
        payload: {
          entryId: entry.id,
          name: entry.name,
          transferred: 0,
          size: entry.sizeBytes,
          percent: 0,
        },
      }),
    );
    triggerDownload(api.cloudDownloadUrl(entry.id));
  }, []);

  const openFolder = (entry: CloudEntry): void => {
    setView('folder');
    setParentId(entry.id);
    setPage(1);
  };

  /** E9: «Недавние» по умолчанию открываются по дате изменения, свежие сверху. */
  const switchView = (next: View): void => {
    setView(next);
    setParentId(null);
    setPage(1);
    if (next === 'recent') {
      setSort('updated');
      setDirection('desc');
    }
  };

  const toggleSelect = (id: string): void => {
    setSelection((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const openShare = (entry: CloudEntry): void => {
    setShareTarget(entry);
    setCreatedShare(null);
    shareKeyRef.current = null;
    setShareForm({
      expiresInHours: shareTtlHours,
      password: '',
      maxDownloads: '',
      authOnly: false,
    });
  };

  const items = list.data?.items ?? [];
  const usage = list.data?.usage;
  const breadcrumb = list.data?.breadcrumb ?? [];
  const perPage = list.data?.perPage ?? 48;
  const total = list.data?.total ?? 0;
  const maxPage = Math.max(1, Math.ceil(total / perPage));
  const allSelected = items.length > 0 && items.every((entry) => selection.has(entry.id));
  const activeShares = (shares.data ?? []).filter((share) => share.active);

  const folders = useQuery({
    queryKey: ['cloud-folders'],
    queryFn: () => api.listCloud({ view: 'all', entryType: 'folder', perPage: 100 }),
    enabled: moveTarget !== null,
  });

  const shareList = useMemo(
    () =>
      shareTarget ? (shares.data ?? []).filter((share) => share.entryId === shareTarget.id) : [],
    [shareTarget, shares.data],
  );

  // E6: срок по умолчанию берётся из limits.share_ttl_hours, а не из 7 дней.
  const expiryOptions = useMemo(() => {
    const options = [...EXPIRY_OPTIONS];
    if (shareTtlHours > 0 && !options.some((option) => option.value === shareTtlHours)) {
      options.unshift({
        value: shareTtlHours,
        label: `По умолчанию (${formatTtlHours(shareTtlHours)})`,
      });
    }
    return options;
  }, [shareTtlHours]);

  const archiveJobs = (exportJobs.data ?? []).filter(
    (job) => job.status === 'completed' || job.status === 'failed',
  );

  const deleteArchive = async (id: string): Promise<void> => {
    try {
      await api.deleteCloudExport(id);
      await queryClient.invalidateQueries({ queryKey: ['cloud-exports'] });
      toast('Архив удалён.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  return (
    <>
      <PageHead
        eyebrow="Пространство"
        title="Cloud Storage"
        description="Личное файловое хранилище на дисках сервера: папки, возобновляемые загрузки, предпросмотр, ссылки и архивы."
        actions={
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            {canCreateFolder ? (
              <Button
                icon={<FolderPlus aria-hidden="true" />}
                onClick={() => {
                  setFolderName('');
                  setFolderModal(true);
                }}
              >
                Папка
              </Button>
            ) : null}
            {canUpload ? (
              <>
                <Button
                  variant="ghost"
                  icon={<FolderPlus aria-hidden="true" />}
                  title={`До ${FOLDER_UPLOAD_MAX_FILES} файлов и ${FOLDER_UPLOAD_MAX_DEPTH} уровней вложенности`}
                  onClick={() => folderInputRef.current?.click()}
                >
                  Загрузить папку
                </Button>
                <Button
                  variant="primary"
                  icon={<UploadCloud aria-hidden="true" />}
                  onClick={() => fileInputRef.current?.click()}
                >
                  Загрузить
                </Button>
              </>
            ) : null}
            <input
              ref={fileInputRef}
              type="file"
              multiple
              data-testid="cloud-file-input"
              className="hidden"
              onChange={(event) => {
                if (event.target.files) enqueueFiles(event.target.files);
                event.target.value = '';
              }}
            />
            <input
              ref={folderInputRef}
              type="file"
              multiple
              data-testid="cloud-folder-input"
              className="hidden"
              // Загрузка папки с сохранением относительной структуры.
              {...({ webkitdirectory: '', directory: '' } as Record<string, string>)}
              onChange={(event) => {
                if (event.target.files) enqueueFolder(event.target.files);
                event.target.value = '';
              }}
            />
          </div>
        }
      />

      <div className="stats-grid">
        <StatCard
          label="Занято"
          icon={<HardDrive aria-hidden="true" />}
          value={formatBytes(usage?.usedBytes ?? 0)}
          sub={
            quota.data && quota.data.quotaBytes > 0
              ? `из ${formatBytes(quota.data.quotaBytes)} · ${Math.round(
                  ((usage?.usedBytes ?? 0) / quota.data.quotaBytes) * 100,
                )}%`
              : `${usage?.fileCount ?? 0} ${pluralize(usage?.fileCount ?? 0, 'файл', 'файла', 'файлов')}`
          }
          progress={
            quota.data && quota.data.quotaBytes > 0
              ? ((usage?.usedBytes ?? 0) / quota.data.quotaBytes) * 100
              : undefined
          }
          tone={
            quota.data &&
            quota.data.quotaBytes > 0 &&
            (usage?.usedBytes ?? 0) / quota.data.quotaBytes > 0.9
              ? 'warn'
              : 'default'
          }
        />
        <StatCard
          label="Файлов и папок"
          icon={<Cloud aria-hidden="true" />}
          value={total}
          sub={VIEW_LABELS[view]}
        />
        <StatCard
          label="Активные ссылки"
          icon={<Link2 aria-hidden="true" />}
          value={activeShares.length}
          sub="общий доступ"
        />
        <StatCard
          label="Лимит файла"
          icon={<ArrowUpDown aria-hidden="true" />}
          value={formatBytes(config.data?.maxUploadBytes ?? 0)}
          sub={`чанк ${formatBytes(config.data?.chunkMaxBytes ?? 0)}`}
        />
      </div>

      <div className="cloud-bar">
        <div className="cloud-bar__row">
          <div className="cloud-tabs" role="tablist" aria-label="Разделы хранилища">
            {(Object.keys(VIEW_LABELS) as View[]).map((key) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={view === key}
                className={`cloud-tab${view === key ? ' cloud-tab--active' : ''}`}
                onClick={() => {
                  switchView(key);
                }}
              >
                {VIEW_LABELS[key]}
              </button>
            ))}
          </div>
          {view === 'recent' ? (
            <Badge tone="info" plain>
              За {RECENT_DAYS} дней
            </Badge>
          ) : null}
          <div className="grow" />
          <div className="segmented" role="group" aria-label="Вид списка">
            <button
              type="button"
              aria-pressed={layout === 'grid'}
              onClick={() => setLayout('grid')}
              title="Сетка"
            >
              <LayoutGrid aria-hidden="true" />
            </button>
            <button
              type="button"
              aria-pressed={layout === 'list'}
              onClick={() => setLayout('list')}
              title="Список"
            >
              <List aria-hidden="true" />
            </button>
          </div>
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title="Обновить"
            aria-label="Обновить"
            onClick={() => void list.refetch()}
          >
            <RefreshCw aria-hidden="true" />
          </Button>
        </div>

        <div className="cloud-bar__row">
          <form
            className="input-wrap cloud-search"
            onSubmit={(event) => {
              event.preventDefault();
              setSearch(searchInput.trim());
              setPage(1);
            }}
          >
            <Search aria-hidden="true" />
            <input
              className="input input--with-icon"
              type="search"
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
              placeholder="Поиск по имени…"
              aria-label="Поиск файлов"
            />
          </form>
          <select
            className="select cloud-sort"
            value={typeFilter}
            aria-label="Фильтр по типу"
            onChange={(event) => {
              setTypeFilter(event.target.value);
              setPage(1);
            }}
          >
            {TYPE_FILTERS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
          <select
            className="select cloud-sort"
            value={sort}
            aria-label="Сортировка"
            onChange={(event) => {
              setSort(event.target.value);
              setPage(1);
            }}
          >
            {SORT_LABELS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title={direction === 'asc' ? 'По возрастанию' : 'По убыванию'}
            aria-label="Направление сортировки"
            onClick={() => setDirection((current) => (current === 'asc' ? 'desc' : 'asc'))}
          >
            <ArrowUpDown aria-hidden="true" />
          </Button>
          {search ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setSearch('');
                setSearchInput('');
                setPage(1);
              }}
            >
              Сбросить
            </Button>
          ) : null}
        </div>
      </div>

      {view === 'folder' ? (
        <nav className="cloud-crumbs" aria-label="Навигация по папкам">
          <button
            type="button"
            className="cloud-crumb"
            onClick={() => {
              setParentId(null);
              setPage(1);
            }}
          >
            <Home aria-hidden="true" />
            Корень
          </button>
          {breadcrumb.map((crumb, index) => (
            <span className="cloud-crumb-group" key={crumb.id}>
              <ChevronRight aria-hidden="true" />
              <button
                type="button"
                className={`cloud-crumb${index === breadcrumb.length - 1 ? ' cloud-crumb--last' : ''}`}
                onClick={() => {
                  setParentId(crumb.id);
                  setPage(1);
                }}
              >
                {crumb.name}
              </button>
            </span>
          ))}
        </nav>
      ) : null}

      {selection.size > 0 ? (
        <div className="cloud-selection" role="toolbar" aria-label="Действия с выбранными файлами">
          <span>
            Выбрано: <strong>{selection.size}</strong>
          </span>
          <div className="grow" />
          {canDownload ? (
            <Button
              size="sm"
              icon={<FileArchive aria-hidden="true" />}
              loading={busyExport !== null}
              onClick={() => void runExport([...selection])}
            >
              Скачать ZIP
            </Button>
          ) : null}
          {canDelete && view !== 'trash' ? (
            <Button
              size="sm"
              variant="ghost"
              icon={<Trash2 aria-hidden="true" />}
              loading={trashBusy}
              onClick={() => void trashSelected()}
            >
              В корзину
            </Button>
          ) : null}
          <Button size="sm" variant="ghost" onClick={() => setSelection(new Set())}>
            Снять
          </Button>
        </div>
      ) : null}

      {busyExport || archiveJobs.length > 0 || downloads.length > 0 ? (
        <NoticeStack label="Скачивания и архивы">
        {busyExport ? (
          <div className="notice-card cloud-export">
            <Loader2 className="spin" aria-hidden="true" />
            <div className="grow">
              <div className="row row--between" style={{ fontSize: 12 }}>
                <span>Формирование архива</span>
                <span className="mono">{Math.round(busyExport.progress)}%</span>
              </div>
              <div className="meter" aria-hidden="true">
                <div
                  className="meter__fill"
                  style={{ width: `${Math.min(100, busyExport.progress)}%` }}
                />
              </div>
            </div>
          </div>
        ) : null}

        {archiveJobs.length > 0 ? (
          <div className="notice-card">
            <SectionTitle
              title="Архивы"
              meta={`${archiveJobs.length} ${pluralize(archiveJobs.length, 'архив', 'архива', 'архивов')} · хранятся ${formatTtlHours(config.data?.exportTtlHours ?? 6)}`}
            />
            <div className="stack" style={{ gap: 8 }}>
              {archiveJobs.map((job) => (
                <div className="row row--between" key={job.id} style={{ gap: 12 }}>
                  <div className="grow" style={{ minWidth: 0 }}>
                    <div className="truncate">
                      Архив от {formatDate(job.finishedAt ?? job.createdAt)}
                    </div>
                    <div className="tiny muted">
                      {job.status === 'completed'
                        ? `готов · ${formatDate(job.createdAt)}`
                        : `ошибка: ${job.error ?? 'не удалось сформировать'}`}
                    </div>
                  </div>
                  {job.status === 'completed' ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<Download aria-hidden="true" />}
                      onClick={() => triggerDownload(api.cloudExportUrl(job.id))}
                    >
                      Скачать
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<Trash2 aria-hidden="true" />}
                    onClick={() => void deleteArchive(job.id)}
                  >
                    Удалить архив
                  </Button>
                </div>
              ))}
            </div>
          </div>
        ) : null}

        {downloads.length > 0 ? (
          <div className="notice-card">
            <div className="row row--between" style={{ marginBottom: 10 }}>
              <SectionTitle
                title="Скачивание"
                meta={`${downloads.length} ${pluralize(downloads.length, 'файл', 'файла', 'файлов')}`}
              />
              <Button
                size="sm"
                variant="ghost"
                onClick={() =>
                  setDownloads((current) => current.filter((item) => item.state === 'active'))
                }
              >
                Очистить завершённые
              </Button>
            </div>
            <div className="stack" style={{ gap: 10 }}>
              {downloads.map((item) => {
                const percent = Math.min(100, Math.max(0, item.percent));
                return (
                  <div className="upload-item" key={item.entryId}>
                    <div className="row row--between" style={{ gap: 12 }}>
                      <span className="truncate" title={item.name}>
                        {item.name}
                      </span>
                      <span className="mono tiny">
                        {formatBytes(item.transferred)} / {formatBytes(item.size)}
                      </span>
                    </div>
                    <div className="meter" aria-hidden="true">
                      <div
                        className={`meter__fill${item.state === 'failed' ? ' meter__fill--warn' : ''}`}
                        style={{ width: `${percent}%` }}
                      />
                    </div>
                    <div className="row row--between" style={{ gap: 10 }}>
                      <span className="tiny muted">
                        {item.state === 'done'
                          ? 'Скачано'
                          : item.state === 'failed'
                            ? (item.message ?? 'Скачивание прервано.')
                            : `Скачивание ${Math.round(percent)}%`}
                      </span>
                      {item.state === 'active' ? null : (
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          aria-label="Убрать из списка"
                          onClick={() =>
                            setDownloads((current) => dismissDownload(current, item.entryId))
                          }
                        >
                          <X aria-hidden="true" />
                        </Button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        ) : null}
        </NoticeStack>
      ) : null}

      <div
        className={`cloud-drop${dragActive ? ' cloud-drop--active' : ''}`}
        onDragOver={(event) => {
          if (!canUpload || view === 'trash') return;
          event.preventDefault();
          setDragActive(true);
        }}
        onDragLeave={() => setDragActive(false)}
        onDrop={(event) => {
          event.preventDefault();
          setDragActive(false);
          if (canUpload && event.dataTransfer.files) enqueueFiles(event.dataTransfer.files);
        }}
      >
        <div className="tab-panel" key={`${view}-${layout}`}>
          {view === 'shares' ? (
            <SharesView
            shares={shares.data ?? []}
            loading={shares.isLoading}
            error={shares.isError ? errorMessage(shares.error) : null}
            canShare={canShare}
            onRevoke={(id) => revoke(id)}
          />
        ) : list.isLoading ? (
          <div className="card">
            <LoadingInline label="Загрузка файлов" />
          </div>
        ) : list.isError ? (
          <Alert tone="danger">Не удалось загрузить список: {errorMessage(list.error)}</Alert>
        ) : items.length === 0 ? (
          <div className="card">
            <EmptyState
              icon={<FolderOpen aria-hidden="true" />}
              title={
                search
                  ? 'Ничего не найдено'
                  : view === 'trash'
                    ? 'Корзина пуста'
                    : view === 'starred'
                      ? 'Нет избранных файлов'
                      : 'Здесь пока пусто'
              }
              action={
                canUpload && view !== 'trash' ? (
                  <Button
                    variant="primary"
                    icon={<UploadCloud aria-hidden="true" />}
                    onClick={() => fileInputRef.current?.click()}
                  >
                    Загрузить файлы
                  </Button>
                ) : undefined
              }
            >
              {search
                ? 'Измените запрос или сбросьте поиск.'
                : view === 'trash'
                  ? 'Удалённые записи будут появляться здесь до окончательной очистки.'
                  : 'Перетащите файлы в эту область или выберите их через кнопку загрузки.'}
            </EmptyState>
          </div>
        ) : layout === 'grid' ? (
          <div className="cloud-grid">
            {items.map((entry) => {
              const selected = selection.has(entry.id);
              const entryShares = sharesByEntry.get(entry.id) ?? [];
              const showThumb = entry.entryType === 'file' && isImageFile(entry.name);
              return (
                <article
                  key={entry.id}
                  className={`cloud-card${selected ? ' cloud-card--selected' : ''}`}
                  data-testid="cloud-card"
                  data-entry-id={entry.id}
                  data-entry-name={entry.name}
                  data-entry-type={entry.entryType}
                >
                  <div className="cloud-card__top">
                    <button
                      type="button"
                      className={`cloud-check${selected ? ' cloud-check--on' : ''}`}
                      onClick={() => toggleSelect(entry.id)}
                      aria-pressed={selected}
                      aria-label={selected ? 'Снять выбор' : 'Выбрать'}
                    >
                      {selected ? <Check aria-hidden="true" /> : null}
                    </button>
                    {entry.starred ? (
                      <Star aria-hidden="true" className="cloud-card__star" />
                    ) : null}
                    <div className="grow" />
                    {entryShares.length > 0 ? (
                      <Badge tone="info" plain>
                        <Link2 aria-hidden="true" style={{ width: 11, height: 11 }} />
                        {entryShares.length}
                      </Badge>
                    ) : null}
                  </div>

                  <button
                    type="button"
                    className={`cloud-card__icon${showThumb ? ' cloud-card__icon--thumb' : ''}`}
                    onClick={() => {
                      if (entry.entryType === 'folder') openFolder(entry);
                      else if (canPreview(entry)) setPreviewTarget(entry);
                      else if (canDownload) startDownload(entry);
                    }}
                    title={entry.entryType === 'folder' ? 'Открыть папку' : 'Предпросмотр'}
                  >
                    {showThumb ? <CloudThumb entry={entry} /> : <FileGlyph entry={entry} />}
                  </button>

                  <button
                    type="button"
                    className="cloud-card__name"
                    onClick={() => {
                      if (entry.entryType === 'folder') openFolder(entry);
                      else if (canPreview(entry)) setPreviewTarget(entry);
                    }}
                    title={entry.name}
                  >
                    {entry.name}
                  </button>

                  <div className="cloud-card__meta">
                    {entry.entryType === 'folder' ? 'Папка' : formatBytes(entry.sizeBytes)}
                    <span> · {relativeTime(entry.updatedAt)}</span>
                  </div>
                  {view === 'trash' && entry.trashedAt ? (
                    <div className="tiny muted">
                      удалится{' '}
                      {trashDeadline(entry.trashedAt, trashRetentionDays)
                        ? formatDate(trashDeadline(entry.trashedAt, trashRetentionDays) as string)
                        : '—'}
                    </div>
                  ) : null}

                  <div className="cloud-card__actions">
                    {entry.entryType === 'file' && canDownload ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        iconOnly
                        title="Скачать"
                        aria-label="Скачать"
                        onClick={() => startDownload(entry)}
                      >
                        <Download aria-hidden="true" />
                      </Button>
                    ) : null}
                    {entry.entryType === 'file' && canShare && view !== 'trash' ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        iconOnly
                        title="Ссылка"
                        aria-label="Создать ссылку"
                        onClick={() => openShare(entry)}
                      >
                        <Share2 aria-hidden="true" />
                      </Button>
                    ) : null}
                    {view === 'trash' ? (
                      <>
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          title="Восстановить"
                          aria-label="Восстановить"
                          onClick={() => void restoreOne(entry.id)}
                        >
                          <RotateCcw aria-hidden="true" />
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          title="Удалить навсегда"
                          aria-label="Удалить навсегда"
                          onClick={() => setPurgeTarget(entry)}
                        >
                          <Trash2 aria-hidden="true" />
                        </Button>
                      </>
                    ) : (
                      <>
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          title={entry.starred ? 'Убрать из избранного' : 'В избранное'}
                          aria-label="Избранное"
                          onClick={() => void toggleStar(entry)}
                        >
                          <Star aria-hidden="true" fill={entry.starred ? 'currentColor' : 'none'} />
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          title="Переименовать"
                          aria-label="Переименовать"
                          onClick={() => {
                            setRenameTarget(entry);
                            setRenameValue(entry.name);
                          }}
                        >
                          <Pencil aria-hidden="true" />
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          iconOnly
                          title="Переместить"
                          aria-label="Переместить"
                          onClick={() => {
                            setMoveTarget(entry);
                            setMoveParent(entry.parentId);
                          }}
                        >
                          <FolderInput aria-hidden="true" />
                        </Button>
                        {canDelete ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            title="В корзину"
                            aria-label="В корзину"
                            onClick={() => void trashOne(entry.id)}
                          >
                            <Trash2 aria-hidden="true" />
                          </Button>
                        ) : null}
                      </>
                    )}
                  </div>
                </article>
              );
            })}
          </div>
        ) : (
          <div className="table-card">
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th style={{ width: 34 }}>
                      <button
                        type="button"
                        className={`cloud-check${allSelected ? ' cloud-check--on' : ''}`}
                        aria-label="Выбрать все"
                        onClick={() =>
                          setSelection(
                            allSelected ? new Set() : new Set(items.map((item) => item.id)),
                          )
                        }
                      >
                        {allSelected ? <Check aria-hidden="true" /> : null}
                      </button>
                    </th>
                    <th>Имя</th>
                    <th>Тип</th>
                    <th>Размер</th>
                    <th>{view === 'trash' ? 'Удалён' : 'Изменён'}</th>
                    <th style={{ textAlign: 'right' }}>Действия</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((entry) => (
                    <tr key={entry.id}>
                      <td>
                        <button
                          type="button"
                          className={`cloud-check${selection.has(entry.id) ? ' cloud-check--on' : ''}`}
                          aria-label="Выбрать"
                          onClick={() => toggleSelect(entry.id)}
                        >
                          {selection.has(entry.id) ? <Check aria-hidden="true" /> : null}
                        </button>
                      </td>
                      <td>
                        <button
                          type="button"
                          className="cloud-row-name"
                          onClick={() => {
                            if (entry.entryType === 'folder') openFolder(entry);
                            else if (canPreview(entry)) setPreviewTarget(entry);
                          }}
                        >
                          <FileGlyph entry={entry} />
                          <span className="truncate">{entry.name}</span>
                          {entry.starred ? (
                            <Star aria-hidden="true" className="cloud-card__star" />
                          ) : null}
                        </button>
                      </td>
                      <td className="muted tiny">
                        {entry.entryType === 'folder' ? 'папка' : 'файл'}
                      </td>
                      <td className="mono tiny">
                        {entry.entryType === 'folder' ? '—' : formatBytes(entry.sizeBytes)}
                      </td>
                      <td className="tiny muted">
                        {view === 'trash' && entry.trashedAt
                          ? `удалён ${formatDate(entry.trashedAt)}${
                              trashDeadline(entry.trashedAt, trashRetentionDays)
                                ? ` · удалится ${formatDate(
                                    trashDeadline(entry.trashedAt, trashRetentionDays) as string,
                                  )}`
                                : ''
                            }`
                          : formatDate(entry.updatedAt)}
                      </td>
                      <td>
                        <div className="table__actions">
                          {entry.entryType === 'file' && canDownload ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Скачать"
                              onClick={() => startDownload(entry)}
                            >
                              <Download aria-hidden="true" />
                            </Button>
                          ) : null}
                          {entry.entryType === 'file' && canShare && view !== 'trash' ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Ссылка"
                              onClick={() => openShare(entry)}
                            >
                              <Share2 aria-hidden="true" />
                            </Button>
                          ) : null}
                          {view === 'trash' ? (
                            <>
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="Восстановить"
                                onClick={() => void restoreOne(entry.id)}
                              >
                                <RotateCcw aria-hidden="true" />
                              </Button>
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="Удалить навсегда"
                                onClick={() => setPurgeTarget(entry)}
                              >
                                <Trash2 aria-hidden="true" />
                              </Button>
                            </>
                          ) : (
                            <>
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="Переименовать"
                                onClick={() => {
                                  setRenameTarget(entry);
                                  setRenameValue(entry.name);
                                }}
                              >
                                <Pencil aria-hidden="true" />
                              </Button>
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="В корзину"
                                onClick={() => void trashOne(entry.id)}
                              >
                                <Trash2 aria-hidden="true" />
                              </Button>
                            </>
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
          )}
        </div>
      </div>

      {total > perPage ? (
        <div className="pagination">
          <span>
            Всего: {total} · стр. {list.data?.page ?? 1} из {maxPage}
          </span>
          <div className="pagination__controls">
            <Button
              size="sm"
              disabled={page <= 1}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              Назад
            </Button>
            <Button
              size="sm"
              disabled={page >= maxPage}
              onClick={() => setPage((value) => Math.min(maxPage, value + 1))}
            >
              Вперёд
            </Button>
          </div>
        </div>
      ) : null}

      {view === 'trash' && items.length > 0 && canDelete ? (
        <div className="row" style={{ justifyContent: 'flex-end', marginTop: 12 }}>
          <Button
            variant="danger"
            icon={<Trash2 aria-hidden="true" />}
            onClick={() => setConfirmEmptyTrash(true)}
          >
            Очистить корзину
          </Button>
        </div>
      ) : null}

      <Modal
        open={folderModal}
        onClose={() => setFolderModal(false)}
        title="Новая папка"
        description={
          parentId
            ? 'Папка будет создана внутри текущего каталога.'
            : 'Папка будет создана в корне хранилища.'
        }
        footer={
          <>
            <Button onClick={() => setFolderModal(false)}>Отмена</Button>
            <Button
              variant="primary"
              loading={folderBusy}
              disabled={folderName.trim().length === 0}
              onClick={() => void createFolder()}
            >
              Создать
            </Button>
          </>
        }
      >
        <Field label="Название" htmlFor="cloud-folder-name">
          <input
            id="cloud-folder-name"
            className="input"
            value={folderName}
            maxLength={200}
            onChange={(event) => setFolderName(event.target.value)}
            placeholder="Например, Документы"
          />
        </Field>
      </Modal>

      <Modal
        open={renameTarget !== null}
        onClose={() => setRenameTarget(null)}
        title="Переименовать"
        footer={
          <>
            <Button onClick={() => setRenameTarget(null)}>Отмена</Button>
            <Button
              variant="primary"
              disabled={renameValue.trim().length === 0}
              onClick={() => void saveRename()}
            >
              Сохранить
            </Button>
          </>
        }
      >
        <Field label="Новое имя" htmlFor="cloud-rename">
          <input
            id="cloud-rename"
            className="input"
            value={renameValue}
            maxLength={200}
            onChange={(event) => setRenameValue(event.target.value)}
          />
        </Field>
      </Modal>

      <Modal
        open={moveTarget !== null}
        onClose={() => setMoveTarget(null)}
        title="Переместить"
        description={moveTarget ? `Запись «${moveTarget.name}»` : undefined}
        footer={
          <>
            <Button onClick={() => setMoveTarget(null)}>Отмена</Button>
            <Button variant="primary" onClick={() => void saveMove()}>
              Переместить
            </Button>
          </>
        }
      >
        {folders.isLoading ? (
          <LoadingInline label="Загрузка папок" />
        ) : folders.isError ? (
          <Alert tone="danger">{errorMessage(folders.error)}</Alert>
        ) : (
          <Field label="Целевая папка" hint="Корень хранилища доступен всегда.">
            <select
              className="select"
              value={moveParent ?? ''}
              onChange={(event) => setMoveParent(event.target.value || null)}
            >
              <option value="">Корень хранилища</option>
              {(folders.data?.items ?? [])
                .filter((folder) => folder.id !== moveTarget?.id)
                .map((folder) => (
                  <option key={folder.id} value={folder.id}>
                    {folder.name}
                  </option>
                ))}
            </select>
          </Field>
        )}
      </Modal>

      <Modal
        open={shareTarget !== null}
        onClose={() => {
          setShareTarget(null);
          setCreatedShare(null);
        }}
        title="Ссылка общего доступа"
        description={shareTarget?.name}
        footer={
          createdShare ? (
            <Button onClick={() => setShareTarget(null)}>Готово</Button>
          ) : (
            <>
              <Button onClick={() => setShareTarget(null)}>Отмена</Button>
              <Button
                variant="primary"
                loading={shareBusy}
                onClick={async () => {
                  if (!shareTarget) return;
                  setShareBusy(true);
                  const idempotencyKey = shareKeyRef.current ?? newIdempotencyKey();
                  shareKeyRef.current = idempotencyKey;
                  try {
                    const result = await api.createCloudShare(
                      {
                        entryId: shareTarget.id,
                        expiresInHours: shareForm.expiresInHours,
                        password: shareForm.password.trim() || undefined,
                        maxDownloads: shareForm.maxDownloads
                          ? Number(shareForm.maxDownloads)
                          : null,
                        authOnly: shareForm.authOnly,
                      },
                      idempotencyKey,
                    );
                    setCreatedShare({
                      url: result.url,
                      password: Boolean(result.share.hasPassword),
                      authOnly: result.share.authOnly,
                    });
                    void queryClient.invalidateQueries({ queryKey: ['cloud-shares'] });
                    toast('Ссылка создана.', 'success');
                  } catch (error) {
                    toast(errorMessage(error), 'error');
                  } finally {
                    setShareBusy(false);
                  }
                }}
              >
                Создать ссылку
              </Button>
            </>
          )
        }
      >
        {createdShare ? (
          <div className="stack">
            <Field label="Ссылка">
              <div className="row" style={{ gap: 8 }}>
                <input className="input mono" readOnly value={createdShare.url} />
                <Button
                  icon={<Copy aria-hidden="true" />}
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(createdShare.url);
                      toast('Ссылка скопирована.', 'success');
                    } catch {
                      toast('Скопируйте ссылку вручную.', 'warning');
                    }
                  }}
                >
                  Копировать
                </Button>
              </div>
            </Field>
            {createdShare.password ? (
              <Alert tone="info">Ссылка защищена паролем, который вы указали.</Alert>
            ) : null}
            {createdShare.authOnly ? (
              <Alert tone="info">
                Ссылка доступна только авторизованным пользователям NEXUS: гость увидит форму входа.
              </Alert>
            ) : null}
          </div>
        ) : (
          <div className="stack">
            <Field
              label="Срок действия"
              htmlFor="share-expiry"
              hint={`По умолчанию — ${formatTtlHours(shareTtlHours)} (limits.share_ttl_hours).`}
            >
              <select
                id="share-expiry"
                className="select"
                value={shareForm.expiresInHours}
                onChange={(event) =>
                  setShareForm((form) => ({ ...form, expiresInHours: Number(event.target.value) }))
                }
              >
                {expiryOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              label="Пароль (необязательно)"
              htmlFor="share-password"
              hint="Не менее 4 символов."
            >
              <input
                id="share-password"
                className="input"
                type="text"
                value={shareForm.password}
                onChange={(event) =>
                  setShareForm((form) => ({ ...form, password: event.target.value }))
                }
                autoComplete="new-password"
              />
            </Field>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={shareForm.authOnly}
                onChange={(event) =>
                  setShareForm((form) => ({ ...form, authOnly: event.target.checked }))
                }
              />
              <span>Только для авторизованных</span>
            </label>
            <Field label="Лимит скачиваний (необязательно)" htmlFor="share-limit">
              <input
                id="share-limit"
                className="input"
                type="number"
                min={1}
                value={shareForm.maxDownloads}
                onChange={(event) =>
                  setShareForm((form) => ({ ...form, maxDownloads: event.target.value }))
                }
                placeholder="Без ограничений"
              />
            </Field>

            {shareList.length > 0 ? (
              <div className="stack" style={{ gap: 8 }}>
                <span className="field__label">Ссылки этого файла</span>
                {shareList.map((share) => (
                  <div className="cloud-share-row" key={share.id}>
                    <div className="grow">
                      <div className="tiny">
                        {share.downloadCount}
                        {share.maxDownloads ? ` / ${share.maxDownloads}` : ''} скачиваний
                        {share.hasPassword ? ' · пароль' : ''}
                        {share.authOnly ? ' · только авторизованные' : ''}
                      </div>
                      <div className="tiny muted">
                        {share.active
                          ? share.expiresAt
                            ? `до ${formatDate(share.expiresAt)}`
                            : 'бессрочно'
                          : 'неактивна'}
                      </div>
                    </div>
                    {share.active ? (
                      <Button size="sm" variant="ghost" onClick={() => void revoke(share.id)}>
                        Отозвать
                      </Button>
                    ) : null}
                  </div>
                ))}
              </div>
            ) : null}
          </div>
        )}
      </Modal>

      {previewTarget ? (
        <CloudPreviewModal entry={previewTarget} onClose={() => setPreviewTarget(null)} />
      ) : null}

      <ConfirmDialog
        open={purgeTarget !== null}
        title="Удалить навсегда?"
        description={
          purgeTarget
            ? `Запись «${purgeTarget.name}» и её содержимое будут удалены без возможности восстановления.`
            : ''
        }
        confirmLabel="Удалить"
        busy={purgeBusy}
        onConfirm={() => void purgeConfirmed()}
        onCancel={() => setPurgeTarget(null)}
      />

      <ConfirmDialog
        open={confirmEmptyTrash}
        title="Очистить корзину?"
        description="Все записи из корзины будут удалены безвозвратно."
        confirmLabel="Очистить"
        busy={trashBusy}
        onConfirm={() => void emptyTrashConfirmed()}
        onCancel={() => setConfirmEmptyTrash(false)}
      />
    </>
  );
}

export default CloudStoragePage;
