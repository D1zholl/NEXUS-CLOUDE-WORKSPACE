import { useState } from 'react';
import type { FormEvent } from 'react';
import { Navigate, Link, useLocation, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  ArrowUpRight,
  Boxes,
  Cloud,
  Fingerprint,
  LockKeyhole,
  ShieldCheck,
  Terminal,
} from 'lucide-react';

import { LogoMark } from '../components/Brand';
import { PasswordField } from '../components/ui';
import { api, errorMessage } from '../services/api';
import { useAuth } from '../stores/auth';

interface LocationState {
  from?: string;
}

export function LoginPage() {
  const status = useAuth((state) => state.status);
  const login = useAuth((state) => state.login);
  const navigate = useNavigate();
  const location = useLocation();

  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const setup = useQuery({
    queryKey: ['setup-status'],
    queryFn: api.setupStatus,
    retry: false,
    staleTime: 30_000,
  });

  const state = location.state as LocationState | null;
  const target = state?.from && state.from !== '/login' ? state.from : '/';

  if (status === 'authenticated') {
    return <Navigate to={target} replace />;
  }

  const submit = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (busy) return;
    setError(null);
    setBusy(true);
    try {
      await login(username.trim(), password);
      navigate(target, { replace: true });
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="auth">
      <section className="auth__intro">
        <Link
          to="/developers"
          className="auth__brand"
          title="О разработчиках NEXUS WORKSPACE"
          aria-label="О разработчиках NEXUS WORKSPACE"
        >
          <LogoMark size={22} />
          <div>
            <div className="brand__text">NEXUS WORKSPACE</div>
            <span className="brand__sub">ЦИФРОВОЕ ПРОСТРАНСТВО</span>
          </div>
          <span className="auth__brand-hint">
            Разработчики
            <ArrowUpRight aria-hidden="true" />
          </span>
        </Link>

        <div className="auth__hero">
          <div className="eyebrow">Закрытая платформа</div>
          <h1 className="display">
            Облачное рабочее пространство <span> для различных задач</span>
          </h1>
          <p>
            Единая точка входа для управления релизами, хранения артефактов и совместной работы
            команды. Доступ выдаётся администратором, каждое действие фиксируется в журнале.
          </p>
          <div className="auth__features">
            <span className="auth__feature">
              <Boxes aria-hidden="true" />
              Minecraft Build Manager
            </span>
            <span className="auth__feature">
              <Cloud aria-hidden="true" />
              NEXUS Cloud Storage
            </span>
            <span className="auth__feature">
              <Fingerprint aria-hidden="true" />
              Роли и права
            </span>
          </div>
        </div>

        <div className="auth__secure">
          <ShieldCheck aria-hidden="true" />
          Серверные сессии · HttpOnly · CSRF-защита
        </div>
      </section>

      <section className="auth__panel">
        <div className="auth__card">
          <h2>Вход в систему</h2>
          <div className="auth__sub">
            Используйте учётную запись, выданную администратором NEXUS.
          </div>

          <form className="auth__form" data-testid="login-form" onSubmit={submit}>
            <div className="field">
              <label className="field__label" htmlFor="login-username">
                Имя пользователя
              </label>
              <input
                id="login-username"
                className="input"
                type="text"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                autoComplete="username"
                autoCapitalize="none"
                spellCheck={false}
                placeholder="username"
                maxLength={64}
                required
                autoFocus
              />
            </div>

            <div className="field">
              <label className="field__label" htmlFor="login-password">
                Пароль
              </label>
              <PasswordField
                id="login-password"
                value={password}
                onChange={setPassword}
                autoComplete="current-password"
                placeholder="••••••••••"
              />
            </div>

            {error ? (
              <div className="auth__error" role="alert">
                <AlertTriangle aria-hidden="true" />
                <span>{error}</span>
              </div>
            ) : null}

            {setup.data?.needsSetup ? (
              <div className="auth__error auth__error--warn" role="status">
                <Terminal aria-hidden="true" />
                <span>
                  В системе ещё нет пользователей. Создайте первого администратора на сервере:
                  <code style={{ display: 'block', marginTop: 6, fontSize: 10.5 }}>
                    cargo run -- create-admin --username admin
                  </code>
                </span>
              </div>
            ) : null}

            <button className="btn btn--primary btn--block" type="submit" disabled={busy}>
              {busy ? (
                <span className="btn__spinner" aria-hidden="true" />
              ) : (
                <LockKeyhole aria-hidden="true" />
              )}
              {busy ? 'Проверка доступа…' : 'Войти в пространство'}
            </button>
          </form>

          <div className="auth__secure-row">
            <LockKeyhole aria-hidden="true" />
            Пароль передаётся только по защищённому соединению
          </div>

          {setup.data?.needsSetup ? (
            <div className="setup-note">
              Сервер работает, база подключена. После создания администратора обновите эту страницу.
              Пароль должен содержать не менее {setup.data.minPasswordLength} символов.
            </div>
          ) : null}
        </div>
      </section>

      <div className="auth__footer">
        NEXUS WORKSPACE · локальная инфраструктура · доступ по приглашению
        <Link to="/developers" className="auth__footer-link">
          {' '}
          · Разработчики
        </Link>
      </div>
    </div>
  );
}
