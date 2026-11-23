import { useQuery } from '@tanstack/react-query';

import { api } from '../services/api';

/**
 * Индикатор доступности API. Опирается на публичный health-эндпоинт
 * и не показывает никаких внутренних данных.
 */
export function StatusPill({ compact = false }: { compact?: boolean }) {
  const health = useQuery({
    queryKey: ['health'],
    queryFn: api.health,
    refetchInterval: 30_000,
    retry: 1,
  });

  const down = health.isError || (health.data && health.data.status !== 'ok');
  const label = health.isPending ? 'Проверка связи' : down ? 'Сервер недоступен' : 'Система в сети';

  return (
    <span
      className={`status-pill${down ? ' status-pill--down' : ''}`}
      title={
        health.data
          ? `NEXUS ${health.data.version} · БД ${health.data.database === 'up' ? 'доступна' : 'недоступна'}`
          : undefined
      }
    >
      <span className="status-pill__dot" aria-hidden="true" />
      {compact ? null : label}
    </span>
  );
}
