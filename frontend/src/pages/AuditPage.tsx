import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight, RefreshCw, ScrollText } from 'lucide-react';

import { Alert, Badge, Button, EmptyState, LoadingInline, PageHead } from '../components/ui';
import { api, errorMessage } from '../services/api';
import { formatDate } from '../lib/format';

const EVENT_TYPES = [
  'auth.login.success',
  'auth.login.failed',
  'auth.logout',
  'auth.password.changed',
  'auth.sessions.revoked',
  'user.created',
  'user.updated',
  'user.deleted',
  'user.password.reset',
  'user.unlocked',
  'user.sessions.revoked',
  'role.created',
  'role.updated',
  'role.deleted',
  'module.toggled',
  'settings.user.updated',
  'settings.system.updated',
];

export function AuditPage() {
  const [page, setPage] = useState(1);
  const [eventType, setEventType] = useState('');

  const audit = useQuery({
    queryKey: ['audit', page, eventType],
    queryFn: () => api.listAudit({ page, perPage: 25, eventType }),
  });

  const total = audit.data?.total ?? 0;
  const perPage = audit.data?.perPage ?? 25;
  const maxPage = Math.max(1, Math.ceil(total / perPage));

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Журнал событий"
        description="Аудит входов, изменений пользователей и настроек. Секреты, пароли и содержимое файлов в журнал не попадают."
        actions={
          <Button
            size="sm"
            variant="ghost"
            icon={<RefreshCw aria-hidden="true" />}
            onClick={() => void audit.refetch()}
            loading={audit.isFetching}
          >
            Обновить
          </Button>
        }
      />

      <div className="table-card">
        <div className="toolbar">
          <select
            className="select"
            value={eventType}
            onChange={(event) => {
              setEventType(event.target.value);
              setPage(1);
            }}
            aria-label="Тип события"
          >
            <option value="">Все события</option>
            {EVENT_TYPES.map((type) => (
              <option key={type} value={type}>
                {type}
              </option>
            ))}
          </select>
          <div className="grow" />
          <span className="tiny muted">
            Всего: {total} · стр. {audit.data?.page ?? 1} из {maxPage}
          </span>
        </div>

        {audit.isPending ? (
          <LoadingInline label="Загрузка журнала" />
        ) : audit.isError ? (
          <div style={{ padding: 16 }}>
            <Alert>{errorMessage(audit.error)}</Alert>
          </div>
        ) : audit.data.items.length === 0 ? (
          <EmptyState icon={<ScrollText aria-hidden="true" />} title="События не найдены">
            {eventType
              ? 'По выбранному типу событий записей пока нет.'
              : 'Журнал пуст. События появятся после первых действий в системе.'}
          </EmptyState>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Время</th>
                  <th>Событие</th>
                  <th>Актор</th>
                  <th>Объект</th>
                  <th>Результат</th>
                  <th>IP</th>
                  <th>Детали</th>
                </tr>
              </thead>
              <tbody>
                {audit.data.items.map((row) => (
                  <tr key={row.id}>
                    <td className="nowrap">{formatDate(row.createdAt)}</td>
                    <td>
                      <span className="mono" style={{ fontSize: 11 }}>
                        {row.eventType}
                      </span>
                    </td>
                    <td>
                      <div>{row.actorUsername || 'система'}</div>
                      {row.actorId ? (
                        <div className="tiny muted mono">{row.actorId.slice(0, 8)}…</div>
                      ) : null}
                    </td>
                    <td>
                      <div>{row.targetType || '—'}</div>
                      {row.targetId ? (
                        <div className="tiny muted mono">{row.targetId.slice(0, 18)}</div>
                      ) : null}
                    </td>
                    <td>
                      <Badge
                        tone={
                          row.result === 'success'
                            ? 'ok'
                            : row.result === 'failure'
                              ? 'danger'
                              : 'default'
                        }
                      >
                        {row.result}
                      </Badge>
                    </td>
                    <td className="mono tiny">{row.ip || '—'}</td>
                    <td>
                      <details className="audit-details">
                        <summary>JSON</summary>
                        <pre>{JSON.stringify(row.details ?? {}, null, 2)}</pre>
                      </details>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        <div className="pagination">
          <span>
            Показано {audit.data?.items.length ?? 0} из {total}
          </span>
          <div className="pagination__controls">
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              aria-label="Предыдущая страница"
              disabled={page <= 1}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              <ChevronLeft aria-hidden="true" />
            </Button>
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              aria-label="Следующая страница"
              disabled={page >= maxPage}
              onClick={() => setPage((value) => Math.min(maxPage, value + 1))}
            >
              <ChevronRight aria-hidden="true" />
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
