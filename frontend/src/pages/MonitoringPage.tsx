import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  Activity,
  Cpu,
  Database,
  Download,
  Gauge,
  HardDrive,
  MemoryStick,
  Network,
  RefreshCw,
  Server,
  Upload,
} from 'lucide-react';

import {
  Alert,
  Badge,
  Button,
  LoadingInline,
  PageHead,
  SectionTitle,
  Segmented,
  StatCard,
} from '../components/ui';
import { Sparkline, type SeriesPoint } from '../components/Sparkline';
import { api, errorMessage, type MonitoringCurrent } from '../services/api';
import { formatBytes, formatNumber, formatPercent, formatUptime } from '../lib/format';

const WINDOWS = [
  { value: '5m', label: '5 минут' },
  { value: '15m', label: '15 минут' },
  { value: '1h', label: 'Час' },
  { value: '24h', label: '24 часа' },
  { value: '7d', label: '7 дней' },
];

function rate(value: number): string {
  return `${formatBytes(value)}/с`;
}

export function MonitoringPage() {
  const [range, setRange] = useState('5m');
  const [live, setLive] = useState(true);

  const current = useQuery({
    queryKey: ['monitoring-current'],
    queryFn: () => api.monitoringCurrent(),
    refetchInterval: live ? 3000 : false,
  });

  const history = useQuery({
    queryKey: ['monitoring-history', range],
    queryFn: () => api.monitoringHistory(range),
    refetchInterval: live ? 10000 : false,
  });

  const modules = useQuery({
    queryKey: ['monitoring-modules'],
    queryFn: () => api.monitoringModules(),
    refetchInterval: live ? 15000 : false,
  });

  const data: MonitoringCurrent | undefined = current.data;
  const historySeries = history.data?.series;
  const series = useMemo(() => historySeries ?? {}, [historySeries]);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setTick((value) => value + 1), 3000);
    return () => window.clearInterval(timer);
  }, [live]);

  const cpuPoints: SeriesPoint[] = useMemo(() => series['cpu.percent'] ?? [], [series]);
  const memPoints: SeriesPoint[] = useMemo(() => series['memory.used'] ?? [], [series]);
  const rxPoints: SeriesPoint[] = useMemo(() => series['network.rx_rate'] ?? [], [series]);
  const txPoints: SeriesPoint[] = useMemo(() => series['network.tx_rate'] ?? [], [series]);

  const memoryPercent =
    data && data.memory.total > 0 ? (data.memory.used / data.memory.total) * 100 : 0;
  const diskTotal = data?.disks.reduce((sum, disk) => sum + disk.totalBytes, 0) ?? 0;
  const diskFree = data?.disks.reduce((sum, disk) => sum + disk.availableBytes, 0) ?? 0;
  const diskUsedPercent = diskTotal > 0 ? ((diskTotal - diskFree) / diskTotal) * 100 : 0;

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Мониторинг"
        description="Реальные показатели сервера: процессор, память, диски, сеть, приложение и модули. История — из таблицы метрик."
        actions={
          <div className="row" style={{ gap: 8 }}>
            <Button
              size="sm"
              variant={live ? 'primary' : 'default'}
              icon={<Activity aria-hidden="true" />}
              onClick={() => setLive((value) => !value)}
              aria-pressed={live}
              title={
                live
                  ? 'Автообновление включено — нажмите, чтобы остановить опрос'
                  : 'Автообновление выключено — нажмите, чтобы возобновить опрос'
              }
            >
              {live ? 'Автообновление: вкл' : 'Автообновление: выкл'}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              icon={<RefreshCw aria-hidden="true" />}
              onClick={() => {
                void current.refetch();
                void history.refetch();
                void modules.refetch();
              }}
            >
              Обновить
            </Button>
          </div>
        }
      />

      {current.isError ? <Alert>{errorMessage(current.error)}</Alert> : null}

      <div className="monitor-bar">
        <Segmented
          value={range}
          onChange={setRange}
          options={WINDOWS.map((item) => ({ value: item.value, label: item.label }))}
        />
        <span className="tiny muted" aria-live="polite">
          {live
            ? `автообновление: каждые 3 с · тик ${tick}`
            : 'автообновление выключено — данные не обновляются'}
        </span>
      </div>

            {current.isLoading || !data ? (
        <div className="card">
          <LoadingInline label="Сбор показателей" />
        </div>
      ) : (
          <>
            <div className="stats-grid">
              <StatCard
                label="Процессор"
                icon={<Cpu aria-hidden="true" />}
                value={formatPercent(data.cpu.percent)}
                sub={`${data.cpu.cores} ядер · LA ${data.cpu.loadAverage[0].toFixed(2)}`}
                progress={data.cpu.percent}
              />
              <StatCard
                label="Память"
                icon={<MemoryStick aria-hidden="true" />}
                value={formatBytes(data.memory.used)}
                sub={`из ${formatBytes(data.memory.total)} · процесс ${formatBytes(data.memory.process)}`}
                progress={memoryPercent}
                tone={memoryPercent > 85 ? 'warn' : 'default'}
              />
              <StatCard
                label="Диски"
                icon={<HardDrive aria-hidden="true" />}
                value={formatBytes(diskFree)}
                unit="свободно"
                sub={`из ${formatBytes(diskTotal)}`}
                progress={diskUsedPercent}
                tone={diskUsedPercent > 90 ? 'warn' : 'default'}
              />
              <StatCard
                label="Сеть"
                icon={<Network aria-hidden="true" />}
                value={rate(data.network.rxRate)}
                sub={`отдача ${rate(data.network.txRate)}`}
              />
            </div>

            <div className="monitor-grid">
              <div className="card">
                <SectionTitle
                  title="Загрузка процессора"
                  meta={`окно ${history.data?.window ?? range} · шаг ${history.data?.bucketSeconds ?? '—'} с`}
                />
                <Sparkline
                  points={cpuPoints}
                  label="CPU"
                  max={100}
                  format={(value) => formatPercent(value)}
                />
                <div className="core-grid">
                  {data.cpu.perCore.map((core) => (
                    <div className="core" key={core.name}>
                      <span className="core__label mono">{core.name}</span>
                      <div className="meter" aria-hidden="true">
                        <div
                          className="meter__fill"
                          style={{ width: `${Math.min(100, core.usage)}%` }}
                        />
                      </div>
                      <span className="core__value mono">{Math.round(core.usage)}%</span>
                    </div>
                  ))}
                </div>
              </div>

              <div className="card">
                <SectionTitle title="Оперативная память" meta={formatBytes(data.memory.total)} />
                <Sparkline
                  points={memPoints}
                  color="var(--accent-2)"
                  label="Занято"
                  max={data.memory.total}
                  format={(value) => formatBytes(value)}
                />
                <div className="info-list">
                  <div className="info-list__row">
                    <span>Использовано</span>
                    <strong>{formatBytes(data.memory.used)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Свободно</span>
                    <strong>{formatBytes(data.memory.available)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Процесс NEXUS</span>
                    <strong>{formatBytes(data.memory.process)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Swap</span>
                    <strong>
                      {formatBytes(data.memory.swapUsed)} / {formatBytes(data.memory.swapTotal)}
                    </strong>
                  </div>
                </div>
              </div>

              <div className="card">
                <SectionTitle
                  title="Сеть"
                  meta={`всего ${formatBytes(data.network.receivedTotal)}`}
                />
                <Sparkline
                  points={rxPoints}
                  color="var(--success)"
                  label="Входящий"
                  format={rate}
                />
                <Sparkline
                  points={txPoints}
                  color="var(--warning)"
                  label="Исходящий"
                  format={rate}
                />
                <div className="info-list">
                  <div className="info-list__row">
                    <span>
                      <Download aria-hidden="true" style={{ width: 12, height: 12 }} /> Принято
                    </span>
                    <strong>{formatBytes(data.network.receivedTotal)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>
                      <Upload aria-hidden="true" style={{ width: 12, height: 12 }} /> Отправлено
                    </span>
                    <strong>{formatBytes(data.network.transmittedTotal)}</strong>
                  </div>
                </div>
              </div>

              <div className="card">
                <SectionTitle
                  title="Приложение"
                  meta={`uptime ${formatUptime(data.uptimeSeconds)}`}
                />
                <div className="info-list">
                  <div className="info-list__row">
                    <span>
                      <Gauge aria-hidden="true" style={{ width: 12, height: 12 }} /> Запросов в
                      секунду
                    </span>
                    <strong>{data.app.requestsPerSecond.toFixed(2)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Средняя задержка API</span>
                    <strong>{data.app.averageLatencyMs.toFixed(1)} мс</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Ошибки за минуту</span>
                    <strong>
                      {data.app.errorsLastMinute} ({formatPercent(data.app.errorRate * 100)})
                    </strong>
                  </div>
                  <div className="info-list__row">
                    <span>Активные сессии</span>
                    <strong>{formatNumber(data.app.sessions)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Пользователи</span>
                    <strong>{formatNumber(data.app.users)}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Задач в очереди</span>
                    <strong>{data.app.jobsPending}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>Активных загрузок</span>
                    <strong>{data.app.activeUploads}</strong>
                  </div>
                  <div className="info-list__row">
                    <span>
                      <Database aria-hidden="true" style={{ width: 12, height: 12 }} /> База данных
                    </span>
                    <Badge tone={data.app.database === 'up' ? 'ok' : 'danger'}>
                      {data.app.database}
                    </Badge>
                  </div>
                  <div className="info-list__row">
                    <span>
                      <Server aria-hidden="true" style={{ width: 12, height: 12 }} /> Хост
                    </span>
                    <strong className="truncate">
                      {data.app.hostname} · {data.app.os} {data.app.osVersion}
                    </strong>
                  </div>
                </div>
              </div>
            </div>

            <SectionTitle title="Диски и хранилища" meta={`${data.disks.length} дисков`} />
            <div className="table-card">
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Диск</th>
                      <th>Точка монтирования</th>
                      <th>ФС</th>
                      <th>Всего</th>
                      <th>Свободно</th>
                      <th>Заполнено</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.disks.map((disk) => {
                      const used = disk.totalBytes - disk.availableBytes;
                      const percent = disk.totalBytes > 0 ? (used / disk.totalBytes) * 100 : 0;
                      return (
                        <tr key={`${disk.mountPoint}-${disk.name}`}>
                          <td>
                            {disk.name}
                            {disk.removable ? <Badge plain>съёмный</Badge> : null}
                          </td>
                          <td className="mono tiny">{disk.mountPoint}</td>
                          <td className="tiny muted">{disk.fileSystem || '—'}</td>
                          <td className="mono tiny">{formatBytes(disk.totalBytes)}</td>
                          <td className="mono tiny">{formatBytes(disk.availableBytes)}</td>
                          <td>
                            <div className="row" style={{ gap: 8 }}>
                              <div className="meter grow" aria-hidden="true">
                                <div
                                  className={`meter__fill${percent > 90 ? ' meter__fill--warn' : ''}`}
                                  style={{ width: `${percent}%` }}
                                />
                              </div>
                              <span className="mono tiny">{formatPercent(percent)}</span>
                            </div>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>

            {data.volumes.length > 0 ? (
              <div className="row" style={{ gap: 8, flexWrap: 'wrap', marginTop: 10 }}>
                {data.volumes.map((volume) => (
                  <Badge
                    key={volume.name}
                    tone={volume.state === 'online' && volume.enabled ? 'ok' : 'warn'}
                  >
                    {volume.name}: {formatBytes(volume.availableBytes)} свободно
                  </Badge>
                ))}
              </div>
            ) : null}

            <SectionTitle title="Модули" meta="реальные счётчики" />
            {modules.isError ? (
              <Alert>{errorMessage(modules.error)}</Alert>
            ) : modules.data ? (
              <div className="monitor-grid">
                <div className="card">
                  <SectionTitle
                    title="Minecraft Build Manager"
                    meta={
                      <Badge tone={modules.data.minecraft.enabled ? 'ok' : 'warn'}>
                        {modules.data.minecraft.enabled ? 'включён' : 'отключён'}
                      </Badge>
                    }
                  />
                  <div className="info-list">
                    <div className="info-list__row">
                      <span>Сборок / опубликовано</span>
                      <strong>
                        {modules.data.minecraft.builds} / {modules.data.minecraft.published}
                      </strong>
                    </div>
                    <div className="info-list__row">
                      <span>Файлов</span>
                      <strong>{modules.data.minecraft.files}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Объём файлов</span>
                      <strong>{formatBytes(modules.data.minecraft.fileBytes)}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Релизов</span>
                      <strong>{modules.data.minecraft.releases}</strong>
                    </div>
                  </div>
                </div>
                <div className="card">
                  <SectionTitle
                    title="Cloud Storage"
                    meta={
                      <Badge tone={modules.data.cloud.enabled ? 'ok' : 'warn'}>
                        {modules.data.cloud.enabled ? 'включён' : 'отключён'}
                      </Badge>
                    }
                  />
                  <div className="info-list">
                    <div className="info-list__row">
                      <span>Файлов</span>
                      <strong>{modules.data.cloud.files}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Объём</span>
                      <strong>{formatBytes(modules.data.cloud.bytes)}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Активных ссылок</span>
                      <strong>{modules.data.cloud.activeShares}</strong>
                    </div>
                  </div>
                </div>
                <div className="card">
                  <SectionTitle
                    title="Хранилище"
                    meta={`${modules.data.storage.objects} объектов`}
                  />
                  <div className="info-list">
                    <div className="info-list__row">
                      <span>Объём объектов</span>
                      <strong>{formatBytes(modules.data.storage.bytes)}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Недоступных объектов</span>
                      <strong>{modules.data.storage.unavailableObjects}</strong>
                    </div>
                  </div>
                </div>
                <div className="card">
                  <SectionTitle title="Ошибки и загрузки" meta="за 24 часа" />
                  <div className="info-list">
                    <div className="info-list__row">
                      <span>Ошибок в журнале</span>
                      <strong>{modules.data.errors24h}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Скачиваний лаунчером</span>
                      <strong>{modules.data.launcher.downloads24h}</strong>
                    </div>
                    <div className="info-list__row">
                      <span>Отдано артефактов</span>
                      <strong>{formatBytes(modules.data.launcher.bytes24h)}</strong>
                    </div>
                  </div>
                </div>
              </div>
            ) : (
              <LoadingInline label="Счётчики модулей" />
                      )}
        </>
      )}
    </>
  );
}

export default MonitoringPage;
