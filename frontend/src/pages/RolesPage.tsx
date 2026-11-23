import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { KeyRound, Pencil, Plus, ShieldCheck, Trash2, UsersRound } from 'lucide-react';

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
import { api, errorMessage, type Permission, type Role } from '../services/api';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

type RoleEditor = { mode: 'create' } | { mode: 'edit'; role: Role };

const MODULE_NAMES: Record<string, string> = {
  minecraft: 'Minecraft Build Manager',
  cloud: 'NEXUS Cloud Storage',
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

export function RolesPage() {
  const queryClient = useQueryClient();
  const canViewSystem = useCan('system.view');
  const [editor, setEditor] = useState<RoleEditor | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Role | null>(null);

  const roles = useQuery({ queryKey: ['roles'], queryFn: api.listRoles });
  const permissions = useQuery({
    queryKey: ['permissions'],
    queryFn: api.listPermissions,
    staleTime: 300_000,
    enabled: canViewSystem,
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteRole(id),
    onSuccess: async () => {
      toast('Роль удалена.', 'success');
      setDeleteTarget(null);
      await queryClient.invalidateQueries({ queryKey: ['roles'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const refreshRoles = async (message: string): Promise<void> => {
    setEditor(null);
    toast(message, 'success');
    await queryClient.invalidateQueries({ queryKey: ['roles'] });
    await queryClient.invalidateQueries({ queryKey: ['users'] });
  };

  return (
    <>
      <PageHead
        eyebrow="Система"
        title="Роли и права"
        description="Роли задают базовый набор разрешений. Персональные исключения пользователя имеют приоритет над ролью."
        actions={
          <Button
            variant="primary"
            icon={<Plus aria-hidden="true" />}
            onClick={() => setEditor({ mode: 'create' })}
            disabled={!canViewSystem}
          >
            Новая роль
          </Button>
        }
      />

      {!canViewSystem ? (
        <Alert tone="warn">
          Для редактирования ролей требуется право <span className="mono">system.view</span> — без
          него каталог прав недоступен. Обратитесь к администратору.
        </Alert>
      ) : null}

      {roles.isPending ? (
        <LoadingInline label="Загрузка ролей" />
      ) : roles.isError ? (
        <Alert>{errorMessage(roles.error)}</Alert>
      ) : roles.data.length === 0 ? (
        <EmptyState icon={<KeyRound aria-hidden="true" />} title="Роли не настроены">
          Создайте роль, чтобы выдать пользователям доступ к модулям.
        </EmptyState>
      ) : (
        <div className="role-grid">
          {roles.data.map((role) => {
            const extra = role.permissions.length - 7;
            return (
              <article className="role-card" key={role.id}>
                <div className="role-card__head">
                  <div>
                    <div className="role-card__name">
                      {role.name}{' '}
                      {role.isSystem ? (
                        <Badge tone="info" plain>
                          системная
                        </Badge>
                      ) : null}
                    </div>
                    <div className="role-card__key">{role.key}</div>
                  </div>
                  <div className="table__actions">
                    <Button
                      size="sm"
                      variant="ghost"
                      iconOnly
                      aria-label={`Редактировать роль ${role.name}`}
                      title={role.isSystem ? 'Системную роль нельзя изменять' : 'Редактировать'}
                      disabled={role.isSystem || !canViewSystem}
                      onClick={() => setEditor({ mode: 'edit', role })}
                    >
                      <Pencil aria-hidden="true" />
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      iconOnly
                      aria-label={`Удалить роль ${role.name}`}
                      title={role.isSystem ? 'Системную роль нельзя удалить' : 'Удалить'}
                      disabled={role.isSystem}
                      onClick={() => setDeleteTarget(role)}
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  </div>
                </div>

                <div className="role-card__desc">{role.description || 'Описание не задано.'}</div>

                <div className="permission-chips">
                  {role.permissions.length === 0 ? (
                    <span className="tiny muted">Права не назначены</span>
                  ) : (
                    <>
                      {role.permissions.slice(0, 7).map((permission) => (
                        <span className="chip" key={permission}>
                          {permission}
                        </span>
                      ))}
                      {extra > 0 ? <span className="chip">+{extra}</span> : null}
                    </>
                  )}
                </div>

                <div className="role-card__foot">
                  <span className="row" style={{ gap: 6 }}>
                    <UsersRound aria-hidden="true" style={{ width: 13, height: 13 }} />
                    {role.userCount} пользователей
                  </span>
                  <span className="mono">{role.permissions.length} прав</span>
                </div>
              </article>
            );
          })}
        </div>
      )}

      <RoleEditorModal
        editor={editor}
        permissions={permissions.data ?? []}
        permissionsLoading={permissions.isPending}
        permissionsError={permissions.isError ? errorMessage(permissions.error) : null}
        onClose={() => setEditor(null)}
        onSaved={refreshRoles}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        title="Удалить роль?"
        description={
          deleteTarget
            ? `Роль «${deleteTarget.name}» будет удалена. Если она назначена пользователям, сервер откажет в удалении.`
            : ''
        }
        confirmLabel="Удалить роль"
        busy={remove.isPending}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) remove.mutate(deleteTarget.id);
        }}
      />

      <div className="card">
        <SectionTitle title="Как работают права" meta="Модель доступа" />
        <div className="info-list">
          <div className="info-list__row">
            <span>Суперадминистратор</span>
            <span>
              <ShieldCheck
                aria-hidden="true"
                style={{
                  display: 'inline',
                  width: 13,
                  height: 13,
                  marginRight: 5,
                  verticalAlign: -2,
                }}
              />
              обходит проверки прав
            </span>
          </div>
          <div className="info-list__row">
            <span>Роли</span>
            <span>объединяются, права суммируются</span>
          </div>
          <div className="info-list__row">
            <span>Персональные исключения</span>
            <span>переопределяют роль: разрешить или запретить</span>
          </div>
          <div className="info-list__row">
            <span>Сервер</span>
            <span>проверяет право в каждом защищённом обработчике</span>
          </div>
        </div>
      </div>
    </>
  );
}

function RoleEditorModal({
  editor,
  permissions,
  permissionsLoading,
  permissionsError,
  onClose,
  onSaved,
}: {
  editor: RoleEditor | null;
  permissions: Permission[];
  permissionsLoading: boolean;
  permissionsError: string | null;
  onClose: () => void;
  onSaved: (message: string) => Promise<void>;
}) {
  const [key, setKey] = useState('');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!editor) return;
    setError(null);
    if (editor.mode === 'create') {
      setKey('');
      setName('');
      setDescription('');
      setSelected(new Set());
    } else {
      setKey(editor.role.key);
      setName(editor.role.name);
      setDescription(editor.role.description);
      setSelected(new Set(editor.role.permissions));
    }
  }, [editor]);

  const grouped = useMemo(() => groupPermissions(permissions), [permissions]);

  if (!editor) return null;

  const isCreate = editor.mode === 'create';

  const toggle = (permission: string): void => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(permission)) next.delete(permission);
      else next.add(permission);
      return next;
    });
  };

  const save = async (): Promise<void> => {
    setError(null);
    const trimmedName = name.trim();
    if (trimmedName.length === 0) {
      setError('Укажите название роли.');
      return;
    }
    if (isCreate) {
      const trimmedKey = key.trim().toLowerCase();
      if (!/^[a-z][a-z0-9._-]{2,47}$/.test(trimmedKey)) {
        setError(
          'Ключ роли: 3–48 символов, латиница в нижнем регистре, цифры, точка, дефис, подчёркивание.',
        );
        return;
      }
    }
    setBusy(true);
    try {
      if (isCreate) {
        await api.createRole({
          key: key.trim().toLowerCase(),
          name: trimmedName,
          description: description.trim(),
          permissions: [...selected],
        });
        await onSaved('Роль создана.');
      } else {
        await api.updateRole(editor.role.id, {
          name: trimmedName,
          description: description.trim(),
          permissions: [...selected],
        });
        await onSaved('Роль обновлена.');
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
      title={isCreate ? 'Новая роль' : `Роль «${editor.role.name}»`}
      description={
        isCreate
          ? 'Ключ роли используется в API и журнале аудита и не меняется после создания.'
          : 'Изменения прав вступят в силу при следующей проверке доступа.'
      }
      wide
      footer={
        <>
          <span className="tiny muted" style={{ marginRight: 'auto' }}>
            Выбрано прав: {selected.size}
          </span>
          <Button onClick={onClose} disabled={busy}>
            Отмена
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={busy}>
            {isCreate ? 'Создать роль' : 'Сохранить'}
          </Button>
        </>
      }
    >
      <div className="form-grid">
        <Field label="Ключ" htmlFor="role-key" hint="Например: build-editor">
          <input
            id="role-key"
            className="input mono"
            value={key}
            onChange={(event) => setKey(event.target.value)}
            disabled={!isCreate}
            maxLength={48}
            placeholder="role-key"
            required
          />
        </Field>
        <Field label="Название" htmlFor="role-name">
          <input
            id="role-name"
            className="input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            maxLength={80}
            required
          />
        </Field>
        <Field label="Описание" htmlFor="role-description" full>
          <textarea
            id="role-description"
            className="textarea"
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            rows={2}
            maxLength={400}
            placeholder="Кому и зачем нужна эта роль"
          />
        </Field>
      </div>

      {permissionsLoading ? (
        <LoadingInline label="Загрузка каталога прав" />
      ) : permissionsError ? (
        <Alert>{permissionsError}</Alert>
      ) : (
        <div className="stack" style={{ gap: 10 }}>
          {grouped.map((group) => (
            <div className="permission-group" key={group.module}>
              <div className="permission-group__head">{group.label}</div>
              <div className="permission-group__list">
                {group.items.map((permission) => (
                  <label className="checkbox" key={permission.key}>
                    <input
                      type="checkbox"
                      checked={selected.has(permission.key)}
                      onChange={() => toggle(permission.key)}
                    />
                    <span>
                      {permission.label}
                      <span className="tiny muted mono" style={{ display: 'block' }}>
                        {permission.key}
                      </span>
                    </span>
                  </label>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}

      {error ? <Alert>{error}</Alert> : null}
    </Modal>
  );
}
