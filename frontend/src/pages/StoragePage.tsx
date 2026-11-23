import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ArrowUp,
  Database,
  FolderPlus,
  HardDrive,
  RefreshCw,
  Server,
  ShieldAlert,
  Trash2,
  TriangleAlert,
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
  Switch,
} from '../components/ui';
import { api, errorMessage, type BrowseResponse, type StorageVolume } from '../services/api';
import { formatBytes, relativeTime } from '../lib/format';
import { toast } from '../stores/ui';

const GIB = 1024 ** 3;

function stateBadge(volume: StorageVolume) {
  if (!volume.enabled) return <Badge plain>Отключено</Badge>;
  if (volume.state === 'online') return <Badge tone="ok">Онлайн</Badge>;
  if (volume.state === 'offline') return <Badge tone="warn">Недоступно</Badge>;
  return <Badge tone="danger">Ошибка</Badge>;
}

function scopeBadge(volume: StorageVolume) {
  if (volume.scope === 'cloud')
    return (
      <Badge tone="info" plain>
        Cloud
      </Badge>
    );
  if (volume.scope === 'minecraft')
    return (
      <Badge tone="ok" plain>
        Minecraft
      </Badge>
    );
  if (volume.scope === 'system') return <Badge plain>Система</Badge>;
  return <Badge plain>{volume.scope}</Badge>;
}

interface EditorState {
  mode: 'create' | 'edit';
  volume?: StorageVolume;
}

export function StoragePage() {
  const queryClient = useQueryClient();
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<StorageVolume | null>(null);
  const [confirmDisable, setConfirmDisable] = useState<StorageVolume | null>(null);

  const disks = useQuery({ queryKey: ['storage-disks'], queryFn: () => api.listStorageDisks() });
  const volumes = useQuery({
    queryKey: ['storage-volumes'],
    queryFn: () => api.listStorageVolumes(),
  });
  const summary = useQuery({
    queryKey: ['storage-summary'],
    queryFn: () => api.storageSummary(),
  });

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: ['storage-disks'] });
    void queryClient.invalidateQueries({ queryKey: ['storage-volumes'] });
    void queryClient.invalidateQueries({ queryKey: ['storage-summary'] });
  };

  const checkVolume = useMutation({
    mutationFn: (id: string) => api.checkStorageVolume(id),
    onSuccess: (outcome) => {
      invalidate();
      toast(outcome.report.message, outcome.row.state === 'online' ? 'success' : 'error');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const toggleVolume = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.updateStorageVolume(id, { enabled }),
    onSuccess: (volume) => {
      invalidate();
      toast(volume.enabled ? 'Хранилище включено.' : 'Хранилище отключено.', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const deleteVolume = useMutation({
    mutationFn: (id: string) => api.deleteStorageVolume(id),
    onSuccess: () => {
      invalidate();
      setConfirmDelete(null);
      toast('Хранилище удалено.', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const diskList = disks.data ?? [];
  const volumeList = useMemo(() => volumes.data ?? [], [volumes.data]);
  const stats = summary.data;

  const volumesByDisk = useMemo(() => {
    const map = new Map<string, StorageVolume[]>();
    for (const volume of volumeList) {
      const list = map.get(volume.diskMount) ?? [];
      list.push(volume);
      map.set(volume.diskMount, list);
    }
    return map;
  }, [volumeList]);

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Хранилища и диски"
        description="Физические диски сервера и зарегистрированные каталоги хранения. Файлы размещаются на выбранном диске, имена и структура папок хранятся в базе данных."
        actions={
          <Button
            variant="primary"
            icon={<FolderPlus aria-hidden="true" />}
            onClick={() => setEditor({ mode: 'create' })}
          >
            Добавить каталог
          </Button>
        }
      />

      {stats ? (
        <div className="stats-grid">
          <StatCard
            label="Хранилища"
            icon={<Server aria-hidden="true" />}
            value={stats.online}
            unit={`из ${stats.volumes}`}
            sub="доступно онлайн"
          />
          <StatCard
            label="Объектов"
            icon={<Database aria-hidden="true" />}
            value={stats.objects}
            sub={formatBytes(stats.objectsBytes)}
          />
          <StatCard
            label="Общий объём"
            icon={<HardDrive aria-hidden="true" />}
            value={formatBytes(stats.totalBytes)}
            sub="по зарегистрированным дискам"
          />
          <StatCard
            label="Свободно"
            value={formatBytes(stats.availableBytes)}
            sub={
              stats.reservedBytes > 0 ? `резерв ${formatBytes(stats.reservedBytes)}` : 'без резерва'
            }
            progress={
              stats.totalBytes > 0
                ? ((stats.totalBytes - stats.availableBytes) / stats.totalBytes) * 100
                : 0
            }
          />
        </div>
      ) : null}

      <SectionTitle
        title="Зарегистрированные хранилища"
        meta={volumes.isFetching ? 'обновление…' : `${volumeList.length} шт.`}
      />

      {volumes.isLoading ? (
        <div className="card">
          <LoadingInline label="Загрузка хранилищ" />
        </div>
      ) : volumes.isError ? (
        <Alert tone="danger">Не удалось загрузить хранилища: {errorMessage(volumes.error)}</Alert>
      ) : volumeList.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<FolderPlus aria-hidden="true" />}
            title="Хранилища не зарегистрированы"
            action={
              <Button
                icon={<FolderPlus aria-hidden="true" />}
                onClick={() => setEditor({ mode: 'create' })}
              >
                Добавить каталог
              </Button>
            }
          >
            Выберите каталог на одном из дисков сервера. Файлы будут храниться внутри него в
            служебной структуре <span className="mono">.nexus</span>.
          </EmptyState>
        </div>
      ) : (
        <div className="stack">
          {volumeList.map((volume) => (
            <article className="card volume-card" key={volume.id}>
              <div className="volume-card__head">
                <div>
                  <div className="row" style={{ gap: 8, alignItems: 'center' }}>
                    <span className="volume-card__title">{volume.name}</span>
                    {stateBadge(volume)}
                    {scopeBadge(volume)}
                    <Badge plain>{volume.mode === 'manual' ? 'ручной' : 'авто'}</Badge>
                  </div>
                  <div className="volume-card__path">{volume.rootPath}</div>
                </div>
                <Switch
                  checked={volume.enabled}
                  disabled={toggleVolume.isPending}
                  label={volume.enabled ? 'Включено' : 'Отключено'}
                  onChange={(next) => {
                    if (!next) {
                      setConfirmDisable(volume);
                      return;
                    }
                    toggleVolume.mutate({ id: volume.id, enabled: next });
                  }}
                />
              </div>

              <div className="volume-card__meta">
                <span>
                  Диск: <span className="mono">{volume.diskMount || '—'}</span>
                </span>
                <span>
                  Свободно: <strong>{formatBytes(volume.availableBytes)}</strong> из{' '}
                  {formatBytes(volume.totalBytes)}
                </span>
                <span>
                  Объектов: <strong>{volume.objects}</strong> · {formatBytes(volume.objectsBytes)}
                </span>
                <span>Приоритет: {volume.priority}</span>
                <span>
                  Область:{' '}
                  <strong>
                    {volume.scope === 'system'
                      ? 'система (оба модуля)'
                      : volume.scope === 'cloud'
                        ? 'Cloud'
                        : 'Minecraft'}
                  </strong>
                </span>
                <span>
                  Владелец:{' '}
                  <strong>
                    {volume.ownerUsername ||
                      (volume.ownerUserId ? volume.ownerUserId : 'общий том')}
                  </strong>
                </span>
                {volume.reservedBytes > 0 ? (
                  <span>Резерв: {formatBytes(volume.reservedBytes)}</span>
                ) : null}
                {volume.maxBytes ? <span>Лимит: {formatBytes(volume.maxBytes)}</span> : null}
                <span>
                  Проверка:{' '}
                  {volume.lastCheckedAt ? relativeTime(volume.lastCheckedAt) : 'не выполнялась'}
                </span>
              </div>

              {volume.state !== 'online' || !volume.enabled ? (
                <Alert tone={volume.state === 'error' ? 'danger' : 'warn'}>
                  <span className="row" style={{ gap: 8, alignItems: 'center' }}>
                    <TriangleAlert aria-hidden="true" style={{ width: 15, height: 15 }} />
                    {volume.enabled
                      ? volume.stateMessage || 'Хранилище недоступно.'
                      : 'Хранилище отключено. Новые записи на этот том не выполняются.'}
                  </span>
                </Alert>
              ) : null}

              <div className="volume-card__actions">
                <Button
                  size="sm"
                  icon={<RefreshCw aria-hidden="true" />}
                  loading={checkVolume.isPending && checkVolume.variables === volume.id}
                  onClick={() => checkVolume.mutate(volume.id)}
                >
                  Проверить
                </Button>
                <Button size="sm" onClick={() => setEditor({ mode: 'edit', volume })}>
                  Изменить
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Trash2 aria-hidden="true" />}
                  onClick={() => setConfirmDelete(volume)}
                >
                  Удалить
                </Button>
              </div>
            </article>
          ))}
        </div>
      )}

      <SectionTitle title="Диски сервера" meta={`${diskList.length} обнаружено`} />
      {disks.isLoading ? (
        <div className="card">
          <LoadingInline label="Обнаружение дисков" />
        </div>
      ) : disks.isError ? (
        <Alert tone="danger">Не удалось получить список дисков: {errorMessage(disks.error)}</Alert>
      ) : (
        <div className="table-card">
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Диск</th>
                  <th>Название</th>
                  <th>ФС</th>
                  <th>Тип</th>
                  <th>Занято</th>
                  <th>Свободно</th>
                  <th>Хранилищ</th>
                </tr>
              </thead>
              <tbody>
                {diskList.map((disk) => {
                  const used = disk.totalBytes > 0 ? (disk.usedBytes / disk.totalBytes) * 100 : 0;
                  return (
                    <tr key={disk.mountPoint}>
                      <td>
                        <div className="table__user">
                          <span className="mono">{disk.mountPoint}</span>
                        </div>
                      </td>
                      <td>
                        {disk.name || '—'}
                        {disk.system ? (
                          <>
                            {' '}
                            <Badge plain>системный</Badge>
                          </>
                        ) : null}
                        {disk.removable ? (
                          <>
                            {' '}
                            <Badge tone="info" plain>
                              съёмный
                            </Badge>
                          </>
                        ) : null}
                        {disk.remote ? (
                          <>
                            {' '}
                            <Badge tone="warn" plain>
                              сетевой
                            </Badge>
                          </>
                        ) : null}
                      </td>
                      <td className="mono">{disk.fileSystem || '—'}</td>
                      <td>{disk.kind}</td>
                      <td>
                        {formatBytes(disk.usedBytes)}
                        <div className="meter" aria-hidden="true">
                          <div
                            className={`meter__fill${used > 85 ? ' meter__fill--warn' : ''}`}
                            style={{ width: `${Math.min(100, used)}%` }}
                          />
                        </div>
                      </td>
                      <td>{formatBytes(disk.availableBytes)}</td>
                      <td>{(volumesByDisk.get(disk.mountPoint) ?? []).length}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div className="card">
        <div className="info-list">
          <div className="info-list__row">
            <span>Каталог служебных данных</span>
            <span className="mono">.nexus/objects</span>
          </div>
          <div className="info-list__row">
            <span>Правила размещения</span>
            <span>файл целиком на одном томе, без автоматического переноса</span>
          </div>
          <div className="info-list__row">
            <span>Проверка путей</span>
            <span>канонизация, запрет ссылок и junction, защита границ тома</span>
          </div>
        </div>
      </div>

      {editor ? (
        <VolumeEditor
          state={editor}
          onClose={() => setEditor(null)}
          onSaved={() => {
            invalidate();
            setEditor(null);
          }}
        />
      ) : null}

      <ConfirmDialog
        open={confirmDelete !== null}
        title="Удалить хранилище?"
        description={
          confirmDelete
            ? `Регистрация каталога «${confirmDelete.name}» будет удалена. Файлы на диске не удаляются и остаются на месте. Если в каталоге есть файлы, которых нет в базе, сервер откажет.`
            : ''
        }
        confirmLabel="Удалить"
        busy={deleteVolume.isPending}
        onCancel={() => setConfirmDelete(null)}
        onConfirm={() => {
          if (confirmDelete) deleteVolume.mutate(confirmDelete.id);
        }}
      />

      <ConfirmDialog
        open={confirmDisable !== null}
        title="Отключить хранилище?"
        description={
          confirmDisable
            ? `Новые записи в «${confirmDisable.name}» будут запрещены. Существующие объекты сохранятся.`
            : ''
        }
        confirmLabel="Отключить"
        busy={toggleVolume.isPending}
        onCancel={() => setConfirmDisable(null)}
        onConfirm={() => {
          if (confirmDisable) {
            toggleVolume.mutate({ id: confirmDisable.id, enabled: false });
          }
          setConfirmDisable(null);
        }}
      />
    </>
  );
}

interface VolumeEditorProps {
  state: EditorState;
  onClose: () => void;
  onSaved: () => void;
}

function VolumeEditor({ state, onClose, onSaved }: VolumeEditorProps) {
  const isCreate = state.mode === 'create';
  const volume = state.volume;
  const [name, setName] = useState(volume?.name ?? '');
  const [path, setPath] = useState(volume?.rootPath ?? '');
  const [mode, setMode] = useState<'auto' | 'manual'>(
    volume?.mode === 'manual' ? 'manual' : 'auto',
  );
  const [scope, setScope] = useState<'system' | 'cloud' | 'minecraft'>(
    volume?.scope === 'cloud' ? 'cloud' : volume?.scope === 'minecraft' ? 'minecraft' : 'system',
  );
  const [ownerId, setOwnerId] = useState(volume?.ownerUserId ?? '');
  const [priority, setPriority] = useState(String(volume?.priority ?? 100));
  const [reserved, setReserved] = useState(
    volume?.reservedBytes ? String(Math.round((volume.reservedBytes / GIB) * 10) / 10) : '0',
  );
  const [maxBytes, setMaxBytes] = useState(
    volume?.maxBytes ? String(Math.round((volume.maxBytes / GIB) * 10) / 10) : '',
  );
  const [enabled, setEnabled] = useState(volume?.enabled ?? true);
  const [browsePath, setBrowsePath] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [adoptOrphans, setAdoptOrphans] = useState(false);

  const browse = useQuery({
    queryKey: ['storage-browse', browsePath],
    queryFn: () => api.browseStorage(browsePath ?? undefined),
    enabled: isCreate,
    staleTime: 5_000,
  });

  const scan = useQuery({
    queryKey: ['storage-scan', path],
    queryFn: () => api.scanStoragePath(path.trim()),
    enabled: isCreate && path.trim() !== '',
    staleTime: 0,
    retry: false,
  });

  const owners = useQuery({
    queryKey: ['storage-owners'],
    queryFn: () => api.listStorageOwners(),
    staleTime: 60_000,
  });

  useEffect(() => {
    if (isCreate && !browsePath && browse.data?.path) {
      setBrowsePath(browse.data.path);
    }
  }, [browse.data, browsePath, isCreate]);

  useEffect(() => {
    setAdoptOrphans(false);
  }, [path]);

  const save = useMutation({
    mutationFn: () => {
      const priorityValue = Number(priority);
      if (!Number.isFinite(priorityValue) || priorityValue < 0 || priorityValue > 1000) {
        throw new Error('Приоритет должен быть числом от 0 до 1000.');
      }
      const reservedValue = Number(reserved || '0');
      if (!Number.isFinite(reservedValue) || reservedValue < 0) {
        throw new Error('Резерв задан некорректно.');
      }
      const maxValue = maxBytes.trim() === '' ? null : Number(maxBytes);
      if (maxValue !== null && (!Number.isFinite(maxValue) || maxValue <= 0)) {
        throw new Error('Лимит тома задан некорректно.');
      }
      if (isCreate) {
        if (path.trim() === '') {
          throw new Error('Выберите каталог на сервере.');
        }
        return api.createStorageVolume({
          name: name.trim(),
          path: path.trim(),
          mode,
          priority: priorityValue,
          reservedBytes: Math.round(reservedValue * GIB),
          maxBytes: maxValue === null ? null : Math.round(maxValue * GIB),
          enabled,
          scope,
          ownerUserId: ownerId === '' ? null : ownerId,
          adoptOrphans: adoptOrphans || undefined,
        });
      }
      return api.updateStorageVolume(volume!.id, {
        name: name.trim(),
        mode,
        priority: priorityValue,
        reservedBytes: Math.round(reservedValue * GIB),
        maxBytes: maxValue === null ? 0 : Math.round(maxValue * GIB),
        enabled,
        scope,
        ownerUserId: ownerId === '' ? null : ownerId,
      });
    },
    onSuccess: () => {
      toast(
        isCreate
          ? adoptOrphans
            ? 'Каталог добавлен, найденные объекты импортированы в учёт.'
            : 'Каталог хранения добавлен.'
          : 'Хранилище обновлено.',
        'success',
      );
      onSaved();
    },
    onError: (error) => {
      const message = errorMessage(error);
      setFormError(message);
      toast(message, 'error');
    },
  });

  const scanHasOrphans = isCreate && (scan.data?.objectsFound ?? 0) > 0;
  const scanBlocks =
    isCreate &&
    (scan.isLoading || scan.data?.truncated === true || (scanHasOrphans && !adoptOrphans));

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={isCreate ? 'Добавить каталог хранения' : `Хранилище: ${volume?.name}`}
      description="Каталог регистрируется сервером: путь проверяется, канонизируется и защищается от выхода за границы тома."
      footer={
        <>
          <Button onClick={onClose} disabled={save.isPending}>
            Отмена
          </Button>
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={name.trim().length < 2 || (isCreate && path.trim() === '') || scanBlocks}
            onClick={() => {
              setFormError(null);
              save.mutate();
            }}
          >
            {isCreate ? 'Зарегистрировать' : 'Сохранить'}
          </Button>
        </>
      }
    >
      {formError ? <Alert tone="danger">{formError}</Alert> : null}
      <div className="form-grid">
        <Field label="Название" htmlFor="volume-name" hint="Видно в списке хранилищ">
          <input
            id="volume-name"
            className="input"
            value={name}
            maxLength={64}
            placeholder="Например: Storage SSD"
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label="Режим выбора" htmlFor="volume-mode" hint="Авто — по приоритету и месту">
          <select
            id="volume-mode"
            className="select"
            value={mode}
            onChange={(event) => setMode(event.target.value as 'auto' | 'manual')}
          >
            <option value="auto">Автоматический</option>
            <option value="manual">Ручной</option>
          </select>
        </Field>
        <Field
          label="Область"
          htmlFor="volume-scope"
          hint="Система — общий том для Cloud и Minecraft"
        >
          <select
            id="volume-scope"
            className="select"
            value={scope}
            onChange={(event) => setScope(event.target.value as 'system' | 'cloud' | 'minecraft')}
          >
            <option value="system">Система (оба модуля)</option>
            <option value="cloud">Только Cloud</option>
            <option value="minecraft">Только Minecraft</option>
          </select>
        </Field>
        <Field
          label="Владелец"
          htmlFor="volume-owner"
          hint="Общий том доступен всем, у кого есть права модуля"
        >
          <select
            id="volume-owner"
            className="select"
            value={ownerId}
            onChange={(event) => setOwnerId(event.target.value)}
            disabled={owners.isLoading}
          >
            <option value="">Общий том</option>
            {(owners.data ?? []).map((owner) => (
              <option key={owner.id} value={owner.id}>
                {owner.displayName || owner.username} ({owner.username})
              </option>
            ))}
          </select>
        </Field>
        <Field label="Приоритет" htmlFor="volume-priority" hint="0–1000, больше — выше">
          <input
            id="volume-priority"
            className="input"
            type="number"
            min={0}
            max={1000}
            value={priority}
            onChange={(event) => setPriority(event.target.value)}
          />
        </Field>
        <Field label="Резерв, ГБ" htmlFor="volume-reserved" hint="Место, которое не занимает NEXUS">
          <input
            id="volume-reserved"
            className="input"
            type="number"
            min={0}
            step={0.5}
            value={reserved}
            onChange={(event) => setReserved(event.target.value)}
          />
        </Field>
        <Field label="Лимит тома, ГБ" htmlFor="volume-max" hint="Пусто — без лимита">
          <input
            id="volume-max"
            className="input"
            type="number"
            min={0}
            step={1}
            value={maxBytes}
            onChange={(event) => setMaxBytes(event.target.value)}
          />
        </Field>
        <Field
          label="Включено"
          htmlFor="volume-enabled"
          hint="Отключённый том не принимает новые файлы"
        >
          <Switch checked={enabled} label="Включено" onChange={(next) => setEnabled(next)} />
        </Field>

        {isCreate ? (
          <Field
            label="Каталог на сервере"
            htmlFor="volume-path"
            full
            hint="Путь выбирается через серверный обозреватель каталогов, не через браузерный input"
          >
            <input
              id="volume-path"
              className="input mono"
              value={path}
              readOnly
              placeholder="Каталог не выбран"
            />
          </Field>
        ) : (
          <Field label="Каталог на сервере" htmlFor="volume-path" full>
            <input id="volume-path" className="input mono" value={path} readOnly />
          </Field>
        )}
      </div>

      {isCreate ? (
        <div className="stack" style={{ gap: 8 }}>
          <div className="tiny muted">Обозреватель каталогов сервера</div>
          <BrowsePanel
            data={browse.data}
            loading={browse.isLoading}
            error={browse.isError ? errorMessage(browse.error) : null}
            currentPath={browsePath}
            onNavigate={setBrowsePath}
            onSelect={(selected) => setPath(selected)}
          />
          {browsePath && !isDriveRoot(browsePath) ? (
            <Button size="sm" variant="ghost" onClick={() => setPath(browsePath)}>
              Выбрать текущий каталог: <span className="mono">{browsePath}</span>
            </Button>
          ) : (
            <div className="tiny muted">
              Корень диска регистрировать нельзя — зайдите внутрь и выберите подкаталог.
            </div>
          )}
          {path.trim() !== '' ? (
            <>
              {scan.isLoading ? <LoadingInline label="Проверка каталога" /> : null}
              {scan.isError ? (
                <Alert tone="warn">
                  Не удалось проверить каталог заранее: {errorMessage(scan.error)}. Сервер повторит
                  проверку при регистрации.
                </Alert>
              ) : null}
              {scan.data ? (
                <>
                  {scan.data.truncated ? (
                    <Alert tone="danger">
                      Каталог слишком большой или содержит недопустимые ссылки для безопасной
                      проверки. Регистрация заблокирована: укажите каталог с меньшим объёмом данных
                      или очистите его вручную.
                    </Alert>
                  ) : null}
                  {scan.data.objectsFound > 0 ? (
                    <Alert tone="warn">
                      <div>
                        В каталоге уже есть объекты прежнего хранилища:{' '}
                        <strong>{scan.data.objectsFound}</strong> файлов на{' '}
                        {formatBytes(scan.data.objectsBytes)}. В базе они не учтены.
                      </div>
                      <div style={{ marginTop: 8 }}>
                        <Switch
                          checked={adoptOrphans}
                          label="Импортировать найденные объекты в учёт"
                          onChange={setAdoptOrphans}
                        />
                      </div>
                      <div className="tiny muted" style={{ marginTop: 6 }}>
                        Исходные имена и владельцы восстановить нельзя, но файлы не останутся
                        «осиротевшими» и не будут удалены автоматической очисткой. Без импорта
                        регистрация непустого каталога отклоняется.
                      </div>
                    </Alert>
                  ) : !scan.data.truncated ? (
                    <div className="tiny muted">Объектов прежнего хранилища не найдено.</div>
                  ) : null}
                  {scan.data.stagingFiles > 0 || scan.data.quarantineFiles > 0 ? (
                    <div className="tiny muted">
                      Незавершённых загрузок: {scan.data.stagingFiles}, файлов в карантине:{' '}
                      {scan.data.quarantineFiles}. NEXUS их не импортирует.
                    </div>
                  ) : null}
                  {scan.data.foreignEntries > 0 ? (
                    <div className="tiny muted">
                      Посторонних записей верхнего уровня: {scan.data.foreignEntries}. NEXUS их не
                      изменяет.
                    </div>
                  ) : null}
                  {scan.data.skippedEntries > 0 ? (
                    <div className="tiny muted">
                      Пропущено ссылок и нечитаемых записей: {scan.data.skippedEntries}.
                    </div>
                  ) : null}
                </>
              ) : null}
            </>
          ) : null}
        </div>
      ) : null}
    </Modal>
  );
}

function isDriveRoot(path: string): boolean {
  return /^[a-zA-Z]:[\\/]?$/.test(path) || path === '/';
}

interface BrowsePanelProps {
  data: BrowseResponse | undefined;
  loading: boolean;
  error: string | null;
  currentPath: string | null;
  onNavigate: (path: string | null) => void;
  onSelect: (path: string) => void;
}

function BrowsePanel({
  data,
  loading,
  error,
  currentPath,
  onNavigate,
  onSelect,
}: BrowsePanelProps) {
  return (
    <div className="browse">
      <div className="browse__head">
        {currentPath && data?.parent !== null ? (
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            aria-label="Вверх"
            onClick={() => onNavigate(data?.parent ?? null)}
          >
            <ArrowUp aria-hidden="true" />
          </Button>
        ) : null}
        <span className="truncate">{data?.path ?? 'Диски сервера'}</span>
      </div>
      {loading ? (
        <div className="browse__row">
          <LoadingInline label="Чтение каталога" />
        </div>
      ) : error ? (
        <div className="browse__row">
          <span className="tiny" style={{ color: 'var(--danger)' }}>
            {error}
          </span>
        </div>
      ) : (data?.entries.length ?? 0) === 0 ? (
        <div className="browse__row">
          <span className="tiny muted">Подкаталогов нет.</span>
        </div>
      ) : (
        <div className="browse__list">
          {data?.entries.map((entry) => (
            <div className="browse__row" key={entry.path}>
              <button
                type="button"
                className="browse__name"
                onClick={() => onNavigate(entry.path)}
                title={entry.path}
              >
                {entry.isDisk ? (
                  <HardDrive aria-hidden="true" style={{ width: 14, height: 14 }} />
                ) : (
                  <FolderPlus aria-hidden="true" style={{ width: 14, height: 14 }} />
                )}
                <span className="truncate">{entry.name}</span>
              </button>
              {!entry.isDisk && !isDriveRoot(entry.path) ? (
                <Button size="sm" variant="ghost" onClick={() => onSelect(entry.path)}>
                  Выбрать
                </Button>
              ) : (
                <span className="browse__size">
                  {entry.totalBytes > 0 ? formatBytes(entry.availableBytes) : ''}
                </span>
              )}
            </div>
          ))}
        </div>
      )}
      {data?.truncated ? (
        <div className="browse__row">
          <span className="tiny muted">Список каталогов обрезан: показаны первые записи.</span>
        </div>
      ) : null}
      {currentPath ? null : (
        <div className="browse__row">
          <span className="tiny muted">
            <ShieldAlert aria-hidden="true" style={{ width: 13, height: 13 }} /> Системные каталоги
            регистрировать запрещено.
          </span>
        </div>
      )}
    </div>
  );
}
