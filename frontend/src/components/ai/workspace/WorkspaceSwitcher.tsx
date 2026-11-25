/** Переключение и управление workspace из верхней панели. */
import { useEffect, useRef, useState } from 'react';
import {
  Check,
  ChevronDown,
  HardDrive,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Trash2,
} from 'lucide-react';

import { Button, Field, Modal } from '../../ui';
import { formatBytes, relativeTime } from '../../../lib/format';
import type { AiWorkspace } from '../../../services/ai';
import type { DeletedWorkspace, WorkspaceOverview } from '../../../services/aiWorkspace';
import { aiWorkspaceApi } from '../../../services/aiWorkspace';
import { errorMessage } from '../../../services/api';
import { toast } from '../../../stores/ui';

interface WorkspaceSwitcherProps {
  workspaces: AiWorkspace[];
  activeId: string | null;
  overview: WorkspaceOverview | undefined;
  canWrite: boolean;
  loading: boolean;
  onSelect: (id: string) => void;
  onCreate: (body: { name: string; description?: string }) => Promise<void>;
  onRename: (id: string, body: { name?: string; description?: string }) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onRestore: (id: string) => Promise<void>;
  onRefreshUsage: () => void;
}

export function WorkspaceSwitcher({
  workspaces,
  activeId,
  overview,
  canWrite,
  loading,
  onSelect,
  onCreate,
  onRename,
  onDelete,
  onRestore,
  onRefreshUsage,
}: WorkspaceSwitcherProps) {
  const [open, setOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deletedOpen, setDeletedOpen] = useState(false);
  const [deleted, setDeleted] = useState<DeletedWorkspace[]>([]);
  const [deletedLoading, setDeletedLoading] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [pending, setPending] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const active = workspaces.find((workspace) => workspace.id === activeId) ?? null;

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    window.addEventListener('mousedown', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const submitCreate = async () => {
    if (!name.trim()) {
      toast('Укажите название workspace.', 'warning');
      return;
    }
    setPending(true);
    try {
      await onCreate({ name: name.trim(), description: description.trim() });
      setCreateOpen(false);
      setName('');
      setDescription('');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const submitRename = async () => {
    if (!activeId || !name.trim()) return;
    setPending(true);
    try {
      await onRename(activeId, { name: name.trim(), description: description.trim() });
      setRenameOpen(false);
      toast('Workspace обновлён.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const submitDelete = async () => {
    if (!activeId) return;
    setPending(true);
    try {
      await onDelete(activeId);
      setDeleteOpen(false);
      toast('Workspace удалён (файлы сохранены).', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const openDeleted = async () => {
    setDeletedOpen(true);
    setDeletedLoading(true);
    try {
      const response = await aiWorkspaceApi.listDeleted();
      setDeleted(response.items);
      setRetentionDays(response.retentionDays);
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setDeletedLoading(false);
    }
  };

  const submitRestore = async (id: string) => {
    setPending(true);
    try {
      await onRestore(id);
      const response = await aiWorkspaceApi.listDeleted();
      setDeleted(response.items);
      toast('Workspace восстановлен.', 'success');
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setPending(false);
    }
  };

  const quota = overview?.quotaBytes ?? 0;
  const used = overview?.bytes ?? 0;
  const percent = quota > 0 ? Math.min(100, Math.round((used / quota) * 100)) : 0;

  return (
    <div className="ws-switcher" ref={rootRef}>
      <button
        type="button"
        className="ws-switcher__button"
        onClick={() => setOpen((current) => !current)}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <HardDrive aria-hidden="true" />
        <span className="ws-switcher__name">{active?.name ?? 'Выберите workspace'}</span>
        {overview ? (
          <span
            className={`ws-switcher__dot${overview.runtimeOnline ? ' is-online' : ' is-offline'}`}
            title={overview.runtimeOnline ? 'Runtime подключён' : 'Runtime offline'}
          />
        ) : null}
        <ChevronDown aria-hidden="true" />
      </button>
      {open ? (
        <div className="ws-switcher__menu" role="menu">
          <div className="ws-switcher__menu-head">
            <span>Рабочие пространства</span>
            <button
              type="button"
              className="ai-icon-btn"
              title="Обновить usage"
              onClick={onRefreshUsage}
              disabled={!activeId}
            >
              <RefreshCw aria-hidden="true" />
            </button>
          </div>
          {loading ? <div className="ws-switcher__empty">Загрузка…</div> : null}
          {!loading && workspaces.length === 0 ? (
            <div className="ws-switcher__empty">Workspace ещё не создан.</div>
          ) : null}
          {workspaces.map((workspace) => (
            <button
              key={workspace.id}
              type="button"
              className={`ws-switcher__item${workspace.id === activeId ? ' is-active' : ''}`}
              onClick={() => {
                onSelect(workspace.id);
                setOpen(false);
              }}
            >
              <span className="ws-switcher__item-name">
                {workspace.id === activeId ? <Check aria-hidden="true" /> : null}
                {workspace.name}
              </span>
              <span className="ws-switcher__item-meta">
                {workspace.usageFiles} файлов · {formatBytes(workspace.usageBytes)}
              </span>
            </button>
          ))}
          {active ? (
            <div className="ws-switcher__info">
              <div className="ws-switcher__info-row">
                <span>Создан</span>
                <span>{relativeTime(active.createdAt)}</span>
              </div>
              <div className="ws-switcher__info-row">
                <span>Активность</span>
                <span>
                  {overview?.lastActivityAt ? relativeTime(overview.lastActivityAt) : 'нет данных'}
                </span>
              </div>
              <div className="ws-switcher__info-row">
                <span>Задачи</span>
                <span>
                  {overview?.tasksTotal ?? 0} · активных {overview?.tasksActive ?? 0}
                </span>
              </div>
              <div className="ws-switcher__quota">
                <div className="ws-switcher__quota-bar">
                  <span style={{ width: `${percent}%` }} />
                </div>
                <span className="muted">
                  {formatBytes(used)} из {formatBytes(quota)} ({percent}%)
                </span>
              </div>
              <div className="ws-switcher__actions">
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Pencil aria-hidden="true" />}
                  disabled={!canWrite}
                  onClick={() => {
                    setName(active.name);
                    setDescription(overview?.description ?? '');
                    setRenameOpen(true);
                    setOpen(false);
                  }}
                >
                  Изменить
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Trash2 aria-hidden="true" />}
                  disabled={!canWrite}
                  onClick={() => {
                    setDeleteOpen(true);
                    setOpen(false);
                  }}
                >
                  Удалить
                </Button>
              </div>
            </div>
          ) : null}
          <div className="ws-switcher__footer">
            <Button
              size="sm"
              variant="primary"
              block
              icon={<Plus aria-hidden="true" />}
              disabled={!canWrite}
              onClick={() => {
                setName('');
                setDescription('');
                setCreateOpen(true);
                setOpen(false);
              }}
            >
              Новый workspace
            </Button>
            <Button
              size="sm"
              variant="ghost"
              block
              icon={<RotateCcw aria-hidden="true" />}
              disabled={!canWrite}
              onClick={() => {
                setOpen(false);
                void openDeleted();
              }}
            >
              Удалённые workspace
            </Button>
          </div>
        </div>
      ) : null}

      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="Новый workspace"
        description="Runtime создаст изолированный каталог. Физический путь на сервере не показывается."
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreateOpen(false)}>
              Отмена
            </Button>
            <Button variant="primary" loading={pending} onClick={() => void submitCreate()}>
              Создать
            </Button>
          </>
        }
      >
        <Field label="Название">
          <input
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="SaaS Landing"
            maxLength={80}
          />
        </Field>
        <Field label="Описание" hint="Необязательно, до 2000 символов">
          <textarea
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            rows={3}
            maxLength={2000}
          />
        </Field>
      </Modal>

      <Modal
        open={renameOpen}
        onClose={() => setRenameOpen(false)}
        title="Изменить workspace"
        footer={
          <>
            <Button variant="ghost" onClick={() => setRenameOpen(false)}>
              Отмена
            </Button>
            <Button variant="primary" loading={pending} onClick={() => void submitRename()}>
              Сохранить
            </Button>
          </>
        }
      >
        <Field label="Название">
          <input
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            maxLength={80}
          />
        </Field>
        <Field label="Описание">
          <textarea
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            rows={3}
            maxLength={2000}
          />
        </Field>
      </Modal>

      <Modal
        open={deleteOpen}
        onClose={() => setDeleteOpen(false)}
        title="Удалить workspace?"
        description="Workspace исчезнет из списка, но файлы сохраняются до истечения срока хранения. Восстановить его можно в разделе «Удалённые workspace»."
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleteOpen(false)}>
              Отмена
            </Button>
            <Button variant="danger" loading={pending} onClick={() => void submitDelete()}>
              Удалить
            </Button>
          </>
        }
      >
        <p>{active?.name}</p>
      </Modal>

      <Modal
        open={deletedOpen}
        onClose={() => setDeletedOpen(false)}
        title="Удалённые workspace"
        description={`Восстановление доступно ${retentionDays} дней. После этого срока Runtime физически удалит каталог; snapshots и usage будут очищены.`}
        footer={
          <Button variant="ghost" onClick={() => setDeletedOpen(false)}>
            Закрыть
          </Button>
        }
      >
        {deletedLoading ? <p className="muted">Загрузка…</p> : null}
        {!deletedLoading && deleted.length === 0 ? (
          <p className="muted">Удалённых workspace нет.</p>
        ) : null}
        <div className="ws-switcher__deleted-list">
          {deleted.map((item) => (
            <div key={item.workspaceId} className="ws-switcher__deleted-item">
              <div>
                <strong>{item.originalName}</strong>
                <div className="muted">
                  Удалён {relativeTime(item.requestedAt)} · очистка после{' '}
                  {new Date(item.purgeAfter).toLocaleString()}
                </div>
                {item.state === 'retry' && item.attempts > 0 ? (
                  <div className="muted">Повторов очистки: {item.attempts}</div>
                ) : null}
              </div>
              <Button
                size="sm"
                variant="primary"
                loading={pending}
                disabled={pending}
                onClick={() => void submitRestore(item.workspaceId)}
              >
                Восстановить
              </Button>
            </div>
          ))}
        </div>
      </Modal>
    </div>
  );
}
