import { Link2 } from 'lucide-react';

import { Alert, Badge, Button, EmptyState, LoadingInline } from '../../components/ui';
import { formatDate } from '../../lib/format';
import type { CloudShare } from '../../services/api';

/** Раздел «Общий доступ»: таблица активных и отозванных ссылок. */
export function SharesView({
  shares,
  loading,
  error,
  canShare,
  onRevoke,
}: {
  shares: CloudShare[];
  loading: boolean;
  error: string | null;
  canShare: boolean;
  onRevoke: (id: string) => void;
}) {
  if (loading) {
    return (
      <div className="card">
        <LoadingInline label="Загрузка ссылок" />
      </div>
    );
  }
  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!canShare) {
    return (
      <div className="card">
        <EmptyState icon={<Link2 aria-hidden="true" />} title="Нет прав на общий доступ">
          Обратитесь к администратору для выдачи права cloud.file.share.
        </EmptyState>
      </div>
    );
  }
  if (shares.length === 0) {
    return (
      <div className="card">
        <EmptyState icon={<Link2 aria-hidden="true" />} title="Активных ссылок нет">
          Создайте ссылку из карточки файла, чтобы поделиться им.
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="card">
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>Файл</th>
              <th>Срок</th>
              <th>Лимит</th>
              <th>Скачиваний</th>
              <th>Статус</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {shares.map((share) => (
              <tr key={share.id} className={share.active ? '' : 'muted'}>
                <td className="truncate">
                  <div className="row" style={{ gap: 6 }}>
                    <span className="truncate">{share.entryName}</span>
                    {share.authOnly ? (
                      <Badge tone="info" plain>
                        только авторизованные
                      </Badge>
                    ) : null}
                  </div>
                </td>
                <td>{share.expiresAt ? formatDate(share.expiresAt) : 'бессрочно'}</td>
                <td>{share.maxDownloads ?? '—'}</td>
                <td>
                  {share.downloadCount}
                  {share.maxDownloads ? ` / ${share.maxDownloads}` : ''}
                </td>
                <td>
                  {share.revokedAt ? (
                    <Badge tone="danger" plain>
                      отозвана
                    </Badge>
                  ) : share.active ? (
                    <Badge tone="ok" plain>
                      активна
                    </Badge>
                  ) : (
                    <Badge tone="warn" plain>
                      истекла
                    </Badge>
                  )}
                </td>
                <td>
                  {share.active ? (
                    <Button size="sm" variant="ghost" onClick={() => onRevoke(share.id)}>
                      Отозвать
                    </Button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
