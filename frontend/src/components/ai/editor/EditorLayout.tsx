/** Центральная панель: вкладки, редактор, preview, diff, сохранение. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  Braces,
  Code2,
  Download,
  Eye,
  FileQuestion,
  GitCompare,
  Loader2,
  RefreshCw,
  Save,
} from 'lucide-react';

import { AiMarkdown } from '../../AiMarkdown';
import { Button, EmptyState, LoadingInline } from '../../ui';
import { formatBytes } from '../../../lib/format';
import { basename, fileKind } from '../../../lib/fileLanguage';
import { highlightLanguage } from '../../../lib/fileLanguage';
import { aiWorkspaceApi, saveBlob, saveConflictDetails } from '../../../services/aiWorkspace';
import { errorMessage } from '../../../services/api';
import { useAgentWorkspace } from '../../../stores/ai/agentWorkspace';
import { toast } from '../../../stores/ui';
import { CodeEditor } from './CodeEditor';
import { DiffViewer } from './DiffViewer';
import { EditorTabs } from './EditorTabs';

type ViewMode = 'code' | 'preview' | 'diff';

interface EditorLayoutProps {
  workspaceId: string;
  canWrite: boolean;
  onFilesChanged: () => void;
  onRequestNewFile: () => void;
}

function prettyJson(text: string): { value: string; error: string | null } {
  try {
    return { value: JSON.stringify(JSON.parse(text), null, 2), error: null };
  } catch (error) {
    return { value: text, error: errorMessage(error) };
  }
}

export function EditorLayout({
  workspaceId,
  canWrite,
  onFilesChanged,
  onRequestNewFile,
}: EditorLayoutProps) {
  const queryClient = useQueryClient();
  const tabs = useAgentWorkspace((state) => state.tabs);
  const activePath = useAgentWorkspace((state) => state.activePath);
  const drafts = useAgentWorkspace((state) => state.drafts);
  const closeTab = useAgentWorkspace((state) => state.closeTab);
  const setActive = useAgentWorkspace((state) => state.setActive);
  const beginEdit = useAgentWorkspace((state) => state.beginEdit);
  const setDraft = useAgentWorkspace((state) => state.setDraft);
  const markSaved = useAgentWorkspace((state) => state.markSaved);

  const [mode, setMode] = useState<ViewMode>('code');
  const [autosave, setAutosave] = useState(true);
  const autosaveTimer = useRef<number | null>(null);
  const kind = activePath ? fileKind(activePath) : 'text';
  const editable = kind === 'text' || kind === 'markdown' || kind === 'json';
  const draft = activePath ? drafts[activePath] : undefined;

  const fileQuery = useQuery({
    queryKey: ['ai-file', workspaceId, activePath],
    queryFn: ({ signal }) => aiWorkspaceApi.content(workspaceId, activePath!, signal),
    enabled: Boolean(activePath) && editable,
    staleTime: 5 * 60_000,
    retry: false,
  });
  const truncatedFile = Boolean(fileQuery.data?.truncated);
  const readOnly = !canWrite || truncatedFile;

  useEffect(() => {
    if (!activePath || !fileQuery.data) return;
    const current = useAgentWorkspace.getState().drafts[activePath];
    if (!current) {
      beginEdit(activePath, fileQuery.data.content, fileQuery.data.sha256);
    }
  }, [activePath, fileQuery.data, beginEdit]);

  useEffect(() => {
    setMode('code');
    if (autosaveTimer.current) window.clearTimeout(autosaveTimer.current);
  }, [activePath]);

  const mutation = useMutation({
    mutationFn: async (payload: { content: string; baseSha256: string }) => {
      if (!activePath) throw new Error('Файл не открыт.');
      return aiWorkspaceApi.save(workspaceId, {
        path: activePath,
        content: payload.content,
        baseSha256: payload.baseSha256,
      });
    },
    onMutate: () => {
      if (activePath) setDraft(activePath, { saving: true, error: null });
    },
    onSuccess: (result, variables) => {
      if (!activePath) return;
      markSaved(activePath, variables.content, result.sha256);
      const current = useAgentWorkspace.getState().drafts[activePath];
      setDraft(activePath, {
        saving: false,
        error: null,
        conflict: null,
        dirty: current ? current.content !== variables.content : false,
      });
      void queryClient.invalidateQueries({ queryKey: ['ai-overview', workspaceId] });
      void queryClient.invalidateQueries({ queryKey: ['ai-changes', workspaceId] });
      onFilesChanged();
      toast(`Сохранено: ${basename(activePath)}`, 'success');
    },
    onError: (error) => {
      if (!activePath) return;
      const conflict = saveConflictDetails(error);
      if (conflict) {
        setDraft(activePath, {
          saving: false,
          error: 'Файл изменён на Runtime. Автосохранение остановлено.',
          conflict: { content: conflict.currentContent, sha256: conflict.currentSha256 },
        });
        toast('Конфликт версий: файл изменён другим процессом.', 'error');
        return;
      }
      setDraft(activePath, { saving: false, error: errorMessage(error) });
      toast(errorMessage(error), 'error');
    },
  });

  const save = useCallback(() => {
    if (readOnly || !draft || !draft.dirty || draft.saving) return;
    mutation.mutate({ content: draft.content, baseSha256: draft.baseSha256 });
  }, [readOnly, draft, mutation]);

  // Автосохранение с паузой; при конфликте и усечённом файле не выполняется.
  useEffect(() => {
    if (!autosave || readOnly || !draft || !draft.dirty || draft.saving || draft.conflict) {
      return;
    }
    if (autosaveTimer.current) window.clearTimeout(autosaveTimer.current);
    autosaveTimer.current = window.setTimeout(() => {
      if (!draft) return;
      mutation.mutate({ content: draft.content, baseSha256: draft.baseSha256 });
    }, 1400);
    return () => {
      if (autosaveTimer.current) window.clearTimeout(autosaveTimer.current);
    };
  }, [autosave, readOnly, draft, mutation]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        save();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [save]);

  const dirtyMap = useMemo(() => {
    const map: Record<string, boolean> = {};
    for (const [path, value] of Object.entries(drafts)) map[path] = value.dirty;
    return map;
  }, [drafts]);

  const close = (path: string) => {
    const current = drafts[path];
    if (current?.dirty) {
      const confirmed = window.confirm(
        `В файле ${basename(path)} есть несохранённые изменения. Закрыть без сохранения?`,
      );
      if (!confirmed) return;
    }
    closeTab(path);
  };

  const reload = async () => {
    if (!activePath) return;
    if (draft?.dirty) {
      const confirmed = window.confirm('Несохранённые изменения будут потеряны. Перечитать файл?');
      if (!confirmed) return;
    }
    const result = await fileQuery.refetch();
    if (result.data) {
      beginEdit(activePath, result.data.content, result.data.sha256);
      toast('Файл перечитан с Runtime.', 'success');
    }
  };

  const json = useMemo(() => {
    if (kind !== 'json' || !draft) return null;
    return prettyJson(draft.content);
  }, [kind, draft]);

  const markdown = draft?.content ?? fileQuery.data?.content ?? '';

  return (
    <section className="editor-pane" aria-label="Редактор">
      <EditorTabs
        tabs={tabs}
        activePath={activePath}
        dirty={dirtyMap}
        onSelect={setActive}
        onClose={close}
      />
      {activePath ? (
        <div className="editor-pane__bar">
          <span className="editor-pane__path" title={activePath}>
            {activePath}
          </span>
          {fileQuery.data ? (
            <span className="editor-pane__meta">
              {formatBytes(fileQuery.data.content.length)}
              {fileQuery.data.truncated ? ' · файл усечён лимитом чтения' : ''}
            </span>
          ) : null}
          {editable ? (
            <div className="editor-pane__modes" role="group" aria-label="Режим просмотра">
              <button
                type="button"
                className={`editor-mode${mode === 'code' ? ' is-active' : ''}`}
                onClick={() => setMode('code')}
                title="Код"
              >
                <Code2 aria-hidden="true" />
              </button>
              <button
                type="button"
                className={`editor-mode${mode === 'preview' ? ' is-active' : ''}`}
                onClick={() => setMode('preview')}
                title={kind === 'json' ? 'JSON' : 'Предпросмотр'}
              >
                {kind === 'json' ? <Braces aria-hidden="true" /> : <Eye aria-hidden="true" />}
              </button>
              {draft && draft.content !== draft.original ? (
                <button
                  type="button"
                  className={`editor-mode${mode === 'diff' ? ' is-active' : ''}`}
                  onClick={() => setMode('diff')}
                  title="Изменения"
                >
                  <GitCompare aria-hidden="true" />
                </button>
              ) : null}
            </div>
          ) : null}
          <div className="editor-pane__actions">
            {editable ? (
              <label className="editor-autosave" title="Сохранять автоматически после паузы">
                <input
                  type="checkbox"
                  checked={autosave}
                  onChange={(event) => setAutosave(event.target.checked)}
                  disabled={!canWrite}
                />
                автосохранение
              </label>
            ) : null}
            {editable ? (
              <Button
                size="sm"
                variant="ghost"
                icon={<RefreshCw aria-hidden="true" />}
                onClick={() => void reload()}
                title="Перечитать файл"
              />
            ) : null}
            {editable ? (
              <Button
                size="sm"
                variant="primary"
                icon={draft?.saving ? <Loader2 className="spin" aria-hidden="true" /> : <Save aria-hidden="true" />}
                disabled={readOnly || !draft?.dirty || draft?.saving}
                onClick={save}
              >
                Сохранить
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}

      <div className="editor-pane__content">
        {!activePath ? (
          <EmptyState
            icon={<FileQuestion aria-hidden="true" />}
            title="Файл не открыт"
            action={
              canWrite ? (
                <Button variant="primary" size="sm" onClick={onRequestNewFile}>
                  Создать файл
                </Button>
              ) : undefined
            }
          >
            <p>Выберите файл в Explorer или создайте новый файл в рабочем пространстве.</p>
          </EmptyState>
        ) : !editable ? (
          <MediaViewer workspaceId={workspaceId} path={activePath} />
        ) : fileQuery.isLoading ? (
          <LoadingInline label="Чтение файла через Runtime" />
        ) : fileQuery.error ? (
          <EmptyState
            icon={<AlertTriangle aria-hidden="true" />}
            title="Не удалось открыть файл"
            action={
              <Button size="sm" variant="ghost" onClick={() => void reload()}>
                Повторить
              </Button>
            }
          >
            <p>{errorMessage(fileQuery.error)}</p>
          </EmptyState>
        ) : draft ? (
          <>
            {!canWrite ? (
              <div className="editor-notice">
                <AlertTriangle aria-hidden="true" /> У вас нет права записи в workspace — файл
                доступен только для чтения.
              </div>
            ) : null}
            {truncatedFile ? (
              <div className="editor-notice">
                <AlertTriangle aria-hidden="true" /> Файл превышает лимит чтения (8 МБ или лимит
                workspace). Редактирование отключено, чтобы не сохранить усечённую версию.
              </div>
            ) : null}
            {draft.conflict ? (
              <div className="editor-conflict">
                <div className="editor-conflict__head">
                  <AlertTriangle aria-hidden="true" />
                  <span>{draft.error ?? 'Конфликт версий'}</span>
                </div>
                <div className="editor-conflict__actions">
                  <Button
                    size="sm"
                    variant="default"
                    onClick={() => setMode('diff')}
                    disabled={!draft.conflict.content}
                  >
                    Сравнить с текущей версией
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    onClick={() => {
                      setDraft(activePath, {
                        content: draft.conflict?.content ?? draft.content,
                        baseSha256: draft.conflict?.sha256 ?? draft.baseSha256,
                        original: draft.conflict?.content ?? draft.original,
                        dirty: false,
                        conflict: null,
                        error: null,
                      });
                    }}
                    disabled={!draft.conflict.content}
                  >
                    Принять версию Runtime
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      setDraft(activePath, {
                        conflict: null,
                        error: null,
                        baseSha256: draft.conflict?.sha256 ?? draft.baseSha256,
                      })
                    }
                    disabled={!draft.conflict.sha256}
                  >
                    Оставить мою и перезаписать
                  </Button>
                </div>
                {draft.conflict.content ? (
                  <DiffViewer
                    path={`${activePath} (Runtime)`}
                    before={draft.content}
                    after={draft.conflict.content}
                  />
                ) : (
                  <p className="muted">Файл бинарный или слишком большой для сравнения.</p>
                )}
              </div>
            ) : draft.error ? (
              <div className="editor-notice editor-notice--error">
                <AlertTriangle aria-hidden="true" /> {draft.error}
              </div>
            ) : null}
            {mode === 'diff' ? (
              <DiffViewer path={activePath} before={draft.original} after={draft.content} />
            ) : mode === 'preview' ? (
              kind === 'json' ? (
                <div className="editor-preview">
                  {json?.error ? (
                    <div className="editor-notice editor-notice--error">
                      JSON невалиден: {json.error}
                    </div>
                  ) : null}
                  <pre className="editor-json">{json?.value}</pre>
                </div>
              ) : (
                <div className="editor-preview editor-preview--markdown">
                  <AiMarkdown content={markdown} />
                </div>
              )
            ) : (
              <CodeEditor
                path={activePath}
                content={draft.content}
                language={highlightLanguage(activePath)}
                readOnly={readOnly}
                onChange={(value) =>
                  setDraft(activePath, {
                    content: value,
                    dirty: value !== draft.original,
                  })
                }
                onSave={save}
              />
            )}
          </>
        ) : (
          <LoadingInline label="Подготовка редактора" />
        )}
      </div>
    </section>
  );
}

function MediaViewer({ workspaceId, path }: { workspaceId: string; path: string }) {
  const kind = fileKind(path);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [size, setSize] = useState<number | null>(null);

  useEffect(() => {
    let active = true;
    let objectUrl: string | null = null;
    setLoading(true);
    setError(null);
    setUrl(null);
    const controller = new AbortController();
    Promise.all([
      kind === 'image'
        ? aiWorkspaceApi.raw(workspaceId, path, controller.signal)
        : Promise.resolve(null),
      aiWorkspaceApi.stat(workspaceId, path).catch(() => null),
    ])
      .then(([blob, stat]) => {
        if (!active) return;
        if (stat) setSize(stat.size);
        if (blob) {
          objectUrl = URL.createObjectURL(blob);
          setUrl(objectUrl);
        }
      })
      .catch((cause) => {
        if (active) setError(errorMessage(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [workspaceId, path, kind]);

  const download = async () => {
    try {
      const blob = await aiWorkspaceApi.download(workspaceId, path);
      saveBlob(blob, basename(path));
    } catch (cause) {
      toast(errorMessage(cause), 'error');
    }
  };

  if (loading) return <LoadingInline label="Загрузка файла" />;
  if (error) {
    return (
      <EmptyState
        icon={<AlertTriangle aria-hidden="true" />}
        title="Не удалось прочитать файл"
      >
        <p>{error}</p>
      </EmptyState>
    );
  }
  return (
    <div className="media-viewer">
      <div className="media-viewer__bar">
        <span>{basename(path)}</span>
        <span className="muted">{size !== null ? formatBytes(size) : ''}</span>
        <Button size="sm" variant="ghost" icon={<Download aria-hidden="true" />} onClick={() => void download()}>
          Скачать
        </Button>
      </div>
      {kind === 'image' && url ? (
        <div className="media-viewer__image">
          <img src={url} alt={basename(path)} />
        </div>
      ) : (
        <EmptyState
          icon={<FileQuestion aria-hidden="true" />}
          title="Бинарный файл"
          action={
            <Button size="sm" variant="primary" onClick={() => void download()}>
              Скачать файл
            </Button>
          }
        >
          <p>Предпросмотр недоступен. Файл можно скачать и открыть локально.</p>
        </EmptyState>
      )}
    </div>
  );
}
