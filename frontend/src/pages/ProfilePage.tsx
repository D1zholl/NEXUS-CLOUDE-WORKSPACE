import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { KeyRound, LogOut, Palette, ShieldCheck, UserRound } from 'lucide-react';

import {
  Alert,
  Avatar,
  Badge,
  Button,
  ConfirmDialog,
  Field,
  PageHead,
  PasswordField,
  SectionTitle,
} from '../components/ui';
import { api, errorMessage } from '../services/api';
import { formatDate } from '../lib/format';
import { can, useAuth, useSession } from '../stores/auth';
import { toast } from '../stores/ui';

export function ProfilePage() {
  const session = useSession();
  const navigate = useNavigate();
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);

  const setup = useQuery({
    queryKey: ['setup-status'],
    queryFn: api.setupStatus,
    staleTime: 300_000,
  });
  const minLength = setup.data?.minPasswordLength ?? 12;

  const revokeAll = useMutation({
    mutationFn: api.revokeAllSessions,
    onSuccess: () => {
      useAuth.getState().clearSession();
      toast('Все сессии завершены. Войдите заново.', 'info');
      navigate('/login', { replace: true });
    },
    onError: (caught) => toast(errorMessage(caught), 'error'),
  });

  if (!session) return null;

  const submitPassword = async (): Promise<void> => {
    setError(null);
    if (newPassword.length < minLength) {
      setError(`Новый пароль должен содержать не менее ${minLength} символов.`);
      return;
    }
    if (newPassword !== confirmPassword) {
      setError('Пароли не совпадают.');
      return;
    }
    if (newPassword === currentPassword) {
      setError('Новый пароль должен отличаться от текущего.');
      return;
    }
    setBusy(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      setCurrentPassword('');
      setNewPassword('');
      setConfirmPassword('');
      toast('Пароль изменён. Остальные сессии завершены.', 'success');
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHead
        eyebrow="Аккаунт"
        title="Профиль"
        description="Данные вашей учётной записи, активная сессия и управление паролем."
        actions={
          <Button
            variant="ghost"
            icon={<Palette aria-hidden="true" />}
            onClick={() => navigate('/system/background')}
          >
            Настроить фон
          </Button>
        }
      />

      <div className="grid-2">
        <div className="card">
          <div className="row" style={{ gap: 14, marginBottom: 18 }}>
            <Avatar name={session.user.displayName || session.user.username} />
            <div>
              <div className="h2">{session.user.displayName || session.user.username}</div>
              <div className="mono tiny muted">@{session.user.username}</div>
            </div>
            <div className="grow" />
            {session.user.isSuperadmin ? <Badge tone="info">superadmin</Badge> : null}
          </div>

          <div className="info-list">
            <div className="info-list__row">
              <span>Идентификатор</span>
              <span className="mono">{session.user.id}</span>
            </div>
            <div className="info-list__row">
              <span>Роли</span>
              <span>
                {session.user.roles.length === 0
                  ? 'без ролей'
                  : session.user.roles.map((role) => role.name).join(', ')}
              </span>
            </div>
            <div className="info-list__row">
              <span>Разрешений</span>
              <span>{session.user.permissions.length}</span>
            </div>
            <div className="info-list__row">
              <span>Сессия истекает</span>
              <span>{formatDate(session.session.expiresAt)}</span>
            </div>
            <div className="info-list__row">
              <span>Режим доступа</span>
              <span>
                {session.user.isSuperadmin ? 'суперадминистратор' : 'по ролям и исключениям'}
              </span>
            </div>
          </div>

          {can(session, 'system.view') ? (
            <div style={{ marginTop: 16 }}>
              <Link to="/system" className="btn btn--sm btn--ghost">
                <ShieldCheck aria-hidden="true" />
                Панель системы
              </Link>
            </div>
          ) : null}
        </div>

        <div className="card">
          <SectionTitle title="Смена пароля" meta={`минимум ${minLength} символов`} />
          <div className="stack" style={{ gap: 14 }}>
            <Field label="Текущий пароль" htmlFor="current-password">
              <PasswordField
                id="current-password"
                value={currentPassword}
                onChange={setCurrentPassword}
                autoComplete="current-password"
              />
            </Field>
            <Field
              label="Новый пароль"
              htmlFor="new-password"
              hint={`Не менее ${minLength} символов`}
            >
              <PasswordField
                id="new-password"
                value={newPassword}
                onChange={setNewPassword}
                autoComplete="new-password"
              />
            </Field>
            <Field label="Повторите новый пароль" htmlFor="confirm-password">
              <PasswordField
                id="confirm-password"
                value={confirmPassword}
                onChange={setConfirmPassword}
                autoComplete="new-password"
              />
            </Field>
            {error ? <Alert>{error}</Alert> : null}
            <Button
              variant="primary"
              onClick={() => void submitPassword()}
              loading={busy}
              icon={<KeyRound aria-hidden="true" />}
              disabled={!currentPassword || !newPassword || !confirmPassword}
            >
              Изменить пароль
            </Button>
            <div className="tiny muted" style={{ lineHeight: 1.6 }}>
              После смены пароля все остальные сессии вашей учётной записи завершаются. Текущая
              сессия сохраняется.
            </div>
          </div>
        </div>
      </div>

      <div className="card">
        <SectionTitle title="Права доступа" meta="действуют в этой сессии" />
        {session.user.isSuperadmin ? (
          <Alert tone="info">
            Суперадминистратор обходит проверки прав: доступны все модули и системные операции.
          </Alert>
        ) : session.user.permissions.length === 0 ? (
          <div className="empty-mini muted" style={{ fontSize: 12.5 }}>
            Права не назначены. Обратитесь к администратору.
          </div>
        ) : (
          <div className="permission-chips" style={{ maxWidth: 'none' }}>
            {session.user.permissions.map((permission) => (
              <span className="chip" key={permission}>
                {permission}
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="card" style={{ borderColor: 'rgba(var(--danger-rgb), 0.28)' }}>
        <SectionTitle title="Сессии" meta="безопасность" />
        <div className="row row--between wrap" style={{ gap: 12 }}>
          <div style={{ maxWidth: 640 }}>
            <div className="row" style={{ gap: 8, marginBottom: 4 }}>
              <UserRound
                aria-hidden="true"
                style={{ width: 15, height: 15, color: 'var(--accent)' }}
              />
              <strong style={{ fontSize: 13 }}>Завершить все сессии</strong>
            </div>
            <div className="tiny muted" style={{ lineHeight: 1.65 }}>
              Завершает все активные сессии, включая текущую. Потребуется повторный вход.
              Используйте, если считаете, что cookie могла быть скомпрометирована.
            </div>
          </div>
          <Button
            variant="danger"
            icon={<LogOut aria-hidden="true" />}
            onClick={() => setRevokeOpen(true)}
          >
            Завершить все
          </Button>
        </div>
      </div>

      <ConfirmDialog
        open={revokeOpen}
        title="Завершить все сессии?"
        description="Все активные сессии будут отозваны, включая текущую. Вы вернётесь на экран входа."
        confirmLabel="Завершить"
        busy={revokeAll.isPending}
        onCancel={() => setRevokeOpen(false)}
        onConfirm={() => revokeAll.mutate()}
      />
    </>
  );
}
