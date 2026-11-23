import { useEffect, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ChevronLeft,
  ChevronRight,
  KeyRound,
  LogOut,
  Pencil,
  Plus,
  Search,
  ShieldAlert,
  Trash2,
  Unlock,
  UserRound,
} from 'lucide-react';

import {
  Alert,
  Avatar,
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  Field,
  LoadingInline,
  Modal,
  PageHead,
  PasswordField,
} from '../components/ui';
import {
  api,
  errorMessage,
  type Permission,
  type Role,
  type UserDetail,
  type UserSummary,
} from '../services/api';
import { formatDate, formatBytes, relativeTime } from '../lib/format';
import { useCan, useSession } from '../stores/auth';
import { toast } from '../stores/ui';

type EditorState = { mode: 'create' } | { mode: 'edit'; user: UserDetail };

const MODULE_NAMES: Record<string, string> = {
  minecraft: 'Minecraft',
  cloud: 'Cloud Storage',
  system: 'Система',
};

interface PermissionGroup {
  module: string;
  label: string;
  items: Permission[];
}

function groupPermissions(permissions: Permission[]): PermissionGroup[] {
  const map = new Map<string, Permission[]>();
  for (const permission of permissions) {
    const list = map.get(permission.module) ?? [];
    list.push(permission);
    map.set(permission.module, list);
  }
  return [...map.entries()].map(([module, items]) => ({
    module,
    label: MODULE_NAMES[module] ?? module,
    items: items.sort((a, b) => a.key.localeCompare(b.key)),
  }));
}

export function UsersPage() {
  const queryClient = useQueryClient();
  const session = useSession();
  const canViewSystem = useCan('system.view');

  const [page, setPage] = useState(1);
  const [searchInput, setSearchInput] = useState('');
  const [search, setSearch] = useState('');
  const [status, setStatus] = useState('');
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [detailLoadingId, setDetailLoadingId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<UserSummary | null>(null);
  const [resetTarget, setResetTarget] = useState<UserSummary | null>(null);
  const [sessionsTarget, setSessionsTarget] = useState<UserSummary | null>(null);

  const users = useQuery({
    queryKey: ['users', page, search, status],
    queryFn: () => api.listUsers({ page, perPage: 25, q: search, status }),
  });

  const roles = useQuery({
    queryKey: ['roles'],
    queryFn: api.listRoles,
    staleTime: 120_000,
  });

  const permissions = useQuery({
    queryKey: ['permissions'],
    queryFn: api.listPermissions,
    staleTime: 300_000,
    enabled: canViewSystem,
  });

  const setup = useQuery({
    queryKey: ['setup-status'],
    queryFn: api.setupStatus,
    staleTime: 60_000,
  });
  const minPasswordLength = setup.data?.minPasswordLength ?? 12;

  const invalidateUsers = async (): Promise<void> => {
    await queryClient.invalidateQueries({ queryKey: ['users'] });
  };

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteUser(id),
    onSuccess: async () => {
      toast('Пользователь удалён.', 'success');
      setDeleteTarget(null);
      await invalidateUsers();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const unlock = useMutation({
    mutationFn: (id: string) => api.unlockUser(id),
    onSuccess: async () => {
      toast('Блокировка снята.', 'success');
      await invalidateUsers();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const revokeSessions = useMutation({
    mutationFn: (id: string) => api.revokeUserSessions(id),
    onSuccess: async (result) => {
      toast(`Отозвано сессий: ${result.revoked}.`, 'success');
      setSessionsTarget(null);
      await invalidateUsers();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const submitSearch = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    setPage(1);
    setSearch(searchInput.trim());
  };

  const resetFilters = (): void => {
    setSearchInput('');
    setSearch('');
    setStatus('');
    setPage(1);
  };

  const openEditor = async (user: UserSummary): Promise<void> => {
    setDetailLoadingId(user.id);
    try {
      const detail = await api.getUser(user.id);
      setEditor({ mode: 'edit', user: detail });
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setDetailLoadingId(null);
    }
  };

  const total = users.data?.total ?? 0;
  const perPage = users.data?.perPage ?? 25;
  const maxPage = Math.max(1, Math.ceil(total / perPage));

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Пользователи"
        description="Учётные записи, роли и персональные переопределения прав. Пароли хранятся только в виде Argon2id-хешей."
        actions={
          <Button
            variant="primary"
            icon={<Plus aria-hidden="true" />}
            onClick={() => setEditor({ mode: 'create' })}
            disabled={roles.isError}
          >
            Новый пользователь
          </Button>
        }
      />

      {roles.isError ? <Alert>Не удалось загрузить роли: {errorMessage(roles.error)}</Alert> : null}

      <div className="table-card">
        <form className="toolbar" onSubmit={submitSearch}>
          <div className="input-wrap">
            <Search aria-hidden="true" />
            <input
              className="input input--with-icon"
              type="search"
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
              placeholder="Имя, логин…"
              aria-label="Поиск пользователей"
              maxLength={120}
            />
          </div>
          <select
            className="select"
            value={status}
            onChange={(event) => {
              setStatus(event.target.value);
              setPage(1);
            }}
            aria-label="Фильтр по статусу"
          >
            <option value="">Все статусы</option>
            <option value="active">Активные</option>
            <option value="disabled">Отключённые</option>
          </select>
          <Button type="submit" size="sm" variant="primary">
            Найти
          </Button>
          {search || status ? (
            <Button type="button" size="sm" variant="ghost" onClick={resetFilters}>
              Сбросить
            </Button>
          ) : null}
          <div className="grow" />
          <span className="tiny muted">
            Всего: {total} · стр. {users.data?.page ?? 1} из {maxPage}
          </span>
        </form>

        {users.isPending ? (
          <LoadingInline label="Загрузка пользователей" />
        ) : users.isError ? (
          <div style={{ padding: 16 }}>
            <Alert>{errorMessage(users.error)}</Alert>
          </div>
        ) : users.data.items.length === 0 ? (
          <EmptyState
            icon={<UserRound aria-hidden="true" />}
            title="Пользователи не найдены"
            action={
              search || status ? (
                <Button size="sm" onClick={resetFilters}>
                  Сбросить фильтры
                </Button>
              ) : undefined
            }
          >
            {search || status
              ? 'По заданным условиям нет ни одной учётной записи.'
              : 'Создайте первого пользователя, чтобы выдать доступ к пространству.'}
          </EmptyState>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Пользователь</th>
                  <th>Роли</th>
                  <th>Статус</th>
                  <th>Последний вход</th>
                  <th>Хранилище</th>
                  <th>Minecraft</th>
                  <th>Cloud</th>
                  <th style={{ textAlign: 'right' }}>Действия</th>
                </tr>
              </thead>
              <tbody>
                {users.data.items.map((user) => {
                  const locked =
                    user.lockedUntil !== null && new Date(user.lockedUntil).getTime() > Date.now();
                  const isSelf = user.id === session?.user.id;
                  return (
                    <tr key={user.id}>
                      <td>
                        <div className="table__user">
                          <Avatar name={user.displayName || user.username} small />
                          <div>
                            <div className="table__user-name">
                              {user.displayName || user.username}{' '}
                              {user.isSuperadmin ? (
                                <Badge tone="info" plain>
                                  superadmin
                                </Badge>
                              ) : null}
                            </div>
                            <div className="table__user-handle">@{user.username}</div>
                          </div>
                        </div>
                      </td>
                      <td>
                        <div className="permission-chips">
                          {user.roles.length === 0 ? (
                            <span className="tiny muted">без ролей</span>
                          ) : (
                            user.roles.map((role) => (
                              <span className="chip" key={role.id}>
                                {role.name}
                              </span>
                            ))
                          )}
                        </div>
                      </td>
                      <td>
                        <div className="stack" style={{ gap: 4 }}>
                          <Badge tone={user.status === 'active' ? 'ok' : 'danger'}>
                            {user.status === 'active' ? 'активен' : 'отключён'}
                          </Badge>
                          {locked ? <Badge tone="warn">временно заблокирован</Badge> : null}
                          {isSelf ? <span className="tiny muted">это вы</span> : null}
                        </div>
                      </td>
                      <td>
                        <div>{relativeTime(user.lastLoginAt)}</div>
                        <div className="tiny muted">создан {formatDate(user.createdAt)}</div>
                      </td>
                      <td>
                        <div className="mono">{formatBytes(user.storageUsed)}</div>
                        <div className="tiny muted">
                          {user.quotaBytes
                            ? `квота ${formatBytes(user.quotaBytes)}`
                            : 'квота по умолчанию'}
                        </div>
                      </td>
                      <td>
                        <Badge
                          tone={user.modules.minecraft ? 'ok' : 'default'}
                          plain
                          title={
                            user.modules.minecraft
                              ? 'Доступ к Minecraft Build Manager'
                              : 'Нет права minecraft.view'
                          }
                        >
                          {user.modules.minecraft ? 'да' : 'нет'}
                        </Badge>
                      </td>
                      <td>
                        <Badge
                          tone={user.modules.cloud ? 'ok' : 'default'}
                          plain
                          title={
                            user.modules.cloud ? 'Доступ к Cloud Storage' : 'Нет права cloud.view'
                          }
                        >
                          {user.modules.cloud ? 'да' : 'нет'}
                        </Badge>
                      </td>
                      <td>
                        <div className="table__actions">
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            title="Редактировать"
                            aria-label={`Редактировать ${user.username}`}
                            loading={detailLoadingId === user.id}
                            onClick={() => void openEditor(user)}
                          >
                            <Pencil aria-hidden="true" />
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            title="Сбросить пароль"
                            aria-label={`Сбросить пароль ${user.username}`}
                            disabled={isSelf}
                            onClick={() => setResetTarget(user)}
                          >
                            <KeyRound aria-hidden="true" />
                          </Button>
                          {locked ? (
                            <Button
                              size="sm"
                              variant="ghost"
                              iconOnly
                              title="Снять блокировку"
                              aria-label={`Снять блокировку ${user.username}`}
                              loading={unlock.isPending && unlock.variables === user.id}
                              onClick={() => unlock.mutate(user.id)}
                            >
                              <Unlock aria-hidden="true" />
                            </Button>
                          ) : null}
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            title="Отозвать сессии"
                            aria-label={`Отозвать сессии ${user.username}`}
                            onClick={() => setSessionsTarget(user)}
                          >
                            <LogOut aria-hidden="true" />
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            iconOnly
                            title="Удалить"
                            aria-label={`Удалить ${user.username}`}
                            disabled={isSelf || user.isSuperadmin}
                            onClick={() => setDeleteTarget(user)}
                          >
                            <Trash2 aria-hidden="true" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}

        <div className="pagination">
          <span>
            Показано {users.data?.items.length ?? 0} из {total}
          </span>
          <div className="pagination__controls">
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              aria-label="Предыдущая страница"
              disabled={page <= 1}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              <ChevronLeft aria-hidden="true" />
            </Button>
            <Button
              size="sm"
              variant="ghost"
              iconOnly
              aria-label="Следующая страница"
              disabled={page >= maxPage}
              onClick={() => setPage((value) => Math.min(maxPage, value + 1))}
            >
              <ChevronRight aria-hidden="true" />
            </Button>
          </div>
        </div>
      </div>

      <UserEditorModal
        editor={editor}
        roles={roles.data ?? []}
        rolesLoading={roles.isPending}
        permissions={permissions.data ?? []}
        permissionsLoading={permissions.isPending}
        permissionsError={permissions.isError ? errorMessage(permissions.error) : null}
        minPasswordLength={minPasswordLength}
        onClose={() => setEditor(null)}
        onSaved={async (message) => {
          setEditor(null);
          toast(message, 'success');
          await invalidateUsers();
        }}
      />

      <ResetPasswordDialog
        user={resetTarget}
        minPasswordLength={minPasswordLength}
        onClose={() => setResetTarget(null)}
        onSaved={async () => {
          setResetTarget(null);
          toast('Пароль изменён, сессии пользователя отозваны.', 'success');
          await invalidateUsers();
        }}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        title="Удалить пользователя?"
        description={
          deleteTarget ? `Учётная запись @${deleteTarget.username} будет удалена безвозвратно.` : ''
        }
        confirmLabel="Удалить"
        busy={remove.isPending}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) remove.mutate(deleteTarget.id);
        }}
      />

      <ConfirmDialog
        open={sessionsTarget !== null}
        title="Отозвать все сессии?"
        description={
          sessionsTarget
            ? `Все активные сессии @${sessionsTarget.username} будут завершены. Пользователю потребуется войти заново.`
            : ''
        }
        confirmLabel="Отозвать"
        busy={revokeSessions.isPending}
        onCancel={() => setSessionsTarget(null)}
        onConfirm={() => {
          if (sessionsTarget) revokeSessions.mutate(sessionsTarget.id);
        }}
      />
    </>
  );
}

type OverrideState = 'inherit' | 'allow' | 'deny';

function UserEditorModal({
  editor,
  roles,
  rolesLoading,
  permissions,
  permissionsLoading,
  permissionsError,
  minPasswordLength,
  onClose,
  onSaved,
}: {
  editor: EditorState | null;
  roles: Role[];
  rolesLoading: boolean;
  permissions: Permission[];
  permissionsLoading: boolean;
  permissionsError: string | null;
  minPasswordLength: number;
  onClose: () => void;
  onSaved: (message: string) => Promise<void>;
}) {
  const [username, setUsername] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [status, setStatus] = useState('active');
  const [roleIds, setRoleIds] = useState<Set<string>>(new Set());
  const [overrides, setOverrides] = useState<Record<string, OverrideState>>({});
  const [quotaGb, setQuotaGb] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!editor) return;
    setError(null);
    setPassword('');
    if (editor.mode === 'create') {
      setUsername('');
      setDisplayName('');
      setStatus('active');
      setRoleIds(new Set(roles.filter((role) => role.key === 'user').map((role) => role.id)));
      setOverrides({});
      setQuotaGb('');
      return;
    }
    const user = editor.user;
    setUsername(user.username);
    setDisplayName(user.displayName);
    setStatus(user.status);
    setRoleIds(new Set(user.roles.map((role) => role.id)));
    setQuotaGb(
      user.quotaBytes ? String(Math.round((user.quotaBytes / (1024 * 1024 * 1024)) * 10) / 10) : '',
    );
    const map: Record<string, OverrideState> = {};
    for (const override of user.permissionOverrides) {
      map[override.key] = override.granted ? 'allow' : 'deny';
    }
    setOverrides(map);
  }, [editor, roles]);

  const grouped = useMemo(() => groupPermissions(permissions), [permissions]);

  if (!editor) return null;

  const isCreate = editor.mode === 'create';
  const isSuperadmin = !isCreate && editor.user.isSuperadmin;

  const toggleRole = (id: string): void => {
    setRoleIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const save = async (): Promise<void> => {
    setError(null);
    const trimmedUsername = username.trim();
    const trimmedDisplay = displayName.trim();
    if (trimmedDisplay.length === 0) {
      setError('Укажите отображаемое имя.');
      return;
    }
    if (isCreate) {
      if (!(trimmedUsername.length >= 3 && trimmedUsername.length <= 32)) {
        setError('Имя пользователя: от 3 до 32 символов.');
        return;
      }
      if (!/^[a-zA-Z0-9._-]+$/.test(trimmedUsername)) {
        setError('Имя пользователя: только латиница, цифры, точка, дефис и подчёркивание.');
        return;
      }
      if (password.length < minPasswordLength) {
        setError(`Пароль должен содержать не менее ${minPasswordLength} символов.`);
        return;
      }
    }

    setBusy(true);
    try {
      if (isCreate) {
        await api.createUser({
          username: trimmedUsername,
          displayName: trimmedDisplay,
          password,
          roleIds: [...roleIds],
          status,
        });
        await onSaved('Пользователь создан.');
      } else {
        const permissionsMap: Record<string, boolean> = {};
        for (const [key, state] of Object.entries(overrides)) {
          if (state === 'allow') permissionsMap[key] = true;
          if (state === 'deny') permissionsMap[key] = false;
        }
        await api.updateUser(editor.user.id, {
          displayName: trimmedDisplay,
          status,
          roleIds: [...roleIds],
          permissions: permissionsMap,
          quotaBytes:
            quotaGb.trim() === '' ? null : Math.round(Number(quotaGb) * 1024 * 1024 * 1024),
        });
        await onSaved('Пользователь обновлён.');
      }
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={busy ? () => undefined : onClose}
      title={isCreate ? 'Новый пользователь' : `Пользователь @${editor.user.username}`}
      description={
        isCreate
          ? 'Учётная запись получит доступ к пространству сразу после создания.'
          : 'Изменения прав применяются при следующем запросе пользователя.'
      }
      wide
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Отмена
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={busy}>
            {isCreate ? 'Создать' : 'Сохранить'}
          </Button>
        </>
      }
    >
      <div className="form-grid">
        <Field
          label="Имя пользователя"
          htmlFor="user-username"
          hint="3–32 символа, латиница и цифры"
        >
          <input
            id="user-username"
            className="input"
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            disabled={!isCreate}
            autoComplete="off"
            maxLength={32}
            required
          />
        </Field>
        <Field label="Отображаемое имя" htmlFor="user-display">
          <input
            id="user-display"
            className="input"
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            maxLength={80}
            required
          />
        </Field>
        {isCreate ? (
          <Field
            label="Пароль"
            htmlFor="user-password"
            hint={`Не менее ${minPasswordLength} символов`}
          >
            <PasswordField
              id="user-password"
              value={password}
              onChange={setPassword}
              autoComplete="new-password"
              placeholder="Временный пароль"
            />
          </Field>
        ) : null}
        <Field label="Статус" htmlFor="user-status">
          <select
            id="user-status"
            className="select"
            value={status}
            onChange={(event) => setStatus(event.target.value)}
          >
            <option value="active">Активен</option>
            <option value="disabled">Отключён</option>
          </select>
        </Field>
        {!isCreate ? (
          <Field
            label="Квота хранения, ГБ"
            htmlFor="user-quota"
            hint="Пусто — значение по умолчанию из системных настроек"
          >
            <input
              id="user-quota"
              className="input"
              type="number"
              min={1}
              step={1}
              placeholder="по умолчанию"
              value={quotaGb}
              onChange={(event) => setQuotaGb(event.target.value)}
            />
          </Field>
        ) : null}
      </div>

      <div>
        <div className="section-title">
          <h2>Роли</h2>
          <span>Права суммируются</span>
        </div>
        {rolesLoading ? (
          <LoadingInline label="Загрузка ролей" />
        ) : (
          <div className="permission-group">
            <div className="permission-group__head">Назначенные роли</div>
            <div className="permission-group__list">
              {roles.map((role) => (
                <label className="checkbox" key={role.id}>
                  <input
                    type="checkbox"
                    checked={roleIds.has(role.id)}
                    onChange={() => toggleRole(role.id)}
                  />
                  <span>
                    {role.name}
                    <span className="tiny muted" style={{ display: 'block' }}>
                      {role.key} · {role.permissions.length} прав
                    </span>
                  </span>
                </label>
              ))}
            </div>
          </div>
        )}
      </div>

      {isCreate ? null : (
        <div>
          <div className="section-title">
            <h2>Персональные исключения</h2>
            <span>Приоритетнее ролей</span>
          </div>
          {isSuperadmin ? (
            <Alert tone="info">
              Суперадминистратор всегда имеет все права. Исключения сохраняются, но не влияют на
              доступ.
            </Alert>
          ) : null}
          {permissionsLoading ? (
            <LoadingInline label="Загрузка каталога прав" />
          ) : permissionsError ? (
            <Alert>{permissionsError}</Alert>
          ) : (
            <div className="stack" style={{ gap: 10 }}>
              {grouped.map((group) => (
                <div className="permission-group" key={group.module}>
                  <div className="permission-group__head">{group.label}</div>
                  <div className="permission-group__list" style={{ gridTemplateColumns: '1fr' }}>
                    {group.items.map((permission) => (
                      <div
                        key={permission.key}
                        className="row row--between"
                        style={{ gap: 12, alignItems: 'center' }}
                      >
                        <div style={{ minWidth: 0 }}>
                          <div style={{ fontSize: 12 }}>{permission.label}</div>
                          <div className="tiny muted mono">{permission.key}</div>
                        </div>
                        <select
                          className="select"
                          style={{
                            width: 132,
                            height: 30,
                            padding: '0 28px 0 9px',
                            fontSize: 11.5,
                          }}
                          aria-label={`Право ${permission.key}`}
                          value={overrides[permission.key] ?? 'inherit'}
                          onChange={(event) =>
                            setOverrides((current) => ({
                              ...current,
                              [permission.key]: event.target.value as OverrideState,
                            }))
                          }
                        >
                          <option value="inherit">Наследовать</option>
                          <option value="allow">Разрешить</option>
                          <option value="deny">Запретить</option>
                        </select>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {error ? <Alert>{error}</Alert> : null}

      {!isCreate && editor.user.lockedUntil ? (
        <div className="tiny muted row" style={{ gap: 6 }}>
          <ShieldAlert aria-hidden="true" style={{ width: 14, height: 14 }} />
          Блокировка активна до {formatDate(editor.user.lockedUntil)}. Снять её можно в списке
          пользователей.
        </div>
      ) : null}
    </Modal>
  );
}

function ResetPasswordDialog({
  user,
  minPasswordLength,
  onClose,
  onSaved,
}: {
  user: UserSummary | null;
  minPasswordLength: number;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (user) {
      setPassword('');
      setError(null);
    }
  }, [user]);

  if (!user) return null;

  const submit = async (): Promise<void> => {
    setError(null);
    if (password.length < minPasswordLength) {
      setError(`Пароль должен содержать не менее ${minPasswordLength} символов.`);
      return;
    }
    setBusy(true);
    try {
      await api.resetUserPassword(user.id, password);
      await onSaved();
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={busy ? () => undefined : onClose}
      title="Сбросить пароль"
      description={`Новый временный пароль для @${user.username}. Все сессии пользователя будут завершены.`}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Отмена
          </Button>
          <Button variant="danger" onClick={() => void submit()} loading={busy}>
            Сбросить пароль
          </Button>
        </>
      }
    >
      <Field
        label="Новый пароль"
        htmlFor="reset-password"
        hint={`Не менее ${minPasswordLength} символов`}
      >
        <PasswordField
          id="reset-password"
          value={password}
          onChange={setPassword}
          autoComplete="new-password"
          placeholder="Временный пароль"
        />
      </Field>
      {error ? <Alert>{error}</Alert> : null}
    </Modal>
  );
}
