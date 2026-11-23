import { useCallback, useEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useParams } from 'react-router-dom';
import {
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  Copy,
  Download,
  EyeOff,
  History,
  ImagePlus,
  Package,
  Play,
  Rocket,
  Trash2,
  XCircle,
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
  Segmented,
  StatCard,
} from '../components/ui';
import { BuildFileManager } from '../components/BuildFileManager';
import {
  api,
  errorMessage,
  type BuildRow,
  type BuildValidationReport,
  type ReleaseHistoryRow,
  type ReleaseRow,
} from '../services/api';
import { formatBytes, formatDate, pluralize, relativeTime } from '../lib/format';
import { realtime, isBuildEvent } from '../lib/realtime';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

function releaseBadge(status: string) {
  if (status === 'published') return <Badge tone="ok">Опубликован</Badge>;
  if (status === 'deprecated') return <Badge tone="warn">Устарел</Badge>;
  if (status === 'unpublished') return <Badge tone="warn">Снят с публикации</Badge>;
  if (status === 'archived') return <Badge plain>Архив</Badge>;
  return <Badge tone="info">Черновик</Badge>;
}

type Tab = 'files' | 'releases' | 'history';

export function MinecraftBuildPage() {
  const { id = '' } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const canEdit = useCan('minecraft.build.edit');
  const canPublish = useCan('minecraft.build.publish');
  const canUpload = useCan('minecraft.artifacts.upload');

  const [tab, setTab] = useState<Tab>('files');
  const iconInputRef = useRef<HTMLInputElement>(null);
  const [iconBusy, setIconBusy] = useState(false);
  const [report, setReport] = useState<BuildValidationReport | null>(null);
  const [validateBusy, setValidateBusy] = useState(false);
  const [releaseModal, setReleaseModal] = useState(false);
  const [releaseForm, setReleaseForm] = useState<{
    version: string;
    changelog: string;
    channel: 'stable' | 'beta';
  }>({ version: '', changelog: '', channel: 'stable' });
  const [manifestTarget, setManifestTarget] = useState<ReleaseRow | null>(null);
  const [manifest, setManifest] = useState<Record<string, unknown> | null>(null);
  const [launcherManifest, setLauncherManifest] = useState<Record<string, unknown> | null>(null);
  const [manifestKind, setManifestKind] = useState<'launcher' | 'internal'>('launcher');
  const [compareLeft, setCompareLeft] = useState('');
  const [compareRight, setCompareRight] = useState('');
  const [compareResult, setCompareResult] = useState<Record<string, unknown> | null>(null);
  const [unpublishOpen, setUnpublishOpen] = useState(false);
  const [publishWarningsFor, setPublishWarningsFor] = useState<ReleaseRow | null>(null);
  const [busy, setBusy] = useState(false);

  const buildQuery = useQuery({
    queryKey: ['mc-build', id],
    queryFn: () => api.getBuild(id),
    enabled: Boolean(id),
  });

  const filesQuery = useQuery({
    queryKey: ['mc-files', id],
    queryFn: () => api.listBuildFiles(id),
    enabled: Boolean(id),
  });

  const releasesQuery = useQuery({
    queryKey: ['mc-releases', id],
    queryFn: () => api.listReleases(id),
    enabled: Boolean(id),
  });

  const historyQuery = useQuery({
    queryKey: ['mc-history', id],
    queryFn: () => api.buildHistory(id),
    enabled: Boolean(id) && tab === 'history',
  });

  const uploadConfig = useQuery({
    queryKey: ['mc-upload-config'],
    queryFn: () => api.minecraftUploadConfig(),
    enabled: canUpload,
  });

  const invalidate = useCallback((): void => {
    void queryClient.invalidateQueries({ queryKey: ['mc-build', id] });
    void queryClient.invalidateQueries({ queryKey: ['mc-files', id] });
    void queryClient.invalidateQueries({ queryKey: ['mc-releases', id] });
    void queryClient.invalidateQueries({ queryKey: ['mc-history', id] });
    void queryClient.invalidateQueries({ queryKey: ['mc-builds'] });
    void queryClient.invalidateQueries({ queryKey: ['mc-stats'] });
  }, [id, queryClient]);

  // Realtime дополняет опрос: результат проверки и публикации виден сразу,
  // в том числе если действие запущено в другой вкладке.
  useEffect(() => {
    if (!id) return;
    const unsubscribers = [
      realtime.on('build.validation.completed', (event) => {
        if (!isBuildEvent(event, id)) return;
        const { valid, errors, warnings } = event.payload;
        toast(
          valid
            ? warnings > 0
              ? `Проверка завершена: ошибок нет, предупреждений ${warnings}.`
              : 'Проверка завершена: сборка готова к публикации.'
            : `Проверка завершена: ошибок ${errors}, предупреждений ${warnings}.`,
          valid ? (warnings > 0 ? 'warning' : 'success') : 'error',
        );
        invalidate();
      }),
      realtime.on('build.published', (event) => {
        if (!isBuildEvent(event, id)) return;
        toast(`Релиз ${event.payload.version} опубликован.`, 'success');
        invalidate();
      }),
      realtime.on('build.publish.failed', (event) => {
        if (!isBuildEvent(event, id)) return;
        toast(`Публикация отклонена: ${event.payload.message}`, 'error');
        invalidate();
      }),
      realtime.on('build.unpublished', (event) => {
        if (!isBuildEvent(event, id)) return;
        toast('Сборка снята с публикации.', 'success');
        invalidate();
      }),
    ];
    return () => {
      for (const unsubscribe of unsubscribers) unsubscribe();
    };
  }, [id, invalidate]);

  useEffect(() => {
    setReport(null);
  }, [id]);

  const build: BuildRow | undefined = buildQuery.data?.build;
  const fileStats = filesQuery.data?.stats;
  const releases = releasesQuery.data ?? [];

  const runValidation = async (): Promise<BuildValidationReport | null> => {
    setValidateBusy(true);
    try {
      const result = await api.validateBuild(id);
      setReport(result);
      return result;
    } catch (error) {
      toast(errorMessage(error), 'error');
      return null;
    } finally {
      setValidateBusy(false);
    }
  };

  const uploadIcon = async (file: File): Promise<void> => {
    if (file.size > 2 * 1024 * 1024) {
      toast('Иконка не должна превышать 2 МБ.', 'error');
      return;
    }
    if (!/^image\/(png|jpeg|webp)$/.test(file.type)) {
      toast('Поддерживаются только PNG, JPEG и WEBP.', 'error');
      return;
    }
    setIconBusy(true);
    try {
      await api.uploadBuildIcon(id, file);
      invalidate();
      toast('Иконка обновлена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setIconBusy(false);
    }
  };

  const removeIcon = async (): Promise<void> => {
    setIconBusy(true);
    try {
      await api.deleteBuildIcon(id);
      invalidate();
      toast('Иконка удалена.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setIconBusy(false);
    }
  };

  const createRelease = async (): Promise<void> => {
    setBusy(true);
    try {
      const release = await api.createRelease(id, {
        version: releaseForm.version.trim() || undefined,
        changelog: releaseForm.changelog,
        channel: releaseForm.channel,
      });
      setReleaseModal(false);
      setReleaseForm({ version: '', changelog: '', channel: 'stable' });
      // Сразу открываем релизы: черновик нужно опубликовать отдельным
      // действием, и кнопка публикации должна быть на виду.
      setTab('releases');
      invalidate();
      toast(`Релиз ${release.version} создан. Опубликуйте его в списке релизов.`, 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handlePublishError = (error: unknown): void => {
    const details = (error as { details?: BuildValidationReport }).details;
    if (details && typeof details === 'object' && 'errors' in details) {
      setReport(details as BuildValidationReport);
      setTab('files');
    }
    toast(errorMessage(error), 'error');
  };

  const doPublish = async (release: ReleaseRow, acceptWarnings: boolean): Promise<void> => {
    await api.publishRelease(id, release.id, acceptWarnings);
    invalidate();
    toast(`Релиз ${release.version} опубликован.`, 'success');
  };

  // Публикация всегда начинается с проверки: ошибки блокируют действие,
  // предупреждения требуют явного подтверждения администратора.
  const publishRelease = async (release: ReleaseRow): Promise<void> => {
    setBusy(true);
    try {
      const result = await api.validateBuild(id);
      setReport(result);
      if (!result.valid) {
        setTab('files');
        toast(`Проверка нашла ошибки: ${result.errors.length}. Публикация отменена.`, 'error');
        return;
      }
      if (result.warnings.length > 0) {
        setPublishWarningsFor(release);
        return;
      }
      await doPublish(release, false);
    } catch (error) {
      handlePublishError(error);
    } finally {
      setBusy(false);
    }
  };

  const confirmPublishWarnings = async (): Promise<void> => {
    const release = publishWarningsFor;
    if (!release) return;
    setPublishWarningsFor(null);
    setBusy(true);
    try {
      await doPublish(release, true);
    } catch (error) {
      handlePublishError(error);
    } finally {
      setBusy(false);
    }
  };

  const makeBuildPublic = async (): Promise<void> => {
    setBusy(true);
    try {
      await api.updateBuild(id, { visibility: 'public' });
      invalidate();
      toast('Сборка стала публичной и появится в каталоге MyLauncher.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const unpublishBuild = async (): Promise<void> => {
    setBusy(true);
    try {
      await api.unpublishBuild(id);
      setUnpublishOpen(false);
      invalidate();
      toast('Сборка снята с публикации.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const openManifest = async (release: ReleaseRow): Promise<void> => {
    setManifestTarget(release);
    setManifest(null);
    setLauncherManifest(null);
    setManifestKind('launcher');
    try {
      const [internal, launcher] = await Promise.allSettled([
        api.releaseManifest(id, release.id),
        api.launcherReleaseManifest(id, release.id),
      ]);
      if (internal.status === 'fulfilled') setManifest(internal.value);
      if (launcher.status === 'fulfilled') setLauncherManifest(launcher.value);
      if (internal.status === 'rejected' && launcher.status === 'rejected') {
        throw internal.reason;
      }
    } catch (error) {
      toast(errorMessage(error), 'error');
      setManifestTarget(null);
    }
  };

  const runCompare = async (): Promise<void> => {
    if (!compareLeft || !compareRight) return;
    try {
      const result = await api.compareReleases(id, compareLeft, compareRight);
      setCompareResult(result as unknown as Record<string, unknown>);
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

    if (buildQuery.isLoading) {
    return <LoadingInline label="Загрузка сборки" />;
  }
  if (buildQuery.isError || !build) {
    return (
      <Alert tone="danger">
        {buildQuery.isError ? errorMessage(buildQuery.error) : 'Сборка не найдена.'}
      </Alert>
    );
  }

  const manifestFiles = Array.isArray(manifest?.files) ? (manifest.files as unknown[]) : [];

  return (
    <>
      <PageHead
        eyebrow={
          <button type="button" className="link-back" onClick={() => navigate('/workspace/builds')}>
            <ArrowLeft aria-hidden="true" style={{ width: 13, height: 13 }} />
            Все сборки
          </button>
        }
        title={
          <span className="row" style={{ gap: 10, flexWrap: 'wrap' }}>
            {build.iconUrl ? (
              <img
                src={build.iconUrl}
                alt=""
                width={34}
                height={34}
                style={{ borderRadius: 8, objectFit: 'cover' }}
              />
            ) : null}
            {build.name}
          </span>
        }
        description={
          <span className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            <span className="mono">{build.slug}</span>
            <Badge
              tone={
                build.status === 'published'
                  ? 'ok'
                  : build.status === 'archived'
                    ? 'default'
                    : 'warn'
              }
            >
              {build.status === 'published'
                ? 'Опубликована'
                : build.status === 'archived'
                  ? 'Архив'
                  : 'Черновик'}
            </Badge>
            <Badge tone={build.visibility === 'public' ? 'info' : 'warn'}>
              {build.visibility === 'public' ? 'Публичная' : 'Приватная'}
            </Badge>
            <span className="muted">
              Minecraft {build.minecraftVersion || '—'} ·{' '}
              {build.loaderType === 'vanilla'
                ? 'Vanilla'
                : `${build.loaderType} ${build.loaderVersion}`}
            </span>
          </span>
        }
        actions={
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            {canEdit ? (
              <>
                <input
                  ref={iconInputRef}
                  type="file"
                  accept="image/png,image/jpeg,image/webp"
                  className="hidden"
                  data-testid="build-icon-input"
                  onChange={(event) => {
                    const file = event.target.files?.[0];
                    if (file) void uploadIcon(file);
                    event.target.value = '';
                  }}
                />
                <Button
                  icon={<ImagePlus aria-hidden="true" />}
                  loading={iconBusy}
                  onClick={() => iconInputRef.current?.click()}
                >
                  {build.iconUrl ? 'Сменить иконку' : 'Иконка'}
                </Button>
                {build.iconUrl ? (
                  <Button
                    variant="ghost"
                    iconOnly
                    aria-label="Удалить иконку"
                    title="Удалить иконку"
                    onClick={() => void removeIcon()}
                  >
                    <Trash2 aria-hidden="true" />
                  </Button>
                ) : null}
              </>
            ) : null}
            <Button
              icon={<CheckCircle2 aria-hidden="true" />}
              loading={validateBusy}
              onClick={() => void runValidation()}
            >
              Проверить
            </Button>
            {canPublish && build.status === 'published' ? (
              <Button
                icon={<EyeOff aria-hidden="true" />}
                loading={busy}
                onClick={() => setUnpublishOpen(true)}
              >
                Снять с публикации
              </Button>
            ) : null}
            {canPublish ? (
              <Button
                variant="primary"
                icon={<Rocket aria-hidden="true" />}
                onClick={() => {
                  setReleaseForm({ version: '', changelog: '', channel: 'stable' });
                  setReleaseModal(true);
                }}
              >
                Новый релиз
              </Button>
            ) : null}
          </div>
        }
      />

      {build.status === 'published' && build.visibility !== 'public' ? (
        <Alert tone="warn">
          <div className="row row--between" style={{ gap: 12, flexWrap: 'wrap' }}>
            <span>
              Сборка опубликована, но не видна в MyLauncher: каталог лаунчера без авторизации
              показывает только публичные сборки, а stock-клиент не передаёт токен. Сделайте сборку
              публичной, чтобы она появилась в каталоге.
            </span>
            {canEdit ? (
              <Button size="sm" loading={busy} onClick={() => void makeBuildPublic()}>
                Сделать публичной
              </Button>
            ) : null}
          </div>
        </Alert>
      ) : null}

      <div className="stats-grid">
        <StatCard
          label="Файлы"
          icon={<Package aria-hidden="true" />}
          value={fileStats?.files ?? build.fileCount}
          sub={`${fileStats?.folders ?? 0} ${pluralize(fileStats?.folders ?? 0, 'папка', 'папки', 'папок')}`}
        />
        <StatCard
          label="Размер"
          value={formatBytes(fileStats?.totalBytes ?? build.totalBytes)}
          sub="артефакты сборки"
        />
        <StatCard
          label="Текущий релиз"
          value={build.currentReleaseVersion ?? '—'}
          sub={build.currentReleaseId ? 'опубликован' : 'нет публикации'}
        />
        <StatCard
          label="Java"
          value={build.javaMajor}
          sub={build.javaMin === build.javaMajor ? 'минимум совпадает' : `минимум ${build.javaMin}`}
        />
      </div>

      {report ? (
        <div className="validation-report">
          <div className="row row--between" style={{ marginBottom: 10 }}>
            <SectionTitle
              title="Результат проверки"
              meta={
                report.valid
                  ? `файлов ${report.fileCount}, ошибок нет`
                  : `ошибок ${report.errors.length}, предупреждений ${report.warnings.length}`
              }
            />
            <Button size="sm" variant="ghost" onClick={() => setReport(null)}>
              Скрыть
            </Button>
          </div>
          {report.errors.length > 0 ? (
            <div className="stack" style={{ gap: 6 }}>
              {report.errors.map((issue) => (
                <div className="issue issue--error" key={`e-${issue.code}-${issue.path ?? ''}`}>
                  <XCircle aria-hidden="true" />
                  <div>
                    <strong>{issue.code}</strong> — {issue.message}
                    {issue.path ? <span className="mono"> ({issue.path})</span> : null}
                  </div>
                </div>
              ))}
            </div>
          ) : null}
          {report.warnings.length > 0 ? (
            <div className="stack" style={{ gap: 6, marginTop: report.errors.length ? 8 : 0 }}>
              {report.warnings.map((issue) => (
                <div className="issue issue--warn" key={`w-${issue.code}-${issue.path ?? ''}`}>
                  <AlertTriangle aria-hidden="true" />
                  <div>
                    <strong>{issue.code}</strong> — {issue.message}
                  </div>
                </div>
              ))}
            </div>
          ) : null}
          {report.valid && report.warnings.length === 0 ? (
            <div className="issue issue--ok">
              <CheckCircle2 aria-hidden="true" />
              <div>Сборка готова к публикации.</div>
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="build-tabs">
        <Segmented
          value={tab}
          onChange={(value) => setTab(value as Tab)}
          options={[
            { value: 'files', label: 'Файлы' },
            { value: 'releases', label: 'Релизы' },
            { value: 'history', label: 'История' },
          ]}
        />
      </div>

      <div className="tab-panel" key={tab}>
        {tab === 'files' ? (
          <BuildFileManager
            buildId={id}
            canEdit={canEdit}
            canUpload={canUpload}
            chunkMaxBytes={uploadConfig.data?.chunkMaxBytes}
            maxUploadBytes={uploadConfig.data?.maxUploadBytes}
            onChanged={invalidate}
          />
        ) : null}
        {tab === 'releases' ? (
          <>
            {releasesQuery.isLoading ? (
              <LoadingInline label="Загрузка релизов" />
            ) : releases.length === 0 ? (
              <div className="card">
                <EmptyState
                  icon={<Rocket aria-hidden="true" />}
                  title="Релизов пока нет"
                  action={
                    canPublish ? (
                      <Button
                        variant="primary"
                        icon={<Rocket aria-hidden="true" />}
                        onClick={() => {
                          setReleaseForm({ version: '', changelog: '', channel: 'stable' });
                          setReleaseModal(true);
                        }}
                      >
                        Создать релиз
                      </Button>
                    ) : undefined
                  }
                >
                  Релиз фиксирует снимок файлов сборки. После публикации изменить его нельзя.
                </EmptyState>
              </div>
            ) : (
              <div className="stack">
                {releases.map((release) => (
                  <article className="card release-card" key={release.id}>
                    <div className="row row--between" style={{ gap: 12, flexWrap: 'wrap' }}>
                      <div>
                        <div className="row" style={{ gap: 8, alignItems: 'center' }}>
                          <span className="release-card__version mono">{release.version}</span>
                          {releaseBadge(release.status)}
                          <Badge tone={release.channel === 'beta' ? 'warn' : 'ok'} plain>
                            {release.channel === 'beta' ? 'beta' : 'stable'}
                          </Badge>
                          {release.isCurrent ? (
                            <Badge tone="ok" plain>
                              текущий
                            </Badge>
                          ) : null}
                        </div>
                        <div className="tiny muted" style={{ marginTop: 4 }}>
                          {release.fileCount}{' '}
                          {pluralize(release.fileCount, 'файл', 'файла', 'файлов')} ·{' '}
                          {formatBytes(release.totalBytes)} ·{' '}
                          {release.publishedAt
                            ? `опубликован ${formatDate(release.publishedAt)}`
                            : `создан ${formatDate(release.createdAt)}`}
                        </div>
                      </div>
                      <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
                        <Button size="sm" onClick={() => void openManifest(release)}>
                          Манифест
                        </Button>
                        {canPublish &&
                        release.status !== 'published' &&
                        release.status !== 'archived' ? (
                          <Button
                            size="sm"
                            variant="primary"
                            icon={<Play aria-hidden="true" />}
                            loading={busy}
                            onClick={() => void publishRelease(release)}
                          >
                            Опубликовать
                          </Button>
                        ) : null}
                        {canPublish && release.status === 'published' && !release.isCurrent ? (
                          <Button
                            size="sm"
                            onClick={() =>
                              void api.activateRelease(id, release.id).then(invalidate)
                            }
                          >
                            Сделать текущим
                          </Button>
                        ) : null}
                        {canPublish && release.status === 'published' ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() =>
                              void api.deprecateRelease(id, release.id).then(invalidate)
                            }
                          >
                            Устарел
                          </Button>
                        ) : null}
                      </div>
                    </div>
                    {release.changelog ? (
                      <pre className="release-card__log">{release.changelog}</pre>
                    ) : null}
                  </article>
                ))}

                {releases.length >= 2 ? (
                  <div className="card">
                    <SectionTitle
                      title="Сравнение релизов"
                      meta="добавленные, удалённые и изменённые файлы"
                    />
                    <div className="row compare-row" style={{ gap: 8, flexWrap: 'wrap' }}>
                      <select
                        className="select"
                        value={compareLeft}
                        onChange={(event) => setCompareLeft(event.target.value)}
                        aria-label="Первый релиз"
                      >
                        <option value="">Слева…</option>
                        {releases.map((release) => (
                          <option key={release.id} value={release.id}>
                            {release.version}
                          </option>
                        ))}
                      </select>
                      <select
                        className="select"
                        value={compareRight}
                        onChange={(event) => setCompareRight(event.target.value)}
                        aria-label="Второй релиз"
                      >
                        <option value="">Справа…</option>
                        {releases.map((release) => (
                          <option key={release.id} value={release.id}>
                            {release.version}
                          </option>
                        ))}
                      </select>
                      <Button
                        size="sm"
                        disabled={!compareLeft || !compareRight || compareLeft === compareRight}
                        onClick={() => void runCompare()}
                      >
                        Сравнить
                      </Button>
                    </div>
                    {compareResult ? (
                      <div className="stack" style={{ gap: 10, marginTop: 12 }}>
                        {(['added', 'changed', 'removed'] as const).map((key) => {
                          const list = Array.isArray(compareResult[key])
                            ? (compareResult[key] as Array<{ path: string; size: number }>)
                            : [];
                          const label =
                            key === 'added'
                              ? 'Добавлены'
                              : key === 'changed'
                                ? 'Изменены'
                                : 'Удалены';
                          return (
                            <div key={key}>
                              <div className="tiny muted">
                                {label}: {list.length}
                              </div>
                              {list.length > 0 ? (
                                <div className="stack" style={{ gap: 2 }}>
                                  {list.map((entry) => (
                                    <div className="mono tiny" key={`${key}-${entry.path}`}>
                                      {entry.path} · {formatBytes(entry.size)}
                                    </div>
                                  ))}
                                </div>
                              ) : null}
                            </div>
                          );
                        })}
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            )}
          </>
        ) : null}

        {tab === 'history' ? (
          historyQuery.isLoading ? (
            <LoadingInline label="Загрузка истории" />
          ) : (historyQuery.data ?? []).length === 0 ? (
            <div className="card">
              <EmptyState icon={<History aria-hidden="true" />} title="История пуста">
                События появятся после создания релизов и публикаций.
              </EmptyState>
            </div>
          ) : (
            <div className="table-card">
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Событие</th>
                      <th>Автор</th>
                      <th>Детали</th>
                      <th>Когда</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(historyQuery.data ?? []).map((row: ReleaseHistoryRow) => (
                      <tr key={row.id}>
                        <td className="mono tiny">{row.event}</td>
                        <td className="tiny">{row.actorUsername || 'system'}</td>
                        <td className="tiny muted">
                          {typeof row.details === 'object' && row.details !== null
                            ? Object.entries(row.details as Record<string, unknown>)
                                .map(([key, value]) => `${key}=${String(value)}`)
                                .join(', ')
                            : '—'}
                        </td>
                        <td className="tiny muted" title={formatDate(row.createdAt)}>
                          {relativeTime(row.createdAt)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )
        ) : null}
      </div>

      <ConfirmDialog
        open={unpublishOpen}
        title="Снять сборку с публикации?"
        description={`Сборка «${build.name}» и её релизы перестанут отдаваться лаунчеру и исчезнут из каталога. Файлы и снимки релизов сохранятся — публикацию можно вернуть, опубликовав релиз снова.`}
        confirmLabel="Снять с публикации"
        busy={busy}
        onConfirm={() => void unpublishBuild()}
        onCancel={() => setUnpublishOpen(false)}
      />

      <ConfirmDialog
        open={publishWarningsFor !== null}
        title="Опубликовать с предупреждениями?"
        description={
          publishWarningsFor
            ? `Проверка вернула предупреждений: ${report?.warnings.length ?? 0}. ${
                report?.warnings
                  .slice(0, 3)
                  .map((warning) => warning.message)
                  .join(' ') ?? ''
              } Публикация с предупреждениями может сделать сборку неполной.`
            : ''
        }
        confirmLabel="Опубликовать"
        busy={busy}
        onConfirm={() => void confirmPublishWarnings()}
        onCancel={() => setPublishWarningsFor(null)}
      />

      <Modal
        open={releaseModal}
        onClose={() => setReleaseModal(false)}
        title="Новый релиз"
        description="Релиз фиксирует текущие файлы сборки. Версия генерируется автоматически, если оставить поле пустым."
        footer={
          <>
            <Button onClick={() => setReleaseModal(false)}>Отмена</Button>
            <Button variant="primary" loading={busy} onClick={() => void createRelease()}>
              Создать релиз
            </Button>
          </>
        }
      >
        <div className="stack">
          <Field label="Версия" htmlFor="release-version" hint="Например, 1.0.1">
            <input
              id="release-version"
              className="input mono"
              value={releaseForm.version}
              maxLength={32}
              onChange={(event) =>
                setReleaseForm((prev) => ({ ...prev, version: event.target.value }))
              }
            />
          </Field>
          <Field
            label="Канал"
            htmlFor="release-channel"
            hint="latest выбирается отдельно для каждого канала"
          >
            <select
              id="release-channel"
              className="select"
              value={releaseForm.channel}
              onChange={(event) =>
                setReleaseForm((prev) => ({
                  ...prev,
                  channel: event.target.value === 'beta' ? 'beta' : 'stable',
                }))
              }
            >
              <option value="stable">Стабильный (stable)</option>
              <option value="beta">Бета (beta)</option>
            </select>
          </Field>
          <Field label="Changelog" htmlFor="release-changelog">
            <textarea
              id="release-changelog"
              className="textarea"
              value={releaseForm.changelog}
              maxLength={8000}
              onChange={(event) =>
                setReleaseForm((prev) => ({ ...prev, changelog: event.target.value }))
              }
            />
          </Field>
        </div>
      </Modal>

      <Modal
        open={manifestTarget !== null}
        onClose={() => setManifestTarget(null)}
        wide
        title="Манифест релиза"
        description={
          manifestTarget
            ? `${build.slug} · ${manifestTarget.version} · канал ${manifestTarget.channel}`
            : ''
        }
        footer={
          <>
            <Button
              icon={<Copy aria-hidden="true" />}
              onClick={async () => {
                if (!manifestTarget) return;
                const url = `${window.location.origin}/api/packs/${build.slug}/versions/${encodeURIComponent(manifestTarget.version)}/manifest?channel=${manifestTarget.channel}`;
                try {
                  await navigator.clipboard.writeText(url);
                  toast('URL манифеста скопирован.', 'success');
                } catch {
                  toast(url, 'warning');
                }
              }}
            >
              Копировать URL
            </Button>
            <Button
              icon={<Download aria-hidden="true" />}
              onClick={() => {
                const payload = manifestKind === 'launcher' ? launcherManifest : manifest;
                if (!payload) return;
                const blob = new Blob([JSON.stringify(payload, null, 2)], {
                  type: 'application/json',
                });
                const url = URL.createObjectURL(blob);
                const link = document.createElement('a');
                link.href = url;
                link.download = `${build.slug}-${manifestTarget?.version ?? 'release'}-${manifestKind}.json`;
                link.click();
                URL.revokeObjectURL(url);
              }}
            >
              Скачать JSON
            </Button>
            <Button variant="primary" onClick={() => setManifestTarget(null)}>
              Закрыть
            </Button>
          </>
        }
      >
        <div className="row" style={{ gap: 6, marginBottom: 10, flexWrap: 'wrap' }}>
          <Button
            size="sm"
            variant={manifestKind === 'launcher' ? 'primary' : 'ghost'}
            onClick={() => setManifestKind('launcher')}
          >
            MyLauncher (manifestVersion 1)
          </Button>
          <Button
            size="sm"
            variant={manifestKind === 'internal' ? 'primary' : 'ghost'}
            onClick={() => setManifestKind('internal')}
          >
            Внутренний (schema_version 1.0)
          </Button>
          <Badge tone={manifestTarget?.channel === 'beta' ? 'warn' : 'ok'} plain>
            {manifestTarget?.channel === 'beta' ? 'beta' : 'stable'}
          </Badge>
        </div>
        {manifestKind === 'launcher' ? (
          launcherManifest === null ? (
            <Alert tone="danger">
              Манифест MyLauncher недоступен: сборка не проходит launcher-ready проверку. Запустите
              проверку на вкладке файлов.
            </Alert>
          ) : (
            <>
              <div className="tiny muted" style={{ marginBottom: 8 }}>
                Файлов: {(launcherManifest.files as unknown[] | undefined)?.length ?? 0}. Это точный
                JSON, который получит лаунчер.
              </div>
              <pre className="cloud-preview-text">{JSON.stringify(launcherManifest, null, 2)}</pre>
            </>
          )
        ) : manifest === null ? (
          <LoadingInline label="Формирование манифеста" />
        ) : (
          <>
            <div className="tiny muted" style={{ marginBottom: 8 }}>
              Файлов в манифесте: {manifestFiles.length}
            </div>
            <pre className="cloud-preview-text">{JSON.stringify(manifest, null, 2)}</pre>
          </>
        )}
      </Modal>
    </>
  );
}
