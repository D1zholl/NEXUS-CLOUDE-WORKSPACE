import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Copy,
  KeyRound,
  RefreshCw,
  Server,
  ShieldCheck,
  Terminal,
  Trash2,
  UserRound,
} from 'lucide-react';

import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  Field,
  LoadingInline,
  Modal,
  PageHead,
  SectionTitle,
} from '../components/ui';
import {
  api,
  errorMessage,
  launcherRequest,
  probeRepository,
  type LauncherToken,
  type LauncherTokenIssued,
} from '../services/api';
import { formatDate, relativeTime } from '../lib/format';
import { useCan, useSession } from '../stores/auth';
import { toast } from '../stores/ui';

const EXPIRY_OPTIONS = [
  { value: 24, label: '24 часа' },
  { value: 24 * 30, label: '30 дней' },
  { value: 24 * 90, label: '90 дней' },
  { value: 24 * 365, label: '1 год' },
];

export function LauncherPage() {
  const queryClient = useQueryClient();
  const session = useSession();
  const canManage = useCan('launcher.manage');

  const [createOpen, setCreateOpen] = useState(false);
  const [revokeTarget, setRevokeTarget] = useState<LauncherToken | null>(null);
  const [busy, setBusy] = useState(false);
  const [issued, setIssued] = useState<LauncherTokenIssued | null>(null);
  const [form, setForm] = useState({
    name: '',
    username: session?.user.username ?? '',
    password: '',
    scopes: ['launcher.catalog', 'launcher.artifacts'] as string[],
    expiresInHours: 24 * 30,
  });
  const [probeToken, setProbeToken] = useState('');
  const [probeResult, setProbeResult] = useState<string | null>(null);
  const [probeBusy, setProbeBusy] = useState(false);
  const [catalogChannel, setCatalogChannel] = useState<'stable' | 'beta'>('stable');
  const [catalogProbe, setCatalogProbe] = useState<string | null>(null);
  const [catalogBusy, setCatalogBusy] = useState(false);

  const tokens = useQuery({
    queryKey: ['launcher-tokens'],
    queryFn: () => api.listLauncherTokens(),
    enabled: canManage,
  });

  const repository = useQuery({
    queryKey: ['launcher-repository'],
    queryFn: () => api.repositoryInfo(),
    enabled: canManage,
  });

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: ['launcher-tokens'] });
  };

  const issueToken = async (): Promise<void> => {
    setBusy(true);
    try {
      const result = await api.issueLauncherToken({
        name: form.name.trim(),
        username: form.username.trim(),
        password: form.password,
        scopes: form.scopes,
        expiresInHours: form.expiresInHours,
      });
      setIssued(result);
      setCreateOpen(false);
      setForm((prev) => ({ ...prev, name: '', password: '' }));
      invalidate();
      toast('Токен лаунчера создан.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (): Promise<void> => {
    if (!revokeTarget) return;
    setBusy(true);
    try {
      await api.revokeLauncherToken(revokeTarget.id);
      setRevokeTarget(null);
      invalidate();
      toast('Токен отозван.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const probeCatalog = async (): Promise<void> => {
    setCatalogBusy(true);
    setCatalogProbe(null);
    try {
      const result = await probeRepository(catalogChannel);
      setCatalogProbe(
        `Каталог ${catalogChannel} доступен: ${result.packCount} сборок.` +
          (result.manifestVersion !== null
            ? `\nПервый манифест: manifestVersion=${result.manifestVersion}, файлов ${result.fileCount}.`
            : '\nПубличных сборок нет — манифест проверять нечего.') +
          (result.names.length ? `\n${result.names.join('\n')}` : ''),
      );
    } catch (error) {
      setCatalogProbe(`Ошибка: ${errorMessage(error)}`);
    } finally {
      setCatalogBusy(false);
    }
  };

  const probe = async (): Promise<void> => {
    const token = probeToken.trim();
    if (!token) return;
    setProbeBusy(true);
    setProbeResult(null);
    try {
      const catalog = await launcherRequest<{
        count: number;
        builds: Array<{ slug: string; name: string; latestRelease: string | null }>;
      }>(token, '/launcher/catalog');
      const list = catalog.builds
        .map((build) => `${build.name} (${build.slug}) → ${build.latestRelease ?? '—'}`)
        .join('\n');
      setProbeResult(
        `Каталог доступен: ${catalog.count} сборок.\n${list || 'Публичных сборок нет.'}`,
      );
    } catch (error) {
      setProbeResult(`Ошибка: ${errorMessage(error)}`);
    } finally {
      setProbeBusy(false);
    }
  };

  const list = tokens.data ?? [];
  const active = list.filter((token) => !token.revokedAt);

  return (
    <>
      <PageHead
        eyebrow="Пространство"
        title="Launcher API"
        description="Токены для собственного лаунчера, каталог опубликованных сборок, манифесты и артефакты с поддержкой Range."
        actions={
          canManage ? (
            <Button
              variant="primary"
              icon={<KeyRound aria-hidden="true" />}
              onClick={() => setCreateOpen(true)}
            >
              Выдать токен
            </Button>
          ) : undefined
        }
      />

      <div className="card launcher-integration">
        <SectionTitle
          title="Подключение MyLauncher"
          meta={repository.data ? `${repository.data.publicPacks} публичных сборок` : 'репозиторий'}
        />
        {repository.isLoading ? (
          <LoadingInline label="Параметры репозитория" />
        ) : repository.isError ? (
          <Alert>{errorMessage(repository.error)}</Alert>
        ) : repository.data ? (
          <div className="stack">
            <Field
              label="Адрес репозитория"
              htmlFor="repository-base-url"
              hint="Вставьте в MyLauncher → «Собственные сборки» → «Адрес репозитория»."
            >
              <div className="row" style={{ gap: 8 }}>
                <input
                  id="repository-base-url"
                  className="input mono"
                  readOnly
                  value={repository.data.baseUrl}
                />
                <Button
                  icon={<Copy aria-hidden="true" />}
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(repository.data!.baseUrl);
                      toast('Адрес скопирован.', 'success');
                    } catch {
                      toast('Скопируйте адрес вручную.', 'warning');
                    }
                  }}
                >
                  Копировать
                </Button>
              </div>
            </Field>
            <ol className="launcher-steps">
              <li>Откройте MyLauncher → «Собственные сборки».</li>
              <li>Вставьте адрес выше и выберите канал stable или beta.</li>
              <li>Нажмите «Подключить» — появится каталог опубликованных публичных сборок.</li>
            </ol>
            <Alert tone="info">
              В анонимный каталог попадают только сборки с видимостью «Публичная» и опубликованным
              релизом. Stock MyLauncher не передаёт токен, поэтому приватная сборка не отобразится в
              нём даже после публикации — смените видимость на странице сборки.
            </Alert>
            <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
              <select
                className="select"
                value={catalogChannel}
                onChange={(event) =>
                  setCatalogChannel(event.target.value === 'beta' ? 'beta' : 'stable')
                }
                aria-label="Канал для проверки"
              >
                <option value="stable">stable</option>
                <option value="beta">beta</option>
              </select>
              <Button
                icon={<Server aria-hidden="true" />}
                loading={catalogBusy}
                onClick={() => void probeCatalog()}
              >
                Проверить каталог и манифест
              </Button>
              <Badge tone="info" plain>
                <Terminal aria-hidden="true" style={{ width: 11, height: 11 }} /> Client:{' '}
                {repository.data.client}
              </Badge>
            </div>
            {catalogProbe ? <pre className="cloud-preview-text">{catalogProbe}</pre> : null}
            <Alert tone="info">{repository.data.tokenNote}</Alert>
            <details>
              <summary className="tiny muted">Публичные endpoints репозитория</summary>
              <pre className="cloud-preview-text">
                {Object.entries(repository.data.endpoints)
                  .map(([name, path]) => `${name.padEnd(9)} ${path}`)
                  .join('\n')}
              </pre>
            </details>
          </div>
        ) : null}
      </div>

      <div className="launcher-grid">
        <div className="card">
          <SectionTitle
            title="Токены лаунчера"
            meta={`${active.length} активных из ${list.length}`}
          />
          {!canManage ? (
            <Alert tone="info">
              Управление токенами доступно с правом <span className="mono">launcher.manage</span>.
            </Alert>
          ) : tokens.isLoading ? (
            <LoadingInline label="Загрузка токенов" />
          ) : tokens.isError ? (
            <Alert>{errorMessage(tokens.error)}</Alert>
          ) : list.length === 0 ? (
            <EmptyState icon={<KeyRound aria-hidden="true" />} title="Токенов пока нет">
              Лаунчер авторизуется по отдельному токену. Выдайте его после настройки сборок.
            </EmptyState>
          ) : (
            <div className="stack">
              {list.map((token) => {
                const expired =
                  token.expiresAt !== null && new Date(token.expiresAt).getTime() < Date.now();
                return (
                  <article className="token-card" key={token.id}>
                    <div className="row row--between" style={{ gap: 10, flexWrap: 'wrap' }}>
                      <div style={{ minWidth: 0 }}>
                        <div className="row" style={{ gap: 8, alignItems: 'center' }}>
                          <span className="token-card__name">{token.name}</span>
                          {token.revokedAt ? (
                            <Badge plain>отозван</Badge>
                          ) : expired ? (
                            <Badge tone="warn">истёк</Badge>
                          ) : (
                            <Badge tone="ok">активен</Badge>
                          )}
                        </div>
                        <div className="tiny muted" style={{ marginTop: 4 }}>
                          <UserRound aria-hidden="true" style={{ width: 11, height: 11 }} />{' '}
                          {token.username} · создан {formatDate(token.createdAt)}
                          {token.lastUsedAt
                            ? ` · использован ${relativeTime(token.lastUsedAt)}`
                            : ''}
                          {token.expiresAt ? ` · до ${formatDate(token.expiresAt)}` : ''}
                        </div>
                        <div className="permission-chips" style={{ marginTop: 6 }}>
                          {token.scopes.map((scope) => (
                            <span className="chip" key={scope}>
                              {scope}
                            </span>
                          ))}
                        </div>
                      </div>
                      {!token.revokedAt ? (
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={<Trash2 aria-hidden="true" />}
                          onClick={() => setRevokeTarget(token)}
                        >
                          Отозвать
                        </Button>
                      ) : null}
                    </div>
                  </article>
                );
              })}
            </div>
          )}
        </div>

        <div className="card">
          <SectionTitle title="Проверка токена" meta="запрос к /launcher/catalog" />
          <div className="stack">
            <Field
              label="Токен"
              htmlFor="probe-token"
              hint="Токен передаётся в заголовке Authorization: Bearer."
            >
              <input
                id="probe-token"
                className="input mono"
                value={probeToken}
                onChange={(event) => setProbeToken(event.target.value)}
                placeholder="nxl_…"
                autoComplete="off"
              />
            </Field>
            <Button
              icon={<ShieldCheck aria-hidden="true" />}
              loading={probeBusy}
              disabled={probeToken.trim().length === 0}
              onClick={() => void probe()}
            >
              Проверить каталог
            </Button>
            {probeResult ? <pre className="cloud-preview-text">{probeResult}</pre> : null}
          </div>
        </div>
      </div>

      <div className="card launcher-docs">
        <SectionTitle title="Контракт API" meta="репозиторий + внутренний Launcher API" />
        <pre className="cloud-preview-text">{`# MyLauncher (публичный, без токена)
GET  /api/packs?channel=stable|beta
GET  /api/packs/{packId}/versions?channel=stable|beta
GET  /api/packs/{packId}/versions/{version}/manifest?channel=stable|beta
GET  /api/packs/{packId}/icon
GET  /api/files/sha256:{hex}?packId=&version=&channel=

# Внутренний Launcher API (Bearer, для других клиентов)
GET  /api/v1/launcher/catalog
GET  /api/v1/launcher/builds
GET  /api/v1/launcher/builds/{slug}
GET  /api/v1/launcher/builds/{slug}/releases
GET  /api/v1/launcher/builds/{slug}/latest/manifest
GET  /api/v1/launcher/builds/{slug}/releases/{version}/manifest
GET  /api/v1/launcher/artifacts/{artifact_id}
POST /api/v1/launcher/auth/token      {"name","username","password","scopes?","expiresInHours?"}
POST /api/v1/launcher/auth/revoke

Заголовок: Authorization: Bearer nxl_…
Scopes:    launcher.catalog, launcher.artifacts`}</pre>
        <div className="row" style={{ gap: 8, marginTop: 10, flexWrap: 'wrap' }}>
          <Badge tone="info" plain>
            <Terminal aria-hidden="true" style={{ width: 11, height: 11 }} /> Range и 416
            поддерживаются
          </Badge>
          <Badge tone="info" plain>
            SHA-256 в заголовке X-File-SHA256
          </Badge>
          <Badge tone="info" plain>
            только опубликованные сборки
          </Badge>
        </div>
      </div>

      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="Новый токен лаунчера"
        description="Токен привязан к пользователю и набору scopes. Пароль не сохраняется."
        footer={
          <>
            <Button onClick={() => setCreateOpen(false)}>Отмена</Button>
            <Button
              variant="primary"
              loading={busy}
              disabled={
                form.name.trim().length === 0 ||
                form.username.trim().length === 0 ||
                form.password.length === 0 ||
                form.scopes.length === 0
              }
              onClick={() => void issueToken()}
            >
              Выдать токен
            </Button>
          </>
        }
      >
        <div className="stack">
          <Field label="Имя клиента" htmlFor="token-name" hint="Например, «Лаунчер — ноутбук».">
            <input
              id="token-name"
              className="input"
              value={form.name}
              maxLength={80}
              onChange={(event) => setForm((prev) => ({ ...prev, name: event.target.value }))}
            />
          </Field>
          <Field label="Пользователь" htmlFor="token-user">
            <input
              id="token-user"
              className="input mono"
              value={form.username}
              onChange={(event) => setForm((prev) => ({ ...prev, username: event.target.value }))}
              autoComplete="username"
            />
          </Field>
          <Field label="Пароль" htmlFor="token-password" hint="Проверяется один раз при выдаче.">
            <input
              id="token-password"
              className="input"
              type="password"
              value={form.password}
              onChange={(event) => setForm((prev) => ({ ...prev, password: event.target.value }))}
              autoComplete="current-password"
            />
          </Field>
          <Field label="Срок действия" htmlFor="token-expiry">
            <select
              id="token-expiry"
              className="select"
              value={form.expiresInHours}
              onChange={(event) =>
                setForm((prev) => ({ ...prev, expiresInHours: Number(event.target.value) }))
              }
            >
              {EXPIRY_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Разрешённые scopes">
            <div className="stack" style={{ gap: 6 }}>
              {[
                { value: 'launcher.catalog', label: 'Каталог и манифесты' },
                { value: 'launcher.artifacts', label: 'Скачивание артефактов' },
              ].map((scope) => (
                <label className="checkbox" key={scope.value}>
                  <input
                    type="checkbox"
                    checked={form.scopes.includes(scope.value)}
                    onChange={(event) =>
                      setForm((prev) => ({
                        ...prev,
                        scopes: event.target.checked
                          ? [...prev.scopes, scope.value]
                          : prev.scopes.filter((value) => value !== scope.value),
                      }))
                    }
                  />
                  {scope.label} <span className="mono tiny muted">{scope.value}</span>
                </label>
              ))}
            </div>
          </Field>
        </div>
      </Modal>

      <Modal
        open={issued !== null}
        onClose={() => setIssued(null)}
        title="Токен создан"
        description="Скопируйте его сейчас: повторно токен показать нельзя."
        footer={
          <Button variant="primary" onClick={() => setIssued(null)}>
            Готово
          </Button>
        }
      >
        {issued ? (
          <div className="stack">
            <Field label="Токен">
              <div className="row" style={{ gap: 8 }}>
                <input className="input mono" readOnly value={issued.token} />
                <Button
                  icon={<Copy aria-hidden="true" />}
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(issued.token);
                      toast('Токен скопирован.', 'success');
                    } catch {
                      toast('Скопируйте токен вручную.', 'warning');
                    }
                  }}
                >
                  Копировать
                </Button>
              </div>
            </Field>
            <Alert tone="info">
              Scopes: {issued.scopes.join(', ')}
              {issued.expiresAt ? ` · действует до ${formatDate(issued.expiresAt)}` : ''}
            </Alert>
          </div>
        ) : null}
      </Modal>

      <ConfirmDialog
        open={revokeTarget !== null}
        title="Отозвать токен?"
        description={
          revokeTarget
            ? `Клиент «${revokeTarget.name}» потеряет доступ к каталогу и артефактам.`
            : ''
        }
        confirmLabel="Отозвать"
        busy={busy}
        onConfirm={() => void revoke()}
        onCancel={() => setRevokeTarget(null)}
      />

      <span className="hidden">
        <RefreshCw aria-hidden="true" />
      </span>
    </>
  );
}

export default LauncherPage;
