/**
 * Типы API NEXUS WORKSPACE.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

export interface RoleRef {
  id: string;
  key: string;
  name: string;
}

export interface PermissionOverride {
  key: string;
  granted: boolean;
}

export interface UserSummary {
  id: string;
  username: string;
  displayName: string;
  status: 'active' | 'disabled' | string;
  isSuperadmin: boolean;
  roles: RoleRef[];
  storageUsed: number;
  quotaBytes: number | null;
  createdAt: string;
  lastLoginAt: string | null;
  lockedUntil: string | null;
  modules: { minecraft: boolean; cloud: boolean };
}

export interface UserDetail extends UserSummary {
  permissionOverrides: PermissionOverride[];
}

export interface Role {
  id: string;
  key: string;
  name: string;
  description: string;
  isSystem: boolean;
  permissions: string[];
  userCount: number;
}

export interface Permission {
  key: string;
  module: string;
  label: string;
}

export interface SessionUser {
  id: string;
  username: string;
  displayName: string;
  isSuperadmin: boolean;
  roles: RoleRef[];
  permissions: string[];
}

export interface MeResponse {
  user: SessionUser;
  csrfToken: string;
  session: { expiresAt: string };
  modules: Record<string, { enabled: boolean }>;
}

export interface Page<T> {
  items: T[];
  page: number;
  perPage: number;
  total: number;
}

export interface AuditRow {
  id: string;
  createdAt: string;
  eventType: string;
  actorId: string | null;
  actorUsername: string;
  targetType: string;
  targetId: string;
  result: string;
  ip: string;
  details: unknown;
}

export interface ModuleStateRow {
  key: string;
  enabled: boolean;
  updatedAt: string;
}

export interface DiskInfo {
  name: string;
  mountPoint: string;
  fileSystem: string;
  totalBytes: number;
  availableBytes: number;
  removable: boolean;
}

export interface SystemMetrics {
  capturedAt: string;
  cpuPercent: number;
  cpuCores: number;
  memoryTotal: number;
  memoryUsed: number;
  processMemory: number;
  hostname: string;
  osName: string;
  osVersion: string;
  kernelVersion: string;
  disks: DiskInfo[];
}

export interface MyUsage {
  usedBytes: number;
  quotaBytes: number;
  explicitQuota: number | null;
  remainingBytes: number;
}

export interface SystemSettingSpec {
  key: string;
  kind: 'int' | 'bool' | 'string' | 'enum' | 'json';
  min: number | null;
  max: number | null;
  requiresRestart: boolean;
  permission: string;
  label: string;
  options: string[] | null;
}

export interface SystemSettingsResponse {
  values: Record<string, unknown>;
  meta: Record<string, { requiresRestart: boolean; kind: string }>;
  catalog: SystemSettingSpec[];
  /** Эффективные значения: то, что реально действует сейчас. */
  effective: Record<string, unknown>;
}

export interface Overview {
  version: string;
  startedAt: string;
  uptimeSeconds: number;
  database: { status: string; sizeBytes: number };
  users: { total: number; active: number };
  sessions: { active: number };
  audit: { events: number };
  modules: Record<string, boolean>;
  system: SystemMetrics;
}

export interface HealthResponse {
  status: string;
  version: string;
  uptimeSeconds: number;
  database: string;
}

export interface SetupStatus {
  needsSetup: boolean;
  minPasswordLength: number;
}

/* --- Storage Engine --- */

export interface DetectedDisk {
  mountPoint: string;
  name: string;
  fileSystem: string;
  kind: string;
  totalBytes: number;
  availableBytes: number;
  usedBytes: number;
  removable: boolean;
  system: boolean;
  /** Сетевой диск: служба LocalSystem его не использует. */
  remote: boolean;
}

export interface StorageVolume {
  id: string;
  name: string;
  rootPath: string;
  diskMount: string;
  mode: 'auto' | 'manual' | string;
  priority: number;
  reservedBytes: number;
  maxBytes: number | null;
  enabled: boolean;
  state: 'online' | 'offline' | 'error' | string;
  stateMessage: string;
  totalBytes: number;
  availableBytes: number;
  lastCheckedAt: string | null;
  createdAt: string;
  updatedAt: string;
  scope: 'system' | 'cloud' | 'minecraft' | string;
  ownerUserId: string | null;
  ownerUsername: string | null;
  objects: number;
  objectsBytes: number;
}

export interface VolumeOwner {
  id: string;
  username: string;
  displayName: string;
}

export interface StorageSummary {
  volumes: number;
  online: number;
  totalBytes: number;
  availableBytes: number;
  reservedBytes: number;
  objects: number;
  objectsBytes: number;
}

export interface BrowseEntry {
  name: string;
  path: string;
  isDisk: boolean;
  totalBytes: number;
  availableBytes: number;
}

export interface BrowseResponse {
  path: string | null;
  parent: string | null;
  entries: BrowseEntry[];
  truncated: boolean;
}

export interface VolumeCheckReport {
  reachable: boolean;
  writable: boolean;
  totalBytes: number;
  availableBytes: number;
  objectsChecked: number;
  objectsMissing: number;
  objectsSizeMismatch: number;
  message: string;
}

export interface VolumeCheckOutcome {
  row: StorageVolume;
  report: VolumeCheckReport;
}

export interface VolumeScanReport {
  path: string;
  nexusPresent: boolean;
  objectsFound: number;
  objectsBytes: number;
  stagingFiles: number;
  stagingBytes: number;
  quarantineFiles: number;
  quarantineBytes: number;
  foreignEntries: number;
  skippedEntries: number;
  truncated: boolean;
  message: string;
}

export interface CreateVolumeInput {
  name: string;
  path: string;
  mode?: 'auto' | 'manual';
  priority?: number;
  reservedBytes?: number;
  maxBytes?: number | null;
  enabled?: boolean;
  scope?: 'system' | 'cloud' | 'minecraft';
  ownerUserId?: string | null;
  /** Импортировать объекты, уже лежащие в `.nexus/objects`. */
  adoptOrphans?: boolean;
}

export interface UpdateVolumeInput {
  name?: string;
  mode?: 'auto' | 'manual';
  priority?: number;
  reservedBytes?: number;
  /** `0` очищает лимит тома. */
  maxBytes?: number;
  enabled?: boolean;
  scope?: 'system' | 'cloud' | 'minecraft';
  /** `null` делает том общим. */
  ownerUserId?: string | null;
}

/* --- Cloud Storage --- */

export interface CloudEntry {
  id: string;
  ownerId: string;
  parentId: string | null;
  name: string;
  entryType: 'file' | 'folder' | string;
  sizeBytes: number;
  contentType: string;
  starred: boolean;
  createdAt: string;
  updatedAt: string;
  trashedAt: string | null;
}

export interface CloudCrumb {
  id: string;
  name: string;
}

export interface CloudUsage {
  usedBytes: number;
  fileCount: number;
}

export interface CloudListResponse {
  items: CloudEntry[];
  page: number;
  perPage: number;
  total: number;
  breadcrumb: CloudCrumb[];
  usage: CloudUsage;
}

export interface CloudConfig {
  chunkMaxBytes: number;
  maxUploadBytes: number;
  maxUploadMb: number;
  uploadTtlHours: number;
  uploadConcurrency: number;
  shareTtlHours: number;
  exportTtlHours: number;
  trashRetentionDays: number;
  serverTime: string;
}

export interface CloudEntryDetails {
  entry: CloudEntry;
  breadcrumb: CloudCrumb[];
  preview: string | null;
  activeShares: number;
}

export interface CloudUploadSession {
  id: string;
  userId: string;
  volumeId: string;
  parentId: string | null;
  fileName: string;
  contentType: string;
  declaredSize: number | null;
  receivedBytes: number;
  status: string;
  createdAt: string;
  expiresAt: string;
}

export interface CloudShare {
  id: string;
  entryId: string;
  entryName: string;
  createdAt: string;
  expiresAt: string | null;
  maxDownloads: number | null;
  downloadCount: number;
  hasPassword: boolean;
  authOnly: boolean;
  revokedAt: string | null;
  active: boolean;
}

export interface CloudShareCreated {
  share: CloudShare;
  token: string;
  url: string;
}

export type CloudExportStatus = 'pending' | 'running' | 'completed' | 'failed' | string;

export interface CloudExportJob {
  id: string;
  status: CloudExportStatus;
  progress: number;
  totalUnits: number | null;
  doneUnits: number | null;
  error: string | null;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

export interface ListCloudParams {
  page?: number;
  perPage?: number;
  parentId?: string | null;
  q?: string;
  sort?: string;
  direction?: string;
  view?: string;
  entryType?: string;
  /** Категория файлов: image, video, audio, document, archive, other. */
  category?: string;
  /** Глубина «Недавних» в днях (по умолчанию 30). */
  sinceDays?: number;
}

export type UploadConflictStrategy = 'replace' | 'rename';

/* --- Minecraft Build Manager --- */

export type BuildStatus = 'draft' | 'published' | 'archived' | string;
export type LoaderType = 'vanilla' | 'fabric' | 'forge' | 'neoforge' | 'quilt' | string;
export type ReleaseStatus =
  | 'draft'
  | 'validating'
  | 'ready'
  | 'published'
  | 'archived'
  | 'deprecated'
  | 'unpublished'
  | string;

export interface BuildRow {
  id: string;
  slug: string;
  name: string;
  description: string;
  version: string;
  category: string;
  visibility: 'public' | 'private' | string;
  status: BuildStatus;
  minecraftVersion: string;
  loaderType: LoaderType;
  loaderVersion: string;
  javaMajor: number;
  javaMin: number;
  javaNotes: string;
  iconUrl: string | null;
  currentReleaseId: string | null;
  currentReleaseVersion: string | null;
  fileCount: number;
  totalBytes: number;
  createdAt: string;
  updatedAt: string;
  archivedAt: string | null;
}

export interface BuildStats {
  total: number;
  published: number;
  drafts: number;
  archived: number;
  files: number;
  totalBytes: number;
  recentUpdated: BuildRef[];
  recentPublished: BuildRef[];
}

export interface BuildRef {
  id: string;
  name: string;
  slug: string;
  version: string | null;
  at: string;
}

export interface BuildFileRow {
  id: string;
  buildId: string;
  kind: 'file' | 'dir' | string;
  path: string;
  category: string;
  side: 'client' | 'server' | 'both' | string;
  required: boolean;
  enabled: boolean;
  description: string;
  objectId: string | null;
  sizeBytes: number;
  sha256: string;
  createdAt: string;
  updatedAt: string;
}

export interface ReleaseRow {
  id: string;
  buildId: string;
  version: string;
  status: ReleaseStatus;
  channel: 'stable' | 'beta' | string;
  changelog: string;
  fileCount: number;
  totalBytes: number;
  createdAt: string;
  publishedAt: string | null;
  archivedAt: string | null;
  deprecatedAt: string | null;
  isCurrent: boolean;
}

export interface BuildIssue {
  code: string;
  message: string;
  fileId: string | null;
  path: string | null;
}

export interface BuildValidationReport {
  valid: boolean;
  errors: BuildIssue[];
  warnings: BuildIssue[];
  fileCount: number;
  totalBytes: number;
}

export interface ReleaseHistoryRow {
  id: string;
  buildId: string;
  releaseId: string | null;
  event: string;
  actorUsername: string;
  details: unknown;
  createdAt: string;
}

export interface CreateBuildInput {
  name: string;
  slug?: string;
  description?: string;
  version?: string;
  category?: string;
  visibility?: 'public' | 'private';
  minecraftVersion?: string;
  loaderType?: LoaderType;
  loaderVersion?: string;
  javaMajor?: number;
  javaMin?: number;
  javaNotes?: string;
}

export type UpdateBuildInput = Partial<CreateBuildInput>;

export interface CloudEntryDetailResponse {
  build: BuildRow;
  files: { files: number; folders: number; totalBytes: number };
  releases: ReleaseRow[];
}

/* --- Launcher API --- */

export interface LauncherToken {
  id: string;
  name: string;
  scopes: string[];
  lastUsedAt: string | null;
  expiresAt: string | null;
  revokedAt: string | null;
  createdAt: string;
  username: string;
}

export interface LauncherTokenIssued {
  token: string;
  tokenType: string;
  scopes: string[];
  expiresAt: string | null;
}

export interface LauncherRepositoryInfo {
  baseUrl: string;
  catalogUrl: string;
  channels: string[];
  publicPacks: number;
  publishedReleases: number;
  client: string;
  clientTokenSupport: boolean;
  tokenNote: string;
  endpoints: Record<string, string>;
}

export interface CreateLauncherTokenInput {
  name: string;
  username: string;
  password: string;
  scopes?: string[];
  expiresInHours?: number;
}

export interface LauncherDownloadRow {
  id: string;
  path: string;
  bytesSent: number;
  status: number;
  createdAt: string;
  ip: string;
}

/* --- System Management --- */

export interface MonitoringCore {
  name: string;
  usage: number;
}

export interface MonitoringCurrent {
  capturedAt: string;
  uptimeSeconds: number;
  cpu: {
    percent: number;
    cores: number;
    perCore: MonitoringCore[];
    loadAverage: [number, number, number];
  };
  memory: {
    total: number;
    used: number;
    available: number;
    swapTotal: number;
    swapUsed: number;
    process: number;
  };
  network: {
    receivedTotal: number;
    transmittedTotal: number;
    rxRate: number;
    txRate: number;
    activeConnections?: number;
  };
  disks: DiskInfo[];
  volumes: Array<{
    name: string;
    state: string;
    enabled: boolean;
    totalBytes: number;
    availableBytes: number;
  }>;
  app: {
    sessions: number;
    users: number;
    requestsPerSecond: number;
    averageLatencyMs: number;
    errorRate: number;
    requestsLastMinute: number;
    errorsLastMinute: number;
    jobsPending: number;
    activeUploads: number;
    activeDownloads: number;
    activeWebSockets: number;
    database: string;
    hostname: string;
    os: string;
    osVersion: string;
    kernel: string;
  };
}

export type MetricPoint = { at: string; value: number };

export interface MonitoringHistory {
  window: string;
  bucketSeconds: number;
  since: string;
  series: Record<string, MetricPoint[]>;
}

export interface MonitoringModules {
  minecraft: {
    enabled: boolean;
    builds: number;
    published: number;
    buildBytes: number;
    files: number;
    fileBytes: number;
    releases: number;
    uploads24h?: number;
    downloads24h?: number;
    errors24h?: number;
  };
  cloud: {
    enabled: boolean;
    files: number;
    bytes: number;
    activeShares: number;
    uploads24h?: number;
    downloads24h?: number;
    errors24h?: number;
    activeDownloads?: number;
  };
  storage: {
    objects: number;
    bytes: number;
    unavailableObjects: number;
    processReadBytesPerSec?: number;
    processWriteBytesPerSec?: number;
    diskIoNote?: string;
  };
  errors24h: number;
  launcher: { downloads24h: number; bytes24h: number };
  connections?: {
    activeRequests: number;
    activeDownloads: number;
    activeWebSockets: number;
  };
}

export interface BackgroundJob {
  id: string;
  jobType: string;
  status: string;
  progress: number;
  totalUnits: number | null;
  doneUnits: number | null;
  error: string | null;
  payload: unknown;
  cancellable: boolean;
  /** Запрошена пауза, но текущая единица работы ещё выполняется. */
  pauseRequested: boolean;
  pausedAt: string | null;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  actor: string | null;
}

export interface JobsResponse {
  items: BackgroundJob[];
  pending: number;
  running: number;
  paused: number;
  failed: number;
  completed: number;
  /** Лимит параллельных задач из `jobs.max_concurrent`. */
  maxConcurrent: number;
}

export interface InterfaceSettings {
  accent: string;
}

export interface BackgroundSettings {
  enabled: boolean;
  preset: string;
  palette: string;
  intensity: number;
  quality: string;
  fpsLimit: number;
  count: number;
  sizeMin: number;
  sizeMax: number;
  speed: number;
  rotation: number;
  opacity: number;
  glow: number;
  spacing: number;
  depth: number;
  spread: number;
  wireframe: boolean;
  stars: number;
  starBrightness: number;
  twinkle: number;
  starSize: number;
  starDepth: number;
  backgroundSpeed: number;
  parallax: number;
  mouseSensitivity: number;
  perspective: number;
  cameraAmplitude: number;
  vignette: number;
  grain: number;
  aberration: number;
}

export interface ListUsersParams {
  page?: number;
  perPage?: number;
  q?: string;
  status?: string;
}

export interface CreateUserInput {
  username: string;
  displayName: string;
  password: string;
  roleIds: string[];
  status?: string;
}

export interface UpdateUserInput {
  displayName?: string;
  status?: string;
  roleIds?: string[];
  permissions?: Record<string, boolean>;
  /** `null` — вернуть квоту по умолчанию. */
  quotaBytes?: number | null;
}

export interface CreateRoleInput {
  key: string;
  name: string;
  description?: string;
  permissions: string[];
}

export interface UpdateRoleInput {
  name?: string;
  description?: string;
  permissions?: string[];
}

export interface ListAuditParams {
  page?: number;
  perPage?: number;
  eventType?: string;
}
