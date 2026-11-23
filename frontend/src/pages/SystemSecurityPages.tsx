import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { KeyRound, ShieldCheck, Trash2, RefreshCw } from 'lucide-react';

import {
  Alert,
  Badge,
  Button,
  EmptyState,
  LoadingInline,
  PageHead,
  SectionTitle,
} from '../components/ui';
import { api, errorMessage, type LauncherToken } from '../services/api';
import { formatDate } from '../lib/format';
import { useAuth } from '../stores/auth';
import { toast } from '../stores/ui';
import { SettingsGroupPage } from './SystemSettingsPage';

/** Безопасность: политика паролей, сессии, чек-лист production. */
export function SystemSecurityPage() {
  return (
    <SettingsGroupPage
      eyebrow="Система"
      title="Безопасность"
      description="Политика паролей и срок жизни сессии. Показаны эффективные значения; изменения применяются без перезапуска."
      keys={[
        'security.session_ttl_hours',
        'security.login_max_failures',
        'security.password_min_length',
      ]}
      extra={<SecurityExtras />}
    />
  );
}

function SecurityExtras() {
  const session = useAuth((state) => state.session);
  const revokeAll = useMutation({
    mutationFn: api.revokeAllSessions,
    onSuccess: () => toast('Все сессии завершены. Войдите снова.', 'success'),
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  return (
    <>
      <div className="card">
        <SectionTitle title="Текущая сессия" meta={session ? session.user.username : '—'} />
        <div className="info-list">
          <div className="info-list__row">
            <span>Истекает</span>
            <span>{session ? formatDate(session.session.expiresAt) : '—'}</span>
          </div>
          <div className="info-list__row">
            <span>Cookie сессии</span>
            <span>HttpOnly · SameSite=Lax · Secure по конфигурации</span>
          </div>
        </div>
        <Button
          variant="danger"
          size="sm"
          icon={<Trash2 aria-hidden="true" />}
          loading={revokeAll.isPending}
          onClick={() => revokeAll.mutate()}
        >
          Завершить все сессии
        </Button>
      </div>

      <div className="card">
        <SectionTitle title="Чек-лист production" />
        <ul className="stack" style={{ gap: 8, paddingLeft: 4 }}>
          <CheckItem done label="Пароли хранятся через Argon2id" />
          <CheckItem done label="Серверные сессии с HttpOnly cookie" />
          <CheckItem done label="CSRF-защита изменяющих запросов" />
          <CheckItem done label="Гранулярные права проверяются на сервере" />
          <CheckItem done label="Ограничение попыток входа и блокировка аккаунта" />
          <CheckItem done label="Аудит входов и опасных операций" />
        </ul>
      </div>
    </>
  );
}

function CheckItem({ label, done }: { label: string; done: boolean }) {
  return (
    <li className="row" style={{ gap: 8, listStyle: 'none', fontSize: 12.5 }}>
      <span style={{ color: done ? 'var(--accent)' : 'var(--text-secondary)' }}>
        {done ? '✓' : '○'}
      </span>
      {label}
    </li>
  );
}

/** Обслуживание: задачи, история запусков, целостность, staging. */
export function SystemMaintenancePage() {
  const queryClient = useQueryClient();
  const [pendingKind, setPendingKind] = useState<string | null>(null);
  const jobs = useQuery({
    queryKey: ['jobs', 'maintenance'],
    queryFn: () => api.listJobs({ limit: 20 }),
    refetchInterval: 5000,
  });
  const storage = useQuery({
    queryKey: ['storage-summary'],
    queryFn: api.storageSummary,
    refetchInterval: 30_000,
  });

  const run = useMutation({
    mutationFn: (kind: string) => api.runMaintenance(kind),
    onMutate: (kind) => setPendingKind(kind),
    onSuccess: async () => {
      toast('Задача обслуживания запущена.', 'success');
      await queryClient.invalidateQueries({ queryKey: ['jobs'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
    onSettled: () => setPendingKind(null),
  });

  const tasks: Array<{ kind: string; label: string; description: string }> = [
    {
      kind: 'storage_check',
      label: 'Проверить целостность',
      description: 'Сверяет объекты хранилища с диском: наличие и размеры файлов.',
    },
    {
      kind: 'cleanup_uploads',
      label: 'Очистить временные загрузки',
      description: 'Удаляет просроченные сессии загрузки и их staging-файлы.',
    },
    {
      kind: 'cleanup_shares',
      label: 'Удалить истёкшие ссылки',
      description: 'Удаляет старые ссылки общего доступа и токены лаунчера.',
    },
    {
      kind: 'metrics_prune',
      label: 'Очистить старые метрики',
      description: 'Удаляет историю метрик и завершённые задачи старше срока хранения.',
    },
  ];

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Обслуживание"
        description="Фоновые задачи обслуживания, состояние томов и проверка целостности данных."
      />
      <div className="card">
        <SectionTitle title="Задачи обслуживания" />
        <div className="stack" style={{ gap: 12 }}>
          {tasks.map((task) => (
            <div key={task.kind} className="row row--between" style={{ gap: 14 }}>
              <div style={{ minWidth: 0 }}>
                <div style={{ fontSize: 13, fontWeight: 600 }}>{task.label}</div>
                <div className="tiny muted">{task.description}</div>
              </div>
              <Button
                size="sm"
                variant="ghost"
                icon={<RefreshCw aria-hidden="true" />}
                loading={pendingKind === task.kind}
                onClick={() => run.mutate(task.kind)}
              >
                Запустить
              </Button>
            </div>
          ))}
        </div>
      </div>

      <div className="card">
        <SectionTitle
          title="История запусков"
          meta={jobs.data ? `${jobs.data.items.length} записей` : undefined}
        />
        {jobs.isPending ? (
          <LoadingInline label="Загрузка задач" />
        ) : jobs.isError || !jobs.data ? (
          <Alert>{errorMessage(jobs.error)}</Alert>
        ) : jobs.data.items.length === 0 ? (
          <EmptyState icon={<RefreshCw aria-hidden="true" />} title="Запусков ещё не было">
            Запустите задачу обслуживания, чтобы увидеть результат.
          </EmptyState>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Тип</th>
                  <th>Статус</th>
                  <th>Прогресс</th>
                  <th>Запущена</th>
                  <th>Завершена</th>
                </tr>
              </thead>
              <tbody>
                {jobs.data.items.map((job) => (
                  <tr key={job.id}>
                    <td>{job.jobType}</td>
                    <td>
                      <Badge
                        tone={
                          job.status === 'completed'
                            ? 'ok'
                            : job.status === 'failed'
                              ? 'danger'
                              : 'info'
                        }
                        plain
                      >
                        {job.status}
                      </Badge>
                    </td>
                    <td>{Math.round(job.progress)}%</td>
                    <td>{formatDate(job.createdAt)}</td>
                    <td>{job.finishedAt ? formatDate(job.finishedAt) : '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className="card">
        <SectionTitle title="Состояние хранилища" />
        {storage.isPending ? (
          <LoadingInline label="Загрузка" />
        ) : storage.isError || !storage.data ? (
          <Alert>{errorMessage(storage.error)}</Alert>
        ) : (
          <div className="info-list">
            <div className="info-list__row">
              <span>Тома онлайн</span>
              <span>
                {storage.data.online} из {storage.data.volumes}
              </span>
            </div>
            <div className="info-list__row">
              <span>Объектов в хранилище</span>
              <span>{storage.data.objects}</span>
            </div>
            <div className="info-list__row">
              <span>Свободно</span>
              <span>{formatDate(new Date().toISOString())} — обновляется автоматически</span>
            </div>
          </div>
        )}
        <Alert tone="info">
          Проверка целостности может занять время на больших объёмах. Следите за прогрессом в
          разделе «Фоновые задачи».
        </Alert>
      </div>
    </>
  );
}

/** Управление токенами лаунчера (для системного раздела). */
export function LauncherTokensPanel() {
  const queryClient = useQueryClient();
  const tokens = useQuery({
    queryKey: ['launcher-tokens'],
    queryFn: api.listLauncherTokens,
  });
  const revoke = useMutation({
    mutationFn: api.revokeLauncherToken,
    onSuccess: async () => {
      toast('Токен отозван.', 'success');
      await queryClient.invalidateQueries({ queryKey: ['launcher-tokens'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  if (tokens.isPending) return <LoadingInline label="Загрузка токенов" />;
  if (tokens.isError || !tokens.data) return <Alert>{errorMessage(tokens.error)}</Alert>;
  if (tokens.data.length === 0) {
    return (
      <EmptyState icon={<KeyRound aria-hidden="true" />} title="Активных токенов нет">
        Токены выдаются на странице Launcher API.
      </EmptyState>
    );
  }

  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Имя</th>
            <th>Пользователь</th>
            <th>Scopes</th>
            <th>Использован</th>
            <th>Статус</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {tokens.data.map((token: LauncherToken) => (
            <tr key={token.id}>
              <td>{token.name}</td>
              <td>{token.username}</td>
              <td className="tiny">{token.scopes.join(', ')}</td>
              <td>{token.lastUsedAt ? formatDate(token.lastUsedAt) : '—'}</td>
              <td>
                <Badge tone={token.revokedAt ? 'danger' : 'ok'} plain>
                  {token.revokedAt ? 'отозван' : 'активен'}
                </Badge>
              </td>
              <td>
                {!token.revokedAt ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<ShieldCheck aria-hidden="true" />}
                    onClick={() => revoke.mutate(token.id)}
                  >
                    Отозвать
                  </Button>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
