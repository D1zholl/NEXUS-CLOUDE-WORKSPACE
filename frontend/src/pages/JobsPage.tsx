import { useEffect, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Ban,
  CheckCircle2,
  Clock,
  Cog,
  Eraser,
  FileSearch,
  History,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  XCircle,
} from 'lucide-react';

import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  LoadingInline,
  PageHead,
  SectionTitle,
  Segmented,
  StatCard,
} from '../components/ui';
import { api, errorMessage, type BackgroundJob, type JobsResponse } from '../services/api';
import { realtime, type RealtimeEvent } from '../lib/realtime';
import { applyJobEvent } from '../lib/realtimeEvents';
import { formatDate, formatNumber, pluralize, relativeTime } from '../lib/format';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

const TYPE_LABELS: Record<string, string> = {
  'cloud.export': 'ZIP-архив Cloud',
  'system.storage.check': 'Проверка целостности объектов',
  'system.objects.verify': 'Сверка SHA-256 объектов',
  'system.uploads.cleanup': 'Очистка временных загрузок',
  'system.shares.cleanup': 'Удаление истёкших ссылок',
  'system.metrics.prune': 'Очистка старых метрик',
  'system.idempotency.cleanup': 'Очистка ключей идемпотентности',
};

/** Причина, по которой управляющие действия недоступны. */
function actionHint(job: BackgroundJob): string | null {
  if (!['pending', 'running', 'paused'].includes(job.status)) {
    return null;
  }
  if (!job.cancellable) {
    return 'Задача критична для системы: пауза и отмена недоступны.';
  }
  if (job.status === 'running' && job.pauseRequested) {
    return 'Пауза уже запрошена: задача остановится после текущей единицы работы.';
  }
  return null;
}

const MAINTENANCE = [
  {
    kind: 'storage_check',
    title: 'Проверка целостности',
    description: 'Сверить объекты хранилища с диском: наличие и размер файлов.',
    icon: <FileSearch aria-hidden="true" />,
  },
  {
    kind: 'cleanup_uploads',
    title: 'Очистка загрузок',
    description: 'Удалить просроченные сессии Cloud и Minecraft, а также старые архивы.',
    icon: <Eraser aria-hidden="true" />,
  },
  {
    kind: 'cleanup_shares',
    title: 'Истёкшие ссылки',
    description: 'Удалить старые публичные ссылки и отозванные токены лаунчера.',
    icon: <History aria-hidden="true" />,
  },
  {
    kind: 'metrics_prune',
    title: 'Старые метрики',
    description: 'Удалить метрики и завершённые задачи старше срока хранения.',
    icon: <Cog aria-hidden="true" />,
  },
];

function statusBadge(status: string) {
  if (status === 'completed') return <Badge tone="ok">Завершена</Badge>;
  if (status === 'running') return <Badge tone="info">Выполняется</Badge>;
  if (status === 'failed') return <Badge tone="danger">Ошибка</Badge>;
  if (status === 'cancelled') return <Badge tone="warn">Отменена</Badge>;
  if (status === 'paused') return <Badge tone="warn">Пауза</Badge>;
  return <Badge plain>В очереди</Badge>;
}

export function JobsPage() {
  const queryClient = useQueryClient();
  const canManage = useCan('system.jobs.manage');
  const [filter, setFilter] = useState('');
  const [cancelTarget, setCancelTarget] = useState<BackgroundJob | null>(null);
  const [busy, setBusy] = useState(false);
  const [jobBusy, setJobBusy] = useState<string | null>(null);

  const jobs = useQuery({
    queryKey: ['jobs', filter],
    queryFn: () => api.listJobs({ status: filter || undefined, limit: 100 }),
    refetchInterval: 4000,
  });

  // Realtime дополняет опрос: статус и прогресс обновляются немедленно.
  // Терминальные события дополнительно синхронизируют счётчики очереди.
  useEffect(() => {
    const patch = (event: RealtimeEvent): void => {
      queryClient.setQueriesData<JobsResponse>({ queryKey: ['jobs'] }, (current) =>
        current ? applyJobEvent(current, event) : current,
      );
    };
    const patchAndSync = (event: RealtimeEvent): void => {
      patch(event);
      void queryClient.invalidateQueries({ queryKey: ['jobs'] });
    };
    const unsubscribes = [
      realtime.on('job.progress', patch),
      realtime.on('job.paused', patch),
      realtime.on('job.resumed', patch),
      realtime.on('job.completed', patchAndSync),
      realtime.on('job.failed', patchAndSync),
      realtime.on('job.cancelled', patchAndSync),
    ];
    return () => {
      for (const unsubscribe of unsubscribes) unsubscribe();
    };
  }, [queryClient]);

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: ['jobs'] });
  };

  const start = async (kind: string): Promise<void> => {
    setBusy(true);
    try {
      await api.runMaintenance(kind);
      invalidate();
      toast('Задача поставлена в очередь.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const pause = async (job: BackgroundJob): Promise<void> => {
    setJobBusy(`${job.id}:pause`);
    try {
      await api.pauseJob(job.id);
      invalidate();
      toast(
        job.status === 'pending'
          ? 'Задача приостановлена.'
          : 'Пауза запрошена: задача остановится после текущей единицы работы.',
        'success',
      );
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setJobBusy(null);
    }
  };

  const resume = async (job: BackgroundJob): Promise<void> => {
    setJobBusy(`${job.id}:resume`);
    try {
      await api.resumeJob(job.id);
      invalidate();
      toast('Задача возвращена в очередь.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setJobBusy(null);
    }
  };

  const cancel = async (): Promise<void> => {
    if (!cancelTarget) return;
    setBusy(true);
    setJobBusy(`${cancelTarget.id}:cancel`);
    try {
      await api.cancelJob(cancelTarget.id);
      setCancelTarget(null);
      invalidate();
      toast('Запрошена отмена задачи.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
      setJobBusy(null);
    }
  };

  const data = jobs.data;
  const items = data?.items ?? [];

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Фоновые задачи"
        description={`Очередь обслуживания: проверка объектов, очистка загрузок и ссылок, обслуживание метрик. Одновременно выполняется не более ${data?.maxConcurrent ?? '—'} задач (jobs.max_concurrent). Пауза останавливает задачу после текущей единицы работы.`}
        actions={
          <Button
            variant="ghost"
            icon={<RefreshCw aria-hidden="true" />}
            onClick={() => void jobs.refetch()}
          >
            Обновить
          </Button>
        }
      />

      <div className="stats-grid">
        <StatCard
          label="В очереди"
          icon={<Clock aria-hidden="true" />}
          value={data?.pending ?? 0}
          sub="ожидают исполнителя"
        />
        <StatCard
          label="Выполняется"
          icon={<Loader2 aria-hidden="true" />}
          value={data?.running ?? 0}
          sub="сейчас в работе"
        />
        <StatCard
          label="На паузе"
          icon={<Pause aria-hidden="true" />}
          value={data?.paused ?? 0}
          sub="ждут продолжения"
        />
        <StatCard
          label="Завершено"
          icon={<CheckCircle2 aria-hidden="true" />}
          value={data?.completed ?? 0}
          sub="всего"
        />
        <StatCard
          label="С ошибкой"
          icon={<XCircle aria-hidden="true" />}
          value={data?.failed ?? 0}
          sub="требуют внимания"
        />
        <StatCard
          label="Лимит параллельно"
          icon={<Cog aria-hidden="true" />}
          value={data?.maxConcurrent ?? '—'}
          sub="jobs.max_concurrent"
        />
      </div>

      {canManage ? (
        <>
          <SectionTitle title="Обслуживание" meta="запуск вручную" />
          <div className="job-actions">
            {MAINTENANCE.map((action) => (
              <article className="card job-action" key={action.kind}>
                <div className="job-action__icon">{action.icon}</div>
                <div style={{ minWidth: 0 }}>
                  <div className="job-action__title">{action.title}</div>
                  <div className="tiny muted">{action.description}</div>
                </div>
                <Button
                  size="sm"
                  variant="primary"
                  icon={<Play aria-hidden="true" />}
                  loading={busy}
                  onClick={() => void start(action.kind)}
                >
                  Запустить
                </Button>
              </article>
            ))}
          </div>
        </>
      ) : (
        <Alert tone="info">
          Запуск задач доступен с правом <span className="mono">system.jobs.manage</span>.
        </Alert>
      )}

      <div className="monitor-bar">
        <Segmented
          value={filter}
          onChange={setFilter}
          options={[
            { value: '', label: 'Все' },
            { value: 'running', label: 'Активные' },
            { value: 'paused', label: 'На паузе' },
            { value: 'completed', label: 'Завершённые' },
            { value: 'failed', label: 'С ошибкой' },
          ]}
        />
        <span className="tiny muted">
          Всего: {items.length} {pluralize(items.length, 'задача', 'задачи', 'задач')}
        </span>
      </div>

            {jobs.isLoading ? (
          <LoadingInline label="Загрузка задач" />
        ) : jobs.isError ? (
          <Alert>{errorMessage(jobs.error)}</Alert>
        ) : items.length === 0 ? (
          <div className="card">
            <EmptyState icon={<Cog aria-hidden="true" />} title="Задач нет">
              Запустите обслуживание или создайте ZIP-архив в Cloud Storage — задачи появятся здесь.
            </EmptyState>
          </div>
        ) : (
          <div className="table-card">
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Тип</th>
                    <th>Статус</th>
                    <th style={{ minWidth: 160 }}>Прогресс</th>
                    <th>Автор</th>
                    <th>Создана</th>
                    <th>Длительность</th>
                    <th style={{ textAlign: 'right' }}>Действия</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((job) => {
                    const duration =
                      job.startedAt && job.finishedAt
                        ? Math.max(
                            0,
                            (new Date(job.finishedAt).getTime() -
                              new Date(job.startedAt).getTime()) /
                              1000,
                          )
                        : null;
                    const progress = Math.min(100, Math.max(0, job.progress));
                    return (
                      <tr key={job.id}>
                        <td>
                          <div className="tiny">{TYPE_LABELS[job.jobType] ?? job.jobType}</div>
                          <div className="mono tiny muted">{job.jobType}</div>
                        </td>
                        <td>
                          {statusBadge(job.status)}
                          {job.status === 'running' && job.pauseRequested ? (
                            <div className="tiny muted">пауза после единицы</div>
                          ) : null}
                        </td>
                        <td>
                          <div className="row" style={{ gap: 8 }}>
                            <div className="meter grow" aria-hidden="true">
                              <div
                                className={`meter__fill${job.status === 'failed' ? ' meter__fill--warn' : ''}`}
                                style={{ width: `${progress}%` }}
                              />
                            </div>
                            <span className="mono tiny">{Math.round(progress)}%</span>
                          </div>
                          {job.totalUnits ? (
                            <div className="tiny muted">
                              {formatNumber(job.doneUnits ?? 0)} / {formatNumber(job.totalUnits)}
                            </div>
                          ) : null}
                          {job.error ? (
                            <div className="tiny text-danger" title={job.error}>
                              {job.error.slice(0, 80)}
                            </div>
                          ) : null}
                        </td>
                        <td className="tiny">{job.actor ?? 'system'}</td>
                        <td className="tiny muted" title={formatDate(job.createdAt)}>
                          {relativeTime(job.createdAt)}
                        </td>
                        <td className="mono tiny">
                          {duration === null ? '—' : `${duration.toFixed(1)} с`}
                        </td>
                        <td>
                          <div className="table__actions">
                            {canManage && job.cancellable ? (
                              <>
                                {job.status === 'paused' ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Продолжить"
                                    title="Вернуть задачу в очередь"
                                    disabled={jobBusy !== null}
                                    loading={jobBusy === `${job.id}:resume`}
                                    onClick={() => void resume(job)}
                                  >
                                    <Play aria-hidden="true" />
                                  </Button>
                                ) : ['pending', 'running'].includes(job.status) ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Приостановить"
                                    title={
                                      job.pauseRequested
                                        ? 'Пауза уже запрошена: задача остановится после текущей единицы работы'
                                        : 'Приостановить после текущей единицы работы'
                                    }
                                    disabled={job.pauseRequested || jobBusy !== null}
                                    loading={jobBusy === `${job.id}:pause`}
                                    onClick={() => void pause(job)}
                                  >
                                    <Pause aria-hidden="true" />
                                  </Button>
                                ) : null}
                                {['pending', 'running', 'paused'].includes(job.status) ? (
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    iconOnly
                                    aria-label="Отменить"
                                    title="Отменить задачу"
                                    disabled={jobBusy !== null}
                                    loading={jobBusy === `${job.id}:cancel`}
                                    onClick={() => setCancelTarget(job)}
                                  >
                                    <Ban aria-hidden="true" />
                                  </Button>
                                ) : null}
                              </>
                            ) : null}
                          </div>
                          {canManage && actionHint(job) ? (
                            <div
                              className="tiny muted"
                              style={{ maxWidth: 190, textAlign: 'right' }}
                            >
                              {actionHint(job)}
                            </div>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
                  </div>
      )}

      <ConfirmDialog
        open={cancelTarget !== null}
        title="Отменить задачу?"
        description={
          cancelTarget
            ? `Задача «${TYPE_LABELS[cancelTarget.jobType] ?? cancelTarget.jobType}» будет остановлена. Уже обработанные файлы останутся как есть.`
            : ''
        }
        confirmLabel="Отменить"
        cancelLabel="Оставить"
        busy={busy}
        onConfirm={() => void cancel()}
        onCancel={() => setCancelTarget(null)}
      />
    </>
  );
}

export default JobsPage;
