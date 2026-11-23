import { Link } from 'react-router-dom';
import { PowerOff } from 'lucide-react';

import { EmptyState } from '../components/ui';

const MODULE_LABELS: Record<'minecraft' | 'cloud' | 'ai', string> = {
  minecraft: 'Minecraft Build Manager',
  cloud: 'NEXUS Cloud Storage',
  ai: 'NEXUS AI Workspace',
};

/**
 * Экран отключённого администратором модуля.
 *
 * Скрыть ссылку в навигации недостаточно: прямой переход по URL не должен
 * показывать рабочий интерфейс, если серверный гейт уже блокирует API модуля.
 */
export function ModuleDisabledPage({ module }: { module: 'minecraft' | 'cloud' | 'ai' }) {
  return (
    <div className="state-panel" data-testid="module-disabled">
      <EmptyState
        icon={<PowerOff aria-hidden="true" />}
        title="Модуль отключён"
        action={
          <Link to="/" className="btn btn--sm">
            Вернуться на главную
          </Link>
        }
      >
        Администратор отключил модуль «{MODULE_LABELS[module]}». Данные сохранены, но раздел и его
        API недоступны до повторного включения модуля.
      </EmptyState>
    </div>
  );
}
