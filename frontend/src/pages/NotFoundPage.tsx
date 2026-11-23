import { Link } from 'react-router-dom';
import { Compass, ShieldOff } from 'lucide-react';

import { EmptyState } from '../components/ui';

export function NotFoundPage() {
  return (
    <div className="state-panel">
      <EmptyState
        icon={<Compass aria-hidden="true" />}
        title="Страница не найдена"
        action={
          <Link to="/" className="btn btn--sm">
            Вернуться на главную
          </Link>
        }
      >
        Такого раздела нет в NEXUS WORKSPACE. Возможно, ссылка устарела или содержит опечатку.
      </EmptyState>
    </div>
  );
}

export function ForbiddenPage() {
  return (
    <div className="state-panel">
      <EmptyState
        icon={<ShieldOff aria-hidden="true" />}
        title="Недостаточно прав"
        action={
          <Link to="/" className="btn btn--sm">
            Вернуться на главную
          </Link>
        }
      >
        У вашей учётной записи нет разрешения на этот раздел. Права выдаются администратором
        пространства.
      </EmptyState>
    </div>
  );
}
