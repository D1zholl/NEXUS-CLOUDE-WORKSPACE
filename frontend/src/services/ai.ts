/** Сервис NEXUS AI Workspace: Chat, Agent, Runtime, Skills, approvals. */
import { newIdempotencyKey, queryString, request } from './http';
import type { Page } from './types';

export interface AiProvider {
  id: string;
  name: string;
  type?: string;
  baseUrl?: string;
  defaultModel: string;
  models?: string[];
  status: 'unknown' | 'ok' | 'error' | 'unavailable' | string;
  lastCheckedAt?: string | null;
  lastError?: string | null;
  runtimeNodeId?: string | null;
  hasApiKey?: boolean;
  generation?: Record<string, unknown>;
  limits?: Record<string, unknown>;
  enabled?: boolean;
}

export interface AiConversation {
  id: string;
  title: string;
  mode: 'chat' | 'agent';
  providerId: string | null;
  model: string;
  workspaceId: string | null;
  runtimeNodeId: string | null;
  state: 'active' | 'archived';
  createdAt: string;
  updatedAt: string;
  lastMessageAt: string | null;
  streamingMessageId?: string | null;
}

export interface AiAttachment {
  id: string;
  name: string;
  mime: string;
  sizeBytes: number;
}

export interface AiMessage {
  id: string;
  conversationId: string;
  sequence: number;
  role: 'user' | 'assistant' | 'tool' | 'system';
  content: string;
  parts: unknown[];
  model: string;
  status: 'streaming' | 'completed' | 'failed' | 'cancelled';
  usage: Record<string, unknown>;
  toolCalls: unknown[];
  toolCallId: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AiTask {
  id: string;
  conversationId: string;
  ownerId: string;
  runtimeNodeId: string;
  providerId: string | null;
  model: string;
  workspaceId: string | null;
  status: 'queued' | 'running' | 'waiting_approval' | 'completed' | 'failed' | 'cancelled';
  stage: string;
  statusText: string;
  progress: number;
  iterations: number;
  maxIterations: number;
  toolCallsCount: number;
  timeoutSeconds: number;
  cancelRequested: boolean;
  startedAt: string | null;
  finishedAt: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AiTaskEvent {
  id: number;
  kind: string;
  stage: string;
  message: string;
  data: Record<string, unknown>;
  createdAt: string;
}

export interface AiExecution {
  id: string;
  toolCallId: string;
  toolId: string;
  toolVersion: number;
  arguments: Record<string, unknown>;
  argsHash: string;
  risk: string;
  approvalId: string | null;
  status: string;
  resultText: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  exitCode: number | null;
  durationMs: number | null;
  startedAt: string | null;
  finishedAt: string | null;
  createdAt: string;
}

export interface AiApproval {
  id: string;
  taskId: string;
  executionId: string;
  conversationId: string;
  workspaceId: string | null;
  toolId: string;
  risk: string;
  arguments: Record<string, unknown>;
  statusText: string;
  expiresAt: string;
  createdAt: string;
}

export interface AiWorkspace {
  id: string;
  ownerId: string;
  runtimeNodeId: string;
  name: string;
  rootPath: string | null;
  quotaBytes: number;
  maxFiles: number;
  maxFileBytes: number;
  maxParallelTasks: number;
  maxTaskSeconds: number;
  allowedTools: string[];
  allowedPrograms: string[];
  status: 'active' | 'suspended' | string;
  usageBytes: number;
  usageFiles: number;
  usageCheckedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AiUsage {
  workspaceId: string;
  quotaBytes: number;
  usedBytes: number;
  maxFiles: number;
  usedFiles: number;
  maxFileBytes: number;
  maxParallelTasks: number;
  maxTaskSeconds: number;
  checkedAt: string | null;
  source: 'runtime' | 'none';
}

export interface AiSkill {
  id: string;
  slug: string;
  name: string;
  description: string;
  version: number;
  checksum?: string;
  sizeBytes?: number;
  fileCount?: number;
  dependencies?: string[];
  enabled?: boolean;
  createdAt?: string;
  updatedAt?: string;
}

export interface AiToolInfo {
  id: string;
  name: string;
  description: string;
  category: string;
  serverSide?: boolean;
  inputSchema?: Record<string, unknown>;
  enabled?: boolean;
  risk: string;
  approvalPolicy?: string;
  maxTimeoutSeconds?: number;
  maxOutputBytes?: number;
  requiresApproval?: boolean;
  timeoutSeconds?: number;
}

export interface AiRuntimeNode {
  id: string;
  name: string;
  platform?: string;
  runtimeVersion?: string;
  protocolVersion?: number;
  status: string;
  connected?: boolean;
  lastSeenAt?: string | null;
  lastHeartbeatAt?: string | null;
  registeredAt?: string;
  revokedAt?: string | null;
  tokenRotatedAt?: string | null;
  activeTasks?: number;
  capabilities?: {
    models?: string[];
    polystrata?: {
      reachable?: boolean;
      version?: string;
      modelCount?: number;
      error?: string | null;
    };
    tools?: AiToolInfo[];
    programs?: unknown[];
  };
}

export interface AiProgram {
  id: string;
  runtimeNodeId: string;
  programId: string;
  name: string;
  executable: string;
  version: string;
  available: boolean;
  risk: string;
  enabled: boolean;
  updatedAt: string;
}

export interface AiOverview {
  moduleEnabled: boolean;
  runtimes: {
    total: number;
    online: number;
    disabledOrRevoked: number;
    connectedNow: number;
  };
  providers: { total: number; enabled: number; error: number };
  tasks: {
    running: number;
    queued: number;
    waitingApproval: number;
    failedLast24h: number;
    queueAgeSeconds: number | null;
  };
  approvals: { pending: number };
  streaming: { activeChats: number; events: number };
  lastHeartbeatAt: string | null;
  nodes: Array<Record<string, unknown>>;
  recentErrors: Array<{ kind: string; message: string; at: string }>;
  checkedAt: string;
}

export interface AiQuota {
  userId: string;
  username: string;
  displayName: string;
  maxConcurrentTasks: number;
  maxTasksPerDay: number;
  maxToolCallsPerTask: number;
  maxTaskSeconds: number;
  maxStorageBytes: number;
  maxFiles: number;
  maxFileBytes: number;
  updatedAt: string;
}

export interface AiEnrollmentCode {
  id: string;
  label: string;
  scopes: string[];
  expiresAt: string;
  usedAt: string | null;
  usedByNode: string | null;
  revokedAt: string | null;
  createdAt: string;
}

export interface AiAssignment {
  id: string;
  scope: 'user' | 'workspace' | 'role';
  userId: string | null;
  workspaceId: string | null;
  roleId: string | null;
  enabled: boolean;
  createdAt: string;
}

const base = '/ai';

export const aiApi = {
  providers: () => request<{ items: AiProvider[] }>(`${base}/providers`),
  conversations: (
    params: {
      search?: string;
      mode?: string;
      archived?: boolean;
      page?: number;
      perPage?: number;
    } = {},
  ) =>
    request<Page<AiConversation>>(
      `${base}/conversations${queryString({
        search: params.search,
        mode: params.mode,
        archived: params.archived ? 'true' : undefined,
        page: params.page,
        perPage: params.perPage,
      })}`,
    ),
  createConversation: (body: {
    mode: 'chat' | 'agent';
    providerId?: string;
    model?: string;
    workspaceId?: string | null;
    title?: string;
  }) => request<AiConversation>(`${base}/conversations`, { method: 'POST', body }),
  conversation: (id: string) => request<AiConversation>(`${base}/conversations/${id}`),
  updateConversation: (
    id: string,
    body: { title?: string; model?: string; workspaceId?: string | null; state?: string },
  ) => request<AiConversation>(`${base}/conversations/${id}`, { method: 'PATCH', body }),
  deleteConversation: (id: string) =>
    request<{ deleted: boolean }>(`${base}/conversations/${id}`, { method: 'DELETE' }),
  messages: (id: string, page = 1, perPage = 100) =>
    request<Page<AiMessage>>(
      `${base}/conversations/${id}/messages${queryString({ page, perPage })}`,
    ),
  sendMessage: (
    id: string,
    body: { content: string; attachmentIds?: string[]; idempotencyKey?: string },
  ) =>
    request<{
      userMessage: AiMessage;
      assistantMessage?: AiMessage;
      task?: AiTask;
    }>(`${base}/conversations/${id}/messages`, {
      method: 'POST',
      body: { ...body, idempotencyKey: body.idempotencyKey ?? newIdempotencyKey() },
    }),
  regenerate: (conversationId: string, messageId: string) =>
    request<{ assistantMessage?: AiMessage; task?: AiTask }>(
      `${base}/conversations/${conversationId}/messages/${messageId}/regenerate`,
      { method: 'POST', body: {} },
    ),
  editMessage: (conversationId: string, messageId: string, content: string) =>
    request<{ assistantMessage?: AiMessage; task?: AiTask }>(
      `${base}/conversations/${conversationId}/messages/${messageId}`,
      { method: 'PATCH', body: { content } },
    ),
  stop: (id: string) =>
    request<{ stopped: boolean; messageId: string; note: string }>(
      `${base}/conversations/${id}/stop`,
      { method: 'POST', body: {} },
    ),
  uploadAttachment: (id: string, file: File) => {
    const formData = new FormData();
    formData.append('file', file);
    return request<AiAttachment>(`${base}/conversations/${id}/attachments`, {
      method: 'POST',
      formData,
    });
  },
  tasks: (
    params: {
      page?: number;
      perPage?: number;
      status?: string;
      conversationId?: string;
    } = {},
  ) => request<Page<AiTask>>(`${base}/tasks${queryString({ ...params, perPage: params.perPage })}`),
  task: (id: string) => request<AiTask>(`${base}/tasks/${id}`),
  taskEvents: (id: string, perPage = 200) =>
    request<{ items: AiTaskEvent[] }>(`${base}/tasks/${id}/events${queryString({ perPage })}`),
  cancelTask: (id: string) =>
    request<{ taskId: string; requested: boolean; status: string }>(`${base}/tasks/${id}/cancel`, {
      method: 'POST',
      body: {},
    }),
  executions: (id: string) => request<{ items: AiExecution[] }>(`${base}/tasks/${id}/executions`),
  pendingApprovals: () => request<{ items: AiApproval[] }>(`${base}/approvals/pending`),
  approve: (id: string, reason = '') =>
    request<Record<string, unknown>>(`${base}/approvals/${id}/approve`, {
      method: 'POST',
      body: { reason },
    }),
  reject: (id: string, reason = '') =>
    request<Record<string, unknown>>(`${base}/approvals/${id}/reject`, {
      method: 'POST',
      body: { reason },
    }),
  workspaces: () => request<{ items: AiWorkspace[] }>(`${base}/workspaces`),
  workspace: (id: string) => request<AiWorkspace>(`${base}/workspaces/${id}`),
  workspaceUsage: (id: string) => request<AiUsage>(`${base}/workspaces/${id}/usage`),
  skills: (workspaceId?: string) =>
    request<{ items: AiSkill[] }>(`${base}/skills${queryString({ workspaceId })}`),
  skill: (id: string) => request<AiSkill>(`${base}/skills/${id}`),
  tools: () => request<{ items: AiToolInfo[] }>(`${base}/tools`),

  // --- System Management ---
  adminOverview: () => request<AiOverview>(`${base}/admin/overview`),
  adminLogs: (params: { page?: number; perPage?: number; search?: string } = {}) =>
    request<{ audit: Page<Record<string, unknown>>; runtime: Page<Record<string, unknown>> }>(
      `${base}/admin/logs${queryString({ ...params, perPage: params.perPage })}`,
    ),
  runtimeNodes: () => request<{ items: AiRuntimeNode[] }>(`${base}/admin/runtime-nodes`),
  runtimeNode: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/runtime-nodes/${id}`),
  revokeRuntime: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/runtime-nodes/${id}/revoke`, {
      method: 'POST',
      body: {},
    }),
  disableRuntime: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/runtime-nodes/${id}/disable`, {
      method: 'POST',
      body: {},
    }),
  enableRuntime: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/runtime-nodes/${id}/enable`, {
      method: 'POST',
      body: {},
    }),
  rotateRuntimeToken: (id: string) =>
    request<{ deviceToken: string; note: string }>(
      `${base}/admin/runtime-nodes/${id}/rotate-token`,
      { method: 'POST', body: {} },
    ),
  enrollmentCodes: () => request<{ items: AiEnrollmentCode[] }>(`${base}/admin/enrollment-codes`),
  createEnrollmentCode: (label: string, ttlSeconds = 900) =>
    request<{ id: string; code: string; expiresAt: string }>(`${base}/admin/enrollment-codes`, {
      method: 'POST',
      body: { label, ttlSeconds },
    }),
  revokeEnrollmentCode: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/enrollment-codes/${id}/revoke`, {
      method: 'POST',
      body: {},
    }),
  createProvider: (body: Record<string, unknown>) =>
    request<AiProvider>(`${base}/admin/providers`, { method: 'POST', body }),
  updateProvider: (id: string, body: Record<string, unknown>) =>
    request<AiProvider>(`${base}/admin/providers/${id}`, { method: 'PATCH', body }),
  deleteProvider: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/providers/${id}`, { method: 'DELETE' }),
  testProvider: (id: string) =>
    request<{ ok: boolean; models: string[]; data: unknown }>(
      `${base}/admin/providers/${id}/test`,
      { method: 'POST', body: {} },
    ),
  providerModels: (id: string) =>
    request<{ models: string[]; checkedAt: string }>(`${base}/admin/providers/${id}/models`),
  providerStatus: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/providers/${id}/status`),
  allWorkspaces: () => request<{ items: AiWorkspace[] }>(`${base}/admin/workspaces`),
  createWorkspace: (body: Record<string, unknown>) =>
    request<AiWorkspace>(`${base}/admin/workspaces`, { method: 'POST', body }),
  updateWorkspace: (id: string, body: Record<string, unknown>) =>
    request<AiWorkspace>(`${base}/admin/workspaces/${id}`, { method: 'PATCH', body }),
  allSkills: () => request<{ items: AiSkill[] }>(`${base}/admin/skills`),
  importSkill: (file: File) => {
    const formData = new FormData();
    formData.append('file', file);
    return request<{ skill: AiSkill; version: number; created: boolean }>(
      `${base}/admin/skills/import`,
      { method: 'POST', formData },
    );
  },
  updateSkill: (id: string, body: Record<string, unknown>) =>
    request<AiSkill>(`${base}/admin/skills/${id}`, { method: 'PATCH', body }),
  enableSkill: (id: string) =>
    request<AiSkill>(`${base}/admin/skills/${id}/enable`, { method: 'POST', body: {} }),
  disableSkill: (id: string) =>
    request<AiSkill>(`${base}/admin/skills/${id}/disable`, { method: 'POST', body: {} }),
  deleteSkill: (id: string) =>
    request<Record<string, unknown>>(`${base}/admin/skills/${id}`, { method: 'DELETE' }),
  skillAssignments: (id: string) =>
    request<{ items: AiAssignment[] }>(`${base}/admin/skills/${id}/assignments`),
  assignSkill: (id: string, body: Record<string, unknown>) =>
    request<Record<string, unknown>>(`${base}/admin/skills/${id}/assignments`, {
      method: 'POST',
      body,
    }),
  removeSkillAssignment: (skillId: string, assignmentId: string) =>
    request<Record<string, unknown>>(
      `${base}/admin/skills/${skillId}/assignments/${assignmentId}`,
      { method: 'DELETE' },
    ),
  toolPolicies: () => request<{ items: AiToolInfo[] }>(`${base}/admin/tools`),
  updateTool: (id: string, body: Record<string, unknown>) =>
    request<AiToolInfo>(`${base}/admin/tools/${id}`, { method: 'PATCH', body }),
  programs: () => request<{ items: AiProgram[] }>(`${base}/admin/programs`),
  updateProgram: (id: string, body: Record<string, unknown>) =>
    request<Record<string, unknown>>(`${base}/admin/programs/${id}`, { method: 'PATCH', body }),
  quotas: () => request<{ items: AiQuota[] }>(`${base}/admin/quotas`),
  setQuota: (userId: string, body: Record<string, unknown>) =>
    request<Record<string, unknown>>(`${base}/admin/quotas/${userId}`, {
      method: 'PUT',
      body,
    }),
};
