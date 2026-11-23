import { useCallback, useEffect, useState } from 'react';
import { useParams } from 'react-router-dom';
import { Download, FileIcon, Link2, Lock, LogIn, ShieldAlert } from 'lucide-react';

import { Button, Field, LoadingScreen, Spinner } from '../components/ui';
import {
  ApiRequestError,
  api,
  fetchPublicShare,
  publicShareDownloadUrl,
  unlockPublicShare,
  type PublicShareInfo,
} from '../services/api';
import { formatBytes, formatDate } from '../lib/format';

function shareErrorMessage(error: unknown): string {
  if (error instanceof ApiRequestError) {
    switch (error.code) {
      case 'SHARE_PASSWORD_INVALID':
      case 'SHARE_PASSWORD_REQUIRED':
        return 'Неверный пароль.';
      case 'DOWNLOAD_LIMIT_REACHED':
        return 'Лимит скачиваний по этой ссылке исчерпан.';
      case 'RATE_LIMITED':
        return 'Слишком много попыток. Повторите позже.';
      case 'SHARE_TICKET_INVALID':
        return 'Срок действия разрешения истёк. Откройте ссылку заново.';
      case 'SHARE_AUTH_REQUIRED':
        return 'Войдите, чтобы скачать файл.';
      case 'NOT_FOUND':
      case 'SHARE_UNAVAILABLE':
        return 'Ссылка не найдена, отозвана или срок её действия истёк.';
      default:
        return error.message;
    }
  }
  return error instanceof Error ? error.message : 'Ошибка скачивания.';
}

/**
 * Публичная страница ссылки Cloud Storage.
 *
 * Доступна без авторизации: показывает только имя, размер и срок действия.
 * Пароль проверяется сервером при скачивании.
 */
export function PublicSharePage() {
  const { token = '' } = useParams<{ token: string }>();
  const [info, setInfo] = useState<PublicShareInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState('');
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [authRequired, setAuthRequired] = useState(false);
  const [loginName, setLoginName] = useState('');
  const [loginPassword, setLoginPassword] = useState('');
  const [loginBusy, setLoginBusy] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);

  const load = useCallback(async (): Promise<void> => {
    setLoading(true);
    try {
      const data = await fetchPublicShare(token);
      setInfo(data);
      setError(null);
      setAuthRequired(false);
    } catch (reason) {
      if (reason instanceof ApiRequestError && reason.status === 401) {
        // E3: ссылка доступна только авторизованным.
        setAuthRequired(true);
        setInfo(null);
        setError(null);
      } else {
        setAuthRequired(false);
        setError(reason instanceof Error ? reason.message : 'Ссылка недоступна.');
      }
    } finally {
      setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    void load();
  }, [load]);

  const login = async (): Promise<void> => {
    setLoginBusy(true);
    setLoginError(null);
    try {
      await api.login(loginName.trim(), loginPassword);
      setLoginPassword('');
      await load();
    } catch (reason) {
      setLoginError(reason instanceof Error ? reason.message : 'Не удалось войти.');
    } finally {
      setLoginBusy(false);
    }
  };

  const download = async (): Promise<void> => {
    if (!info) return;
    setDownloading(true);
    setDownloadError(null);
    try {
      // Пароль проверяется в теле запроса; в ссылке остаётся только тикет.
      const { ticket } = await unlockPublicShare(token, info.hasPassword ? password : undefined);
      const link = document.createElement('a');
      link.href = publicShareDownloadUrl(token, ticket);
      link.rel = 'noopener';
      link.download = info.name;
      link.click();
    } catch (reason) {
      setDownloadError(shareErrorMessage(reason));
    } finally {
      setDownloading(false);
    }
  };

  if (loading) return <LoadingScreen label="Открытие ссылки" />;

  return (
    <div className="public-share">
      <div className="public-share__card glass">
        <div className="public-share__brand">
          <Link2 aria-hidden="true" />
          NEXUS CLOUD
        </div>

        {!info && authRequired ? (
          <div className="stack" style={{ gap: 14 }}>
            <div className="public-share__error">
              <LogIn aria-hidden="true" />
              <h1>Только для авторизованных</h1>
              <p>
                Владелец открыл эту ссылку только для пользователей NEXUS. Войдите, чтобы скачать
                файл.
              </p>
            </div>
            <Field label="Логин" htmlFor="share-login">
              <input
                id="share-login"
                className="input"
                value={loginName}
                onChange={(event) => setLoginName(event.target.value)}
                autoComplete="username"
              />
            </Field>
            <Field label="Пароль" htmlFor="share-login-password">
              <input
                id="share-login-password"
                className="input"
                type="password"
                value={loginPassword}
                onChange={(event) => setLoginPassword(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') void login();
                }}
                autoComplete="current-password"
              />
            </Field>
            <Button
              variant="primary"
              block
              loading={loginBusy}
              disabled={loginName.trim().length === 0 || loginPassword.length === 0}
              onClick={() => void login()}
              icon={loginBusy ? undefined : <LogIn aria-hidden="true" />}
            >
              Войти и скачать
            </Button>
            {loginError ? <div className="public-share__alert">{loginError}</div> : null}
          </div>
        ) : error || !info ? (
          <div className="public-share__error">
            <ShieldAlert aria-hidden="true" />
            <h1>Ссылка недоступна</h1>
            <p>{error ?? 'Файл не найден, ссылка отозвана или срок её действия истёк.'}</p>
          </div>
        ) : (
          <>
            <div className="public-share__file">
              <div className="public-share__icon">
                <FileIcon aria-hidden="true" />
              </div>
              <div style={{ minWidth: 0 }}>
                <h1 className="public-share__name">{info.name}</h1>
                <div className="public-share__meta">
                  {formatBytes(info.size)}
                  {info.expiresAt ? ` · до ${formatDate(info.expiresAt)}` : ' · бессрочно'}
                  {info.maxDownloads
                    ? ` · скачиваний ${info.downloadCount} из ${info.maxDownloads}`
                    : ''}
                </div>
              </div>
            </div>

            {info.hasPassword ? (
              <Field label="Пароль" htmlFor="share-pass" hint="Пароль задал владелец файла.">
                <div className="row" style={{ gap: 8 }}>
                  <div className="input-wrap grow">
                    <Lock aria-hidden="true" />
                    <input
                      id="share-pass"
                      className="input input--with-icon"
                      type="password"
                      value={password}
                      onChange={(event) => setPassword(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') void download();
                      }}
                      autoComplete="off"
                    />
                  </div>
                  <Button
                    variant="primary"
                    onClick={() => void download()}
                    loading={downloading}
                    disabled={password.length === 0}
                    icon={downloading ? undefined : <Download aria-hidden="true" />}
                  >
                    Скачать
                  </Button>
                </div>
              </Field>
            ) : (
              <Button
                variant="primary"
                block
                disabled={downloading}
                onClick={() => void download()}
                icon={downloading ? <Spinner /> : <Download aria-hidden="true" />}
              >
                Скачать файл
              </Button>
            )}

            {downloadError ? <div className="public-share__alert">{downloadError}</div> : null}
          </>
        )}

        <div className="public-share__foot">NEXUS WORKSPACE · защищённая ссылка</div>
      </div>
    </div>
  );
}

export default PublicSharePage;
