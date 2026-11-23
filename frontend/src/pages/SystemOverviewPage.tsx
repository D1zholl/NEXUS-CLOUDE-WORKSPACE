import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import {
  Activity,
  Bot,
  Boxes,
  Cloud,
  Cpu,
  Database,
  HardDrive,
  MemoryStick,
  RefreshCw,
  Server,
  UsersRound,
} from 'lucide-react';

import {
  Badge,
  Button,
  EmptyState,
  LoadingInline,
  PageHead,
  SectionTitle,
  StatCard,
  Switch,
  Alert,
} from '../components/ui';
import { api, errorMessage, type Overview } from '../services/api';
import { Link } from 'react-router-dom';
import { formatBytes, formatDate, formatNumber, formatPercent, formatUptime } from '../lib/format';
import { useAuth, useCan } from '../stores/auth';
import { toast } from '../stores/ui';

const MODULE_LABELS: Record<string, { name: string; description: string; icon: ReactNode }> = {
  minecraft: {
    name: 'Minecraft Build Manager',
    description: 'Сборки, релизы и артефакты Minecraft.',
    icon: <Boxes aria-hidden="true" />,
  },
  cloud: {
    name: 'NEXUS Cloud Storage',
    description: 'Файлы, папки и подключённые диски.',
    icon: <Cloud aria-hidden="true" />,
  },
  ai: {
    name: 'NEXUS AI Workspace',
    description: 'AI Chat, Agent задачи и NEXUS AI Runtime.',
    icon: <Bot aria-hidden="true" />,
  },
};

export function SystemOverviewPage() {
  const queryClient = useQueryClient();
  const canManageSettings = useCan('system.settings.manage');
  const canManageStorage = useCan('system.storage.manage');
  const overview = useQuery({
    queryKey: ['system-overview'],
    queryFn: api.overview,
    refetchInterval: 15_000,
  });
  const storage = useQuery({
    queryKey: ['storage-summary'],
    queryFn: api.storageSummary,
    enabled: canManageStorage,
    refetchInterval: 60_000,
  });

  const toggleModule = useMutation({
    mutationFn: ({ key, enabled }: { key: string; enabled: boolean }) =>
      api.setModule(key, enabled),
    onSuccess: async (row) => {
      toast(
        `Модуль «${MODULE_LABELS[row.key]?.name ?? row.key}» ${row.enabled ? 'включён' : 'отключён'}.`,
        'success',
      );
      await queryClient.invalidateQueries({ queryKey: ['system-overview'] });
      await useAuth.getState().refresh();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  if (overview.isPending) {
    return <LoadingInline label="Сбор показателей системы" />;
  }

  if (overview.isError || !overview.data) {
    return (
      <>
        <PageHead title="Состояние системы" eyebrow="Система" />
        <Alert>{errorMessage(overview.error)}</Alert>
        <div>
          <Button onClick={() => void overview.refetch()} icon={<RefreshCw aria-hidden="true" />}>
            Повторить запрос
          </Button>
        </div>
      </>
    );
  }

  const data: Overview = overview.data;
  const memoryPercent =
    data.system.memoryTotal > 0 ? (data.system.memoryUsed / data.system.memoryTotal) * 100 : 0;

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Состояние системы"
        description="Реальные показатели процесса, базы данных и подключённых дисков. Обновление каждые 15 секунд."
        actions={
          <>
            <Badge tone={data.database.status === 'up' ? 'ok' : 'danger'}>
              База данных {data.database.status === 'up' ? 'доступна' : 'недоступна'}
            </Badge>
            <Button
              size="sm"
              variant="ghost"
              icon={<RefreshCw aria-hidden="true" />}
              onClick={() => void overview.refetch()}
              loading={overview.isFetching}
            >
              Обновить
            </Button>
          </>
        }
      />

      <div className="stats-grid">
        <StatCard
          label="Загрузка CPU"
          icon={<Cpu aria-hidden="true" />}
          value={formatPercent(data.system.cpuPercent)}
          progress={data.system.cpuPercent}
          tone={data.system.cpuPercent > 85 ? 'warn' : 'default'}
          sub={`${formatNumber(data.system.cpuCores)} логических ядер`}
        />
        <StatCard
          label="Память"
          icon={<MemoryStick aria-hidden="true" />}
          value={formatBytes(data.system.memoryUsed)}
          unit={`из ${formatBytes(data.system.memoryTotal)}`}
          progress={memoryPercent}
          tone={memoryPercent > 88 ? 'warn' : 'default'}
          sub={`Процесс NEXUS: ${formatBytes(data.system.processMemory)}`}
        />
        <StatCard
          label="Активные сессии"
          icon={<UsersRound aria-hidden="true" />}
          value={formatNumber(data.sessions.active)}
          sub={`${formatNumber(data.users.active)} из ${formatNumber(data.users.total)} пользователей активны`}
        />
        <StatCard
          label="События аудита"
          icon={<Activity aria-hidden="true" />}
          value={formatNumber(data.audit.events)}
          sub="Записей в журнале за всё время"
        />
      </div>

      <div className="grid-2">
        <div className="card">
          <SectionTitle title="Сервер" meta={`NEXUS ${data.version}`} />
          <div className="info-list">
            <InfoRow label="Хост" value={data.system.hostname} />
            <InfoRow
              label="Операционная система"
              value={`${data.system.osName} ${data.system.osVersion}`}
            />
            <InfoRow label="Ядро" value={data.system.kernelVersion} />
            <InfoRow label="Запущен" value={formatDate(data.startedAt)} />
            <InfoRow label="Непрерывная работа" value={formatUptime(data.uptimeSeconds)} />
            <InfoRow label="Размер базы данных" value={formatBytes(data.database.sizeBytes)} />
            <InfoRow
              label="CPU / память"
              value={`${formatPercent(data.system.cpuPercent)} · ${formatBytes(data.system.memoryUsed)}`}
            />
          </div>
        </div>

        <div className="card">
          <SectionTitle
            title="Модули"
            meta={canManageSettings ? 'Переключение доступно' : 'Только просмотр'}
          />
          <div className="stack" style={{ gap: 14 }}>
            {Object.entries(MODULE_LABELS).map(([key, meta]) => {
              const enabled = data.modules[key] ?? true;
              const pending = toggleModule.isPending && toggleModule.variables?.key === key;
              return (
                <div
                  key={key}
                  className="row row--between"
                  style={{
                    gap: 14,
                    padding: '12px 14px',
                    borderRadius: 12,
                    border: '1px solid var(--border-soft)',
                    background: 'rgba(255,255,255,0.02)',
                  }}
                >
                  <div className="row" style={{ gap: 11, minWidth: 0 }}>
                    <span
                      className="module-card__icon"
                      style={{ width: 36, height: 36, marginBottom: 0 }}
                    >
                      {meta.icon}
                    </span>
                    <div style={{ minWidth: 0 }}>
                      <div style={{ fontSize: 13, fontWeight: 600 }}>{meta.name}</div>
                      <div className="tiny muted">{meta.description}</div>
                    </div>
                  </div>
                  <Switch
                    checked={enabled}
                    disabled={!canManageSettings || pending}
                    label={enabled ? 'Вкл' : 'Выкл'}
                    onChange={(next) => toggleModule.mutate({ key, enabled: next })}
                  />
                </div>
              );
            })}
          </div>
          <div className="tiny muted" style={{ marginTop: 14, lineHeight: 1.6 }}>
            Отключение скрывает модуль в навигации, но не удаляет данные. Изменение фиксируется в
            журнале.
          </div>
        </div>
      </div>

      <div className="card">
        <SectionTitle title="Диски и хранилища" meta={`${data.system.disks.length} устройств`} />
        {data.system.disks.length === 0 ? (
          <EmptyState icon={<HardDrive aria-hidden="true" />} title="Диски не обнаружены">
            Сервер не вернул ни одного доступного устройства хранения.
          </EmptyState>
        ) : (
          <div className="disk-list">
            {data.system.disks.map((disk) => {
              const used = Math.max(0, disk.totalBytes - disk.availableBytes);
              const percent = disk.totalBytes > 0 ? (used / disk.totalBytes) * 100 : 0;
              return (
                <div className="disk" key={`${disk.mountPoint}-${disk.name}`}>
                  <div>
                    <div className="disk__name">
                      {disk.name || disk.mountPoint}{' '}
                      {disk.removable ? (
                        <Badge tone="warn" plain>
                          съёмный
                        </Badge>
                      ) : null}
                    </div>
                    <div className="disk__meta">
                      {disk.fileSystem || 'файловая система неизвестна'} · {disk.mountPoint}
                    </div>
                  </div>
                  <div className="disk__num">
                    {formatBytes(used)} из {formatBytes(disk.totalBytes)}
                    <br />
                    <span className={percent > 90 ? 'text-danger' : 'muted'}>
                      {percent > 90 ? 'Мало места' : `${formatBytes(disk.availableBytes)} свободно`}
                    </span>
                  </div>
                  <div className="meter" aria-hidden="true">
                    <div
                      className={`meter__fill${percent > 90 ? ' meter__fill--warn' : ''}`}
                      style={{ width: `${Math.min(100, percent)}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {canManageStorage && storage.data ? (
        <div className="card">
          <SectionTitle
            title="Объектное хранение"
            meta={`${storage.data.online} из ${storage.data.volumes} онлайн`}
            actions={
              <Link to="/system/storage" className="btn btn--sm">
                Управление
              </Link>
            }
          />
          <div className="info-list">
            <div className="info-list__row">
              <span>Объектов зафиксировано</span>
              <span>
                {storage.data.objects} · {formatBytes(storage.data.objectsBytes)}
              </span>
            </div>
            <div className="info-list__row">
              <span>Свободно на зарегистрированных дисках</span>
              <span>{formatBytes(storage.data.availableBytes)}</span>
            </div>
          </div>
        </div>
      ) : null}

      <div className="row row--between" style={{ fontSize: 11.5, color: 'var(--text-secondary)' }}>
        <span>
          <Server
            aria-hidden="true"
            style={{ display: 'inline', width: 13, height: 13, marginRight: 6, verticalAlign: -2 }}
          />
          Снимок метрик: {formatDate(data.system.capturedAt)}
        </span>
        <span>
          <Database
            aria-hidden="true"
            style={{ display: 'inline', width: 13, height: 13, marginRight: 6, verticalAlign: -2 }}
          />
          PostgreSQL · {formatBytes(data.database.sizeBytes)}
        </span>
      </div>
    </>
  );
}

function InfoRow({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="info-list__row">
      <span>{label}</span>
      <span>{value}</span>
    </div>
  );
}
