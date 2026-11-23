import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { LogOut, Palette, ShieldCheck, Sparkles, UserRound } from 'lucide-react';

import { can, useAuth, useSession } from '../stores/auth';
import { toast } from '../stores/ui';
import { usePresence } from '../lib/usePresence';
import { Avatar } from './ui';

export function roleLabel(session: ReturnType<typeof useSession>): string {
  if (!session) return 'Гость';
  if (session.user.isSuperadmin) return 'Суперадминистратор';
  if (session.user.roles.length > 0) return session.user.roles[0].name;
  return 'Пользователь';
}

export function UserMenu({ compact = false }: { compact?: boolean }) {
  const session = useSession();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const { mounted, closing } = usePresence(open, 140);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent): void => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  if (!session) return null;

  const logout = async (): Promise<void> => {
    setOpen(false);
    await useAuth.getState().logout();
    toast('Вы вышли из системы.', 'info');
    navigate('/login', { replace: true });
  };

  return (
    <div className="user-menu" ref={wrapRef}>
      <button
        type="button"
        className="user-chip"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="menu"
        aria-expanded={open}
        title="Меню пользователя"
      >
        <Avatar name={session.user.displayName || session.user.username} />
        {compact ? null : (
          <span className="user-chip__name truncate">
            {session.user.displayName || session.user.username}
          </span>
        )}
      </button>

      {mounted ? (
        <div
          className={`dropdown${closing ? ' dropdown--closing' : ''}`}
          role="menu"
          aria-hidden={closing || undefined}
          inert={closing || undefined}
        >
          <div className="dropdown__head">
            <div className="dropdown__name">
              {session.user.displayName || session.user.username}
            </div>
            <div className="dropdown__meta">
              @{session.user.username} · {roleLabel(session)}
            </div>
          </div>
          <Link
            className="dropdown__item"
            to="/profile"
            role="menuitem"
            onClick={() => setOpen(false)}
          >
            <UserRound aria-hidden="true" />
            Профиль
          </Link>
          <Link
            className="dropdown__item"
            to="/system/appearance"
            role="menuitem"
            onClick={() => setOpen(false)}
          >
            <Palette aria-hidden="true" />
            Внешний вид
          </Link>
          <Link
            className="dropdown__item"
            to="/system/background"
            role="menuitem"
            onClick={() => setOpen(false)}
          >
            <Sparkles aria-hidden="true" />
            3D-фон
          </Link>
          {can(session, 'system.view') ? (
            <Link
              className="dropdown__item"
              to="/system"
              role="menuitem"
              onClick={() => setOpen(false)}
            >
              <ShieldCheck aria-hidden="true" />
              Система
            </Link>
          ) : null}
          <button
            type="button"
            className="dropdown__item dropdown__item--danger"
            role="menuitem"
            onClick={logout}
          >
            <LogOut aria-hidden="true" />
            Выйти
          </button>
        </div>
      ) : null}
    </div>
  );
}