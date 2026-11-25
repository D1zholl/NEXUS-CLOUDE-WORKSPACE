/** Изменённые агентом файлы: diff, принятие результата и безопасный откат. */
import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  Eye,
  FilePlus2,
  Loader2,
  Pencil,
  RotateCcw,
  Trash2,
} from 'lucide-react';

import { Badge, Button, Modal } from '../../ui';
import { aiKeys } from '../../../lib/aiKeys';
import { formatDateTimeShort } from '../../../lib/format';
import { aiWorkspaceApi } from '../../../services/aiWorkspace';
import type { FileSnapshot, FileSnapshotDetail } from '../../../services/aiWorkspace';
import { ApiRequestError, errorMessage } from '../../../services/api';
import { toast } from '../../../stores/ui';
import { DiffViewer } from '../editor/DiffViewer';

interface ChangedFilesProps {
  workspaceId: string;
  taskId: string | null;
  canWrite: boolean;
  onOpenFile: (path: string) => void;
  onRestored: () => void;
  compact?: boolean;
}

function actionIcon(action: string) {
  if (action === 'delete') return <Trash2 aria-hidden="true" />;
  if (action === 'create' || action === 'mkdir') return <FilePlus2 aria-hidden="true" />;
  return <Pencil aria-hidden="true" />;
}

function contentOf(value: FileSnapshotDetail['before']): string | null {
  if (!value) return null;
  if (value.encoding === 'utf8' && typeof value.content === 'string') return value.content;
  return null;
}

export function ChangedFiles({
  workspaceId,
  taskId,
  canWrite,
  onOpenFile,
  onRestored,
  compact = false,
}: ChangedFilesProps) {
  const queryClient = useQueryClient();
  const [detailId, setDetailId] = useState<string | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<FileSnapshot | null>(null);
  const [conflicts, setConflicts] = useState<unknown>(null);

  const changes = useQuery({
    queryKey: aiKeys.changes(workspaceId, taskId),
    queryFn: () => aiWorkspaceApi.changes(workspaceId, { taskId: taskId ?? undefined, limit: 200 }),
    refetchInterval: 15_000,
  });

  const detail = useQuery({
    queryKey: aiKeys.snapshot(workspaceId, detailId),
    queryFn: () => aiWorkspaceApi.snapshot(workspaceId, detailId!),
    enabled: Boolean(detailId),
  });

  const restoreFile = useMutation({
    mutationFn: (snapshot: FileSnapshot) =>
      aiWorkspaceApi.restoreSnapshot(workspaceId, snapshot.id),
    onSuccess: () => {
      toast('Файл восстановлен из снимка.', 'success');
      setRestoreTarget(null);
      setConflicts(null);
      void queryClient.invalidateQueries({ queryKey: ['ai-changes', workspaceId] });
      onRestored();
    },
    onError: (error) => {
      if (error instanceof ApiRequestError && error.details) setConflicts(error.details);
      toast(errorMessage(error), 'error');
    },
  });

  const restoreTask = useMutation({
    mutationFn: () => aiWorkspaceApi.restoreTask(workspaceId, taskId!),
    onSuccess: (result) => {
      toast(`Восстановлено файлов: ${result.restored.length}`, 'success');
      setConflicts(null);
      void queryClient.invalidateQueries({ queryKey: ['ai-changes', workspaceId] });
      onRestored();
    },
    onError: (error) => {
      if (error instanceof ApiRequestError && error.details) setConflicts(error.details);
      toast(errorMessage(error), 'error');
    },
  });

  const items = changes.data?.items ?? [];

  return (
    <div className={`changed-files${compact ? ' changed-files--compact' : ''}`}>
      <div className="changed-files__head">
        <span>
          Изменённые файлы {taskId ? 'задачи' : 'workspace'} · {items.length}
        </span>
        <span className="changed-files__spacer" />
        {taskId ? (
          <Button
            size="sm"
            variant="ghost"
            icon={<RotateCcw aria-hidden="true" />}
            disabled={!canWrite || items.length === 0 || restoreTask.isPending}
            loading={restoreTask.isPending}
            onClick={() => restoreTask.mutate()}
            title="Вернуть файлы к состоянию до задачи (только если их не меняли после)"
          >
            Отменить изменения задачи
          </Button>
        ) : null}
      </div>
      {changes.isLoading ? (
        <div className="changed-files__empty">
          <Loader2 className="spin" aria-hidden="true" /> Загрузка снимков…
        </div>
      ) : changes.error ? (
        <div className="changed-files__empty changed-files__empty--error">
          <AlertTriangle aria-hidden="true" /> {errorMessage(changes.error)}
        </div>
      ) : items.length === 0 ? (
        <div className="changed-files__empty muted">
          Агент ещё не изменял файлы в этом workspace.
        </div>
      ) : (
        <div className="changed-files__list">
          {items.map((item) => (
            <div key={item.id} className="changed-file">
              <span className={`changed-file__icon changed-file__icon--${item.action}`}>
                {actionIcon(item.action)}
              </span>
              <button
                type="button"
                className="changed-file__path"
                onClick={() => onOpenFile(item.path)}
                title={item.path}
              >
                {item.path}
              </button>
              <span className="changed-file__counts">
                <span className="changed-file__add">+{item.additions}</span>
                <span className="changed-file__del">-{item.deletions}</span>
              </span>
              {item.truncated ? (
                <Badge tone="warn" title="Снимок усечён — откат недоступен">
                  нет снимка
                </Badge>
              ) : null}
              {item.restoredAt ? <Badge tone="ok">восстановлен</Badge> : null}
              <span className="changed-file__time">{formatDateTimeShort(item.createdAt)}</span>
              <span className="changed-file__actions">
                <button
                  type="button"
                  className="ai-icon-btn"
                  title="Показать diff"
                  onClick={() => setDetailId(item.id)}
                >
                  <Eye aria-hidden="true" />
                </button>
                <button
                  type="button"
                  className="ai-icon-btn"
                  title="Восстановить версию до изменения"
                  disabled={!canWrite || item.truncated || Boolean(item.restoredAt)}
                  onClick={() => setRestoreTarget(item)}
                >
                  <RotateCcw aria-hidden="true" />
                </button>
              </span>
            </div>
          ))}
        </div>
      )}

      <Modal
        open={Boolean(detailId)}
        onClose={() => setDetailId(null)}
        wide
        title={detail.data?.path ?? 'Diff'}
        description="Содержимое до и после операции агента."
        footer={
          <Button variant="ghost" onClick={() => setDetailId(null)}>
            Закрыть
          </Button>
        }
      >
        {detail.isLoading ? (
          <Loader2 className="spin" />
        ) : detail.error ? (
          <p className="field__error">{errorMessage(detail.error)}</p>
        ) : detail.data ? (
          contentOf(detail.data.before) === null && contentOf(detail.data.after) === null ? (
            <p className="muted">
              Бинарный файл или снимок усечён: текстовый diff недоступен.
            </p>
          ) : (
            <DiffViewer
              path={detail.data.path}
              before={contentOf(detail.data.before) ?? ''}
              after={contentOf(detail.data.after) ?? ''}
              truncated={detail.data.truncated}
            />
          )
        ) : null}
      </Modal>

      <Modal
        open={Boolean(restoreTarget)}
        onClose={() => setRestoreTarget(null)}
        title="Восстановить файл?"
        description="Файл будет возвращён к состоянию до изменения агентом. Если файл изменён позже, откат будет отклонён."
        footer={
          <>
            <Button variant="ghost" onClick={() => setRestoreTarget(null)}>
              Отмена
            </Button>
            <Button
              variant="danger"
              loading={restoreFile.isPending}
              onClick={() => restoreTarget && restoreFile.mutate(restoreTarget)}
            >
              Восстановить
            </Button>
          </>
        }
      >
        <p>{restoreTarget?.path}</p>
      </Modal>

      <Modal
        open={Boolean(conflicts)}
        onClose={() => setConflicts(null)}
        wide
        title="Откат отклонён: есть более новые изменения"
        description="NEXUS не перезаписывает файлы, изменённые после задачи."
        footer={
          <Button variant="ghost" onClick={() => setConflicts(null)}>
            Закрыть
          </Button>
        }
      >
        <pre className="conflict-details">{JSON.stringify(conflicts, null, 2)}</pre>
      </Modal>
    </div>
  );
}
