/** Правая панель AI Agent: статус, сообщения, действия, approvals. */
import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, ShieldAlert, Sparkles, X } from 'lucide-react';

import { Badge, Button, Field, Modal, Segmented } from '../../ui';
import { aiKeys } from '../../../lib/aiKeys';
import type { TaskEventStats } from '../../../lib/taskEvents';
import {
  aiApi,
  errorMessage,
} from '../../../services/api';
import type {
  AiApproval,
  AiAttachment,
  AiMessage,
  AiSkill,
  AiTask,
  AiTaskEvent,
} from '../../../services/api';
import { toast } from '../../../stores/ui';
import { AgentActivity } from './AgentActivity';
import { AgentComposer } from './AgentComposer';
import { AgentMessages } from './AgentMessages';
import { AgentTaskStatus } from './AgentTaskStatus';

const PAGE_SIZE = 60;

interface AgentPanelProps {
  workspaceId: string | null;
  conversationId: string | null;
  task: AiTask | undefined;
  historical: boolean;
  canSend: boolean;
  canApprove: boolean;
  canCancel: boolean;
  runtimeOnline: boolean;
  streaming: boolean;
  models: string[];
  model: string;
  onModel: (model: string) => void;
  skills: AiSkill[];
  events: AiTaskEvent[];
  eventsLoading: boolean;
  eventsError: string | null;
  eventStats: TaskEventStats;
  reloadEvents: () => void;
  ensureConversation: () => Promise<string>;
  onSend: (payload: {
    content: string;
    attachmentIds: string[];
    skillIds: string[];
  }) => Promise<void>;
  onStop: () => void;
  onRegenerate: (messageId: string) => Promise<void>;
  onEditMessage: (messageId: string, content: string) => Promise<void>;
  onNewTask: () => void;
  onBackToLive: () => void;
  onTaskChanged: () => void;
}

export function AgentPanel({
  workspaceId,
  conversationId,
  task,
  historical,
  canSend,
  canApprove,
  canCancel,
  runtimeOnline,
  streaming,
  models,
  model,
  onModel,
  skills,
  events,
  eventsLoading,
  eventsError,
  eventStats,
  reloadEvents,
  ensureConversation,
  onSend,
  onStop,
  onRegenerate,
  onEditMessage,
  onNewTask,
  onBackToLive,
  onTaskChanged,
}: AgentPanelProps) {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<'messages' | 'activity'>('messages');
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [attachments, setAttachments] = useState<AiAttachment[]>([]);
  const [attaching, setAttaching] = useState(false);
  const [skillIds, setSkillIds] = useState<string[]>([]);
  const [editTarget, setEditTarget] = useState<AiMessage | null>(null);
  const [editValue, setEditValue] = useState('');
  const [older, setOlder] = useState<AiMessage[]>([]);
  const [olderPage, setOlderPage] = useState(0);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    setOlder([]);
    setOlderPage(0);
    setDraft('');
    setAttachments([]);
    setSkillIds([]);
  }, [conversationId]);

  const messages = useQuery({
    queryKey: aiKeys.messages(conversationId),
    queryFn: () => aiApi.messages(conversationId!, 1, PAGE_SIZE, true),
    enabled: Boolean(conversationId),
    refetchInterval: (query) =>
      query.state.data?.items.some((item) => item.status === 'streaming') ? 3000 : false,
  });

  // Первая загрузка: вычисляем номер последней страницы для «более ранних».
  useEffect(() => {
    const page = messages.data;
    if (!page) return;
    const pages = Math.max(1, Math.ceil(page.total / PAGE_SIZE));
    setOlderPage((current) => (current === 0 ? pages - 1 : current));
  }, [messages.data]);

  const merged = useMemo(() => {
    const map = new Map<string, AiMessage>();
    for (const message of older) map.set(message.id, message);
    for (const message of messages.data?.items ?? []) map.set(message.id, message);
    return [...map.values()].sort((left, right) => left.sequence - right.sequence);
  }, [older, messages.data]);

  const loadOlder = async () => {
    if (!conversationId || olderPage < 1 || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await aiApi.messages(conversationId, olderPage, PAGE_SIZE, false);
      setOlder((current) => {
        const map = new Map(current.map((message) => [message.id, message]));
        for (const message of page.items) map.set(message.id, message);
        return [...map.values()].sort((left, right) => left.sequence - right.sequence);
      });
      setOlderPage(olderPage - 1);
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setLoadingOlder(false);
    }
  };

  const approvals = useQuery({
    queryKey: aiKeys.approvals,
    queryFn: aiApi.pendingApprovals,
    enabled: canApprove,
    refetchInterval: 10_000,
  });
  const taskApprovals: AiApproval[] = useMemo(
    () => (approvals.data?.items ?? []).filter((item) => !task || item.taskId === task.id),
    [approvals.data, task],
  );

  const decision = useMutation({
    mutationFn: async (payload: { id: string; approve: boolean }) =>
      payload.approve ? aiApi.approve(payload.id) : aiApi.reject(payload.id, 'Отклонено пользователем'),
    onSuccess: (_result, variables) => {
      toast(variables.approve ? 'Действие подтверждено.' : 'Действие отклонено.', 'success');
      void queryClient.invalidateQueries({ queryKey: aiKeys.approvals });
      onTaskChanged();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const submitSend = async () => {
    const content = draft.trim();
    if (!content || sending) return;
    setSending(true);
    try {
      await onSend({ content, attachmentIds: attachments.map((item) => item.id), skillIds });
      setDraft('');
      setAttachments([]);
      setSkillIds([]);
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setSending(false);
    }
  };

  const attach = async (file: File) => {
    setAttaching(true);
    try {
      const id = await ensureConversation();
      const attachment = await aiApi.uploadAttachment(id, file);
      setAttachments((current) => [...current, attachment]);
    } catch (error) {
      toast(errorMessage(error), 'error');
    } finally {
      setAttaching(false);
    }
  };

  const submitEdit = async () => {
    if (!editTarget || !editValue.trim()) return;
    try {
      await onEditMessage(editTarget.id, editValue.trim());
      setEditTarget(null);
      setEditValue('');
    } catch (error) {
      toast(errorMessage(error), 'error');
    }
  };

  const disabledReason = !canSend
    ? 'Нет права ai.agent.use.'
    : !workspaceId
      ? 'Выберите workspace.'
      : !runtimeOnline
        ? 'Runtime offline: отправка недоступна.'
        : null;

  return (
    <aside className="agent-pane" aria-label="AI Agent">
      <div className="agent-pane__head">
        <div className="agent-pane__title">
          <Sparkles aria-hidden="true" />
          <span>AI Agent</span>
          {historical ? <Badge tone="warn">история</Badge> : null}
        </div>
        <Button size="sm" variant="ghost" onClick={onNewTask}>
          Новая задача
        </Button>
      </div>
      <AgentTaskStatus
        task={task}
        historical={historical}
        canCancel={canCancel}
        stopping={Boolean(task?.cancelRequested)}
        onCancel={onStop}
        onBackToLive={onBackToLive}
      />
      <div className="agent-pane__tabs">
        <Segmented
          value={tab}
          onChange={setTab}
          options={[
            { value: 'messages', label: 'Сообщения' },
            { value: 'activity', label: 'Действия' },
          ]}
          label="Раздел Agent-панели"
        />
        {task && ['queued', 'running', 'waiting_approval'].includes(task.status) ? (
          <span className="agent-pane__live">
            <span className="agent-pane__live-dot" /> live
          </span>
        ) : null}
      </div>

      {tab === 'messages' ? (
        <AgentMessages
          messages={merged}
          loading={messages.isLoading}
          loadingOlder={loadingOlder}
          hasOlder={olderPage >= 1 && Boolean(conversationId)}
          historical={historical}
          streaming={streaming}
          canSend={canSend}
          onLoadOlder={() => void loadOlder()}
          onEdit={(message) => {
            setEditTarget(message);
            setEditValue(message.content);
          }}
          onRegenerate={() => {
            const lastAssistant = [...merged]
              .reverse()
              .find((message) => message.role === 'assistant' && message.status !== 'streaming');
            if (lastAssistant) void onRegenerate(lastAssistant.id);
          }}
          scrollRef={scrollRef}
        />
      ) : (
        <div className="agent-pane__activity">
          <AgentActivity
            events={events}
            loading={eventsLoading}
            error={eventsError}
            stats={eventStats}
            onReload={reloadEvents}
          />
        </div>
      )}

      {taskApprovals.length > 0 ? (
        <div className="agent-approvals">
          <div className="agent-approvals__head">
            <ShieldAlert aria-hidden="true" /> Требуется подтверждение
          </div>
          {taskApprovals.map((approval) => (
            <div key={approval.id} className="agent-approval">
              <div className="agent-approval__title">
                <Badge tone={approval.risk === 'high' ? 'danger' : 'warn'}>{approval.toolId}</Badge>
                <span className="muted">{approval.statusText}</span>
              </div>
              <pre className="agent-approval__args">
                {JSON.stringify(approval.arguments, null, 2)}
              </pre>
              <div className="agent-approval__actions">
                <Button
                  size="sm"
                  variant="primary"
                  icon={<Check aria-hidden="true" />}
                  loading={decision.isPending}
                  onClick={() => decision.mutate({ id: approval.id, approve: true })}
                >
                  Подтвердить
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  icon={<X aria-hidden="true" />}
                  loading={decision.isPending}
                  onClick={() => decision.mutate({ id: approval.id, approve: false })}
                >
                  Отклонить
                </Button>
              </div>
            </div>
          ))}
        </div>
      ) : null}

      <AgentComposer
        value={draft}
        onChange={setDraft}
        onSend={() => void submitSend()}
        onStop={onStop}
        canSend={canSend && Boolean(workspaceId) && runtimeOnline}
        streaming={streaming}
        sending={sending}
        disabledReason={disabledReason}
        attachments={attachments}
        onAttach={attach}
        onRemoveAttachment={(id) =>
          setAttachments((current) => current.filter((item) => item.id !== id))
        }
        attaching={attaching}
        models={models}
        model={model}
        onModel={onModel}
        skills={skills}
        skillIds={skillIds}
        onSkills={setSkillIds}
      />

      <Modal
        open={Boolean(editTarget)}
        onClose={() => setEditTarget(null)}
        title="Редактировать сообщение"
        description="Будет создана новая задача с изменённым запросом. История сохраняется."
        footer={
          <>
            <Button variant="ghost" onClick={() => setEditTarget(null)}>
              Отмена
            </Button>
            <Button variant="primary" loading={sending} onClick={() => void submitEdit()}>
              Отправить
            </Button>
          </>
        }
      >
        <Field label="Сообщение">
          <textarea
            autoFocus
            rows={6}
            value={editValue}
            onChange={(event) => setEditValue(event.target.value)}
          />
        </Field>
      </Modal>
    </aside>
  );
}
