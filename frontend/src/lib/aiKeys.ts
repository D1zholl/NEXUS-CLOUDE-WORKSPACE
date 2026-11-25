/** Ключи TanStack Query для AI Agent Workspace. */
export const aiKeys = {
  workspaces: ['ai-workspaces'] as const,
  overview: (workspaceId: string | null) => ['ai-overview', workspaceId] as const,
  tasks: (workspaceId: string | null, filter: string) =>
    ['ai-agent-tasks', workspaceId, filter] as const,
  task: (taskId: string | null) => ['ai-agent-task', taskId] as const,
  executions: (taskId: string | null) => ['ai-agent-executions', taskId] as const,
  messages: (conversationId: string | null) =>
    ['ai-agent-messages', conversationId] as const,
  changes: (workspaceId: string | null, taskId: string | null) =>
    ['ai-changes', workspaceId, taskId] as const,
  snapshot: (workspaceId: string | null, snapshotId: string | null) =>
    ['ai-snapshot', workspaceId, snapshotId] as const,
  providers: ['ai-providers'] as const,
  skills: (workspaceId: string | null) => ['ai-agent-skills', workspaceId] as const,
  approvals: ['ai-approvals'] as const,
};
