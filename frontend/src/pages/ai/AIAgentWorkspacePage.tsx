/**
 * AI Agent Workspace — самостоятельная IDE-среда NEXUS.
 *
 * Три области: Explorer (реальные файлы Runtime), Editor (вкладки, diff,
 * preview) и AI Agent (задачи, streaming, approvals). Файловые операции идут
 * только через Runtime tools; локальная ФС браузера не используется.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertTriangle,
  Bot,
  ChevronDown,
  ChevronUp,
  FileCode2,
  HardDrive,
  LayoutPanelLeft,
  ListTodo,
  Loader2,
  MessageSquare,
  PanelBottom,
  PanelRight,
  RefreshCw,
  ShieldAlert,
  Terminal,
} from 'lucide-react';

import { AgentPanel } from '../../components/ai/agent/AgentPanel';
import { ChangedFiles } from '../../components/ai/changes/ChangedFiles';
import { EditorLayout } from '../../components/ai/editor/EditorLayout';
import { ExecutionHistory } from '../../components/ai/tasks/ExecutionHistory';
import { TaskDetails } from '../../components/ai/tasks/TaskDetails';
import { TaskExplorer, TASK_FILTERS } from '../../components/ai/tasks/TaskExplorer';
import type { TaskFilter } from '../../components/ai/tasks/TaskExplorer';
import { ExecutionOutput } from '../../components/ai/terminal/ExecutionOutput';
import { WorkspaceExplorer } from '../../components/ai/workspace/WorkspaceExplorer';
import { WorkspaceSwitcher } from '../../components/ai/workspace/WorkspaceSwitcher';
import { Alert, Button, EmptyState, Field, LoadingInline, Modal, Segmented } from '../../components/ui';
import { aiKeys } from '../../lib/aiKeys';
import { formatBytes, relativeTime } from '../../lib/format';
import { realtime } from '../../lib/realtime';
import { useIncrementalTaskEvents } from '../../lib/taskEvents';
import { aiApi, errorMessage } from '../../services/api';
import type { AiTask } from '../../services/api';
import { aiWorkspaceApi } from '../../services/aiWorkspace';
import type { AiWorkspace } from '../../services/ai';
import type { WorkspaceOverview } from '../../services/aiWorkspace';
import { useAgentWorkspace } from '../../stores/ai/agentWorkspace';
import type { BottomTab, MobilePane } from '../../stores/ai/agentWorkspace';
import { useCan } from '../../stores/auth';
import { toast } from '../../stores/ui';

const WORKSPACE_KEY = 'nexus.ai.agent.workspace';
const CONVERSATIONS_KEY = 'nexus.ai.agent.conversations';

function readConversationMap(): Record<string, string> {
  try {
    const raw = window.localStorage.getItem(CONVERSATIONS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, string>;
    return typeof parsed === 'object' && parsed ? parsed : {};
  } catch {
    return {};
  }
}

function writeConversation(workspaceId: string, conversationId: string): void {
  try {
    const map = readConversationMap();
    map[workspaceId] = conversationId;
    window.localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(map));
  } catch {
    // Не критично: диалог можно открыть из списка задач.
  }
}

function isActive(task: AiTask | undefined): boolean {
  return Boolean(task && ['queued', 'running', 'waiting_approval'].includes(task.status));
}

export function AIAgentWorkspacePage() {
  const queryClient = useQueryClient();
  const canAgent = useCan('ai.agent.use');
  const canWrite = useCan('ai.workspace.write');
  const canRead = useCan('ai.workspace.read');
  const canCancel = useCan('ai.tasks.cancel');
  const canApprove = useCan('ai.actions.approve');

  const layout = useAgentWorkspace((state) => state.layout);
  const setLayout = useAgentWorkspace((state) => state.setLayout);
  const resetLayout = useAgentWorkspace((state) => state.resetLayout);
  const mobilePane = useAgentWorkspace((state) => state.mobilePane);
  const setMobilePane = useAgentWorkspace((state) => state.setMobilePane);
  const closeAllTabs = useAgentWorkspace((state) => state.closeAll);
  const activeEditorPath = useAgentWorkspace((state) => state.activePath);

  const [workspaceId, setWorkspaceId] = useState<string | null>(() => {
    try {
      return window.localStorage.getItem(WORKSPACE_KEY);
    } catch {
      return null;
    }
  });
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [agentTaskId, setAgentTaskId] = useState<string | null>(null);
  const [historical, setHistorical] = useState(false);
  const [taskFilter, setTaskFilter] = useState<TaskFilter>('all');
  const [taskManagerOpen, setTaskManagerOpen] = useState(false);
  const [selectedTask, setSelectedTask] = useState<AiTask | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [newName, setNewName] = useState('');
  const [newDescription, setNewDescription] = useState('');
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [model, setModel] = useState('');
  const [providerId, setProviderId] = useState('');
  const [streamingMessageId, setStreamingMessageId] = useState<string | null>(null);
  const bootstrapped = useRef(false);

  const workspaces = useQuery({
    queryKey: aiKeys.workspaces,
    queryFn: aiApi.workspaces,
    enabled: canRead,
  });

  const workspaceList: AiWorkspace[] = useMemo(
    () => workspaces.data?.items ?? [],
    [workspaces.data],
  );

  // Выбор workspace: сохранённый или первый доступный.
  useEffect(() => {
    if (bootstrapped.current || workspaces.isLoading) return;
    bootstrapped.current = true;
    if (workspaceId && workspaceList.some((item) => item.id === workspaceId)) {
      setConversationId(readConversationMap()[workspaceId] ?? null);
      return;
    }
    const first = workspaceList[0]?.id ?? null;
    setWorkspaceId(first);
    if (first) {
      try {
        window.localStorage.setItem(WORKSPACE_KEY, first);
      } catch {
        // ignore
      }
      setConversationId(readConversationMap()[first] ?? null);
    }
  }, [workspaces.isLoading, workspaceList, workspaceId]);

  const overview = useQuery({
    queryKey: aiKeys.overview(workspaceId),
    queryFn: () => aiWorkspaceApi.overview(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 20_000,
  });
  const overviewData: WorkspaceOverview | undefined = overview.data;

  const providers = useQuery({
    queryKey: aiKeys.providers,
    queryFn: aiApi.providers,
    enabled: canAgent,
    staleTime: 60_000,
  });
  const provider = useMemo(
    () =>
      providers.data?.items.find((item) => item.id === providerId) ??
      providers.data?.items.find((item) => item.enabled !== false) ??
      null,
    [providers.data, providerId],
  );
  const models = useMemo(() => {
    const list = provider?.models?.length ? provider.models : provider ? [provider.defaultModel] : [];
    return [...new Set(list.filter(Boolean))];
  }, [provider]);

  useEffect(() => {
    if (providerId) return;
    const first = providers.data?.items.find((item) => item.enabled !== false);
    if (first) setProviderId(first.id);
  }, [providers.data, providerId]);

  useEffect(() => {
    if (model) return;
    if (provider?.defaultModel) setModel(provider.defaultModel);
  }, [provider, model]);

  const skills = useQuery({
    queryKey: aiKeys.skills(workspaceId),
    queryFn: () => aiApi.skills(workspaceId!),
    enabled: Boolean(workspaceId) && canAgent,
    staleTime: 120_000,
  });

  const activeTasks = useQuery({
    queryKey: aiKeys.tasks(workspaceId, 'active'),
    queryFn: () =>
      aiApi.tasks({ workspaceId: workspaceId!, statuses: 'queued,running,waiting_approval', perPage: 5 }),
    enabled: Boolean(workspaceId) && canAgent,
    refetchInterval: 6000,
  });
  const liveTask = activeTasks.data?.items[0] ?? null;

  const agentTaskQuery = useQuery({
    queryKey: aiKeys.task(agentTaskId),
    queryFn: () => aiApi.task(agentTaskId!),
    enabled: Boolean(agentTaskId),
    refetchInterval: (query) => (isActive(query.state.data) ? 2500 : false),
  });
  const agentTask = agentTaskQuery.data ?? (historical ? selectedTask : null) ?? undefined;
  const agentActive = isActive(agentTask);

  // После F5/повторного входа активная задача должна снова стать текущей:
  // подхватываем её из списка активных задач workspace.
  useEffect(() => {
    if (!liveTask || historical) return;
    if (agentTaskId === liveTask.id) return;
    if (agentTaskId !== null) return;
    setAgentTaskId(liveTask.id);
    setConversationId(liveTask.conversationId);
    if (workspaceId) writeConversation(workspaceId, liveTask.conversationId);
  }, [agentTaskId, historical, liveTask, workspaceId]);

  const executions = useQuery({
    queryKey: aiKeys.executions(agentTask?.id ?? null),
    queryFn: () => aiApi.executions(agentTask!.id),
    enabled: Boolean(agentTask?.id),
    refetchInterval: agentActive ? 4000 : false,
  });
  const executionItems = useMemo(
    () => executions.data?.items ?? [],
    [executions.data],
  );

  const events = useIncrementalTaskEvents(agentTask?.id ?? undefined, Boolean(agentActive));
  const {
    ingestRealtime: ingestTaskEvent,
    reload: reloadTaskEvents,
    items: taskEvents,
    loading: taskEventsLoading,
    error: taskEventsError,
    stats: taskEventStats,
  } = events;

  // Смена workspace сбрасывает вкладки, диалог и активную задачу.
  const selectWorkspace = (id: string) => {
    if (id === workspaceId) return;
    setWorkspaceId(id);
    setConversationId(readConversationMap()[id] ?? null);
    setAgentTaskId(null);
    setHistorical(false);
    setSelectedTask(null);
    closeAllTabs();
    useAgentWorkspace.getState().setMobilePane('editor');
    try {
      window.localStorage.setItem(WORKSPACE_KEY, id);
    } catch {
      // ignore
    }
  };

  const conversationsRef = useRef<{ workspaceId: string | null; conversationId: string | null }>({
    workspaceId: null,
    conversationId: null,
  });
  conversationsRef.current = { workspaceId, conversationId };

  // Realtime: сообщения и задачи.
  useEffect(() => {
    const unsubscribe = realtime.subscribe((event) => {
      const payload = event.payload as Record<string, unknown>;
      const currentWorkspace = conversationsRef.current.workspaceId;
      const currentConversation = conversationsRef.current.conversationId;
      if (event.kind === 'ai.message.delta') {
        const conversation = String(payload.conversationId ?? '');
        const messageId = String(payload.messageId ?? '');
        const delta = String(payload.delta ?? '');
        if (!conversation || !messageId) return;
        if (conversation !== currentConversation) return;
        setStreamingMessageId(messageId);
        queryClient.setQueryData(
          aiKeys.messages(conversation),
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
        const conversation = String(payload.conversationId ?? '');
        if (conversation) void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversation) });
        return;
      }
      if (event.kind === 'ai.message.completed' || event.kind === 'ai.message.failed') {
        const conversation = String(payload.conversationId ?? '');
        setStreamingMessageId(null);
        if (conversation) {
          void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversation) });
        }
        if (event.kind === 'ai.message.failed') {
          toast(String(payload.message ?? 'Генерация завершилась ошибкой'), 'error');
        }
        return;
      }
      if (
        event.kind.startsWith('ai.task.') ||
        event.kind.startsWith('ai.tool.') ||
        event.kind === 'agent.file.changed'
      ) {
        const taskId = String(payload.taskId ?? '');
        if (taskId) {
          if (taskId === agentTaskId || taskId === liveTask?.id) {
            const applied = ingestTaskEvent(payload, event.kind);
            if (!applied) reloadTaskEvents();
            void queryClient.invalidateQueries({ queryKey: aiKeys.task(taskId) });
            void queryClient.invalidateQueries({ queryKey: aiKeys.executions(taskId) });
          }
          void queryClient.invalidateQueries({ queryKey: aiKeys.tasks(currentWorkspace, 'active') });
        }
        void queryClient.invalidateQueries({ queryKey: aiKeys.approvals });
        if (event.kind === 'agent.file.changed' || event.kind === 'ai.task.completed') {
          setRefreshToken((token) => token + 1);
          if (currentWorkspace) {
            void queryClient.invalidateQueries({ queryKey: ['ai-changes', currentWorkspace] });
            void queryClient.invalidateQueries({ queryKey: ['ai-file', currentWorkspace] });
            void queryClient.invalidateQueries({ queryKey: aiKeys.overview(currentWorkspace) });
          }
        }
        if (event.kind === 'ai.task.completed') {
          toast('AI-задача завершена.', 'success');
        }
        if (event.kind === 'ai.task.failed') {
          toast(String(payload.message ?? 'AI-задача завершилась ошибкой'), 'error');
        }
        if (event.kind === 'ai.tool.waiting_approval') {
          toast('Требуется подтверждение опасного действия.', 'warning');
        }
      }
    });
    return unsubscribe;
  }, [agentTaskId, ingestTaskEvent, liveTask?.id, queryClient, reloadTaskEvents]);

  // Reconnect: догружаем состояние из HTTP (WS не является источником истины).
  useEffect(() => {
    const unsubscribe = realtime.onStatus((connected) => {
      if (!connected) return;
      void queryClient.invalidateQueries({ queryKey: aiKeys.tasks(workspaceId, 'active') });
      void queryClient.invalidateQueries({ queryKey: aiKeys.task(agentTaskId) });
      void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversationId) });
      void queryClient.invalidateQueries({ queryKey: aiKeys.executions(agentTask?.id ?? null) });
      void queryClient.invalidateQueries({ queryKey: aiKeys.overview(workspaceId) });
      reloadTaskEvents();
      setRefreshToken((token) => token + 1);
    });
    return unsubscribe;
  }, [agentTask?.id, agentTaskId, conversationId, queryClient, reloadTaskEvents, workspaceId]);

  const ensureConversation = useCallback(async (): Promise<string> => {
    if (conversationId) return conversationId;
    if (!workspaceId) throw new Error('Workspace не выбран.');
    const created = await aiApi.createConversation({
      mode: 'agent',
      providerId: provider?.id,
      model: model || provider?.defaultModel,
      workspaceId,
      title: 'AI Agent',
    });
    setConversationId(created.id);
    writeConversation(workspaceId, created.id);
    return created.id;
  }, [conversationId, model, provider, workspaceId]);

  const startTask = (task: AiTask) => {
    setAgentTaskId(task.id);
    setHistorical(false);
    setSelectedTask(task);
  };

  const sendMessage = async (payload: {
    content: string;
    attachmentIds: string[];
    skillIds: string[];
  }) => {
    const conversation = await ensureConversation();
    const result = await aiApi.sendMessage(conversation, {
      content: payload.content,
      attachmentIds: payload.attachmentIds.length ? payload.attachmentIds : undefined,
      skillIds: payload.skillIds.length ? payload.skillIds : undefined,
    });
    if (result.task) startTask(result.task);
    void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversation) });
    void queryClient.invalidateQueries({ queryKey: aiKeys.tasks(workspaceId, 'active') });
    void queryClient.invalidateQueries({ queryKey: aiKeys.tasks(workspaceId, taskFilter) });
  };

  const stop = async () => {
    setStopping(true);
    try {
      if (agentTask && !historical && isActive(agentTask)) {
        const result = await aiApi.cancelTask(agentTask.id);
        toast(
          result.requested
            ? 'Запрошена остановка задачи. Подтверждение придёт от Runtime.'
            : 'Задача уже завершена.',
          'warning',
        );
      } else if (conversationId) {
        await aiApi.stop(conversationId);
        toast('Генерация остановлена.', 'success');
      }
      void queryClient.invalidateQueries({ queryKey: aiKeys.task(agentTask?.id ?? null) });
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setStopping(false);
    }
  };

  const regenerate = async (messageId: string) => {
    if (!conversationId) return;
    try {
      const result = await aiApi.regenerate(conversationId, messageId);
      if (result.task) startTask(result.task);
      void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversationId) });
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const editMessage = async (messageId: string, content: string) => {
    if (!conversationId) return;
    const result = await aiApi.editMessage(conversationId, messageId, content);
    if (result.task) startTask(result.task);
    void queryClient.invalidateQueries({ queryKey: aiKeys.messages(conversationId) });
  };

  const newTask = () => {
    setConversationId(null);
    setAgentTaskId(null);
    setHistorical(false);
    setSelectedTask(null);
    setStreamingMessageId(null);
    if (workspaceId) {
      try {
        const map = readConversationMap();
        delete map[workspaceId];
        window.localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(map));
      } catch {
        // ignore
      }
    }
    toast('Новая задача: опишите её агенту.', 'info');
  };

  const openTaskInPanel = (task: AiTask) => {
    setConversationId(task.conversationId);
    if (workspaceId) writeConversation(workspaceId, task.conversationId);
    setAgentTaskId(task.id);
    setHistorical(!isActive(task));
    setSelectedTask(task);
    setTaskManagerOpen(false);
  };

  const createWorkspace = async () => {
    if (!newName.trim()) {
      toast('Укажите название workspace.', 'warning');
      return;
    }
    const created = await aiWorkspaceApi.createWorkspace({
      name: newName.trim(),
      description: newDescription.trim(),
    });
    const id = String(created.id ?? '');
    setCreateOpen(false);
    setNewName('');
    setNewDescription('');
    await queryClient.invalidateQueries({ queryKey: aiKeys.workspaces });
    if (id) {
      selectWorkspace(id);
    }
    toast('Workspace создан.', 'success');
  };

  const createMutation = useMutation({ mutationFn: createWorkspace });

  const renameWorkspace = async (id: string, body: { name?: string; description?: string }) => {
    await aiWorkspaceApi.updateWorkspace(id, body);
    await queryClient.invalidateQueries({ queryKey: aiKeys.workspaces });
    await queryClient.invalidateQueries({ queryKey: aiKeys.overview(id) });
  };

  const deleteWorkspace = async (id: string) => {
    await aiWorkspaceApi.deleteWorkspace(id);
    await queryClient.invalidateQueries({ queryKey: aiKeys.workspaces });
    if (workspaceId === id) {
      setWorkspaceId(null);
      setConversationId(null);
      setAgentTaskId(null);
      closeAllTabs();
      try {
        window.localStorage.removeItem(WORKSPACE_KEY);
      } catch {
        // ignore
      }
    }
  };

  const restoreWorkspace = async (id: string) => {
    await aiWorkspaceApi.restoreWorkspace(id);
    await queryClient.invalidateQueries({ queryKey: aiKeys.workspaces });
    setWorkspaceId(id);
    try {
      window.localStorage.setItem(WORKSPACE_KEY, id);
    } catch {
      // ignore
    }
  };

  const refreshUsage = async () => {
    if (!workspaceId) return;
    try {
      await aiApi.refreshWorkspaceUsage(workspaceId);
      await queryClient.invalidateQueries({ queryKey: aiKeys.overview(workspaceId) });
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const changedPaths = useQuery({
    queryKey: aiKeys.changes(workspaceId, null),
    queryFn: () => aiWorkspaceApi.changes(workspaceId!, { limit: 200 }),
    enabled: Boolean(workspaceId),
    refetchInterval: 20_000,
  });
  const changedSet = useMemo(
    () => new Set((changedPaths.data?.items ?? []).map((item) => item.path)),
    [changedPaths.data],
  );
  const busySet = useMemo(() => {
    const set = new Set<string>();
    for (const execution of executionItems) {
      if (!['running', 'pending', 'waiting_approval'].includes(execution.status)) continue;
      const args = execution.arguments ?? {};
      for (const key of ['path', 'to', 'from']) {
        const value = args[key];
        if (typeof value === 'string' && value) set.add(value);
      }
      for (const file of execution.changedFiles ?? []) set.add(file.path);
    }
    return set;
  }, [executionItems]);

  const openFileFromAnywhere = useCallback((path: string) => {
    useAgentWorkspace.getState().openFile(path);
    setMobilePane('editor');
  }, [setMobilePane]);

  // Resize panels.
  const dragRef = useRef<{ mode: 'explorer' | 'agent' | 'bottom'; start: number; value: number } | null>(
    null,
  );
  const onPointerMove = useCallback(
    (event: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      if (drag.mode === 'explorer') {
        setLayout({ explorerWidth: Math.min(520, Math.max(180, drag.value + event.clientX - drag.start)) });
      } else if (drag.mode === 'agent') {
        setLayout({ agentWidth: Math.min(720, Math.max(300, drag.value - event.clientX + drag.start)) });
      } else {
        setLayout({
          bottomHeight: Math.min(560, Math.max(120, drag.value - event.clientY + drag.start)),
        });
      }
    },
    [setLayout],
  );
  const endDrag = useCallback(() => {
    dragRef.current = null;
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', endDrag);
  }, [onPointerMove]);
  const beginDrag = (mode: 'explorer' | 'agent' | 'bottom', event: React.PointerEvent) => {
    dragRef.current = {
      mode,
      start: mode === 'bottom' ? event.clientY : event.clientX,
      value:
        mode === 'explorer'
          ? layout.explorerWidth
          : mode === 'agent'
            ? layout.agentWidth
            : layout.bottomHeight,
    };
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', endDrag);
  };
  useEffect(() => () => endDrag(), [endDrag]);

  const problems = useMemo(() => {
    const items: Array<{ level: 'error' | 'warn'; text: string; path?: string }> = [];
    if (overviewData && !overviewData.runtimeOnline) {
      items.push({ level: 'error', text: 'Runtime offline: файловые операции и задачи недоступны.' });
    }
    if (overviewData?.usageError) {
      items.push({ level: 'warn', text: `Usage: ${overviewData.usageError}` });
    }
    if (overviewData && overviewData.quotaBytes > 0 && overviewData.bytes / overviewData.quotaBytes > 0.9) {
      items.push({
        level: 'warn',
        text: `Использовано ${Math.round((overviewData.bytes / overviewData.quotaBytes) * 100)}% квоты workspace.`,
      });
    }
    for (const execution of executionItems) {
      if (execution.status === 'failed') {
        items.push({
          level: 'error',
          text: `${execution.toolId}: ${execution.errorMessage ?? execution.errorCode ?? 'ошибка'}`,
        });
      }
    }
    if (agentTask?.errorMessage) {
      items.push({ level: 'error', text: `${agentTask.errorCode}: ${agentTask.errorMessage}` });
    }
    return items;
  }, [agentTask, executionItems, overviewData]);

  if (!canRead) {
    return (
      <div className="ai-agent-page">
        <EmptyState
          icon={<ShieldAlert aria-hidden="true" />}
          title="Нет доступа к AI Workspace"
          action={<Link to="/">На главную</Link>}
        />
      </div>
    );
  }

  const taskTabs: Array<{ value: BottomTab; label: string; icon: React.ReactNode }> = [
    { value: 'problems', label: `Problems${problems.length ? ` (${problems.length})` : ''}`, icon: <AlertTriangle aria-hidden="true" /> },
    { value: 'output', label: 'Output', icon: <Terminal aria-hidden="true" /> },
    { value: 'executions', label: 'Execution History', icon: <ListTodo aria-hidden="true" /> },
    { value: 'changes', label: 'Changed Files', icon: <FileCode2 aria-hidden="true" /> },
    { value: 'usage', label: 'Workspace Usage', icon: <HardDrive aria-hidden="true" /> },
  ];

  const bottomContent = () => {
    switch (layout.bottomTab) {
      case 'problems':
        return problems.length === 0 ? (
          <div className="bottom-panel__empty muted">Проблем не обнаружено.</div>
        ) : (
          <ul className="problems-list">
            {problems.map((problem, index) => (
              <li key={index} className={`problems-list__item is-${problem.level}`}>
                <AlertTriangle aria-hidden="true" />
                <span>{problem.text}</span>
              </li>
            ))}
          </ul>
        );
      case 'output':
        return <ExecutionOutput executions={executionItems} />;
      case 'executions':
        return (
          <ExecutionHistory
            executions={executionItems}
            loading={executions.isLoading}
            error={executions.error ? errorMessage(executions.error) : null}
          />
        );
      case 'changes':
        return workspaceId ? (
          <ChangedFiles
            workspaceId={workspaceId}
            taskId={agentTask?.id ?? null}
            canWrite={canWrite}
            onOpenFile={openFileFromAnywhere}
            onRestored={() => {
              setRefreshToken((token) => token + 1);
              void queryClient.invalidateQueries({ queryKey: ['ai-file', workspaceId] });
            }}
            compact
          />
        ) : null;
      case 'usage':
        return overviewData ? (
          <div className="usage-panel">
            <div className="usage-panel__row">
              <span>Файлы</span>
              <span>
                {overviewData.files} / {overviewData.maxFiles || '∞'}
              </span>
              <div className="usage-bar">
                <span
                  style={{
                    width: `${
                      overviewData.maxFiles > 0
                        ? Math.min(100, Math.round((overviewData.files / overviewData.maxFiles) * 100))
                        : 0
                    }%`,
                  }}
                />
              </div>
            </div>
            <div className="usage-panel__row">
              <span>Хранилище</span>
              <span>
                {formatBytes(overviewData.bytes)} / {formatBytes(overviewData.quotaBytes)}
              </span>
              <div className="usage-bar">
                <span
                  style={{
                    width: `${
                      overviewData.quotaBytes > 0
                        ? Math.min(100, Math.round((overviewData.bytes / overviewData.quotaBytes) * 100))
                        : 0
                    }%`,
                  }}
                />
              </div>
            </div>
            <div className="usage-panel__meta">
              <span>Макс. файл: {formatBytes(overviewData.maxFileBytes)}</span>
              <span>Параллельных задач: {overviewData.maxParallelTasks}</span>
              <span>
                Проверено:{' '}
                {overviewData.usageCheckedAt ? relativeTime(overviewData.usageCheckedAt) : 'нет данных'}
              </span>
              <span>Снимков: {overviewData.snapshots}</span>
              <span>Инструментов в allowlist: {overviewData.allowedTools.length}</span>
              <Button size="sm" variant="ghost" icon={<RefreshCw aria-hidden="true" />} onClick={() => void refreshUsage()}>
                Обновить из Runtime
              </Button>
            </div>
            <p className="muted usage-panel__note">
              Квоты workspace проверяются Runtime до записи. Пользовательские лимиты
              (storage/files) проверяются backend перед каждой изменяющей операцией.
            </p>
          </div>
        ) : (
          <LoadingInline label="Загрузка usage" />
        );
      default:
        return null;
    }
  };

  const editorVisible = layout.explorerVisible && !layout.editorMaximized;
  const agentVisible = layout.agentVisible && !layout.editorMaximized;
  const showBottom = layout.bottomVisible && !layout.editorMaximized;

  return (
    <div className="ai-agent-page">
      <header className="ai-agent-topbar">
        <div className="ai-agent-topbar__left">
          <Link to="/workspace/ai" className="ai-agent-topbar__brand" title="Обычный AI Chat">
            <Bot aria-hidden="true" />
            <span>NEXUS AI</span>
          </Link>
          <WorkspaceSwitcher
            workspaces={workspaceList}
            activeId={workspaceId}
            overview={overviewData}
            canWrite={canWrite}
            loading={workspaces.isLoading}
            onSelect={selectWorkspace}
            onCreate={async (body) => {
              const created = await aiWorkspaceApi.createWorkspace(body);
              await queryClient.invalidateQueries({ queryKey: aiKeys.workspaces });
              const id = String(created.id ?? '');
              if (id) selectWorkspace(id);
            }}
            onRename={renameWorkspace}
            onDelete={deleteWorkspace}
            onRestore={restoreWorkspace}
            onRefreshUsage={() => void refreshUsage()}
          />
        </div>
        <div className="ai-agent-topbar__status">
          <span
            className={`runtime-pill${overviewData?.runtimeOnline ? ' is-online' : ' is-offline'}`}
            title="Статус NEXUS AI Runtime"
          >
            <span className="runtime-pill__dot" />
            Runtime {overviewData?.runtimeOnline ? 'online' : 'offline'}
          </span>
          {agentTask ? (
            <span className="task-pill" title={agentTask.statusText}>
              <span className={`task-pill__dot is-${agentTask.status}`} />
              {agentTask.status} · {agentTask.toolCallsCount}
            </span>
          ) : (
            <span className="task-pill muted">нет активной задачи</span>
          )}
        </div>
        <div className="ai-agent-topbar__actions">
          <Button
            size="sm"
            variant="ghost"
            icon={<ListTodo aria-hidden="true" />}
            onClick={() => setTaskManagerOpen(true)}
          >
            Задачи
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={<MessageSquare aria-hidden="true" />}
            onClick={newTask}
            disabled={!canAgent}
          >
            Новая задача
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={<LayoutPanelLeft aria-hidden="true" />}
            onClick={() => setSettingsOpen(true)}
            title="Настройки панелей"
          >
            Панели
          </Button>
        </div>
      </header>

      {workspaces.error ? (
        <Alert tone="danger">
          <strong>Не удалось загрузить workspace.</strong> {errorMessage(workspaces.error)}
        </Alert>
      ) : null}

      {workspaceList.length === 0 && !workspaces.isLoading ? (
        <div className="ai-agent-empty">
          <EmptyState
            icon={<HardDrive aria-hidden="true" />}
            title="Создайте рабочее пространство"
            action={
              <Button variant="primary" disabled={!canWrite} onClick={() => setCreateOpen(true)}>
                Создать workspace
              </Button>
            }
          >
            <p>
              Runtime выделит изолированный каталог. Файлы, задачи и снимки изменений
              будут доступны только вам.
            </p>
          </EmptyState>
        </div>
      ) : (
        <div
          className={`ai-agent-layout${layout.editorMaximized ? ' is-maximized' : ''} mobile-${mobilePane}`}
        >
          {editorVisible ? (
            <aside
              className="ai-agent-layout__explorer"
              style={{ width: layout.explorerWidth }}
              aria-label="Explorer"
            >
              {workspaceId ? (
                <WorkspaceExplorer
                  workspaceId={workspaceId}
                  canWrite={canWrite}
                  activePath={activeEditorPath}
                  changedPaths={changedSet}
                  busyPaths={busySet}
                  refreshToken={refreshToken}
                  onOpenFile={openFileFromAnywhere}
                  onFilesChanged={() => {
                    void queryClient.invalidateQueries({ queryKey: aiKeys.overview(workspaceId) });
                    void queryClient.invalidateQueries({ queryKey: ['ai-changes', workspaceId] });
                  }}
                />
              ) : (
                <LoadingInline label="Workspace" />
              )}
              <div
                className="resizer resizer--vertical"
                onPointerDown={(event) => beginDrag('explorer', event)}
                role="separator"
                aria-orientation="vertical"
              />
            </aside>
          ) : null}

          <main className="ai-agent-layout__editor">
            {workspaceId ? (
              <EditorLayout
                workspaceId={workspaceId}
                canWrite={canWrite}
                onFilesChanged={() => {
                  void queryClient.invalidateQueries({ queryKey: aiKeys.overview(workspaceId) });
                }}
                onRequestNewFile={() => {
                  setMobilePane('explorer');
                  toast('Создайте файл в Explorer: иконка «Новый файл».', 'info');
                }}
              />
            ) : (
              <EmptyState icon={<FileCode2 aria-hidden="true" />} title="Выберите workspace" />
            )}
          </main>

          {agentVisible ? (
            <aside
              className="ai-agent-layout__agent"
              style={{ width: layout.agentWidth }}
              aria-label="AI Agent"
            >
              <div
                className="resizer resizer--vertical resizer--left"
                onPointerDown={(event) => beginDrag('agent', event)}
                role="separator"
                aria-orientation="vertical"
              />
              <AgentPanel
                workspaceId={workspaceId}
                conversationId={conversationId}
                task={agentTask}
                historical={historical}
                canSend={canAgent}
                canApprove={canApprove}
                canCancel={canCancel}
                runtimeOnline={overviewData?.runtimeOnline ?? false}
                streaming={Boolean(streamingMessageId) || agentActive}
                models={models}
                model={model}
                onModel={setModel}
                skills={skills.data?.items ?? []}
                events={taskEvents}
                eventsLoading={taskEventsLoading}
                eventsError={taskEventsError}
                eventStats={taskEventStats}
                reloadEvents={reloadTaskEvents}
                ensureConversation={ensureConversation}
                onSend={sendMessage}
                onStop={() => void stop()}
                onRegenerate={regenerate}
                onEditMessage={editMessage}
                onNewTask={newTask}
                onBackToLive={() => {
                  setHistorical(false);
                  setAgentTaskId(liveTask?.id ?? null);
                  if (liveTask) {
                    setConversationId(liveTask.conversationId);
                    if (workspaceId) writeConversation(workspaceId, liveTask.conversationId);
                  }
                }}
                onTaskChanged={() => {
                  void queryClient.invalidateQueries({ queryKey: aiKeys.task(agentTask?.id ?? null) });
                  void queryClient.invalidateQueries({ queryKey: aiKeys.tasks(workspaceId, 'active') });
                }}
              />
            </aside>
          ) : null}
        </div>
      )}

      {showBottom ? (
        <section className="bottom-panel" style={{ height: layout.bottomHeight }}>
          <div
            className="resizer resizer--horizontal"
            onPointerDown={(event) => beginDrag('bottom', event)}
            role="separator"
            aria-orientation="horizontal"
          />
          <div className="bottom-panel__tabs">
            {taskTabs.map((tab) => (
              <button
                key={tab.value}
                type="button"
                className={layout.bottomTab === tab.value ? 'is-active' : ''}
                onClick={() => setLayout({ bottomTab: tab.value })}
              >
                {tab.icon}
                {tab.label}
              </button>
            ))}
            <span className="bottom-panel__spacer" />
            <button
              type="button"
              className="ai-icon-btn"
              title={layout.bottomVisible ? 'Скрыть панель' : 'Показать панель'}
              onClick={() => setLayout({ bottomVisible: false })}
            >
              <ChevronDown aria-hidden="true" />
            </button>
          </div>
          <div className="bottom-panel__body">{bottomContent()}</div>
        </section>
      ) : !layout.editorMaximized ? (
        <button
          type="button"
          className="bottom-panel__restore"
          onClick={() => setLayout({ bottomVisible: true })}
        >
          <ChevronUp aria-hidden="true" /> Показать панель
        </button>
      ) : null}

      <div className="ai-agent-mobile-tabs">
        <Segmented
          value={mobilePane}
          onChange={(pane: MobilePane) => setMobilePane(pane)}
          options={[
            { value: 'explorer', label: 'Explorer' },
            { value: 'editor', label: 'Editor' },
            { value: 'agent', label: 'Agent' },
          ]}
          label="Область экрана"
        />
      </div>

      <Modal
        open={taskManagerOpen}
        onClose={() => setTaskManagerOpen(false)}
        wide
        title="AI Agent · Задачи"
        description="История задач сохраняется в PostgreSQL. Открытие задачи не запускает её повторно."
        footer={
          <Button variant="ghost" onClick={() => setTaskManagerOpen(false)}>
            Закрыть
          </Button>
        }
      >
        {workspaceId ? (
          <div className="task-manager">
            <TaskExplorer
              workspaceId={workspaceId}
              selectedId={selectedTask?.id ?? null}
              filter={taskFilter}
              onFilter={setTaskFilter}
              onSelect={setSelectedTask}
            />
            <div className="task-manager__details">
              {selectedTask ? (
                <TaskDetails
                  key={selectedTask.id}
                  workspaceId={workspaceId}
                  task={selectedTask}
                  canWrite={canWrite}
                  onOpenFile={(path) => {
                    openFileFromAnywhere(path);
                    setTaskManagerOpen(false);
                  }}
                  onOpenInPanel={openTaskInPanel}
                  onRestored={() => setRefreshToken((token) => token + 1)}
                />
              ) : (
                <div className="task-manager__empty muted">
                  Выберите задачу слева. Фильтры: {TASK_FILTERS.map((item) => item.label).join(' · ')}
                </div>
              )}
            </div>
          </div>
        ) : (
          <LoadingInline label="Workspace" />
        )}
      </Modal>

      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="Новый workspace"
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreateOpen(false)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              loading={createMutation.isPending}
              onClick={() => createMutation.mutate()}
            >
              Создать
            </Button>
          </>
        }
      >
        <Field label="Название">
          <input
            autoFocus
            value={newName}
            maxLength={80}
            onChange={(event) => setNewName(event.target.value)}
          />
        </Field>
        <Field label="Описание" hint="Необязательно">
          <textarea
            rows={3}
            maxLength={2000}
            value={newDescription}
            onChange={(event) => setNewDescription(event.target.value)}
          />
        </Field>
      </Modal>

      <Modal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        title="Настройки Agent и панелей"
        footer={
          <>
            <Button variant="ghost" onClick={() => resetLayout()}>
              Сбросить раскладку
            </Button>
            <Button variant="primary" onClick={() => setSettingsOpen(false)}>
              Готово
            </Button>
          </>
        }
      >
        <div className="layout-settings">
          <Field label="Провайдер" hint="Источник модели для новых задач">
            <select
              value={provider?.id ?? ''}
              onChange={(event) => {
                setProviderId(event.target.value);
                const next = providers.data?.items.find(
                  (item) => item.id === event.target.value,
                );
                if (next) setModel(next.defaultModel);
              }}
            >
              {providers.data?.items.map((item) => (
                <option key={item.id} value={item.id} disabled={item.enabled === false}>
                  {item.name}
                  {item.status === 'error' ? ' — ошибка' : ''}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Модель Agent" hint="Список моделей Runtime; применяется к новым задачам">
            {models.length > 0 ? (
              <select value={model} onChange={(event) => setModel(event.target.value)}>
                {models.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            ) : (
              <input value={model} onChange={(event) => setModel(event.target.value)} />
            )}
          </Field>
          <p className="muted">
            Провайдер: {provider?.name ?? 'не настроен'} · Runtime:{' '}
            {overviewData?.runtimeOnline ? 'online' : 'offline'} · workspace:{' '}
            {overviewData?.name ?? '—'}
          </p>
          <label>
            <input
              type="checkbox"
              checked={layout.explorerVisible}
              onChange={(event) => setLayout({ explorerVisible: event.target.checked })}
            />
            <LayoutPanelLeft aria-hidden="true" /> Explorer
          </label>
          <label>
            <input
              type="checkbox"
              checked={layout.agentVisible}
              onChange={(event) => setLayout({ agentVisible: event.target.checked })}
            />
            <PanelRight aria-hidden="true" /> AI Agent
          </label>
          <label>
            <input
              type="checkbox"
              checked={layout.bottomVisible}
              onChange={(event) => setLayout({ bottomVisible: event.target.checked })}
            />
            <PanelBottom aria-hidden="true" /> Нижняя панель
          </label>
          <label>
            <input
              type="checkbox"
              checked={layout.editorMaximized}
              onChange={(event) => setLayout({ editorMaximized: event.target.checked })}
            />
            Полноэкранный редактор
          </label>
          <div className="layout-settings__sizes">
            <Field label={`Explorer ${layout.explorerWidth}px`}>
              <input
                type="range"
                min={180}
                max={520}
                value={layout.explorerWidth}
                onChange={(event) => setLayout({ explorerWidth: Number(event.target.value) })}
              />
            </Field>
            <Field label={`Agent ${layout.agentWidth}px`}>
              <input
                type="range"
                min={300}
                max={720}
                value={layout.agentWidth}
                onChange={(event) => setLayout({ agentWidth: Number(event.target.value) })}
              />
            </Field>
            <Field label={`Нижняя панель ${layout.bottomHeight}px`}>
              <input
                type="range"
                min={120}
                max={560}
                value={layout.bottomHeight}
                onChange={(event) => setLayout({ bottomHeight: Number(event.target.value) })}
              />
            </Field>
          </div>
          <p className="muted">Раскладка сохраняется в этом браузере.</p>
        </div>
      </Modal>

      {stopping ? (
        <div className="ai-agent-busy" aria-live="polite">
          <Loader2 className="spin" aria-hidden="true" /> Остановка задачи…
        </div>
      ) : null}
    </div>
  );
}
