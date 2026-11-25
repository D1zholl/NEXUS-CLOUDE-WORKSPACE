import { useEffect } from 'react';
import type { ReactNode } from 'react';
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import {
  Activity,
  Bot,
  Boxes,
  Cloud,
  Cog,
  Gauge,
  HardDrive,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  Palette,
  ScrollText,
  Settings2,
  ShieldCheck,
  Sparkles,
  UserRound,
  Users,
  Wrench,
} from 'lucide-react';

import { useAuth, useCan, useSession } from '../stores/auth';
import { useUi } from '../stores/ui';
import { Avatar, Button } from './ui';
import { StatusPill } from './StatusPill';
import { UserMenu, roleLabel } from './UserMenu';
import { LogoMark } from './Brand';

const CRUMBS: Record<string, { section: string; title: string }> = {
  '/profile': { section: 'Аккаунт', title: 'Профиль' },
  '/system': { section: 'Система', title: 'Обзор системы' },
  '/system/general': { section: 'Система', title: 'Общие настройки' },
  '/system/limits': { section: 'Система', title: 'Ограничения ресурсов' },
  '/system/network': { section: 'Система', title: 'Сетевые настройки' },
  '/system/security': { section: 'Система', title: 'Безопасность' },
  '/system/maintenance': { section: 'Система', title: 'Обслуживание' },
  '/system/ai': { section: 'Система', title: 'AI Management' },
  '/system/users': { section: 'Система', title: 'Пользователи' },
  '/system/roles': { section: 'Система', title: 'Роли и права' },
  '/system/audit': { section: 'Система', title: 'Журнал событий' },
  '/system/monitoring': { section: 'Система', title: 'Мониторинг' },
  '/system/jobs': { section: 'Система', title: 'Фоновые задачи' },
  '/system/storage': { section: 'Система', title: 'Хранилища' },
  '/system/appearance': { section: 'Аккаунт', title: 'Внешний вид' },
  '/system/background': { section: 'Аккаунт', title: '3D-фон' },
  '/workspace/builds': { section: 'Пространство', title: 'Minecraft Build Manager' },
  '/workspace/builds/:id': { section: 'Пространство', title: 'Сборка' },
  '/workspace/launcher': { section: 'Пространство', title: 'Launcher API' },
  '/workspace/storage': { section: 'Пространство', title: 'NEXUS Cloud Storage' },
  '/workspace/ai': { section: 'Пространство', title: 'AI Workspace' },
  '/workspace/ai/agent': { section: 'Пространство', title: 'AI Agent Workspace' },
  '/s': { section: 'Cloud', title: 'Ссылка общего доступа' },
};

export function AppLayout() {
  const session = useSession();
  const location = useLocation();
  const navigate = useNavigate();
  const sidebarOpen = useUi((state) => state.sidebarOpen);
  const setSidebarOpen = useUi((state) => state.setSidebarOpen);
  const canSystem = useCan('system.view');
  const canUsers = useCan('system.users.manage');
  const canStorage = useCan('system.storage.manage');
  const canSettings = useCan('system.settings.manage');
  const canAi = useCan('ai.access');
  const canAiAdmin = useCan('ai.admin.manage');
  const minecraftOn = session?.modules.minecraft?.enabled ?? true;
  const cloudOn = session?.modules.cloud?.enabled ?? true;
  const aiOn = session?.modules.ai?.enabled ?? true;

  const crumbs = CRUMBS[location.pathname] ?? { section: 'NEXUS', title: 'Рабочее пространство' };

  useEffect(() => {
    setSidebarOpen(false);
  }, [location.pathname, setSidebarOpen]);

  const logout = async (): Promise<void> => {
    await useAuth.getState().logout();
    navigate('/login', { replace: true });
  };

  return (
    <>
      <div className="shell">
        <aside className={`sidebar${sidebarOpen ? ' sidebar--open' : ''}`} aria-label="Навигация">
          <NavLink to="/" className="sidebar__brand" onClick={() => setSidebarOpen(false)}>
            <LogoMark size={18} />
            <div>
              <div className="sidebar__brand-text">NEXUS</div>
              <span className="sidebar__brand-sub">WORKSPACE</span>
            </div>
          </NavLink>

          <nav className="sidebar__section">
            <div className="sidebar__label">Пространство</div>
            <NavItem to="/" icon={<LayoutDashboard aria-hidden="true" />} label="Обзор" end />
            {minecraftOn ? (
              <NavItem
                to="/workspace/builds"
                icon={<Boxes aria-hidden="true" />}
                label="Minecraft Builds"
              />
            ) : null}
            {cloudOn ? (
              <NavItem
                to="/workspace/storage"
                icon={<Cloud aria-hidden="true" />}
                label="Cloud Storage"
              />
            ) : null}
            {minecraftOn ? (
              <NavItem
                to="/workspace/launcher"
                icon={<KeyRound aria-hidden="true" />}
                label="Launcher API"
              />
            ) : null}
            {canAi && aiOn ? (
              <>
                <NavItem to="/workspace/ai" icon={<Bot aria-hidden="true" />} label="AI Workspace" end />
                <NavItem
                  to="/workspace/ai/agent"
                  icon={<Bot aria-hidden="true" />}
                  label="AI Agent IDE"
                />
              </>
            ) : null}
          </nav>

          {canSystem ? (
            <nav className="sidebar__section">
              <div className="sidebar__label">Система</div>
              <NavItem to="/system" icon={<Activity aria-hidden="true" />} label="Состояние" end />
              {canUsers ? (
                <>
                  <NavItem
                    to="/system/users"
                    icon={<Users aria-hidden="true" />}
                    label="Пользователи"
                  />
                  <NavItem
                    to="/system/roles"
                    icon={<KeyRound aria-hidden="true" />}
                    label="Роли и права"
                  />
                </>
              ) : null}
              {canStorage ? (
                <NavItem
                  to="/system/storage"
                  icon={<HardDrive aria-hidden="true" />}
                  label="Хранилища"
                />
              ) : null}
              <NavItem
                to="/system/audit"
                icon={<ScrollText aria-hidden="true" />}
                label="Журнал событий"
              />
              <NavItem
                to="/system/monitoring"
                icon={<Gauge aria-hidden="true" />}
                label="Мониторинг"
              />
              <NavItem to="/system/jobs" icon={<Cog aria-hidden="true" />} label="Фоновые задачи" />
              {canAiAdmin ? (
                <NavItem to="/system/ai" icon={<Bot aria-hidden="true" />} label="AI Management" />
              ) : null}
              {canSettings ? (
                <>
                  <NavItem
                    to="/system/general"
                    icon={<Settings2 aria-hidden="true" />}
                    label="Общие настройки"
                  />
                  <NavItem
                    to="/system/limits"
                    icon={<Wrench aria-hidden="true" />}
                    label="Ограничения"
                  />
                  <NavItem to="/system/network" icon={<Cloud aria-hidden="true" />} label="Сеть" />
                  <NavItem
                    to="/system/security"
                    icon={<ShieldCheck aria-hidden="true" />}
                    label="Безопасность"
                  />
                </>
              ) : null}
              <NavItem
                to="/system/maintenance"
                icon={<Wrench aria-hidden="true" />}
                label="Обслуживание"
              />
            </nav>
          ) : null}

          <nav className="sidebar__section">
            <div className="sidebar__label">Аккаунт</div>
            <NavItem
              to="/system/appearance"
              icon={<Palette aria-hidden="true" />}
              label="Внешний вид"
            />
            <NavItem
              to="/system/background"
              icon={<Sparkles aria-hidden="true" />}
              label="3D-фон"
            />
            <NavItem to="/profile" icon={<UserRound aria-hidden="true" />} label="Профиль" />
          </nav>

          <div className="sidebar__foot">
            {session ? (
              <>
                <div className="sidebar__user">
                  <Avatar name={session.user.displayName || session.user.username} small />
                  <div className="grow" style={{ minWidth: 0 }}>
                    <div className="sidebar__user-name truncate">
                      {session.user.displayName || session.user.username}
                    </div>
                    <div className="sidebar__user-role truncate">{roleLabel(session)}</div>
                  </div>
                </div>
                <div className="sidebar__actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={logout}
                    icon={<LogOut aria-hidden="true" />}
                  >
                    Выйти
                  </Button>
                </div>
              </>
            ) : null}
            <div className="sidebar__label" style={{ padding: 0, margin: 0, textAlign: 'center' }}>
              NEXUS CORE · LOCAL
            </div>
          </div>
        </aside>

        {sidebarOpen ? (
          <div
            className="sidebar-overlay"
            onClick={() => setSidebarOpen(false)}
            aria-hidden="true"
          />
        ) : null}

        <div className="shell__main">
          <header className="topbar">
            <Button
              className="sidebar-toggle"
              size="sm"
              variant="ghost"
              iconOnly
              aria-label="Открыть навигацию"
              onClick={() => setSidebarOpen(true)}
            >
              <Menu aria-hidden="true" />
            </Button>
            <div className="topbar__crumbs">
              <span>{crumbs.section}</span>
              <span>/</span>
              <strong>{crumbs.title}</strong>
            </div>
            <div className="topbar__spacer" />
            <div className="topbar__actions">
              <StatusPill />
              <UserMenu />
            </div>
          </header>

          <main className="content">
            <div className="page-transition" key={location.pathname}>
              <Outlet />
            </div>
          </main>

          <footer
            className="row row--between"
            style={{
              padding: '14px clamp(16px, 3vw, 34px)',
              borderTop: '1px solid var(--border-soft)',
              fontSize: 11,
              color: 'var(--text-secondary)',
            }}
          >
            <span>NEXUS WORKSPACE · локальная инфраструктура</span>
          </footer>
        </div>
      </div>
    </>
  );
}

function NavItem({
  to,
  icon,
  label,
  badge,
  end = false,
}: {
  to: string;
  icon: ReactNode;
  label: ReactNode;
  badge?: string;
  end?: boolean;
}) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) => `nav-link${isActive ? ' active' : ''}`}
      onClick={() => useUi.getState().setSidebarOpen(false)}
    >
      {icon}
      <span className="truncate">{label}</span>
      {badge ? (
        <span className="nav-link__badge" title="В разработке">
          {badge}
        </span>
      ) : null}
    </NavLink>
  );
}
