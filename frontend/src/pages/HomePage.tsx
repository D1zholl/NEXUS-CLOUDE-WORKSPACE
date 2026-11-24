import { Link } from 'react-router-dom';
import type { ReactNode } from 'react';
import { ArrowUpRight, Bot, Boxes, Cloud, Gauge, Lock, ShieldCheck, Sparkles } from 'lucide-react';

import { LogoMark } from '../components/Brand';
import { StatusPill } from '../components/StatusPill';
import { UserMenu } from '../components/UserMenu';
import { Badge } from '../components/ui';
import { can, useSession } from '../stores/auth';
import { formatDate } from '../lib/format';

interface ModuleCard {
  key: 'minecraft' | 'cloud' | 'ai' | 'system';
  title: string;
  description: string;
  to: string;
  icon: ReactNode;
  action: string;
  enabled: boolean;
  allowed: boolean;
  meta: string;
  badge?: string;
}

export function HomePage() {
  const session = useSession();
  if (!session) return null;

  const modules = session.modules;
  const minecraftOn = modules.minecraft?.enabled ?? true;
  const cloudOn = modules.cloud?.enabled ?? true;
  const aiOn = modules.ai?.enabled ?? true;
  const canSystem = can(session, 'system.view');
  const canMinecraft = can(session, 'minecraft.view');
  const canCloud = can(session, 'cloud.view');
  const canAi = can(session, 'ai.access');

  const cards: ModuleCard[] = [
    {
      key: 'minecraft',
      title: 'Minecraft Build Manager',
      description:
        'Сборки, версии, зависимости и артефакты. Публикация релизов с контролем доступа и историей изменений.',
      to: '/workspace/builds',
      icon: <Boxes aria-hidden="true" />,
      action: canMinecraft ? 'Открыть модуль' : 'Нет доступа',
      enabled: minecraftOn,
      allowed: canMinecraft,
      meta: minecraftOn ? 'Модуль включён' : 'Отключён администратором',
    },
    {
      key: 'cloud',
      title: 'NEXUS Cloud Storage',
      description:
        'Файлы и папки на подключённых дисках, квоты, общий доступ и потоковая передача без загрузки в память сервера.',
      to: '/workspace/storage',
      icon: <Cloud aria-hidden="true" />,
      action: canCloud ? 'Открыть модуль' : 'Нет доступа',
      enabled: cloudOn,
      allowed: canCloud,
      meta: cloudOn ? 'Модуль включён' : 'Отключён администратором',
    },
    {
      key: 'ai',
      title: 'AI Workspace',
      description:
        'AI-чат и агентская облачная среда: диалоги с моделями, запуск инструментов и изолированные сессии для задач.',
      to: '/workspace/ai',
      icon: <Bot aria-hidden="true" />,
      action: canAi ? 'Открыть модуль' : 'Нет доступа',
      enabled: aiOn,
      allowed: canAi,
      meta: aiOn
        ? canAi
          ? 'Модуль включён'
          : 'Нет права ai.access'
        : 'Отключён администратором',
    },
  ];

  if (canSystem) {
    cards.push({
      key: 'system',
      title: 'Управление системой',
      description:
        'Пользователи, роли и гранулярные права, журнал событий, состояние сервера и параметры внешнего вида.',
      to: '/system',
      icon: <ShieldCheck aria-hidden="true" />,
      action: 'Открыть панель',
      enabled: true,
      allowed: true,
      meta: session.user.isSuperadmin ? 'Полный доступ' : 'По правам роли',
    });
  }

  return (
    <div className="home">
      <div className="home__top">
        <Link to="/" className="brand">
          <LogoMark size={21} />
          <div>
            <div className="brand__text">NEXUS WORKSPACE</div>
            <span className="brand__sub">ЦИФРОВОЕ ПРОСТРАНСТВО</span>
          </div>
        </Link>

        <div className="top-actions">
          <StatusPill />
          <UserMenu />
        </div>
      </div>

      <section className="home__hero">
        <div className="home__intro">
          <div className="eyebrow">NEXUS CORE · LOCAL INSTANCE</div>
          <h1 className="display">
            Привет, <span>{session.user.displayName || session.user.username}</span>
            <br />
            всё пространство собрано здесь
          </h1>
          <p className="home__sub">
            Единая платформа для сборок Minecraft, облачного хранения и AI-направления. Права
            проверяются на сервере, сессии живут в HttpOnly cookie, а каждое значимое действие
            попадает в журнал аудита.
          </p>
        </div>

        <div className="home__section-head">
          <h2>Модули пространства</h2>
          <span>Направлений: {cards.length}</span>
        </div>

        <div className="module-grid">
          {cards.map((card) =>
            card.enabled && card.allowed ? (
              <Link
                key={card.key}
                to={card.to}
                className="module-card"
                style={{ textDecoration: 'none', color: 'inherit' }}
              >
                <ModuleCardBody card={card} />
              </Link>
            ) : (
              <div key={card.key} className="module-card module-card--off" aria-disabled="true">
                <ModuleCardBody card={card} />
              </div>
            ),
          )}
        </div>

        <div className="home__bottom">
          <span>
            NEXUS WORKSPACE ·{' '}
            {session.user.roles.map((role) => role.name).join(' · ') || 'Пользователь'}
          </span>
          <span>
            <Sparkles
              aria-hidden="true"
              style={{
                display: 'inline',
                width: 12,
                height: 12,
                marginRight: 5,
                verticalAlign: -1,
              }}
            />
            Сессия до {formatDate(session.session.expiresAt)}
          </span>
        </div>
      </section>

      <section style={{ width: '100%', maxWidth: 1120, margin: '0 auto' }}>
        <div className="row wrap" style={{ gap: 10 }}>
          <Badge tone="ok">Авторизация активна</Badge>
          <Badge tone="info">CSRF-защита включена</Badge>
          <Badge tone="info">Аудит включён</Badge>
          <Badge plain>
            <Gauge aria-hidden="true" style={{ width: 12, height: 12 }} />
            Права:{' '}
            {session.user.isSuperadmin
              ? 'суперадминистратор'
              : `${session.user.permissions.length} разрешений`}
          </Badge>
        </div>
      </section>
    </div>
  );
}

function ModuleCardBody({ card }: { card: ModuleCard }) {
  const unavailable = !card.enabled || !card.allowed;
  return (
    <>
      {card.badge ? <span className="module-card__badge">{card.badge}</span> : null}
      <div className="module-card__icon">{card.icon}</div>
      <div className="module-card__title">{card.title}</div>
      <div className="module-card__desc">{card.description}</div>
      <div className="module-card__foot">
        <span>{card.action}</span>
        {unavailable ? <Lock aria-hidden="true" /> : <ArrowUpRight aria-hidden="true" />}
      </div>
      <div className="tiny muted" style={{ marginTop: 8 }}>
        {card.meta}
      </div>
    </>
  );
}
