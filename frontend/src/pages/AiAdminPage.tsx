import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Activity,
  Bot,
  Check,
  Cpu,
  FileArchive,
  KeyRound,
  Network,
  PlugZap,
  RefreshCw,
  Server,
  ShieldAlert,
  Trash2,
  Upload,
  Users,
  Wrench,
} from 'lucide-react';

import { useSession } from '../stores/auth';

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
  StatCard,
  Switch,
} from '../components/ui';
import { formatBytes, formatDate, formatNumber } from '../lib/format';
import { aiApi, api, errorMessage } from '../services/api';
import type {
  AiAssignment,
  AiMcpConnection,
  AiOverview,
  AiProgram,
  AiProvider,
  AiRuntimeNode,
  AiSkill,
  AiToolInfo,
  AiWorkspace,
} from '../services/api';
import { toast } from '../stores/ui';

const TABS = [
  { id: 'overview', label: 'Обзор', icon: <Activity aria-hidden="true" /> },
  { id: 'providers', label: 'Provider', icon: <PlugZap aria-hidden="true" /> },
  { id: 'runtime', label: 'Runtime', icon: <Server aria-hidden="true" /> },
  { id: 'workspaces', label: 'Workspaces', icon: <Cpu aria-hidden="true" /> },
  { id: 'skills', label: 'Skills', icon: <FileArchive aria-hidden="true" /> },
  { id: 'tools', label: 'Tools & Programs', icon: <Wrench aria-hidden="true" /> },
  { id: 'mcp', label: 'MCP', icon: <Network aria-hidden="true" /> },
  { id: 'quotas', label: 'Quotas', icon: <Users aria-hidden="true" /> },
  { id: 'logs', label: 'Security & Logs', icon: <ShieldAlert aria-hidden="true" /> },
] as const;

type TabId = (typeof TABS)[number]['id'];

interface ProviderForm {
  id?: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  defaultModel: string;
  runtimeNodeId: string;
  temperature: string;
  topP: string;
  maxTokens: string;
  enabled: boolean;
}

interface WorkspaceForm {
  id?: string;
  ownerId: string;
  runtimeNodeId: string;
  name: string;
  rootPath: string;
  quotaGb: string;
  maxFiles: string;
  maxFileMb: string;
  maxParallel: string;
  maxTaskSeconds: string;
  allowedTools: string[];
  allowedPrograms: string[];
  status: 'active' | 'suspended';
}

export function AiAdminPage() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<TabId>('overview');
  const [providerForm, setProviderForm] = useState<ProviderForm | null>(null);
  const [workspaceForm, setWorkspaceForm] = useState<WorkspaceForm | null>(null);
  const [providerDelete, setProviderDelete] = useState<string | null>(null);
  const [skillDelete, setSkillDelete] = useState<AiSkill | null>(null);
  const [selectedNode, setSelectedNode] = useState<string | null>(null);
  const [enrollment, setEnrollment] = useState<{ code: string; expiresAt: string } | null>(null);
  const [rotatedToken, setRotatedToken] = useState<string | null>(null);
  const [assignmentSkill, setAssignmentSkill] = useState<AiSkill | null>(null);
  const [logSearch, setLogSearch] = useState('');
  const [mcpForm, setMcpForm] = useState<{
    name: string;
    endpoint: string;
    runtimeNodeId: string;
    secret: string;
    allowPrivateNetwork: boolean;
    enabled: boolean;
  } | null>(null);
  const [mcpToolsFor, setMcpToolsFor] = useState<string | null>(null);
  const [mcpDelete, setMcpDelete] = useState<AiMcpConnection | null>(null);
  const [mcpAssignConnection, setMcpAssignConnection] = useState('');
  const [mcpAssignWorkspace, setMcpAssignWorkspace] = useState('');
  const session = useSession();
  const adminUserId = session?.user.id ?? '';
  const [toolEdit, setToolEdit] = useState<AiToolInfo | null>(null);
  const [quotaEdit, setQuotaEdit] = useState<Record<string, unknown> | null>(null);
  const [quotaUserId, setQuotaUserId] = useState<string>('');

  const overview = useQuery({
    queryKey: ['ai-admin-overview'],
    queryFn: aiApi.adminOverview,
    enabled: tab === 'overview',
    refetchInterval: 15_000,
  });
  const providers = useQuery({
    queryKey: ['ai-admin-providers'],
    queryFn: () => aiApi.adminProviders(),
    enabled: tab === 'providers' || tab === 'workspaces',
  });
  const nodes = useQuery({
    queryKey: ['ai-admin-nodes'],
    queryFn: aiApi.runtimeNodes,
    enabled: tab === 'runtime' || tab === 'providers' || tab === 'workspaces',
    refetchInterval: tab === 'runtime' ? 10_000 : false,
  });
  const nodeDetail = useQuery({
    queryKey: ['ai-admin-node', selectedNode],
    queryFn: () => aiApi.runtimeNode(selectedNode!),
    enabled: !!selectedNode,
    refetchInterval: 5000,
  });
  const enrollments = useQuery({
    queryKey: ['ai-admin-enrollments'],
    queryFn: aiApi.enrollmentCodes,
    enabled: tab === 'runtime',
  });
  const users = useQuery({
    queryKey: ['ai-admin-users'],
    queryFn: () => api.listUsers({ perPage: 100 }),
    enabled: tab === 'workspaces' || tab === 'quotas' || !!assignmentSkill,
  });
  const workspaces = useQuery({
    queryKey: ['ai-admin-workspaces'],
    queryFn: aiApi.allWorkspaces,
    enabled: tab === 'workspaces' || tab === 'skills' || tab === 'mcp',
  });
  const skills = useQuery({
    queryKey: ['ai-admin-skills'],
    queryFn: aiApi.allSkills,
    enabled: tab === 'skills',
  });
  const assignments = useQuery({
    queryKey: ['ai-admin-assignments', assignmentSkill?.id],
    queryFn: () => aiApi.skillAssignments(assignmentSkill!.id),
    enabled: !!assignmentSkill,
  });
  const roles = useQuery({
    queryKey: ['ai-admin-roles'],
    queryFn: api.listRoles,
    enabled: !!assignmentSkill,
  });
  const tools = useQuery({
    queryKey: ['ai-admin-tools'],
    queryFn: aiApi.toolPolicies,
    enabled: tab === 'tools' || tab === 'workspaces',
  });
  const programs = useQuery({
    queryKey: ['ai-admin-programs'],
    queryFn: aiApi.programs,
    enabled: tab === 'tools' || tab === 'workspaces',
  });
  const quotas = useQuery({
    queryKey: ['ai-admin-quotas'],
    queryFn: aiApi.quotas,
    enabled: tab === 'quotas',
  });
  const logs = useQuery({
    queryKey: ['ai-admin-logs', logSearch],
    queryFn: () => aiApi.adminLogs({ search: logSearch, perPage: 50 }),
    enabled: tab === 'logs',
    refetchInterval: 20_000,
  });
  const mcpConnections = useQuery({
    queryKey: ['ai-admin-mcp'],
    queryFn: aiApi.mcpConnections,
    enabled: tab === 'mcp',
    refetchInterval: tab === 'mcp' ? 15_000 : false,
  });
  const mcpTools = useQuery({
    queryKey: ['ai-admin-mcp-tools', mcpToolsFor],
    queryFn: () => aiApi.mcpTools(mcpToolsFor!),
    enabled: !!mcpToolsFor,
  });

  const refreshAll = async (): Promise<void> => {
    await queryClient.invalidateQueries({ queryKey: ['ai-admin'] });
    await queryClient.invalidateQueries({ queryKey: ['ai-admin-providers'] });
    await queryClient.invalidateQueries({ queryKey: ['ai-admin-nodes'] });
  };

  const saveProvider = useMutation({
    mutationFn: async (form: ProviderForm) => {
      const body: Record<string, unknown> = {
        name: form.name,
        baseUrl: form.baseUrl,
        defaultModel: form.defaultModel,
        runtimeNodeId: form.runtimeNodeId || null,
        enabled: form.enabled,
        generation: {
          temperature: form.temperature ? Number(form.temperature) : null,
          topP: form.topP ? Number(form.topP) : null,
          maxTokens: form.maxTokens ? Number(form.maxTokens) : null,
        },
      };
      if (form.apiKey.trim()) body.apiKey = form.apiKey.trim();
      return form.id ? aiApi.updateProvider(form.id, body) : aiApi.createProvider(body);
    },
    onSuccess: async () => {
      setProviderForm(null);
      await refreshAll();
      await queryClient.invalidateQueries({ queryKey: ['ai-providers'] });
      toast('Провайдер сохранён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const testProvider = useMutation({
    mutationFn: (id: string) => aiApi.testProvider(id),
    onSuccess: async (result) => {
      toast(`PolyStrata доступна, моделей: ${result.models.length}`, 'success');
      await refreshAll();
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const refreshModels = useMutation({
    mutationFn: (id: string) => aiApi.providerModels(id),
    onSuccess: async (result) => {
      toast(`Модели обновлены: ${result.models.length}`, 'success');
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-providers'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const disableProvider = useMutation({
    mutationFn: (id: string) => aiApi.deleteProvider(id),
    onSuccess: async () => {
      setProviderDelete(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-providers'] });
      await queryClient.invalidateQueries({ queryKey: ['ai-providers'] });
      toast('Провайдер отключён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const saveMcp = useMutation({
    mutationFn: async (form: NonNullable<typeof mcpForm>) => {
      const body: Record<string, unknown> = {
        name: form.name,
        endpoint: form.endpoint,
        runtimeNodeId: form.runtimeNodeId,
        allowPrivateNetwork: form.allowPrivateNetwork,
        enabled: form.enabled,
      };
      if (form.secret.trim()) body.secret = form.secret.trim();
      return aiApi.createMcpConnection(body);
    },
    onSuccess: async () => {
      setMcpForm(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-mcp'] });
      toast('MCP-соединение создано', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const discoverMcp = useMutation({
    mutationFn: (id: string) => aiApi.discoverMcp(id),
    onSuccess: async (result) => {
      toast(`MCP discovery: найдено tools — ${result.tools.length}`, 'success');
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-mcp'] });
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-mcp-tools'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const toggleMcp = useMutation({
    mutationFn: (connection: AiMcpConnection) =>
      aiApi.updateMcpConnection(connection.id, {
        name: connection.name,
        endpoint: connection.endpoint,
        runtimeNodeId: connection.runtimeNodeId,
        allowPrivateNetwork: connection.allowPrivateNetwork,
        enabled: !connection.enabled,
      }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-mcp'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const deleteMcp = useMutation({
    mutationFn: (id: string) => aiApi.deleteMcpConnection(id),
    onSuccess: async () => {
      setMcpDelete(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-mcp'] });
      toast('MCP-соединение удалено', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const assignMcpMut = useMutation({
    mutationFn: ({ id, body }: { id: string; body: Record<string, unknown> }) =>
      aiApi.assignMcp(id, body),
    onSuccess: () => toast('MCP-соединение назначено', 'success'),
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const refreshUsage = useMutation({
    mutationFn: (id: string) => aiApi.refreshWorkspaceUsage(id),
    onSuccess: async () => {
      toast('Usage обновлён по данным Runtime', 'success');
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-workspaces'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const runtimeAction = useMutation({
    mutationFn: async ({ id, action }: { id: string; action: string }) => {
      if (action === 'revoke') return aiApi.revokeRuntime(id);
      if (action === 'disable') return aiApi.disableRuntime(id);
      if (action === 'enable') return aiApi.enableRuntime(id);
      return aiApi.rotateRuntimeToken(id);
    },
    onSuccess: async (result, variables) => {
      if (variables.action === 'rotate') {
        const token = (result as { deviceToken?: string }).deviceToken;
        if (token) setRotatedToken(token);
      }
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-nodes'] });
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-node'] });
      toast('Runtime обновлён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const createEnrollment = useMutation({
    mutationFn: () => aiApi.createEnrollmentCode('System Management', 900),
    onSuccess: async (result) => {
      setEnrollment({ code: result.code, expiresAt: result.expiresAt });
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-enrollments'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const revokeEnrollment = useMutation({
    mutationFn: (id: string) => aiApi.revokeEnrollmentCode(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-enrollments'] });
      toast('Код отозван', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const saveWorkspace = useMutation({
    mutationFn: async (form: WorkspaceForm) => {
      const body = {
        ownerId: form.ownerId,
        runtimeNodeId: form.runtimeNodeId,
        name: form.name,
        rootPath: form.rootPath,
        quotaBytes: Math.round(Number(form.quotaGb || 1) * 1024 * 1024 * 1024),
        maxFiles: Number(form.maxFiles || 10000),
        maxFileBytes: Math.round(Number(form.maxFileMb || 16) * 1024 * 1024),
        maxParallelTasks: Number(form.maxParallel || 1),
        maxTaskSeconds: Number(form.maxTaskSeconds || 900),
        allowedTools: form.allowedTools,
        allowedPrograms: form.allowedPrograms,
        status: form.status,
      };
      return form.id ? aiApi.updateWorkspace(form.id, body) : aiApi.createWorkspace(body);
    },
    onSuccess: async () => {
      setWorkspaceForm(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-workspaces'] });
      await queryClient.invalidateQueries({ queryKey: ['ai-workspaces'] });
      toast('Workspace сохранён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const importSkill = useMutation({
    mutationFn: (file: File) => aiApi.importSkill(file),
    onSuccess: async (result) => {
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-skills'] });
      toast(
        `Skill «${result.skill.name}» ${result.created ? 'импортирован' : 'обновлён'} (v${result.version})`,
        'success',
      );
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const toggleSkill = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      enabled ? aiApi.enableSkill(id) : aiApi.disableSkill(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-skills'] });
      toast('Skill обновлён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const deleteSkill = useMutation({
    mutationFn: (id: string) => aiApi.deleteSkill(id),
    onSuccess: async () => {
      setSkillDelete(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-skills'] });
      toast('Skill удалён', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const assignSkill = useMutation({
    mutationFn: (body: Record<string, unknown>) => aiApi.assignSkill(assignmentSkill!.id, body),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ['ai-admin-assignments', assignmentSkill?.id],
      });
      toast('Назначение добавлено', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const removeAssignment = useMutation({
    mutationFn: (assignment: AiAssignment) =>
      aiApi.removeSkillAssignment(assignmentSkill!.id, assignment.id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ['ai-admin-assignments', assignmentSkill?.id],
      });
      toast('Назначение удалено', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const updateTool = useMutation({
    mutationFn: ({ tool, body }: { tool: AiToolInfo; body: Record<string, unknown> }) =>
      aiApi.updateTool(tool.id, body),
    onSuccess: async () => {
      setToolEdit(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-tools'] });
      toast('Политика инструмента обновлена', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const updateProgram = useMutation({
    mutationFn: ({ program, body }: { program: AiProgram; body: Record<string, unknown> }) =>
      aiApi.updateProgram(program.id, body),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-programs'] });
      toast('Программа обновлена', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const saveQuota = useMutation({
    mutationFn: ({ userId, body }: { userId: string; body: Record<string, unknown> }) =>
      aiApi.setQuota(userId, body),
    onSuccess: async () => {
      setQuotaEdit(null);
      await queryClient.invalidateQueries({ queryKey: ['ai-admin-quotas'] });
      toast('Квота сохранена', 'success');
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const nodeMap = useMemo(() => {
    const map = new Map<string, AiRuntimeNode>();
    for (const node of nodes.data?.items ?? []) map.set(node.id, node);
    return map;
  }, [nodes.data]);

  return (
    <>
      <PageHead
        eyebrow="Система · NEXUS AI"
        title="AI Management"
        description="Провайдеры, Runtime-узлы, рабочие пространства, Skills, инструменты, квоты и журналы. Все данные — из реального состояния сервера."
        actions={
          <>
            <Link to="/workspace/ai" className="btn btn--sm">
              <Bot aria-hidden="true" style={{ width: 14, height: 14, marginRight: 6 }} />
              AI Workspace
            </Link>
            <Button
              size="sm"
              variant="ghost"
              icon={<RefreshCw aria-hidden="true" />}
              onClick={() => void refreshAll()}
            >
              Обновить
            </Button>
          </>
        }
      />

      <div className="ai-admin-tabs" role="tablist" aria-label="AI Management">
        {TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            className={tab === item.id ? 'active' : ''}
            onClick={() => setTab(item.id)}
          >
            {item.icon}
            {item.label}
          </button>
        ))}
      </div>

      {tab === 'overview' ? (
        <OverviewTab overview={overview.data} loading={overview.isPending} error={overview.error} />
      ) : null}

      {tab === 'providers' ? (
        <div className="stack">
          <SectionTitle
            title="Model Providers"
            meta={`${providers.data?.items.length ?? 0} конфигураций`}
            actions={
              <Button
                size="sm"
                variant="primary"
                icon={<PlugZap aria-hidden="true" />}
                onClick={() =>
                  setProviderForm({
                    name: '',
                    baseUrl: 'http://127.0.0.1:8080/v1',
                    apiKey: '',
                    defaultModel: '',
                    runtimeNodeId: nodes.data?.items[0]?.id ?? '',
                    temperature: '',
                    topP: '',
                    maxTokens: '',
                    enabled: true,
                  })
                }
              >
                Добавить провайдера
              </Button>
            }
          />
          {providers.isPending ? <LoadingInline label="Провайдеры" /> : null}
          <div className="grid-2">
            {providers.data?.items.map((provider) => (
              <div className="card" key={provider.id}>
                <div className="row row--between">
                  <div>
                    <div className="ai-admin-title">{provider.name}</div>
                    <div className="tiny muted">{provider.baseUrl}</div>
                  </div>
                  <Badge
                    tone={
                      provider.status === 'ok'
                        ? 'ok'
                        : provider.status === 'error'
                          ? 'danger'
                          : 'warn'
                    }
                  >
                    {provider.status}
                  </Badge>
                </div>
                <div className="info-list" style={{ marginTop: 10 }}>
                  <Info label="Модель по умолчанию" value={provider.defaultModel || '—'} />
                  <Info
                    label="Runtime"
                    value={
                      provider.runtimeNodeId
                        ? (nodeMap.get(provider.runtimeNodeId)?.name ?? provider.runtimeNodeId)
                        : 'не назначен'
                    }
                  />
                  <Info
                    label="API key"
                    value={provider.hasApiKey ? 'сохранён (скрыт)' : 'не задан'}
                  />
                  <Info
                    label="Модели"
                    value={
                      provider.models && provider.models.length > 0
                        ? provider.models.join(', ')
                        : 'нет данных — выполните refresh'
                    }
                  />
                  <Info label="Последняя проверка" value={formatDate(provider.lastCheckedAt)} />
                  {provider.lastError ? <Info label="Ошибка" value={provider.lastError} /> : null}
                </div>
                <div className="row" style={{ marginTop: 12, flexWrap: 'wrap', gap: 8 }}>
                  <Button
                    size="sm"
                    icon={<PlugZap aria-hidden="true" />}
                    onClick={() => testProvider.mutate(provider.id)}
                    loading={testProvider.isPending}
                    disabled={!provider.runtimeNodeId}
                  >
                    Test connection
                  </Button>
                  <Button
                    size="sm"
                    icon={<RefreshCw aria-hidden="true" />}
                    onClick={() => refreshModels.mutate(provider.id)}
                    loading={refreshModels.isPending}
                    disabled={!provider.runtimeNodeId}
                  >
                    Модели
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<Bot aria-hidden="true" />}
                    onClick={() =>
                      setProviderForm({
                        id: provider.id,
                        name: provider.name,
                        baseUrl: provider.baseUrl ?? '',
                        apiKey: '',
                        defaultModel: provider.defaultModel,
                        runtimeNodeId: provider.runtimeNodeId ?? '',
                        temperature: String(
                          (provider.generation as Record<string, unknown> | undefined)
                            ?.temperature ?? '',
                        ),
                        topP: String(
                          (provider.generation as Record<string, unknown> | undefined)?.topP ?? '',
                        ),
                        maxTokens: String(
                          (provider.generation as Record<string, unknown> | undefined)?.maxTokens ??
                            '',
                        ),
                        enabled: provider.enabled ?? true,
                      })
                    }
                  >
                    Изменить
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    icon={<Trash2 aria-hidden="true" />}
                    onClick={() => setProviderDelete(provider.id)}
                  >
                    Отключить
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {tab === 'runtime' ? (
        <div className="stack">
          <SectionTitle
            title="Runtime Nodes"
            meta={`${nodes.data?.items.length ?? 0} устройств`}
            actions={
              <Button
                size="sm"
                variant="primary"
                icon={<KeyRound aria-hidden="true" />}
                onClick={() => createEnrollment.mutate()}
                loading={createEnrollment.isPending}
              >
                Новый enrollment code
              </Button>
            }
          />
          {nodes.isPending ? <LoadingInline label="Runtime" /> : null}
          {nodes.data?.items.length === 0 ? (
            <div className="card">
              <EmptyState icon={<Server aria-hidden="true" />} title="Runtime не зарегистрирован">
                Создайте одноразовый код, выполните на втором компьютере
                <code> nexus-ai-runtime enroll --code …</code> и запустите службу.
              </EmptyState>
            </div>
          ) : null}
          <div className="grid-2">
            {nodes.data?.items.map((node) => (
              <div className="card" key={node.id}>
                <div className="row row--between">
                  <div>
                    <div className="ai-admin-title">{node.name}</div>
                    <div className="tiny muted">
                      {node.platform} · Runtime {node.runtimeVersion || '—'} · protocol{' '}
                      {node.protocolVersion ?? '—'}
                    </div>
                  </div>
                  <Badge
                    tone={
                      node.status === 'online'
                        ? 'ok'
                        : node.status === 'degraded'
                          ? 'warn'
                          : node.status === 'revoked' || node.status === 'disabled'
                            ? 'danger'
                            : 'default'
                    }
                  >
                    {node.status}
                  </Badge>
                </div>
                <div className="info-list" style={{ marginTop: 10 }}>
                  <Info label="Last seen" value={formatDate(node.lastSeenAt)} />
                  <Info
                    label="PolyStrata"
                    value={
                      node.capabilities?.polystrata?.reachable
                        ? `доступна · моделей ${node.capabilities.polystrata.modelCount ?? 0}`
                        : 'недоступна / нет данных'
                    }
                  />
                  <Info
                    label="Модели"
                    value={(node.capabilities?.models ?? []).join(', ') || '—'}
                  />
                  <Info label="Активные задачи" value={formatNumber(node.activeTasks ?? 0)} />
                </div>
                <div className="row" style={{ marginTop: 12, flexWrap: 'wrap', gap: 8 }}>
                  <Button size="sm" onClick={() => setSelectedNode(node.id)}>
                    Детали
                  </Button>
                  {node.status === 'disabled' ? (
                    <Button
                      size="sm"
                      onClick={() => runtimeAction.mutate({ id: node.id, action: 'enable' })}
                    >
                      Включить
                    </Button>
                  ) : (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => runtimeAction.mutate({ id: node.id, action: 'disable' })}
                    >
                      Отключить
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<KeyRound aria-hidden="true" />}
                    onClick={() => runtimeAction.mutate({ id: node.id, action: 'rotate' })}
                  >
                    Rotate token
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    onClick={() => runtimeAction.mutate({ id: node.id, action: 'revoke' })}
                  >
                    Revoke
                  </Button>
                </div>
              </div>
            ))}
          </div>

          <SectionTitle title="Enrollment codes" meta="одноразовые, с TTL" />
          <div className="card">
            {enrollments.isPending ? <LoadingInline label="Коды" /> : null}
            {enrollments.data?.items.length === 0 ? (
              <div className="tiny muted">Кодов пока нет.</div>
            ) : null}
            <div className="ai-admin-table">
              {enrollments.data?.items.map((code) => (
                <div key={code.id} className="ai-admin-row">
                  <span className="grow truncate">{code.label || '—'}</span>
                  <span className="tiny muted">до {formatDate(code.expiresAt)}</span>
                  <Badge tone={code.revokedAt ? 'danger' : code.usedAt ? 'default' : 'info'}>
                    {code.revokedAt ? 'revoked' : code.usedAt ? 'used' : 'active'}
                  </Badge>
                  {!code.usedAt && !code.revokedAt ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => revokeEnrollment.mutate(code.id)}
                    >
                      Отозвать
                    </Button>
                  ) : null}
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : null}

      {tab === 'workspaces' ? (
        <div className="stack">
          <SectionTitle
            title="User Workspaces"
            meta={`${workspaces.data?.items.length ?? 0} пространств`}
            actions={
              <Button
                size="sm"
                variant="primary"
                icon={<Cpu aria-hidden="true" />}
                onClick={() =>
                  setWorkspaceForm({
                    ownerId: users.data?.items[0]?.id ?? '',
                    runtimeNodeId: nodes.data?.items[0]?.id ?? '',
                    name: '',
                    rootPath: '',
                    quotaGb: '1',
                    maxFiles: '10000',
                    maxFileMb: '16',
                    maxParallel: '1',
                    maxTaskSeconds: '900',
                    allowedTools: (tools.data?.items ?? [])
                      .filter((tool) => !tool.serverSide)
                      .map((tool) => tool.id),
                    allowedPrograms: [],
                    status: 'active',
                  })
                }
              >
                Создать workspace
              </Button>
            }
          />
          {workspaces.isPending ? <LoadingInline label="Workspaces" /> : null}
          {workspaces.data?.items.map((workspace) => (
            <div className="card" key={workspace.id}>
              <div className="row row--between">
                <div>
                  <div className="ai-admin-title">{workspace.name}</div>
                  <div className="tiny muted">
                    owner{' '}
                    {users.data?.items.find((user) => user.id === workspace.ownerId)?.username ??
                      workspace.ownerId}
                    {' · '}
                    runtime {nodeMap.get(workspace.runtimeNodeId)?.name ?? workspace.runtimeNodeId}
                  </div>
                </div>
                <Badge tone={workspace.status === 'active' ? 'ok' : 'warn'}>
                  {workspace.status}
                </Badge>
              </div>
              <div className="info-list" style={{ marginTop: 10 }}>
                <Info label="Root (admin only)" value={workspace.rootPath || 'скрыт'} />
                <Info
                  label="Quota"
                  value={`${formatBytes(workspace.usageBytes)} / ${formatBytes(workspace.quotaBytes)}`}
                />
                <Info label="Files" value={`${workspace.usageFiles} / ${workspace.maxFiles}`} />
                <Info
                  label="Limits"
                  value={`parallel ${workspace.maxParallelTasks}, task ${workspace.maxTaskSeconds}s, file ${formatBytes(workspace.maxFileBytes)}`}
                />
                <Info label="Tools" value={workspace.allowedTools.join(', ') || '—'} />
                <Info label="Programs" value={workspace.allowedPrograms.join(', ') || '—'} />
                <Info
                  label="Usage checked"
                  value={
                    workspace.usageCheckedAt
                      ? formatDate(workspace.usageCheckedAt)
                      : 'нет данных от Runtime'
                  }
                />
              </div>
              <div className="row" style={{ marginTop: 12 }}>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<RefreshCw aria-hidden="true" />}
                  disabled={refreshUsage.isPending}
                  onClick={() => refreshUsage.mutate(workspace.id)}
                >
                  Обновить usage
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    setWorkspaceForm({
                      id: workspace.id,
                      ownerId: workspace.ownerId,
                      runtimeNodeId: workspace.runtimeNodeId,
                      name: workspace.name,
                      rootPath: workspace.rootPath ?? '',
                      quotaGb: String(
                        Math.max(1, Math.round(workspace.quotaBytes / 1024 / 1024 / 1024)),
                      ),
                      maxFiles: String(workspace.maxFiles),
                      maxFileMb: String(Math.round(workspace.maxFileBytes / 1024 / 1024)),
                      maxParallel: String(workspace.maxParallelTasks),
                      maxTaskSeconds: String(workspace.maxTaskSeconds),
                      allowedTools: workspace.allowedTools,
                      allowedPrograms: workspace.allowedPrograms,
                      status: workspace.status === 'suspended' ? 'suspended' : 'active',
                    })
                  }
                >
                  Изменить
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    aiApi
                      .updateWorkspace(workspace.id, {
                        ...workspace,
                        status: workspace.status === 'active' ? 'suspended' : 'active',
                      })
                      .then(async () => {
                        await queryClient.invalidateQueries({ queryKey: ['ai-admin-workspaces'] });
                        toast('Статус workspace изменён', 'success');
                      })
                      .catch((error) => toast(errorMessage(error), 'error'))
                  }
                >
                  {workspace.status === 'active' ? 'Приостановить' : 'Возобновить'}
                </Button>
              </div>
            </div>
          ))}
        </div>
      ) : null}

      {tab === 'skills' ? (
        <div className="stack">
          <SectionTitle
            title="Skills Manager"
            meta={`${skills.data?.items.length ?? 0} навыков`}
            actions={
              <label className="btn btn--sm btn--primary">
                <Upload aria-hidden="true" style={{ width: 14, height: 14, marginRight: 6 }} />
                Импорт ZIP
                <input
                  type="file"
                  accept=".zip"
                  hidden
                  onChange={(event) => {
                    const file = event.target.files?.[0];
                    if (file) importSkill.mutate(file);
                    event.target.value = '';
                  }}
                />
              </label>
            }
          />
          <Alert tone="info">
            Skill — недоверенные инструкции. Скрипты не исполняются автоматически, содержимое
            загружается агентом по требованию через <code>skill_load</code> только после назначения.
          </Alert>
          {skills.isPending ? <LoadingInline label="Skills" /> : null}
          <div className="grid-2">
            {skills.data?.items.map((skill) => (
              <div className="card" key={skill.id}>
                <div className="row row--between">
                  <div>
                    <div className="ai-admin-title">{skill.name}</div>
                    <div className="tiny muted">
                      {skill.slug} · v{skill.version} · файлов {skill.fileCount ?? 0}
                    </div>
                  </div>
                  <Badge tone={skill.enabled ? 'ok' : 'default'}>
                    {skill.enabled ? 'enabled' : 'disabled'}
                  </Badge>
                </div>
                <p className="tiny muted" style={{ margin: '8px 0' }}>
                  {skill.description || 'Без описания'}
                </p>
                <div className="tiny muted">
                  checksum: {skill.checksum?.slice(0, 16) ?? '—'} ·{' '}
                  {formatBytes(skill.sizeBytes ?? 0)}
                </div>
                <div className="row" style={{ marginTop: 12, flexWrap: 'wrap', gap: 8 }}>
                  <Button size="sm" variant="ghost" onClick={() => setAssignmentSkill(skill)}>
                    Назначения
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => toggleSkill.mutate({ id: skill.id, enabled: !skill.enabled })}
                  >
                    {skill.enabled ? 'Отключить' : 'Включить'}
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    icon={<Trash2 aria-hidden="true" />}
                    onClick={() => setSkillDelete(skill)}
                  >
                    Удалить
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {tab === 'tools' ? (
        <div className="stack">
          <SectionTitle title="Tool Registry" meta="deny-by-default" />
          <div className="card">
            {tools.isPending ? <LoadingInline label="Tools" /> : null}
            <div className="ai-admin-table">
              {tools.data?.items.map((tool) => (
                <div key={tool.id} className="ai-admin-row">
                  <span className="grow">
                    <strong>{tool.id}</strong>
                    <span className="tiny muted"> · {tool.category}</span>
                  </span>
                  <Badge tone={tool.enabled ? 'ok' : 'default'}>
                    {tool.enabled ? 'on' : 'off'}
                  </Badge>
                  <Badge
                    tone={tool.risk === 'high' || tool.risk === 'critical' ? 'danger' : 'default'}
                  >
                    {tool.risk}
                  </Badge>
                  <span className="tiny muted">approval: {tool.approvalPolicy ?? '—'}</span>
                  <Button size="sm" variant="ghost" onClick={() => setToolEdit(tool)}>
                    Политика
                  </Button>
                </div>
              ))}
            </div>
          </div>

          <SectionTitle title="Registered Programs" meta="снимок возможностей Runtime" />
          {programs.isPending ? <LoadingInline label="Programs" /> : null}
          {programs.data?.items.length === 0 ? (
            <Alert tone="info">
              Программы не зарегистрированы. Runtime разрешает запуск только программ из локального
              конфигурационного файла; после перезапуска они появятся здесь.
            </Alert>
          ) : null}
          <div className="ai-admin-table card">
            {programs.data?.items.map((program) => (
              <div key={program.id} className="ai-admin-row">
                <span className="grow">
                  <strong>{program.programId}</strong>
                  <span className="tiny muted"> · {program.executable}</span>
                </span>
                <Badge tone={program.available ? 'ok' : 'danger'}>
                  {program.available ? 'available' : 'missing'}
                </Badge>
                <Badge>{program.risk}</Badge>
                <Switch
                  checked={program.enabled}
                  onChange={(value) => updateProgram.mutate({ program, body: { enabled: value } })}
                  label={program.programId}
                />
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {tab === 'mcp' ? (
        <div className="stack">
          <SectionTitle title="MCP Connections" meta="выключены по умолчанию · SSRF-проверка" />
          <Alert tone="info">
            MCP-сервер должен быть явно зарегистрирован и назначен. Loopback/private адреса
            требуют отдельного разрешения. Найденные tools проходят те же approval-правила, что и
            остальные инструменты.
          </Alert>
          <div className="card stack">
            <div className="row wrap">
              <Field label="Имя">
                <input
                  value={mcpForm?.name ?? ''}
                  onChange={(event) =>
                    setMcpForm({
                      name: event.target.value,
                      endpoint: mcpForm?.endpoint ?? '',
                      runtimeNodeId: mcpForm?.runtimeNodeId ?? '',
                      secret: mcpForm?.secret ?? '',
                      allowPrivateNetwork: mcpForm?.allowPrivateNetwork ?? false,
                      enabled: mcpForm?.enabled ?? false,
                    })
                  }
                  placeholder="local-tools"
                />
              </Field>
              <Field label="Endpoint">
                <input
                  value={mcpForm?.endpoint ?? ''}
                  onChange={(event) =>
                    setMcpForm({
                      name: mcpForm?.name ?? '',
                      endpoint: event.target.value,
                      runtimeNodeId: mcpForm?.runtimeNodeId ?? '',
                      secret: mcpForm?.secret ?? '',
                      allowPrivateNetwork: mcpForm?.allowPrivateNetwork ?? false,
                      enabled: mcpForm?.enabled ?? false,
                    })
                  }
                  placeholder="https://host/mcp"
                />
              </Field>
              <Field label="Runtime">
                <select
                  value={mcpForm?.runtimeNodeId ?? ''}
                  onChange={(event) =>
                    setMcpForm({
                      name: mcpForm?.name ?? '',
                      endpoint: mcpForm?.endpoint ?? '',
                      runtimeNodeId: event.target.value,
                      secret: mcpForm?.secret ?? '',
                      allowPrivateNetwork: mcpForm?.allowPrivateNetwork ?? false,
                      enabled: mcpForm?.enabled ?? false,
                    })
                  }
                >
                  <option value="">—</option>
                  {nodes.data?.items.map((node) => (
                    <option key={node.id} value={node.id}>
                      {node.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Bearer secret (необязательно)">
                <input
                  type="password"
                  value={mcpForm?.secret ?? ''}
                  onChange={(event) =>
                    setMcpForm({
                      name: mcpForm?.name ?? '',
                      endpoint: mcpForm?.endpoint ?? '',
                      runtimeNodeId: mcpForm?.runtimeNodeId ?? '',
                      secret: event.target.value,
                      allowPrivateNetwork: mcpForm?.allowPrivateNetwork ?? false,
                      enabled: mcpForm?.enabled ?? false,
                    })
                  }
                />
              </Field>
            </div>
            <div className="row wrap">
              <Switch
                checked={mcpForm?.allowPrivateNetwork ?? false}
                onChange={(value) =>
                  setMcpForm({
                    name: mcpForm?.name ?? '',
                    endpoint: mcpForm?.endpoint ?? '',
                    runtimeNodeId: mcpForm?.runtimeNodeId ?? '',
                    secret: mcpForm?.secret ?? '',
                    allowPrivateNetwork: value,
                    enabled: mcpForm?.enabled ?? false,
                  })
                }
                label="Разрешить private/loopback"
              />
              <Switch
                checked={mcpForm?.enabled ?? false}
                onChange={(value) =>
                  setMcpForm({
                    name: mcpForm?.name ?? '',
                    endpoint: mcpForm?.endpoint ?? '',
                    runtimeNodeId: mcpForm?.runtimeNodeId ?? '',
                    secret: mcpForm?.secret ?? '',
                    allowPrivateNetwork: mcpForm?.allowPrivateNetwork ?? false,
                    enabled: value,
                  })
                }
                label="Включено"
              />
              <Button
                icon={<Network aria-hidden="true" />}
                disabled={
                  !mcpForm?.name.trim() ||
                  !mcpForm?.endpoint.trim() ||
                  !mcpForm?.runtimeNodeId ||
                  saveMcp.isPending
                }
                onClick={() => mcpForm && saveMcp.mutate(mcpForm)}
              >
                Добавить
              </Button>
            </div>
          </div>
          {mcpConnections.isPending ? <LoadingInline label="MCP" /> : null}
          {mcpConnections.data?.items.length === 0 ? (
            <EmptyState title="MCP-соединений нет">
              Добавьте сервер и выполните discovery.
            </EmptyState>
          ) : null}
          <div className="ai-admin-table card">
            {mcpConnections.data?.items.map((connection) => (
              <div key={connection.id} className="ai-admin-row">
                <span className="grow">
                  <strong>{connection.name}</strong>
                  <span className="tiny muted"> · {connection.endpoint}</span>
                  <span className="tiny muted"> · tools: {connection.discoveredCount}</span>
                </span>
                <Badge tone={connection.enabled ? 'ok' : 'default'}>
                  {connection.enabled ? 'enabled' : 'disabled'}
                </Badge>
                <Badge tone={connection.status === 'error' ? 'danger' : 'default'}>
                  {connection.status}
                </Badge>
                {connection.lastError ? (
                  <span className="tiny muted" title={connection.lastError}>
                    {connection.lastError}
                  </span>
                ) : null}
                <Switch
                  checked={connection.enabled}
                  onChange={() => toggleMcp.mutate(connection)}
                  label={connection.name}
                />
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<RefreshCw aria-hidden="true" />}
                  disabled={!connection.enabled || discoverMcp.isPending}
                  onClick={() => discoverMcp.mutate(connection.id)}
                >
                  Discovery
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={!adminUserId}
                  onClick={() =>
                    assignMcpMut.mutate({
                      id: connection.id,
                      body: { scope: 'user', userId: adminUserId },
                    })
                  }
                >
                  Назначить мне
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setMcpToolsFor(connection.id)}>
                  Tools
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  icon={<Trash2 aria-hidden="true" />}
                  onClick={() => setMcpDelete(connection)}
                >
                  Удалить
                </Button>
              </div>
            ))}
          </div>
          <div className="card stack">
            <SectionTitle title="Назначение MCP" meta="user / workspace scope" />
            <div className="row wrap">
              <Field label="Соединение">
                <select
                  value={mcpAssignConnection}
                  onChange={(event) => setMcpAssignConnection(event.target.value)}
                >
                  <option value="">—</option>
                  {mcpConnections.data?.items.map((connection) => (
                    <option key={connection.id} value={connection.id}>
                      {connection.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Workspace">
                <select
                  value={mcpAssignWorkspace}
                  onChange={(event) => setMcpAssignWorkspace(event.target.value)}
                >
                  <option value="">—</option>
                  {workspaces.data?.items.map((workspace) => (
                    <option key={workspace.id} value={workspace.id}>
                      {workspace.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Button
                disabled={!mcpAssignConnection || !mcpAssignWorkspace}
                onClick={() =>
                  assignMcpMut.mutate({
                    id: mcpAssignConnection,
                    body: { scope: 'workspace', workspaceId: mcpAssignWorkspace },
                  })
                }
              >
                Назначить workspace
              </Button>
            </div>
            <Alert tone="info">
              Для вызова MCP-tool также добавьте его tool_id в allowedTools workspace.
            </Alert>
          </div>
          {mcpToolsFor ? (
            <div className="card stack">
              <SectionTitle title="Discovered MCP tools" meta={mcpToolsFor} />
              {mcpTools.isPending ? <LoadingInline label="MCP tools" /> : null}
              <div className="ai-admin-table">
                {mcpTools.data?.items.map((tool) => (
                  <div key={tool.id} className="ai-admin-row">
                    <span className="grow">
                      <strong>{tool.name}</strong>
                      <span className="tiny muted"> · {tool.toolId}</span>
                    </span>
                    <Badge>{tool.risk}</Badge>
                    <Badge tone={tool.enabled ? 'ok' : 'default'}>
                      {tool.enabled ? 'on' : 'off'}
                    </Badge>
                  </div>
                ))}
              </div>
              {mcpTools.data?.items.length === 0 ? (
                <Alert tone="info">Tools не обнаружены. Выполните discovery после включения.</Alert>
              ) : null}
            </div>
          ) : null}
          <ConfirmDialog
            open={!!mcpDelete}
            title="Удалить MCP-соединение?"
            description={`Соединение «${mcpDelete?.name ?? ''}» и его tool snapshots будут удалены.`}
            confirmLabel="Удалить"
            onCancel={() => setMcpDelete(null)}
            onConfirm={() => mcpDelete && deleteMcp.mutate(mcpDelete.id)}
          />
        </div>
      ) : null}

      {tab === 'quotas' ? (
        <div className="stack">
          <SectionTitle title="AI User Quotas" meta="лимиты задач и ресурсов" />
          {quotas.isPending ? <LoadingInline label="Quotas" /> : null}
          <div className="ai-admin-table card">
            {quotas.data?.items.map((quota) => (
              <div key={quota.userId} className="ai-admin-row">
                <span className="grow">
                  <strong>{quota.displayName || quota.username}</strong>
                  <span className="tiny muted"> · @{quota.username}</span>
                </span>
                <span className="tiny muted">
                  parallel {quota.maxConcurrentTasks} · day {quota.maxTasksPerDay} · tools{' '}
                  {quota.maxToolCallsPerTask} · task {quota.maxTaskSeconds}s
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setQuotaUserId(quota.userId);
                    setQuotaEdit({
                      maxConcurrentTasks: quota.maxConcurrentTasks,
                      maxTasksPerDay: quota.maxTasksPerDay,
                      maxToolCallsPerTask: quota.maxToolCallsPerTask,
                      maxTaskSeconds: quota.maxTaskSeconds,
                      maxStorageBytes: quota.maxStorageBytes,
                      maxFiles: quota.maxFiles,
                      maxFileBytes: quota.maxFileBytes,
                    });
                  }}
                >
                  Изменить
                </Button>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {tab === 'logs' ? (
        <div className="stack">
          <SectionTitle title="AI Security & Logs" meta="аудит и журнал Runtime" />
          <input
            className="input"
            placeholder="Поиск по типу события, target или сообщению"
            value={logSearch}
            onChange={(event) => setLogSearch(event.target.value)}
          />
          {logs.isPending ? <LoadingInline label="Журналы" /> : null}
          <div className="grid-2">
            <div className="card">
              <SectionTitle title="AI audit" meta={`${logs.data?.audit.total ?? 0}`} />
              <div className="ai-admin-table">
                {logs.data?.audit.items.map((row) => (
                  <div key={String(row.id)} className="ai-admin-row">
                    <Badge tone={row.result === 'success' ? 'ok' : 'danger'}>
                      {String(row.eventType)}
                    </Badge>
                    <span className="grow truncate">{String(row.actorUsername ?? 'system')}</span>
                    <span className="tiny muted">{formatDate(String(row.createdAt))}</span>
                  </div>
                ))}
              </div>
            </div>
            <div className="card">
              <SectionTitle title="Runtime log" meta={`${logs.data?.runtime.total ?? 0}`} />
              <div className="ai-admin-table">
                {logs.data?.runtime.items.map((row) => (
                  <div key={String(row.id)} className="ai-admin-row">
                    <Badge tone={row.kind === 'error' ? 'danger' : 'default'}>
                      {String(row.kind)}
                    </Badge>
                    <span className="grow truncate">{String(row.message)}</span>
                    <span className="tiny muted">{formatDate(String(row.createdAt))}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      ) : null}

      <Modal
        open={!!providerForm}
        onClose={() => setProviderForm(null)}
        title={providerForm?.id ? 'Изменить провайдера' : 'Новый провайдер'}
        wide
        footer={
          <>
            <Button variant="ghost" onClick={() => setProviderForm(null)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              loading={saveProvider.isPending}
              disabled={!providerForm?.name.trim() || !providerForm?.baseUrl.trim()}
              onClick={() => providerForm && saveProvider.mutate(providerForm)}
            >
              Сохранить
            </Button>
          </>
        }
      >
        {providerForm ? (
          <div className="form-grid">
            <Field label="Имя">
              <input
                className="input"
                value={providerForm.name}
                onChange={(event) => setProviderForm({ ...providerForm, name: event.target.value })}
              />
            </Field>
            <Field
              label="Base URL"
              hint="PolyStrata на компьютере Runtime, обычно http://127.0.0.1:8080/v1"
            >
              <input
                className="input"
                value={providerForm.baseUrl}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, baseUrl: event.target.value })
                }
              />
            </Field>
            <Field
              label="API key"
              hint="Необязательно для локальной PolyStrata. Ключ не возвращается после сохранения."
            >
              <input
                className="input"
                type="password"
                autoComplete="off"
                value={providerForm.apiKey}
                placeholder={providerForm.id ? 'оставить неизменным' : ''}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, apiKey: event.target.value })
                }
              />
            </Field>
            <Field label="Модель по умолчанию">
              <input
                className="input"
                value={providerForm.defaultModel}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, defaultModel: event.target.value })
                }
              />
            </Field>
            <Field label="Runtime">
              <select
                className="input"
                value={providerForm.runtimeNodeId}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, runtimeNodeId: event.target.value })
                }
              >
                <option value="">Не назначен</option>
                {nodes.data?.items.map((node) => (
                  <option key={node.id} value={node.id}>
                    {node.name} ({node.status})
                  </option>
                ))}
              </select>
            </Field>
            <Field label="temperature" hint="Только если модель поддерживает параметр">
              <input
                className="input"
                type="number"
                step="0.1"
                min="0"
                max="2"
                value={providerForm.temperature}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, temperature: event.target.value })
                }
              />
            </Field>
            <Field label="top_p">
              <input
                className="input"
                type="number"
                step="0.05"
                min="0"
                max="1"
                value={providerForm.topP}
                onChange={(event) => setProviderForm({ ...providerForm, topP: event.target.value })}
              />
            </Field>
            <Field label="max_tokens">
              <input
                className="input"
                type="number"
                min="1"
                value={providerForm.maxTokens}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, maxTokens: event.target.value })
                }
              />
            </Field>
            <Field label="Включён">
              <Switch
                checked={providerForm.enabled}
                onChange={(value) => setProviderForm({ ...providerForm, enabled: value })}
                label="Провайдер включён"
              />
            </Field>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={!!workspaceForm}
        onClose={() => setWorkspaceForm(null)}
        title={workspaceForm?.id ? 'Изменить workspace' : 'Новый workspace'}
        wide
        footer={
          <>
            <Button variant="ghost" onClick={() => setWorkspaceForm(null)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              loading={saveWorkspace.isPending}
              disabled={
                !workspaceForm?.ownerId ||
                !workspaceForm?.runtimeNodeId ||
                !workspaceForm?.name.trim() ||
                !workspaceForm?.rootPath.trim()
              }
              onClick={() => workspaceForm && saveWorkspace.mutate(workspaceForm)}
            >
              Сохранить
            </Button>
          </>
        }
      >
        {workspaceForm ? (
          <div className="form-grid">
            <Field label="Пользователь">
              <select
                className="input"
                value={workspaceForm.ownerId}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, ownerId: event.target.value })
                }
              >
                <option value="">Выберите пользователя</option>
                {users.data?.items.map((user) => (
                  <option key={user.id} value={user.id}>
                    {user.displayName || user.username} (@{user.username})
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Runtime">
              <select
                className="input"
                value={workspaceForm.runtimeNodeId}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, runtimeNodeId: event.target.value })
                }
              >
                <option value="">Выберите Runtime</option>
                {nodes.data?.items.map((node) => (
                  <option key={node.id} value={node.id}>
                    {node.name} ({node.status})
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Имя">
              <input
                className="input"
                value={workspaceForm.name}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, name: event.target.value })
                }
              />
            </Field>
            <Field
              label="Корневой каталог на Runtime"
              hint="Абсолютный путь, виден только администратору. Runtime проверит его по allowed_roots."
            >
              <input
                className="input"
                value={workspaceForm.rootPath}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, rootPath: event.target.value })
                }
              />
            </Field>
            <Field label="Quota, ГБ">
              <input
                className="input"
                type="number"
                min="0"
                value={workspaceForm.quotaGb}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, quotaGb: event.target.value })
                }
              />
            </Field>
            <Field label="Максимум файлов">
              <input
                className="input"
                type="number"
                min="0"
                value={workspaceForm.maxFiles}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, maxFiles: event.target.value })
                }
              />
            </Field>
            <Field label="Максимальный файл, МБ">
              <input
                className="input"
                type="number"
                min="0"
                value={workspaceForm.maxFileMb}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, maxFileMb: event.target.value })
                }
              />
            </Field>
            <Field label="Параллельных задач">
              <input
                className="input"
                type="number"
                min="0"
                value={workspaceForm.maxParallel}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, maxParallel: event.target.value })
                }
              />
            </Field>
            <Field label="Таймаут задачи, секунд">
              <input
                className="input"
                type="number"
                min="30"
                max="3600"
                value={workspaceForm.maxTaskSeconds}
                onChange={(event) =>
                  setWorkspaceForm({ ...workspaceForm, maxTaskSeconds: event.target.value })
                }
              />
            </Field>
            <Field label="Статус">
              <select
                className="input"
                value={workspaceForm.status}
                onChange={(event) =>
                  setWorkspaceForm({
                    ...workspaceForm,
                    status: event.target.value as 'active' | 'suspended',
                  })
                }
              >
                <option value="active">active</option>
                <option value="suspended">suspended</option>
              </select>
            </Field>
            <Field label="Разрешённые инструменты" full>
              <div className="ai-checkbox-grid">
                {(tools.data?.items ?? []).map((tool) => (
                  <label key={tool.id} className="ai-checkbox">
                    <input
                      type="checkbox"
                      checked={workspaceForm.allowedTools.includes(tool.id)}
                      onChange={(event) =>
                        setWorkspaceForm({
                          ...workspaceForm,
                          allowedTools: event.target.checked
                            ? [...workspaceForm.allowedTools, tool.id]
                            : workspaceForm.allowedTools.filter((id) => id !== tool.id),
                        })
                      }
                    />
                    <span>
                      {tool.id} <span className="tiny muted">({tool.risk})</span>
                    </span>
                  </label>
                ))}
              </div>
            </Field>
            <Field label="Разрешённые программы" full>
              {(programs.data?.items ?? []).length === 0 ? (
                <div className="tiny muted">Программы не зарегистрированы на Runtime.</div>
              ) : (
                <div className="ai-checkbox-grid">
                  {(programs.data?.items ?? [])
                    .filter((program) => program.runtimeNodeId === workspaceForm.runtimeNodeId)
                    .map((program) => (
                      <label key={program.id} className="ai-checkbox">
                        <input
                          type="checkbox"
                          checked={workspaceForm.allowedPrograms.includes(program.programId)}
                          onChange={(event) =>
                            setWorkspaceForm({
                              ...workspaceForm,
                              allowedPrograms: event.target.checked
                                ? [...workspaceForm.allowedPrograms, program.programId]
                                : workspaceForm.allowedPrograms.filter(
                                    (id) => id !== program.programId,
                                  ),
                            })
                          }
                        />
                        <span>
                          {program.programId}{' '}
                          <span className="tiny muted">
                            ({program.available ? 'available' : 'missing'})
                          </span>
                        </span>
                      </label>
                    ))}
                </div>
              )}
            </Field>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={!!selectedNode}
        onClose={() => setSelectedNode(null)}
        title="Runtime diagnostics"
        wide
      >
        {nodeDetail.isPending ? <LoadingInline label="Runtime" /> : null}
        {nodeDetail.data ? (
          <div className="stack">
            <pre className="ai-json">{JSON.stringify(nodeDetail.data['node'], null, 2)}</pre>
            <SectionTitle
              title="Programs"
              meta={`${(nodeDetail.data['programs'] as unknown[] | undefined)?.length ?? 0}`}
            />
            <pre className="ai-json">{JSON.stringify(nodeDetail.data['programs'], null, 2)}</pre>
            <SectionTitle
              title="Runtime events"
              meta={`pending commands: ${String(nodeDetail.data['pendingCommands'] ?? 0)}`}
            />
            <pre className="ai-json">{JSON.stringify(nodeDetail.data['events'], null, 2)}</pre>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={!!enrollment}
        onClose={() => setEnrollment(null)}
        title="Одноразовый enrollment code"
        footer={
          <Button
            variant="primary"
            onClick={() => {
              if (enrollment) void navigator.clipboard.writeText(enrollment.code);
              toast('Код скопирован', 'success');
            }}
          >
            <Check aria-hidden="true" style={{ width: 14, height: 14, marginRight: 6 }} />
            Скопировать
          </Button>
        }
      >
        {enrollment ? (
          <div className="stack">
            <Alert tone="warn">
              Код показывается один раз и действует до {formatDate(enrollment.expiresAt)}. Передайте
              его на компьютер Runtime.
            </Alert>
            <code className="ai-secret">{enrollment.code}</code>
            <div className="tiny muted">
              Команда: <code>nexus-ai-runtime enroll --config runtime.toml --code …</code>
            </div>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={!!rotatedToken}
        onClose={() => setRotatedToken(null)}
        title="Новый device token"
        footer={
          <Button
            variant="primary"
            onClick={() => {
              if (rotatedToken) void navigator.clipboard.writeText(rotatedToken);
              toast('Токен скопирован', 'success');
            }}
          >
            Скопировать
          </Button>
        }
      >
        <div className="stack">
          <Alert tone="warn">
            Старый токен отозван, соединение Runtime закрыто. Токен показывается один раз.
          </Alert>
          <code className="ai-secret">{rotatedToken}</code>
          <div className="tiny muted">
            На Runtime выполните: <code>nexus-ai-runtime set-token --node-id … --token …</code>
          </div>
        </div>
      </Modal>

      <Modal
        open={!!assignmentSkill}
        onClose={() => setAssignmentSkill(null)}
        title={`Назначения Skill «${assignmentSkill?.name ?? ''}»`}
        wide
      >
        <AssignmentsEditor
          skill={assignmentSkill}
          assignments={assignments.data?.items ?? []}
          users={users.data?.items ?? []}
          workspaces={workspaces.data?.items ?? []}
          roles={roles.data ?? []}
          busy={assignSkill.isPending || removeAssignment.isPending}
          onAssign={(body) => assignSkill.mutate(body)}
          onRemove={(assignment) => removeAssignment.mutate(assignment)}
        />
      </Modal>

      <Modal
        open={!!toolEdit}
        onClose={() => setToolEdit(null)}
        title={`Политика ${toolEdit?.id ?? ''}`}
        footer={
          <>
            <Button variant="ghost" onClick={() => setToolEdit(null)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                if (!toolEdit) return;
                updateTool.mutate({
                  tool: toolEdit,
                  body: {
                    enabled: toolEdit.enabled,
                    risk: toolEdit.risk,
                    approvalPolicy: toolEdit.approvalPolicy,
                    maxTimeoutSeconds: toolEdit.maxTimeoutSeconds,
                    maxOutputBytes: toolEdit.maxOutputBytes,
                  },
                });
              }}
            >
              Сохранить
            </Button>
          </>
        }
      >
        {toolEdit ? (
          <div className="form-grid">
            <Field label="Включён">
              <Switch
                checked={!!toolEdit.enabled}
                onChange={(value) => setToolEdit({ ...toolEdit, enabled: value })}
                label="Инструмент включён"
              />
            </Field>
            <Field label="Risk">
              <select
                className="input"
                value={toolEdit.risk}
                onChange={(event) => setToolEdit({ ...toolEdit, risk: event.target.value })}
              >
                {['low', 'medium', 'high', 'critical'].map((risk) => (
                  <option key={risk} value={risk}>
                    {risk}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Approval policy">
              <select
                className="input"
                value={toolEdit.approvalPolicy}
                onChange={(event) =>
                  setToolEdit({ ...toolEdit, approvalPolicy: event.target.value })
                }
              >
                <option value="never">never</option>
                <option value="risk">risk (medium+)</option>
                <option value="always">always</option>
              </select>
            </Field>
            <Field label="Timeout, секунд">
              <input
                className="input"
                type="number"
                min="1"
                max="3600"
                value={toolEdit.maxTimeoutSeconds ?? 120}
                onChange={(event) =>
                  setToolEdit({ ...toolEdit, maxTimeoutSeconds: Number(event.target.value) })
                }
              />
            </Field>
            <Field label="Max output, байт">
              <input
                className="input"
                type="number"
                min="1024"
                value={toolEdit.maxOutputBytes ?? 262144}
                onChange={(event) =>
                  setToolEdit({ ...toolEdit, maxOutputBytes: Number(event.target.value) })
                }
              />
            </Field>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={!!quotaEdit}
        onClose={() => setQuotaEdit(null)}
        title="AI quota"
        footer={
          <>
            <Button variant="ghost" onClick={() => setQuotaEdit(null)}>
              Отмена
            </Button>
            <Button
              variant="primary"
              onClick={() =>
                quotaEdit && saveQuota.mutate({ userId: quotaUserId, body: quotaEdit })
              }
            >
              Сохранить
            </Button>
          </>
        }
      >
        {quotaEdit ? (
          <div className="form-grid">
            {[
              ['maxConcurrentTasks', 'Параллельных задач'],
              ['maxTasksPerDay', 'Задач в сутки'],
              ['maxToolCallsPerTask', 'Tool calls на задачу'],
              ['maxTaskSeconds', 'Таймаут задачи, с'],
              ['maxStorageBytes', 'Storage quota, байт'],
              ['maxFiles', 'Максимум файлов'],
              ['maxFileBytes', 'Максимальный файл, байт'],
            ].map(([key, label]) => (
              <Field key={key} label={label}>
                <input
                  className="input"
                  type="number"
                  value={String(quotaEdit[key] ?? 0)}
                  onChange={(event) =>
                    setQuotaEdit({ ...quotaEdit, [key]: Number(event.target.value) })
                  }
                />
              </Field>
            ))}
          </div>
        ) : null}
      </Modal>

      <ConfirmDialog
        open={!!providerDelete}
        title="Отключить провайдера?"
        description="Провайдер перестанет использоваться новыми диалогами. История сохранится."
        confirmLabel="Отключить"
        busy={disableProvider.isPending}
        onCancel={() => setProviderDelete(null)}
        onConfirm={() => providerDelete && disableProvider.mutate(providerDelete)}
      />

      <ConfirmDialog
        open={!!skillDelete}
        title="Удалить Skill?"
        description="Удаление возможно только без назначений. Действие необратимо."
        confirmLabel="Удалить"
        busy={deleteSkill.isPending}
        onCancel={() => setSkillDelete(null)}
        onConfirm={() => skillDelete && deleteSkill.mutate(skillDelete.id)}
      />
    </>
  );
}

function OverviewTab({
  overview,
  loading,
  error,
}: {
  overview?: AiOverview;
  loading: boolean;
  error: unknown;
}) {
  if (loading) return <LoadingInline label="AI Overview" />;
  if (error || !overview) {
    return <Alert>{errorMessage(error)}</Alert>;
  }
  return (
    <div className="stack">
      <div className="stats-grid">
        <StatCard
          label="AI-модуль"
          icon={<Bot aria-hidden="true" />}
          value={overview.moduleEnabled ? 'включён' : 'отключён'}
          sub="Module gate применяется сервером"
        />
        <StatCard
          label="Runtime online"
          icon={<Server aria-hidden="true" />}
          value={formatNumber(overview.runtimes.online)}
          unit={`из ${formatNumber(overview.runtimes.total)}`}
          sub={`Подключено сейчас: ${overview.runtimes.connectedNow}`}
        />
        <StatCard
          label="Задачи"
          icon={<Activity aria-hidden="true" />}
          value={formatNumber(overview.tasks.running)}
          sub={`очередь ${overview.tasks.queued} · ожидают approval ${overview.tasks.waitingApproval}`}
        />
        <StatCard
          label="Ошибки за 24 ч"
          icon={<ShieldAlert aria-hidden="true" />}
          value={formatNumber(overview.tasks.failedLast24h)}
          sub={`Провайдеры с ошибкой: ${overview.providers.error}`}
        />
      </div>
      <div className="grid-2">
        <div className="card">
          <SectionTitle
            title="Runtime"
            meta={`heartbeat: ${formatDate(overview.lastHeartbeatAt)}`}
          />
          <div className="ai-admin-table">
            {(overview.nodes as Array<Record<string, unknown>>).map((node) => (
              <div key={String(node.id)} className="ai-admin-row">
                <span className="grow truncate">{String(node.name)}</span>
                <Badge
                  tone={
                    node.status === 'online'
                      ? 'ok'
                      : node.status === 'degraded'
                        ? 'warn'
                        : 'default'
                  }
                >
                  {String(node.status)}
                </Badge>
                <span className="tiny muted">{formatDate(String(node.lastSeenAt))}</span>
              </div>
            ))}
          </div>
        </div>
        <div className="card">
          <SectionTitle
            title="Очередь и approvals"
            meta={`streaming events: ${formatNumber(overview.streaming.events)}`}
          />
          <div className="info-list">
            <Info label="Активные чаты" value={formatNumber(overview.streaming.activeChats)} />
            <Info label="Ожидают подтверждения" value={formatNumber(overview.approvals.pending)} />
            <Info
              label="Возраст очереди"
              value={
                overview.tasks.queueAgeSeconds == null
                  ? 'очередь пуста'
                  : `${Math.round(overview.tasks.queueAgeSeconds)} с`
              }
            />
            <Info
              label="Провайдеры"
              value={`${overview.providers.enabled} включено из ${overview.providers.total}`}
            />
            <Info label="Обновлено" value={formatDate(overview.checkedAt)} />
          </div>
          {overview.recentErrors.length > 0 ? (
            <>
              <SectionTitle title="Последние ошибки" />
              {overview.recentErrors.map((item, index) => (
                <div key={index} className="ai-admin-row">
                  <Badge tone="danger">{item.kind}</Badge>
                  <span className="grow truncate">{item.message}</span>
                  <span className="tiny muted">{formatDate(item.at)}</span>
                </div>
              ))}
            </>
          ) : (
            <div className="tiny muted" style={{ marginTop: 10 }}>
              Ошибок Runtime не зафиксировано.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function AssignmentsEditor({
  skill,
  assignments,
  users,
  workspaces,
  roles,
  busy,
  onAssign,
  onRemove,
}: {
  skill: AiSkill | null;
  assignments: AiAssignment[];
  users: Array<{ id: string; username: string; displayName: string }>;
  workspaces: AiWorkspace[];
  roles: Array<{ id: string; name: string; key: string }>;
  busy: boolean;
  onAssign: (body: Record<string, unknown>) => void;
  onRemove: (assignment: AiAssignment) => void;
}) {
  const [scope, setScope] = useState<'user' | 'workspace' | 'role'>('user');
  const [targetId, setTargetId] = useState('');
  if (!skill) return null;
  return (
    <div className="stack">
      <div className="ai-admin-row">
        <select
          className="input"
          value={scope}
          onChange={(event) => {
            setScope(event.target.value as 'user' | 'workspace' | 'role');
            setTargetId('');
          }}
        >
          <option value="user">Пользователь</option>
          <option value="workspace">Workspace</option>
          <option value="role">Роль</option>
        </select>
        <select
          className="input grow"
          value={targetId}
          onChange={(event) => setTargetId(event.target.value)}
        >
          <option value="">Выберите</option>
          {scope === 'user'
            ? users.map((user) => (
                <option key={user.id} value={user.id}>
                  {user.displayName || user.username}
                </option>
              ))
            : null}
          {scope === 'workspace'
            ? workspaces.map((workspace) => (
                <option key={workspace.id} value={workspace.id}>
                  {workspace.name}
                </option>
              ))
            : null}
          {scope === 'role'
            ? roles.map((role) => (
                <option key={role.id} value={role.id}>
                  {role.name} ({role.key})
                </option>
              ))
            : null}
        </select>
        <Button
          size="sm"
          variant="primary"
          disabled={!targetId || busy}
          onClick={() =>
            onAssign({
              scope,
              userId: scope === 'user' ? targetId : null,
              workspaceId: scope === 'workspace' ? targetId : null,
              roleId: scope === 'role' ? targetId : null,
            })
          }
        >
          Назначить
        </Button>
      </div>
      <div className="ai-admin-table">
        {assignments.length === 0 ? (
          <div className="tiny muted">Назначений нет — Skill недоступен агентам.</div>
        ) : null}
        {assignments.map((assignment) => (
          <div key={assignment.id} className="ai-admin-row">
            <Badge>{assignment.scope}</Badge>
            <span className="grow truncate">
              {assignment.scope === 'user'
                ? users.find((user) => user.id === assignment.userId)?.username
                : assignment.scope === 'workspace'
                  ? workspaces.find((workspace) => workspace.id === assignment.workspaceId)?.name
                  : roles.find((role) => role.id === assignment.roleId)?.name}
            </span>
            <Button
              size="sm"
              variant="ghost"
              icon={<Trash2 aria-hidden="true" />}
              disabled={busy}
              onClick={() => onRemove(assignment)}
            >
              Удалить
            </Button>
          </div>
        ))}
      </div>
    </div>
  );
}

function Info({ label, value }: { label: string; value: unknown }) {
  return (
    <div className="info-row">
      <span>{label}</span>
      <strong>{typeof value === 'string' || typeof value === 'number' ? value : '—'}</strong>
    </div>
  );
}

export type { AiWorkspace, AiProvider, AiProgram, AiToolInfo };
