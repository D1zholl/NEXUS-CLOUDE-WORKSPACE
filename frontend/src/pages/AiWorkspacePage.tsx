import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  Bot,
  Check,
  Clipboard,
  FileText,
  History,
  Loader2,
  MessageSquarePlus,
  MessagesSquare,
  Paperclip,
  Pencil,
  RefreshCw,
  Send,
  Settings2,
  Square,
  Trash2,
  Wrench,
  X,
} from 'lucide-react';

import { AiMarkdown } from '../components/AiMarkdown';
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
  Segmented,
} from '../components/ui';
import { formatBytes, formatDate, formatPercent } from '../lib/format';
import { realtime } from '../lib/realtime';
import { aiApi, errorMessage } from '../services/api';
import type {
  AiApproval,
  AiAttachment,
  AiConversation,
  AiExecution,
  AiMessage,
  AiTask,
  AiTaskEvent,
  AiWorkspace,
} from '../services/api';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

type Mode = 'chat' | 'agent';

function statusTone(status: string): 'default' | 'ok' | 'warn' | 'danger' | 'info' {
  switch (status) {
    case 'completed':
    case 'ok':
      return 'ok';
    case 'running':
    case 'queued':
      return 'info';
    case 'waiting_approval':
      return 'warn';
    case 'failed':
    case 'error':
    case 'revoked':
      return 'danger';
    default:
      return 'default';
  }
}

function statusLabel(status: string): string {
  const map: Record<string, string> = {
    queued: 'В очереди',
    running: 'Выполняется',
    waiting_approval: 'Ждёт подтверждения',
    completed: 'Завершена',
    failed: 'Ошибка',
    cancelled: 'Отменена',
    streaming: 'Генерация',
    pending: 'Ожидает',
    blocked: 'Заблокировано',
    ok: 'OK',
  };
  return map[status] ?? status;
}

export function AiWorkspacePage() {
  const queryClient = useQueryClient();
  const [params, setParams] = useSearchParams();
  const activeId = params.get('c');
  const canAgent = useCan('ai.agent.use');
  const canWorkspace = useCan('ai.workspace.read');
  const canAdmin = useCan('ai.admin.manage');
  const canApprove = useCan('ai.actions.approve');

  const [mode, setMode] = useState<Mode>('chat');
  const [providerId, setProviderId] = useState('');
  const [model, setModel] = useState('');
  const [workspaceId, setWorkspaceId] = useState('');
  const [draft, setDraft] = useState('');
  const [attachments, setAttachments] = useState<AiAttachment[]>([]);
  const [search, setSearch] = useState('');
  const [taskId, setTaskId] = useState<string | null>(null);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState('');
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<AiMessage | null>(null);
  const [editValue, setEditValue] = useState('');
  const scrollRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const providers = useQuery({
    queryKey: ['ai-providers'],
    queryFn: aiApi.providers,
    refetchInterval: 30_000,
  });
  const workspaces = useQuery({
    queryKey: ['ai-workspaces'],
    queryFn: aiApi.workspaces,
    enabled: canWorkspace,
    staleTime: 60_000,
  });
  const conversations = useQuery({
    queryKey: ['ai-conversations', search],
    queryFn: () => aiApi.conversations({ search, perPage: 60 }),
    refetchInterval: 30_000,
  });
  const conversation = useQuery({
    queryKey: ['ai-conversation', activeId],
    queryFn: () => aiApi.conversation(activeId!),
    enabled: !!activeId,
  });
  const messages = useQuery({
    queryKey: ['ai-messages', activeId],
    queryFn: () => aiApi.messages(activeId!, 1, 100),
    enabled: !!activeId,
    refetchInterval: (query) => {
      const page = query.state.data;
      return page?.items.some((item) => item.status === 'streaming') ? 3000 : false;
    },
  });
  const tasks = useQuery({
    queryKey: ['ai-tasks', activeId],
    queryFn: () => aiApi.tasks({ conversationId: activeId!, perPage: 30 }),
    enabled: !!activeId,
    refetchInterval: 4000,
  });
  const currentTask = useMemo<AiTask | undefined>(() => {
    const items = tasks.data?.items ?? [];
    if (taskId) {
      const direct = items.find((item) => item.id === taskId);
      if (direct) return direct;
    }
    return (
      items.find((item) => ['queued', 'running', 'waiting_approval'].includes(item.status)) ??
      items[0]
    );
  }, [tasks.data, taskId]);
  const task = useQuery({
    queryKey: ['ai-task', currentTask?.id],
    queryFn: () => aiApi.task(currentTask!.id),
    enabled: !!currentTask?.id,
    refetchInterval: (query) =>
      query.state.data &&
      ['queued', 'running', 'waiting_approval'].includes(query.state.data.status)
        ? 2000
        : false,
  });
  const executions = useQuery({
    queryKey: ['ai-executions', currentTask?.id],
    queryFn: () => aiApi.executions(currentTask!.id),
    enabled: !!currentTask?.id,
    refetchInterval:
      currentTask && ['queued', 'running', 'waiting_approval'].includes(currentTask.status)
        ? 3000
        : false,
  });
  const events = useQuery({
    queryKey: ['ai-task-events', currentTask?.id],
    queryFn: () => aiApi.taskEvents(currentTask!.id),
    enabled: !!currentTask?.id,
    refetchInterval:
      currentTask && ['queued', 'running', 'waiting_approval'].includes(currentTask.status)
        ? 3000
        : false,
  });
  const approvals = useQuery({
    queryKey: ['ai-approvals'],
    queryFn: aiApi.pendingApprovals,
    refetchInterval: 5000,
  });
  const skills = useQuery({
    queryKey: ['ai-skills', workspaceId],
    queryFn: () => aiApi.skills(workspaceId || undefined),
    enabled: canWorkspace,
    staleTime: 120_000,
  });

  const providerList = useMemo(() => providers.data?.items ?? [], [providers.data]);
  const workspaceList = useMemo(() => workspaces.data?.items ?? [], [workspaces.data]);
  const activeProvider = providerList.find((item) => item.id === providerId) ?? providerList[0];
  const activeWorkspace = workspaceList.find((item) => item.id === workspaceId);
  const messageItems = messages.data?.items ?? [];
  const streaming = messageItems.some((item) => item.status === 'streaming');
  const activeConversation = conversation.data;
  const activeApprovals = (approvals.data?.items ?? []).filter(
    (item) => item.conversationId === activeId,
  );
  const otherApprovals = (approvals.data?.items ?? []).filter(
    (item) => item.conversationId !== activeId,
  );

  useEffect(() => {
    if (!providerId && providerList[0]) {
      setProviderId(providerList[0].id);
      setModel((value) => value || providerList[0].defaultModel);
    }
  }, [providerId, providerList]);

  useEffect(() => {
    if (!workspaceId && workspaceList[0] && mode === 'agent') {
      setWorkspaceId(workspaceList[0].id);
    }
  }, [mode, workspaceId, workspaceList]);

  useEffect(() => {
    if (!activeConversation) return;
    setMode(activeConversation.mode);
    setModel(activeConversation.model);
    if (activeConversation.providerId) setProviderId(activeConversation.providerId);
    setWorkspaceId(activeConversation.workspaceId ?? '');
    setTaskId(null);
  }, [activeConversation?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: 'smooth',
    });
  }, [messageItems.length, activeId, streaming]);

  useEffect(() => {
    const unsubscribe = realtime.onStatus((connected) => {
      if (!connected) return;
      // После reconnect WebSocket не является источником истины: перечитываем
      // сохранённое состояние из PostgreSQL через HTTP API.
      void queryClient.invalidateQueries({ queryKey: ['ai-conversations'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-messages'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-tasks'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-task'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-task-events'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-executions'] });
      void queryClient.invalidateQueries({ queryKey: ['ai-approvals'] });
    });
    return unsubscribe;
  }, [queryClient]);

  useEffect(() => {
    const unsubscribe = realtime.subscribe((event) => {
      const payload = event.payload as Record<string, unknown>;
      if (event.kind === 'ai.message.delta') {
        const conversationId = String(payload.conversationId ?? '');
        const messageId = String(payload.messageId ?? '');
        const delta = String(payload.delta ?? '');
        if (!conversationId || !messageId) return;
        queryClient.setQueryData(
          ['ai-messages', conversationId],
          (old: Awaited<ReturnType<typeof aiApi.messages>> | undefined) => {
            if (!old) return old;
            return {
              ...old,
              items: old.items.map((item) =>
                item.id === messageId
                  ? { ...item, content: `${item.content}${delta}`, status: 'streaming' as const }
                  : item,
              ),
            };
          },
        );
        return;
      }
      if (event.kind === 'ai.message.started') {
        const conversationId = String(payload.conversationId ?? '');
        void queryClient.invalidateQueries({ queryKey: ['ai-messages', conversationId] });
        return;
      }
      if (['ai.message.completed', 'ai.message.failed'].includes(event.kind)) {
        const conversationId = String(payload.conversationId ?? '');
        void queryClient.invalidateQueries({ queryKey: ['ai-messages', conversationId] });
        void queryClient.invalidateQueries({ queryKey: ['ai-conversations', search] });
        if (event.kind === 'ai.message.failed') {
          toast(String(payload.message ?? 'Генерация завершилась ошибкой'), 'error');
        }
        return;
      }
      if (event.kind.startsWith('ai.task.') || event.kind.startsWith('ai.tool.')) {
        const eventTask = String(payload.taskId ?? '');
        if (eventTask) {
          setTaskId((value) => value ?? eventTask);
          void queryClient.invalidateQueries({ queryKey: ['ai-task', eventTask] });
          void queryClient.invalidateQueries({ queryKey: ['ai-task-events', eventTask] });
          void queryClient.invalidateQueries({ queryKey: ['ai-executions', eventTask] });
        }
        void queryClient.invalidateQueries({ queryKey: ['ai-tasks', activeId] });
        void queryClient.invalidateQueries({ queryKey: ['ai-approvals'] });
        if (event.kind === 'ai.task.completed') {
          toast('AI-задача завершена', 'success');
          void queryClient.invalidateQueries({ queryKey: ['ai-messages', activeId] });
          void queryClient.invalidateQueries({ queryKey: ['ai-conversations', search] });
        }
        if (event.kind === 'ai.task.failed') {
          toast(String(payload.message ?? 'AI-задача завершилась ошибкой'), 'error');
        }
        if (event.kind === 'ai.tool.waiting_approval') {
          toast('Требуется подтверждение опасного действия', 'warning');
        }
      }
    });
    return unsubscribe;
  }, [activeId, queryClient, search]);

  const setActive = (id: string | null): void => {
    const next = new URLSearchParams(params);
    if (id) next.set('c', id);
    else next.delete('c');
    setParams(next, { replace: true });
    // Вложения привязаны к конкретному диалогу; при переключении их нельзя
    // переносить в другой диалог.
    setAttachments([]);
    if (!id) {
      setDraft('');
    }
  };

  const createConversation = async (): Promise<AiConversation> => {
    if (!providerId && !providerList[0]) {
      throw new Error('Администратор ещё не настроил провайдера моделей.');
    }
    if (mode === 'agent' && !workspaceId) {
      throw new Error('Для Agent Mode администратор должен назначить рабочее пространство.');
    }
    return aiApi.createConversation({
      mode,
      providerId: providerId || providerList[0]?.id,
      model: model || activeProvider?.defaultModel,
      workspaceId: mode === 'agent' ? workspaceId : undefined,
    });
  };

  const sendMutation = useMutation({
    mutationFn: async () => {
      if (!draft.trim()) throw new Error('Введите сообщение.');
      let conv =
        activeConversation?.mode === mode
          ? activeConversation
          : conversations.data?.items.find((item) => item.id === activeId && item.mode === mode);
      if (!conv) {
        conv = await createConversation();
        setActive(conv.id);
      } else {
        const patch: Record<string, unknown> = {};
        if (model && model !== conv.model) patch.model = model;
        if (providerId && providerId !== conv.providerId) patch.providerId = providerId;
        if (mode === 'agent' && workspaceId && workspaceId !== conv.workspaceId) {
          patch.workspaceId = workspaceId;
        }
        if (Object.keys(patch).length > 0) {
          await aiApi.updateConversation(conv.id, patch);
        }
      }
      return aiApi.sendMessage(conv.id, {
        content: draft.trim(),
        attachmentIds: attachments.map((item) => item.id),
      });
    },
    onSuccess: async (result) => {
      setDraft('');
      setAttachments([]);
      if (result.task) setTaskId(result.task.id);
      const id = result.userMessage.conversationId;
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['ai-messages', id] }),
        queryClient.invalidateQueries({ queryKey: ['ai-conversations', search] }),
        queryClient.invalidateQueries({ queryKey: ['ai-tasks', id] }),
        queryClient.invalidateQueries({ queryKey: ['ai-conversation', id] }),
      ]);
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const stopMutation = useMutation({
    mutationFn: async () => {
      if (streaming && activeId) return aiApi.stop(activeId);
      if (currentTask && ['queued', 'running', 'waiting_approval'].includes(currentTask.status)) {
        return aiApi.cancelTask(currentTask.id);
      }
      return null;
    },
    onSuccess: async () => {
      if (activeId) {
        await queryClient.invalidateQueries({ queryKey: ['ai-messages', activeId] });
        await queryClient.invalidateQueries({ queryKey: ['ai-tasks', activeId] });
      }
      void queryClient.invalidateQueries({ queryKey: ['ai-approvals'] });
      toast('Команда остановки отправлена Runtime', 'info');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const renameMutation = useMutation({
    mutationFn: () => aiApi.updateConversation(activeId!, { title: renameValue.trim() }),
    onSuccess: async () => {
      setRenameOpen(false);
      await queryClient.invalidateQueries({ queryKey: ['ai-conversations', search] });
      await queryClient.invalidateQueries({ queryKey: ['ai-conversation', activeId] });
      toast('Диалог переименован', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const deleteMutation = useMutation({
    mutationFn: () => aiApi.deleteConversation(activeId!),
    onSuccess: async () => {
      setDeleteOpen(false);
      setActive(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-conversations', search] });
      toast('Диалог архивирован', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const regenerateMutation = useMutation({
    mutationFn: (messageId: string) => aiApi.regenerate(activeId!, messageId),
    onSuccess: async (result) => {
      if (result.task) setTaskId(result.task.id);
      await queryClient.invalidateQueries({ queryKey: ['ai-messages', activeId] });
      await queryClient.invalidateQueries({ queryKey: ['ai-tasks', activeId] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const editMutation = useMutation({
    mutationFn: () => aiApi.editMessage(activeId!, editTarget!.id, editValue.trim()),
    onSuccess: async (result) => {
      setEditTarget(null);
      if (result.task) setTaskId(result.task.id);
      await queryClient.invalidateQueries({ queryKey: ['ai-messages', activeId] });
      await queryClient.invalidateQueries({ queryKey: ['ai-tasks', activeId] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const approveMutation = useMutation({
    mutationFn: ({ approval, approve }: { approval: AiApproval; approve: boolean }) =>
      approve ? aiApi.approve(approval.id) : aiApi.reject(approval.id),
    onSuccess: async (_data, variables) => {
      toast(variables.approve ? 'Действие подтверждено' : 'Действие отклонено', 'success');
      setTaskId(variables.approval.taskId);
      await queryClient.invalidateQueries({ queryKey: ['ai-approvals'] });
      await queryClient.invalidateQueries({ queryKey: ['ai-tasks', activeId] });
      await queryClient.invalidateQueries({ queryKey: ['ai-messages', activeId] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const attachMutation = useMutation({
    mutationFn: async (files: File[]) => {
      let id = activeId;
      if (!id) {
        const created = await createConversation();
        setActive(created.id);
        id = created.id;
      }
      const uploaded: AiAttachment[] = [];
      for (const file of files) {
        if (attachments.length + uploaded.length >= 5) {
          throw new Error('Не больше 5 вложений в одном сообщении.');
        }
        uploaded.push(await aiApi.uploadAttachment(id, file));
      }
      return uploaded;
    },
    onSuccess: (uploaded) => {
      setAttachments((current) => [...current, ...uploaded]);
      toast(`Вложений добавлено: ${uploaded.length}`, 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const onModeChange = (next: Mode): void => {
    setMode(next);
    if (activeConversation && activeConversation.mode !== next) {
      setActive(null);
      setDraft('');
      setAttachments([]);
      toast(
        next === 'agent'
          ? 'Следующее сообщение создаст Agent-диалог'
          : 'Следующее сообщение создаст Chat-диалог',
        'info',
      );
    }
  };

  const send = (): void => {
    if (sendMutation.isPending) return;
    sendMutation.mutate();
  };

  if (providers.isError) {
    return (
      <>
        <PageHead title="AI Workspace" eyebrow="NEXUS AI" />
        <Alert>{errorMessage(providers.error)}</Alert>
      </>
    );
  }

  return (
    <>
      <PageHead
        eyebrow="NEXUS AI"
        title="AI Workspace"
        description="Диалог с PolyStrata или агент, выполняющий реальные задачи на изолированном NEXUS AI Runtime."
        actions={
          <>
            <Badge
              tone={activeProvider?.status === 'ok' ? 'ok' : activeProvider ? 'warn' : 'danger'}
            >
              {activeProvider ? `PolyStrata: ${activeProvider.status}` : 'Провайдер не настроен'}
            </Badge>
            {activeProvider?.models ? (
              <span className="tiny muted">моделей: {activeProvider.models.length}</span>
            ) : null}
            {canAdmin ? (
              <Link to="/system/ai" className="btn btn--sm">
                <Settings2 aria-hidden="true" style={{ width: 14, height: 14, marginRight: 6 }} />
                AI Management
              </Link>
            ) : null}
          </>
        }
      />

      {providerList.length === 0 ? (
        <div className="card">
          <EmptyState icon={<Bot aria-hidden="true" />} title="AI ещё не настроен">
            Администратор должен создать Provider, зарегистрировать NEXUS AI Runtime и назначить
            рабочее пространство. После этого AI Chat станет доступен.
          </EmptyState>
        </div>
      ) : (
        <div className="ai-layout">
          <aside className="card ai-sidebar">
            <div className="ai-sidebar__head">
              <Button
                size="sm"
                variant="primary"
                block
                icon={<MessageSquarePlus aria-hidden="true" />}
                onClick={() => {
                  setActive(null);
                  setDraft('');
                }}
              >
                Новый диалог
              </Button>
              <input
                className="input ai-search"
                placeholder="Поиск по диалогам"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                aria-label="Поиск по диалогам"
              />
            </div>
            <div className="ai-conversations">
              {conversations.isPending ? <LoadingInline label="Диалоги" /> : null}
              {conversations.data?.items.length === 0 ? (
                <div className="tiny muted" style={{ padding: 12 }}>
                  Нет сохранённых диалогов.
                </div>
              ) : null}
              {conversations.data?.items.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={`ai-conversation${item.id === activeId ? ' active' : ''}`}
                  onClick={() => setActive(item.id)}
                >
                  <span className="ai-conversation__icon">
                    {item.mode === 'agent' ? (
                      <Bot aria-hidden="true" />
                    ) : (
                      <MessagesSquare aria-hidden="true" />
                    )}
                  </span>
                  <span className="ai-conversation__body">
                    <span className="ai-conversation__title truncate">{item.title}</span>
                    <span className="tiny muted truncate">
                      {item.mode === 'agent' ? 'Agent' : 'Chat'} ·{' '}
                      {formatDate(item.lastMessageAt ?? item.updatedAt)}
                    </span>
                  </span>
                </button>
              ))}
            </div>
            <div className="ai-sidebar__foot">
              <span className="tiny muted">
                Skills: {skills.data?.items.length ?? 0} · Tools:{' '}
                {(activeWorkspace?.allowedTools ?? []).length}
              </span>
              <Link to="/system/ai" className="tiny" hidden={!canAdmin}>
                Управление Runtime
              </Link>
            </div>
          </aside>

          <section className="card ai-chat">
            <div className="ai-toolbar">
              <Segmented
                value={mode}
                options={
                  canAgent
                    ? [
                        { value: 'chat', label: 'Chat' },
                        { value: 'agent', label: 'Agent' },
                      ]
                    : [{ value: 'chat', label: 'Chat' }]
                }
                onChange={onModeChange}
                label="Режим AI"
              />
              <div className="ai-toolbar__spacer" />
              {activeConversation ? (
                <>
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Переименовать"
                    aria-label="Переименовать диалог"
                    onClick={() => {
                      setRenameValue(activeConversation.title);
                      setRenameOpen(true);
                    }}
                  >
                    <Pencil aria-hidden="true" />
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    iconOnly
                    title="Архивировать"
                    aria-label="Архивировать диалог"
                    onClick={() => setDeleteOpen(true)}
                  >
                    <Trash2 aria-hidden="true" />
                  </Button>
                </>
              ) : null}
            </div>

            <div className="ai-selectors">
              <label className="ai-selector">
                <span>Провайдер</span>
                <select
                  className="input"
                  value={providerId}
                  disabled={streaming}
                  onChange={(event) => {
                    const next = providerList.find((item) => item.id === event.target.value);
                    setProviderId(event.target.value);
                    if (next) setModel(next.defaultModel);
                  }}
                >
                  {providerList.map((item) => (
                    <option key={item.id} value={item.id}>
                      {item.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="ai-selector">
                <span>Модель</span>
                <input
                  className="input"
                  list="ai-model-options"
                  value={model}
                  disabled={streaming}
                  onChange={(event) => setModel(event.target.value)}
                  placeholder={activeProvider?.defaultModel || 'model'}
                />
                <datalist id="ai-model-options">
                  {(activeProvider?.models ?? []).map((item) => (
                    <option key={item} value={item} />
                  ))}
                </datalist>
              </label>
              <label className="ai-selector">
                <span>Workspace</span>
                <select
                  className="input"
                  value={workspaceId}
                  disabled={streaming || !canWorkspace}
                  onChange={(event) => setWorkspaceId(event.target.value)}
                >
                  <option value="">{mode === 'agent' ? 'Не выбран' : 'Без workspace'}</option>
                  {workspaceList.map((item) => (
                    <option key={item.id} value={item.id}>
                      {item.name}
                      {item.status !== 'active' ? ' (приостановлен)' : ''}
                    </option>
                  ))}
                </select>
              </label>
              {activeWorkspace ? (
                <span className="tiny muted ai-usage">
                  {formatBytes(activeWorkspace.usageBytes)} /{' '}
                  {formatBytes(activeWorkspace.quotaBytes)}
                  {activeWorkspace.usageCheckedAt ? '' : ' · нет данных usage'}
                </span>
              ) : null}
            </div>

            {mode === 'agent' && !workspaceId ? (
              <Alert tone="warn">
                Для Agent Mode нужно назначенное рабочее пространство. Обратитесь к администратору.
              </Alert>
            ) : null}

            {activeApprovals.length > 0 ? (
              <div className="ai-approvals">
                {activeApprovals.map((approval) => (
                  <ApprovalCard
                    key={approval.id}
                    approval={approval}
                    busy={approveMutation.isPending}
                    canDecide={canApprove}
                    onDecide={(approve) => approveMutation.mutate({ approval, approve })}
                  />
                ))}
              </div>
            ) : null}

            <div className="ai-messages" ref={scrollRef}>
              {messages.isPending ? <LoadingInline label="История" /> : null}
              {messages.isError ? <Alert>{errorMessage(messages.error)}</Alert> : null}
              {!activeId && messageItems.length === 0 ? (
                <div className="ai-welcome">
                  <Bot aria-hidden="true" />
                  <h3>{mode === 'agent' ? 'Agent Mode' : 'AI Chat'}</h3>
                  <p className="muted">
                    {mode === 'agent'
                      ? 'Агент проанализирует задачу, выберет разрешённые инструменты и выполнит её на Runtime. Опасные действия потребуют подтверждения.'
                      : 'Обычный диалог с моделью PolyStrata. В Chat Mode модели не передаются инструменты и доступ к файлам.'}
                  </p>
                </div>
              ) : null}
              {messageItems.map((message) => (
                <MessageBubble
                  key={message.id}
                  message={message}
                  onRegenerate={
                    message.role === 'assistant' && message.status !== 'streaming'
                      ? () => regenerateMutation.mutate(message.id)
                      : undefined
                  }
                  onEdit={
                    message.role === 'user'
                      ? () => {
                          setEditTarget(message);
                          setEditValue(message.content);
                        }
                      : undefined
                  }
                />
              ))}
              {streaming ? (
                <div className="ai-typing">
                  <Loader2 aria-hidden="true" className="spin" />
                  Генерация ответа…
                </div>
              ) : null}
            </div>

            {otherApprovals.length > 0 ? (
              <div className="ai-other-approvals">
                <AlertTriangle aria-hidden="true" />
                <span>Ещё подтверждений в других диалогах: {otherApprovals.length}</span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setActive(otherApprovals[0].conversationId);
                    setTaskId(otherApprovals[0].taskId);
                  }}
                >
                  Перейти
                </Button>
              </div>
            ) : null}

            <div className="ai-composer">
              {attachments.length > 0 ? (
                <div className="ai-attachments">
                  {attachments.map((item) => (
                    <span key={item.id} className="ai-attachment">
                      <FileText aria-hidden="true" />
                      <span className="truncate">{item.name}</span>
                      <span className="tiny muted">{formatBytes(item.sizeBytes)}</span>
                      <button
                        type="button"
                        aria-label={`Удалить вложение ${item.name}`}
                        onClick={() =>
                          setAttachments((current) =>
                            current.filter((entry) => entry.id !== item.id),
                          )
                        }
                      >
                        <X aria-hidden="true" />
                      </button>
                    </span>
                  ))}
                </div>
              ) : null}
              <textarea
                className="input ai-textarea"
                rows={3}
                value={draft}
                maxLength={32000}
                placeholder={
                  mode === 'agent'
                    ? 'Опишите задачу для агента… (Enter — отправить, Shift+Enter — новая строка)'
                    : 'Спросите PolyStrata… (Enter — отправить, Shift+Enter — новая строка)'
                }
                onChange={(event) => setDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault();
                    send();
                  }
                }}
              />
              <div className="ai-composer__actions">
                <input
                  ref={fileInputRef}
                  type="file"
                  hidden
                  multiple
                  accept=".txt,.md,.json,.yaml,.yml,.xml,.csv,.log,.rs,.ts,.tsx,.js,.jsx,.py,.toml,.ini,.sql,.sh,.ps1"
                  onChange={(event) => {
                    const files = Array.from(event.target.files ?? []);
                    if (files.length > 0) attachMutation.mutate(files);
                    event.target.value = '';
                  }}
                />
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Paperclip aria-hidden="true" />}
                  disabled={attachMutation.isPending}
                  onClick={() => fileInputRef.current?.click()}
                >
                  Файл
                </Button>
                <span className="tiny muted">{draft.length}/32000</span>
                <div className="ai-toolbar__spacer" />
                {streaming ||
                (currentTask &&
                  ['queued', 'running', 'waiting_approval'].includes(currentTask.status)) ? (
                  <Button
                    variant="danger"
                    size="sm"
                    icon={<Square aria-hidden="true" />}
                    loading={stopMutation.isPending}
                    onClick={() => stopMutation.mutate()}
                  >
                    Остановить
                  </Button>
                ) : null}
                <Button
                  variant="primary"
                  size="sm"
                  icon={<Send aria-hidden="true" />}
                  loading={sendMutation.isPending}
                  disabled={!draft.trim() || (mode === 'agent' && !workspaceId)}
                  onClick={send}
                >
                  Отправить
                </Button>
              </div>
            </div>
          </section>

          <aside className="card ai-inspector">
            <SectionTitle
              title="Agent Task"
              meta={currentTask ? statusLabel(currentTask.status) : 'нет задачи'}
              actions={
                currentTask ? (
                  <Badge tone={statusTone(currentTask.status)}>
                    {statusLabel(currentTask.status)}
                  </Badge>
                ) : null
              }
            />
            {!currentTask ? (
              <div className="tiny muted" style={{ padding: '8px 0' }}>
                Agent-задачи появятся здесь после отправки запроса в Agent Mode.
              </div>
            ) : (
              <TaskPanel
                task={task.data ?? currentTask}
                executions={executions.data?.items ?? []}
                events={events.data?.items ?? []}
                onCancel={
                  ['queued', 'running', 'waiting_approval'].includes(currentTask.status)
                    ? () => stopMutation.mutate()
                    : undefined
                }
              />
            )}
          </aside>
        </div>
      )}

      <Modal
        open={renameOpen}
        onClose={() => setRenameOpen(false)}
        title="Переименовать диалог"
        footer={
          <>
            <Button variant="ghost" onClick={() => setRenameOpen(false)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              loading={renameMutation.isPending}
              disabled={!renameValue.trim()}
              onClick={() => renameMutation.mutate()}
            >
              Сохранить
            </Button>
          </>
        }
      >
        <Field label="Название">
          <input
            className="input"
            value={renameValue}
            maxLength={120}
            onChange={(event) => setRenameValue(event.target.value)}
          />
        </Field>
      </Modal>

      <Modal
        open={!!editTarget}
        onClose={() => setEditTarget(null)}
        title="Редактировать сообщение"
        description="Последующие сообщения этой ветки будут удалены и сгенерированы заново."
        footer={
          <>
            <Button variant="ghost" onClick={() => setEditTarget(null)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              loading={editMutation.isPending}
              disabled={!editValue.trim()}
              onClick={() => editMutation.mutate()}
            >
              Отправить заново
            </Button>
          </>
        }
      >
        <Field label="Текст сообщения">
          <textarea
            className="input"
            rows={6}
            value={editValue}
            maxLength={32000}
            onChange={(event) => setEditValue(event.target.value)}
          />
        </Field>
      </Modal>

      <ConfirmDialog
        open={deleteOpen}
        title="Архивировать диалог?"
        description="История сохранится в базе, но диалог исчезнет из активного списка. Выполняющиеся задачи будут остановлены."
        confirmLabel="Архивировать"
        busy={deleteMutation.isPending}
        onCancel={() => setDeleteOpen(false)}
        onConfirm={() => deleteMutation.mutate()}
      />
    </>
  );
}

function MessageBubble({
  message,
  onRegenerate,
  onEdit,
}: {
  message: AiMessage;
  onRegenerate?: () => void;
  onEdit?: () => void;
}) {
  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(message.content);
      toast('Сообщение скопировано', 'success');
    } catch {
      toast('Буфер обмена недоступен', 'warning');
    }
  };

  return (
    <article className={`ai-message ai-message--${message.role}`}>
      <div className="ai-message__head">
        <span className="ai-message__role">
          {message.role === 'user'
            ? 'Вы'
            : message.role === 'assistant'
              ? 'NEXUS AI'
              : message.role}
        </span>
        <span className="tiny muted">{formatDate(message.createdAt)}</span>
        {message.status !== 'completed' ? (
          <Badge tone={statusTone(message.status)}>{statusLabel(message.status)}</Badge>
        ) : null}
      </div>
      <div className="ai-message__body">
        {message.role === 'assistant' ? (
          <AiMarkdown content={message.content} />
        ) : (
          <div className="ai-message__plain">{message.content}</div>
        )}
        {message.errorMessage ? (
          <div className="ai-message__error">
            {message.errorCode}: {message.errorMessage}
          </div>
        ) : null}
        {(message.parts as AiAttachment[]).length > 0 ? (
          <div className="ai-message__files">
            {(message.parts as AiAttachment[])
              .filter((part) => part && typeof part === 'object' && 'name' in part)
              .map((part) => (
                <span key={part.id} className="badge">
                  <FileText aria-hidden="true" style={{ width: 12, height: 12 }} /> {part.name}
                </span>
              ))}
          </div>
        ) : null}
      </div>
      <div className="ai-message__actions">
        {message.content ? (
          <Button size="sm" variant="ghost" iconOnly title="Копировать" onClick={() => void copy()}>
            <Clipboard aria-hidden="true" />
          </Button>
        ) : null}
        {onRegenerate ? (
          <Button
            size="sm"
            variant="ghost"
            iconOnly
            title="Сгенерировать заново"
            onClick={onRegenerate}
          >
            <RefreshCw aria-hidden="true" />
          </Button>
        ) : null}
        {onEdit ? (
          <Button size="sm" variant="ghost" iconOnly title="Редактировать" onClick={onEdit}>
            <Pencil aria-hidden="true" />
          </Button>
        ) : null}
      </div>
    </article>
  );
}

function ApprovalCard({
  approval,
  busy,
  canDecide,
  onDecide,
}: {
  approval: AiApproval;
  busy: boolean;
  canDecide: boolean;
  onDecide: (approve: boolean) => void;
}) {
  return (
    <div className={`ai-approval ai-approval--${approval.risk}`}>
      <div className="ai-approval__head">
        <AlertTriangle aria-hidden="true" />
        <strong>Требуется подтверждение: {approval.toolId}</strong>
        <Badge tone="warn">риск: {approval.risk}</Badge>
        <span className="tiny muted">до {formatDate(approval.expiresAt)}</span>
      </div>
      <pre className="ai-approval__args">{JSON.stringify(approval.arguments, null, 2)}</pre>
      <div className="ai-approval__actions">
        <Button variant="ghost" size="sm" disabled={busy || !canDecide} onClick={() => onDecide(false)}>
          Отклонить
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={!canDecide}
          loading={busy}
          onClick={() => onDecide(true)}
        >
          <Check aria-hidden="true" style={{ width: 14, height: 14, marginRight: 6 }} />
          Подтвердить
        </Button>
      </div>
    </div>
  );
}

function TaskPanel({
  task,
  executions,
  events,
  onCancel,
}: {
  task: AiTask;
  executions: AiExecution[];
  events: AiTaskEvent[];
  onCancel?: () => void;
}) {
  const [tab, setTab] = useState<'tools' | 'events'>('tools');
  const progress = Math.max(0, Math.min(1, task.progress || 0));
  return (
    <div className="ai-task">
      <div className="ai-task__status">
        <Badge tone={statusTone(task.status)}>{statusLabel(task.status)}</Badge>
        <span className="tiny muted">{task.statusText}</span>
      </div>
      <div className="ai-task__meta">
        <span>
          итерации: {task.iterations}/{task.maxIterations}
        </span>
        <span>tools: {task.toolCallsCount}</span>
        <span>{formatPercent(progress * 100)}</span>
        {task.cancelStatus && task.cancelStatus !== 'none' ? (
          <span>cancel: {task.cancelStatus}</span>
        ) : null}
      </div>
      <div className="meter">
        <span style={{ width: `${progress * 100}%` }} />
      </div>
      {task.errorMessage ? (
        <Alert tone="danger">
          {task.errorCode}: {task.errorMessage}
        </Alert>
      ) : null}
      <div className="ai-task__actions">
        {onCancel ? (
          <Button
            size="sm"
            variant="danger"
            icon={<Square aria-hidden="true" />}
            onClick={onCancel}
          >
            Отменить
          </Button>
        ) : null}
        <span className="tiny muted">создана {formatDate(task.createdAt)}</span>
      </div>

      <div className="ai-inspector__tabs">
        <button
          type="button"
          className={tab === 'tools' ? 'active' : ''}
          onClick={() => setTab('tools')}
        >
          <Wrench aria-hidden="true" /> Инструменты ({executions.length})
        </button>
        <button
          type="button"
          className={tab === 'events' ? 'active' : ''}
          onClick={() => setTab('events')}
        >
          <History aria-hidden="true" /> События ({events.length})
        </button>
      </div>

      {tab === 'tools' ? (
        <div className="ai-inspector__list">
          {executions.length === 0 ? (
            <div className="tiny muted">Вызовов инструментов ещё нет.</div>
          ) : null}
          {executions.map((execution) => (
            <details key={execution.id} className="ai-execution">
              <summary>
                <Badge tone={statusTone(execution.status)}>{execution.status}</Badge>
                <span className="truncate">{execution.toolId}</span>
                {execution.durationMs != null ? (
                  <span className="tiny muted">{execution.durationMs} ms</span>
                ) : null}
              </summary>
              <div className="ai-execution__body">
                <div className="tiny muted">Аргументы:</div>
                <pre>{JSON.stringify(execution.arguments, null, 2)}</pre>
                {execution.errorMessage ? (
                  <div className="ai-message__error">
                    {execution.errorCode}: {execution.errorMessage}
                  </div>
                ) : null}
                {execution.resultText ? (
                  <>
                    <div className="tiny muted">Результат:</div>
                    <pre>{execution.resultText}</pre>
                  </>
                ) : null}
              </div>
            </details>
          ))}
        </div>
      ) : (
        <div className="ai-inspector__list">
          {events.length === 0 ? <div className="tiny muted">Событий пока нет.</div> : null}
          {events.map((event) => (
            <div key={event.id} className="ai-event">
              <span className="ai-event__kind">{event.kind}</span>
              <span className="ai-event__message">{event.message}</span>
              <span className="tiny muted">{formatDate(event.createdAt)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Не используется напрямую, но сохраняет тип импорта для workspace-селектора. */
export type { AiWorkspace };
