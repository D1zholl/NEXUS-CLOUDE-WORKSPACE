import { useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from 'react-router-dom';
import {
  Archive,
  Boxes,
  CheckCircle2,
  Copy,
  EyeOff,
  FileStack,
  HardDrive,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Trash2,
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
} from '../components/ui';
import { api, errorMessage, type BuildRow, type BuildStats } from '../services/api';
import { formatBytes, formatDate, pluralize, relativeTime } from '../lib/format';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

import { BuildWizard } from '../components/BuildWizard';

const LOADER_LABELS: Record<string, string> = {
  vanilla: 'Vanilla',
  fabric: 'Fabric',
  forge: 'Forge',
  neoforge: 'NeoForge',
  quilt: 'Quilt',
};

export function statusBadge(status: string) {
  if (status === 'published') return <Badge tone="ok">Опубликована</Badge>;
  if (status === 'archived') return <Badge plain>Архив</Badge>;
  return <Badge tone="warn">Черновик</Badge>;
}

function slugifyLocal(name: string): string {
  const map: Record<string, string> = {
    а: 'a',
    б: 'b',
    в: 'v',
    г: 'g',
    д: 'd',
    е: 'e',
    ё: 'e',
    ж: 'zh',
    з: 'z',
    и: 'i',
    й: 'y',
    к: 'k',
    л: 'l',
    м: 'm',
    н: 'n',
    о: 'o',
    п: 'p',
    р: 'r',
    с: 's',
    т: 't',
    у: 'u',
    ф: 'f',
    х: 'h',
    ц: 'ts',
    ч: 'ch',
    ш: 'sh',
    щ: 'sch',
    ъ: '',
    ы: 'y',
    ь: '',
    э: 'e',
    ю: 'yu',
    я: 'ya',
  };
  let out = '';
  let lastDash = false;
  for (const ch of name.toLowerCase()) {
    if (map[ch] !== undefined) {
      out += map[ch];
      lastDash = false;
    } else if (/[a-z0-9]/.test(ch)) {
      out += ch;
      lastDash = false;
    } else if (!lastDash && out) {
      out += '-';
      lastDash = true;
    }
  }
  return out.replace(/^-+|-+$/g, '').slice(0, 64);
}

interface EditorState {
  mode: 'create' | 'edit';
  build?: BuildRow;
}

export function MinecraftBuildsPage() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const canCreate = useCan('minecraft.build.create');
  const canEdit = useCan('minecraft.build.edit');
  const canPublish = useCan('minecraft.build.publish');
  const canDelete = useCan('minecraft.build.delete');

  const [searchInput, setSearchInput] = useState('');
  const [search, setSearch] = useState('');
  const [status, setStatus] = useState('');
  const [loader, setLoader] = useState('');
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<BuildRow | null>(null);
  const [unpublishTarget, setUnpublishTarget] = useState<BuildRow | null>(null);
  const [busy, setBusy] = useState(false);

  const params = useMemo(
    () => ({
      q: search || undefined,
      status: status || undefined,
      loader: loader || undefined,
      perPage: 48,
    }),
    [search, status, loader],
  );

  const stats = useQuery({ queryKey: ['mc-stats'], queryFn: () => api.minecraftStats() });
  const builds = useQuery({
    queryKey: ['mc-builds', params],
    queryFn: () => api.listBuilds(params),
  });

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: ['mc-builds'] });
    void queryClient.invalidateQueries({ queryKey: ['mc-stats'] });
  };

  const items = builds.data?.items ?? [];
  const data: BuildStats | undefined = stats.data;

  const cloneBuild = async (build: BuildRow): Promise<void> => {
    try {
      const copy = await api.cloneBuild(build.id);
      invalidate();
      toast(`Создана копия «${copy.name}».`, 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const archiveBuild = async (build: BuildRow): Promise<void> => {
    try {
      await api.archiveBuild(build.id);
      invalidate();
      toast('Сборка перемещена в архив.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const restoreBuild = async (build: BuildRow): Promise<void> => {
    try {
      await api.restoreBuild(build.id);
      invalidate();
      toast('Сборка восстановлена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const unpublishBuild = async (): Promise<void> => {
    if (!unpublishTarget) return;
    setBusy(true);
    try {
      await api.unpublishBuild(unpublishTarget.id);
      setUnpublishTarget(null);
      invalidate();
      toast('Сборка снята с публикации.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const deleteBuild = async (): Promise<void> => {
    if (!deleteTarget) return;
    setBusy(true);
    try {
      await api.deleteBuild(deleteTarget.id);
      setDeleteTarget(null);
      invalidate();
      toast('Сборка удалена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHead
        eyebrow="Пространство"
        title="Minecraft Builds"
        description="Сборки, файлы, проверка и релизы для собственного лаунчера. Опубликованный релиз неизменяем."
        actions={
          canCreate ? (
            <Button
              variant="primary"
              icon={<Plus aria-hidden="true" />}
              onClick={() => setWizardOpen(true)}
            >
              Новая сборка
            </Button>
          ) : undefined
        }
      />

      <div className="stats-grid">
        <StatCard
          label="Всего сборок"
          icon={<Boxes aria-hidden="true" />}
          value={data?.total ?? 0}
          sub={`${data?.drafts ?? 0} ${pluralize(data?.drafts ?? 0, 'черновик', 'черновика', 'черновиков')}`}
        />
        <StatCard
          label="Опубликовано"
          icon={<CheckCircle2 aria-hidden="true" />}
          value={data?.published ?? 0}
          sub={`${data?.archived ?? 0} в архиве`}
        />
        <StatCard
          label="Файлов"
          icon={<FileStack aria-hidden="true" />}
          value={data?.files ?? 0}
          sub="во всех сборках"
        />
        <StatCard
          label="Объём артефактов"
          icon={<HardDrive aria-hidden="true" />}
          value={formatBytes(data?.totalBytes ?? 0)}
          sub="физические объекты"
        />
      </div>

      {(data?.recentUpdated?.length ?? 0) > 0 || (data?.recentPublished?.length ?? 0) > 0 ? (
        <div className="grid-2">
          <div className="card">
            <SectionTitle title="Последние изменения" meta="5 последних" />
            <div className="stack" style={{ gap: 8 }}>
              {(data?.recentUpdated ?? []).map((item) => (
                <Link
                  key={item.id}
                  to={`/workspace/builds/${item.id}`}
                  className="row row--between"
                  style={{ textDecoration: 'none', fontSize: 12.5 }}
                >
                  <span className="truncate">{item.name}</span>
                  <span className="tiny muted">{relativeTime(item.at)}</span>
                </Link>
              ))}
            </div>
          </div>
          <div className="card">
            <SectionTitle title="Последние публикации" meta="5 последних" />
            {(data?.recentPublished?.length ?? 0) === 0 ? (
              <div className="tiny muted">Публикаций пока не было.</div>
            ) : (
              <div className="stack" style={{ gap: 8 }}>
                {(data?.recentPublished ?? []).map((item) => (
                  <Link
                    key={item.id}
                    to={`/workspace/builds/${item.id}`}
                    className="row row--between"
                    style={{ textDecoration: 'none', fontSize: 12.5 }}
                  >
                    <span className="truncate">
                      {item.name}
                      {item.version ? <span className="tiny muted"> · {item.version}</span> : null}
                    </span>
                    <span className="tiny muted">{relativeTime(item.at)}</span>
                  </Link>
                ))}
              </div>
            )}
          </div>
        </div>
      ) : null}

      <div className="table-card">
        <form
          className="toolbar"
          onSubmit={(event) => {
            event.preventDefault();
            setSearch(searchInput.trim());
          }}
        >
          <div className="input-wrap">
            <Search aria-hidden="true" />
            <input
              className="input input--with-icon"
              type="search"
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
              placeholder="Название или slug…"
              aria-label="Поиск сборок"
            />
          </div>
          <select
            className="select"
            value={status}
            onChange={(event) => setStatus(event.target.value)}
            aria-label="Статус"
          >
            <option value="">Все статусы</option>
            <option value="draft">Черновики</option>
            <option value="published">Опубликованные</option>
            <option value="archived">Архив</option>
          </select>
          <select
            className="select"
            value={loader}
            onChange={(event) => setLoader(event.target.value)}
            aria-label="Загрузчик"
          >
            <option value="">Все загрузчики</option>
            {Object.entries(LOADER_LABELS).map(([key, label]) => (
              <option key={key} value={key}>
                {label}
              </option>
            ))}
          </select>
          <Button type="submit" size="sm" variant="primary">
            Найти
          </Button>
          {search || status || loader ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                setSearch('');
                setSearchInput('');
                setStatus('');
                setLoader('');
              }}
            >
              Сбросить
            </Button>
          ) : null}
          <div className="grow" />
          <Button
            type="button"
            size="sm"
            variant="ghost"
            icon={<RefreshCw aria-hidden="true" />}
            onClick={() => void builds.refetch()}
          >
            Обновить
          </Button>
        </form>

                {builds.isLoading ? (
            <LoadingInline label="Загрузка сборок" />
          ) : builds.isError ? (
            <div style={{ padding: 16 }}>
              <Alert>{errorMessage(builds.error)}</Alert>
            </div>
          ) : items.length === 0 ? (
            <EmptyState
              icon={<Boxes aria-hidden="true" />}
              title={search || status || loader ? 'Сборки не найдены' : 'Сборок пока нет'}
              action={
                canCreate ? (
                  <Button
                    variant="primary"
                    icon={<Plus aria-hidden="true" />}
                    onClick={() => setWizardOpen(true)}
                  >
                    Создать сборку
                  </Button>
                ) : undefined
              }
            >
              {search || status || loader
                ? 'Измените условия поиска или сбросьте фильтры.'
                : 'Создайте первую сборку, добавьте файлы и опубликуйте релиз для лаунчера.'}
            </EmptyState>
          ) : (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Сборка</th>
                    <th>Minecraft</th>
                    <th>Загрузчик</th>
                    <th>Релиз</th>
                    <th>Файлы</th>
                    <th>Размер</th>
                    <th>Изменена</th>
                    <th style={{ textAlign: 'right' }}>Действия</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((build) => (
                    <tr key={build.id}>
                      <td>
                        <div className="build-cell">
                          <div className="build-cell__icon">
                            {build.iconUrl ? (
                              <img
                                src={build.iconUrl}
                                alt=""
                                width={28}
                                height={28}
                                style={{ borderRadius: 6, objectFit: 'cover' }}
                              />
                            ) : (
                              <Boxes aria-hidden="true" />
                            )}
                          </div>
                          <div style={{ minWidth: 0 }}>
                            <Link className="build-cell__name" to={`/workspace/builds/${build.id}`}>
                              {build.name}
                            </Link>
                            <div className="build-cell__meta">
                              <span className="mono">{build.slug}</span>
                              {statusBadge(build.status)}
                              {build.visibility === 'public' ? (
                                <Badge tone="info" plain>
                                  public
                                </Badge>
                              ) : (
                                <Badge plain>private</Badge>
                              )}
                              {build.status === 'published' && build.visibility !== 'public' ? (
                                <Badge tone="warn" plain>
                                  не в лаунчере
                                </Badge>
                              ) : null}
                            </div>
                          </div>
                        </div>
                      </td>
                      <td className="mono tiny">{build.minecraftVersion || '—'}</td>
                      <td className="tiny">
                        {LOADER_LABELS[build.loaderType] ?? build.loaderType}
                        {build.loaderVersion ? (
                          <div className="muted mono" style={{ fontSize: 10.5 }}>
                            {build.loaderVersion}
                          </div>
                        ) : null}
                      </td>
                      <td className="mono tiny">{build.currentReleaseVersion ?? '—'}</td>
                      <td className="mono tiny">{build.fileCount}</td>
                      <td className="mono tiny">{formatBytes(build.totalBytes)}</td>
                      <td className="tiny muted">{formatDate(build.updatedAt)}</td>
                      <td>
                        <div className="table__actions">
                          {canEdit ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Редактировать"
                              onClick={() => setEditor({ mode: 'edit', build })}
                            >
                              <Pencil aria-hidden="true" />
                            </Button>
                          ) : null}
                          {canCreate ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Клонировать"
                              onClick={() => void cloneBuild(build)}
                            >
                              <Copy aria-hidden="true" />
                            </Button>
                          ) : null}
                          {canPublish && build.status === 'published' ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Снять с публикации"
                              title="Снять с публикации"
                              onClick={() => setUnpublishTarget(build)}
                            >
                              <EyeOff aria-hidden="true" />
                            </Button>
                          ) : null}
                          {canEdit ? (
                            build.status === 'archived' ? (
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="Восстановить"
                                onClick={() => void restoreBuild(build)}
                              >
                                <RefreshCw aria-hidden="true" />
                              </Button>
                            ) : (
                              <Button
                                size="sm"
                                variant="ghost"
                                iconOnly
                                aria-label="В архив"
                                onClick={() => void archiveBuild(build)}
                              >
                                <Archive aria-hidden="true" />
                              </Button>
                            )
                          ) : null}
                          {canDelete ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              aria-label="Удалить"
                              onClick={() => setDeleteTarget(build)}
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
        )}
      </div>

      {editor ? (
        <BuildEditorModal
          state={editor}
          onClose={() => setEditor(null)}
          onSaved={() => {
            setEditor(null);
            invalidate();
          }}
        />
      ) : null}

      <ConfirmDialog
        open={unpublishTarget !== null}
        title="Снять сборку с публикации?"
        description={
          unpublishTarget
            ? `Сборка «${unpublishTarget.name}» и её релизы перестанут отдаваться лаунчеру и исчезнут из каталога. Файлы и снимки релизов сохранятся — публикацию можно вернуть, опубликовав релиз снова.`
            : ''
        }
        confirmLabel="Снять с публикации"
        busy={busy}
        onConfirm={() => void unpublishBuild()}
        onCancel={() => setUnpublishTarget(null)}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        title="Удалить сборку?"
        description={
          deleteTarget
            ? `Сборка «${deleteTarget.name}» и её релизы будут удалены. Сначала снимите публикацию, если релизы опубликованы.`
            : ''
        }
        confirmLabel="Удалить"
        busy={busy}
        onConfirm={() => void deleteBuild()}
        onCancel={() => setDeleteTarget(null)}
      />

      <BuildWizard
        open={wizardOpen}
        onClose={() => setWizardOpen(false)}
        onCreated={(id) => {
          setWizardOpen(false);
          void queryClient.invalidateQueries({ queryKey: ['mc-builds'] });
          void queryClient.invalidateQueries({ queryKey: ['mc-stats'] });
          navigate(`/workspace/builds/${id}`);
        }}
      />

      <span className="hidden">{relativeTime(data?.total === 0 ? null : undefined)}</span>
    </>
  );
}

/* --------------------------------- редактор -------------------------------- */
function BuildEditorModal({
  state,
  onClose,
  onSaved,
}: {
  state: EditorState;
  onClose: () => void;
  onSaved: () => void;
}) {
  const build = state.build;
  const [form, setForm] = useState({
    name: build?.name ?? '',
    slug: build?.slug ?? '',
    description: build?.description ?? '',
    version: build?.version ?? '1.0.0',
    category: build?.category ?? 'other',
    visibility: build?.visibility ?? 'private',
    minecraftVersion: build?.minecraftVersion ?? '',
    loaderType: build?.loaderType ?? 'vanilla',
    loaderVersion: build?.loaderVersion ?? '',
    javaMajor: build?.javaMajor ?? 21,
    javaMin: build?.javaMin ?? 21,
    javaNotes: build?.javaNotes ?? '',
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [slugTouched, setSlugTouched] = useState(Boolean(build));

  const versions = useQuery({
    queryKey: ['mc-versions'],
    queryFn: () => api.minecraftVersions(),
    staleTime: 10 * 60 * 1000,
  });
  const loaders = useQuery({
    queryKey: ['mc-loaders', form.loaderType, form.minecraftVersion],
    queryFn: () => api.loaderVersions(form.loaderType, form.minecraftVersion),
    enabled: form.loaderType !== 'vanilla' && form.minecraftVersion.trim().length > 0,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  const loaderVersions = loaders.data?.versions ?? [];

  // Версия загрузчика подбирается автоматически из совместимых с игрой.
  useEffect(() => {
    if (form.loaderType === 'vanilla' || !loaders.isSuccess) return;
    const versions = loaders.data?.versions ?? [];
    const nextVersion = versions.includes(form.loaderVersion)
      ? form.loaderVersion
      : (versions[0] ?? '');
    if (nextVersion === form.loaderVersion) return;
    setForm((prev) => ({ ...prev, loaderVersion: nextVersion }));
  }, [form.loaderType, form.minecraftVersion, form.loaderVersion, loaders.isSuccess, loaders.data]);

  const submit = async (): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      const payload = {
        name: form.name.trim(),
        slug: form.slug.trim() || undefined,
        description: form.description,
        version: form.version,
        category: form.category,
        visibility: form.visibility as 'public' | 'private',
        minecraftVersion: form.minecraftVersion,
        loaderType: form.loaderType,
        loaderVersion: form.loaderVersion,
        javaMajor: form.javaMajor,
        javaMin: form.javaMin,
        javaNotes: form.javaNotes,
      };
      if (state.mode === 'create') {
        await api.createBuild(payload);
        toast('Сборка создана.', 'success');
      } else if (build) {
        await api.updateBuild(build.id, payload);
        toast('Сборка обновлена.', 'success');
      }
      onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={state.mode === 'create' ? 'Новая сборка' : 'Настройки сборки'}
      description="Основные данные, версия игры и загрузчик. Файлы добавляются на странице сборки."
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Отмена
          </Button>
          <Button
            variant="primary"
            loading={busy}
            disabled={
              form.name.trim().length < 2 ||
              !form.minecraftVersion ||
              (form.loaderType !== 'vanilla' && (!form.loaderVersion || !loaders.isSuccess))
            }
            onClick={() => void submit()}
          >
            {state.mode === 'create' ? 'Создать' : 'Сохранить'}
          </Button>
        </>
      }
    >
      {error ? <Alert>{error}</Alert> : null}
      <div className="form-grid">
        <Field label="Название" htmlFor="build-name">
          <input
            id="build-name"
            className="input"
            value={form.name}
            maxLength={96}
            onChange={(event) => {
              const name = event.target.value;
              setForm((prev) => ({
                ...prev,
                name,
                slug: slugTouched ? prev.slug : slugifyLocal(name),
              }));
            }}
          />
        </Field>
        <Field
          label="Slug"
          htmlFor="build-slug"
          hint="Латиница, цифры и дефис: используется в API лаунчера."
        >
          <input
            id="build-slug"
            className="input mono"
            value={form.slug}
            maxLength={64}
            onChange={(event) => {
              setSlugTouched(true);
              setForm((prev) => ({ ...prev, slug: event.target.value }));
            }}
          />
        </Field>
        <Field label="Версия сборки" htmlFor="build-version">
          <input
            id="build-version"
            className="input mono"
            value={form.version}
            maxLength={32}
            onChange={(event) => setForm((prev) => ({ ...prev, version: event.target.value }))}
          />
        </Field>
        <Field label="Категория" htmlFor="build-category">
          <select
            id="build-category"
            className="select"
            value={form.category}
            onChange={(event) => setForm((prev) => ({ ...prev, category: event.target.value }))}
          >
            {['tech', 'magic', 'adventure', 'rpg', 'kitchen-sink', 'optimization', 'other'].map(
              (value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ),
            )}
          </select>
        </Field>
        <Field
          label="Видимость"
          htmlFor="build-visibility"
          hint="В анонимный каталог MyLauncher попадают только публичные сборки. Stock-клиент не передаёт токен, поэтому приватная сборка в нём не появится."
        >
          <select
            id="build-visibility"
            className="select"
            value={form.visibility}
            onChange={(event) => setForm((prev) => ({ ...prev, visibility: event.target.value }))}
          >
            <option value="private">Приватная</option>
            <option value="public">Публичная</option>
          </select>
        </Field>
        <Field
          label="Версия Minecraft"
          htmlFor="build-mc"
          hint={
            versions.isError
              ? 'Список версий недоступен — введите версию вручную.'
              : 'Официальный список версий Mojang.'
          }
        >
          {versions.data ? (
            <select
              id="build-mc"
              className="select"
              value={form.minecraftVersion}
              onChange={(event) =>
                setForm((prev) => ({
                  ...prev,
                  minecraftVersion: event.target.value,
                  loaderVersion: '',
                }))
              }
            >
              <option value="">Выберите версию</option>
              {versions.data.versions.map((version) => (
                <option key={version.id} value={version.id}>
                  {version.id}
                </option>
              ))}
            </select>
          ) : (
            <input
              id="build-mc"
              className="input mono"
              value={form.minecraftVersion}
              placeholder="1.20.1"
              onChange={(event) =>
                setForm((prev) => ({ ...prev, minecraftVersion: event.target.value }))
              }
            />
          )}
        </Field>
        <Field label="Загрузчик" htmlFor="build-loader">
          <select
            id="build-loader"
            className="select"
            value={form.loaderType}
            onChange={(event) =>
              setForm((prev) => ({ ...prev, loaderType: event.target.value, loaderVersion: '' }))
            }
          >
            {Object.entries(LOADER_LABELS).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </Field>
        <Field
          label="Версия загрузчика"
          htmlFor="build-loader-version"
          hint={
            form.loaderType === 'vanilla'
              ? 'Для Vanilla версия загрузчика не требуется.'
              : loaders.isPending
                ? 'Ищем совместимые версии…'
                : loaders.isError
                  ? 'Список совместимых версий недоступен. Сохранение временно невозможно.'
                  : loaderVersions.length === 0
                    ? 'Для этой версии игры нет совместимых версий загрузчика.'
                    : 'Показаны только версии, совместимые с выбранной версией Minecraft.'
          }
        >
          {form.loaderType === 'vanilla' ? (
            <input id="build-loader-version" className="input mono" value="" disabled readOnly />
          ) : loaders.isPending ? (
            <div className="row" style={{ gap: 8 }}>
              <LoadingInline /> Подбор совместимых версий…
            </div>
          ) : loaders.isError ? (
            <Alert tone="warn">{errorMessage(loaders.error)}</Alert>
          ) : loaderVersions.length === 0 ? (
            <Alert tone="warn">
              Для этой версии игры совместимых версий загрузчика не найдено.
            </Alert>
          ) : (
            <select
              id="build-loader-version"
              className="select"
              value={form.loaderVersion}
              onChange={(event) =>
                setForm((prev) => ({ ...prev, loaderVersion: event.target.value }))
              }
            >
              {loaderVersions.map((version) => (
                <option key={version} value={version}>
                  {version}
                </option>
              ))}
            </select>
          )}
        </Field>
        <Field label="Основная версия Java" htmlFor="build-java">
          <input
            id="build-java"
            className="input mono"
            type="number"
            min={8}
            max={30}
            value={form.javaMajor}
            onChange={(event) =>
              setForm((prev) => ({ ...prev, javaMajor: Number(event.target.value) }))
            }
          />
        </Field>
        <Field label="Минимальная версия Java" htmlFor="build-java-min">
          <input
            id="build-java-min"
            className="input mono"
            type="number"
            min={8}
            max={30}
            value={form.javaMin}
            onChange={(event) =>
              setForm((prev) => ({ ...prev, javaMin: Number(event.target.value) }))
            }
          />
        </Field>
        <Field label="Описание" htmlFor="build-desc" full>
          <textarea
            id="build-desc"
            className="textarea"
            value={form.description}
            maxLength={2000}
            onChange={(event) => setForm((prev) => ({ ...prev, description: event.target.value }))}
          />
        </Field>
        <Field label="Требования к Java (примечание)" htmlFor="build-java-notes" full>
          <input
            id="build-java-notes"
            className="input"
            value={form.javaNotes}
            maxLength={300}
            onChange={(event) => setForm((prev) => ({ ...prev, javaNotes: event.target.value }))}
          />
        </Field>
      </div>
    </Modal>
  );
}

export default MinecraftBuildsPage;
