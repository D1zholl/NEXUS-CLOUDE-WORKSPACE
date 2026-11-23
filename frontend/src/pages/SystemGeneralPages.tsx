import { useQuery } from '@tanstack/react-query';
import { Globe, ShieldAlert, Network } from 'lucide-react';

import { Alert, Badge, LoadingInline, SectionTitle, StatCard } from '../components/ui';
import { api, errorMessage } from '../services/api';
import { formatNumber } from '../lib/format';
import { SettingsGroupPage } from './SystemSettingsPage';

/** Общие настройки: название инстанса, часовой пояс, значения по умолчанию. */
export function SystemGeneralPage() {
  const overview = useQuery({
    queryKey: ['system-overview'],
    queryFn: api.overview,
    refetchInterval: 30_000,
  });

  if (overview.isPending) return <LoadingInline label="Загрузка состояния системы" />;
  if (overview.isError || !overview.data) return <Alert>{errorMessage(overview.error)}</Alert>;

  const data = overview.data;

  return (
    <SettingsGroupPage
      eyebrow="Система"
      title="Общие настройки"
      description="Параметры по умолчанию для модулей и системные значения. Показаны эффективные значения; изменения применяются без перезапуска, кроме помеченных."
      keys={['cloud.upload.concurrency', 'jobs.max_concurrent', 'appearance.background.quality']}
      extra={
        <div className="card">
          <SectionTitle title="Проверка конфигурации" meta="из /system/overview" />
          <div className="stats-grid" style={{ marginBottom: 0 }}>
            <StatCard
              label="Версия"
              icon={<Globe aria-hidden="true" />}
              value={`NEXUS ${data.version}`}
              sub={`БД: ${data.database.status}`}
            />
            <StatCard
              label="Пользователи"
              icon={<Network aria-hidden="true" />}
              value={formatNumber(data.users.active)}
              sub={`из ${formatNumber(data.users.total)} активны`}
            />
            <StatCard
              label="Сессии"
              icon={<Globe aria-hidden="true" />}
              value={formatNumber(data.sessions.active)}
              sub="Активные серверные сессии"
            />
          </div>
          <Alert tone={data.database.status === 'up' ? 'info' : 'danger'}>
            Полная проверка выполняется командой <code>nexus check</code> на сервере. Состояние базы
            данных и статики отражено выше.
          </Alert>
        </div>
      }
    />
  );
}

/** Ограничения ресурсов: лимиты загрузки, TTL, квоты, параллельность задач. */
export function SystemLimitsPage() {
  return (
    <SettingsGroupPage
      eyebrow="Система"
      title="Ограничения ресурсов"
      description="Лимиты загрузки и хранения, сроки жизни временных объектов и квоты по умолчанию."
      keys={[
        'limits.max_upload_mb',
        'limits.chunk_max_mb',
        'limits.upload_ttl_hours',
        'limits.share_ttl_hours',
        'limits.export_ttl_hours',
        'limits.trash_retention_days',
        'limits.quota_cloud_gb',
        'limits.quota_minecraft_gb',
        'jobs.max_concurrent',
      ]}
    />
  );
}

/** Сетевые настройки: CORS, доверие прокси, текущие соединения. */
export function SystemNetworkPage() {
  const monitoring = useQuery({
    queryKey: ['monitoring-current'],
    queryFn: api.monitoringCurrent,
    refetchInterval: 10_000,
  });

  return (
    <SettingsGroupPage
      eyebrow="Система"
      title="Сетевые настройки"
      description="Разрешённые источники и доверие заголовкам обратного прокси. network.* применяются при старте службы; host и port меняются в nexus.toml с перезапуском."
      keys={['network.allowed_origins', 'network.trust_proxy']}
      extra={
        <div className="card">
          <SectionTitle title="Текущие соединения" meta="опрос каждые 10 секунд" />
          {monitoring.isPending ? (
            <LoadingInline label="Загрузка" />
          ) : monitoring.isError || !monitoring.data ? (
            <Alert>{errorMessage(monitoring.error)}</Alert>
          ) : (
            <div className="info-list">
              <div className="info-list__row">
                <span>Активные соединения (HTTP + WS)</span>
                <span>{formatNumber(monitoring.data.network.activeConnections ?? 0)}</span>
              </div>
              <div className="info-list__row">
                <span>Активные WebSocket</span>
                <span>{formatNumber(monitoring.data.app.activeWebSockets ?? 0)}</span>
              </div>
              <div className="info-list__row">
                <span>Активные скачивания</span>
                <span>{formatNumber(monitoring.data.app.activeDownloads ?? 0)}</span>
              </div>
              <div className="info-list__row">
                <span>Входящий / исходящий трафик</span>
                <span>
                  {formatNumber(Math.round(monitoring.data.network.rxRate / 1024))} КБ/с ·{' '}
                  {formatNumber(Math.round(monitoring.data.network.txRate / 1024))} КБ/с
                </span>
              </div>
            </div>
          )}
          <Alert tone="warn">
            <ShieldAlert aria-hidden="true" size={14} /> Изменение адреса и порта сервера требует
            перезапуска службы и недоступно из интерфейса.
          </Alert>
          <Badge tone="info" plain>
            Подсчёт соединений = in-flight HTTP + WebSocket; TCP-соединения ОС не считаются.
          </Badge>
        </div>
      }
    />
  );
}
