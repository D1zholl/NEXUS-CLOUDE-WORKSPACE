//! Версионированный протокол NEXUS AI Runtime (клиентская сторона).
//!
//! Контракт совпадает с backend `src/ai/runtime/protocol.rs`. Runtime не
//! доверяет `node_id` в payload: серверный конверт сверяется с device token.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROTOCOL_VERSION: i32 = 1;

pub mod kind {
    pub const HELLO: &str = "runtime.hello";
    pub const HELLO_ACK: &str = "runtime.hello_ack";
    pub const HEARTBEAT: &str = "runtime.heartbeat";
    pub const HEARTBEAT_ACK: &str = "runtime.heartbeat_ack";
    pub const CAPABILITIES: &str = "runtime.capabilities";
    pub const CAPABILITIES_ACK: &str = "runtime.capabilities_ack";
    pub const RUNTIME_ERROR: &str = "runtime.error";

    pub const PROVIDER_START: &str = "provider.chat.start";
    pub const PROVIDER_ACK: &str = "provider.chat.accepted";
    pub const PROVIDER_DELTA: &str = "provider.chat.delta";
    pub const PROVIDER_COMPLETED: &str = "provider.chat.completed";
    pub const PROVIDER_FAILED: &str = "provider.chat.failed";
    pub const PROVIDER_CANCEL: &str = "provider.chat.cancel";
    pub const PROVIDER_REQUEST: &str = "provider.request";
    pub const PROVIDER_RESPONSE: &str = "provider.response";

    pub const TOOL_EXECUTE: &str = "tool.execute";
    pub const TOOL_ACCEPTED: &str = "tool.accepted";
    pub const TOOL_COMPLETED: &str = "tool.completed";
    pub const TOOL_FAILED: &str = "tool.failed";

    pub const TASK_CANCEL: &str = "task.cancel";
    pub const TASK_CANCEL_ACK: &str = "task.cancel_ack";
    pub const TASK_CANCELLED: &str = "task.cancelled";

    pub const WORKSPACE_USAGE: &str = "workspace.usage";
    pub const WORKSPACE_USAGE_RESULT: &str = "workspace.usage_result";
    pub const WORKSPACE_PROVISION: &str = "workspace.provision";
    pub const WORKSPACE_PROVISION_RESULT: &str = "workspace.provision_result";
    pub const WORKSPACE_DELETE: &str = "workspace.delete";
    pub const WORKSPACE_DELETE_RESULT: &str = "workspace.delete_result";

    pub const EXECUTION_STATUS: &str = "execution.status";
    pub const EXECUTION_STATUS_RESULT: &str = "execution.status_result";

    pub const MCP_CONFIGURE: &str = "mcp.configure";
    pub const MCP_CONFIGURE_ACK: &str = "mcp.configure_ack";
    pub const MCP_DISCOVER: &str = "mcp.discover";
    pub const MCP_TOOLS: &str = "mcp.tools";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub protocol_version: i32,
    pub message_id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub timestamp: DateTime<Utc>,
    pub node_id: Uuid,
    #[serde(default)]
    pub correlation_id: Option<Uuid>,
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default)]
    pub sequence: Option<i64>,
    #[serde(default)]
    pub payload: Value,
}

impl Envelope {
    pub fn new(
        kind: &str,
        node_id: Uuid,
        correlation_id: Option<Uuid>,
        task_id: Option<Uuid>,
        sequence: Option<i64>,
        payload: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: Uuid::new_v4(),
            kind: kind.to_owned(),
            timestamp: Utc::now(),
            node_id,
            correlation_id,
            task_id,
            sequence,
            payload,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilities {
    #[serde(default)]
    pub runtime_version: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub protocol_version: i32,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub polystrata: PolystrataStatus,
    #[serde(default)]
    pub tools: Vec<ToolDescriptor>,
    #[serde(default)]
    pub programs: Vec<ProgramDescriptor>,
    #[serde(default)]
    pub network_isolation: NetworkIsolationStatus,
    #[serde(default)]
    pub provisioning: ProvisioningStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningStatus {
    /// true — Runtime поддерживает `workspace.provision`.
    #[serde(default)]
    pub supported: bool,
    /// true — настроен каталог для новых workspace.
    #[serde(default)]
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkIsolationStatus {
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub elevated: bool,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolystrataStatus {
    #[serde(default)]
    pub reachable: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub model_count: i64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub tool_id: String,
    pub version: i32,
    pub name: String,
    pub description: String,
    pub category: String,
    pub risk: String,
    pub input_schema: Value,
    pub timeout_seconds: i64,
    pub max_output_bytes: i64,
    pub requires_permission: String,
    pub idempotent: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramDescriptor {
    pub program_id: String,
    pub name: String,
    pub executable: String,
    pub version: String,
    pub available: bool,
    pub risk: String,
    pub allowed_args: Value,
    #[serde(default = "default_network_policy")]
    pub network_policy: String,
}

fn default_network_policy() -> String {
    "deny".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "tool_type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

fn tool_type() -> String {
    "function".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    #[serde(rename = "type", default = "tool_type")]
    pub kind: String,
    pub function: ToolDefinitionFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinitionFunction {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChatRequest {
    pub provider_id: Option<Uuid>,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i64,
}

fn default_timeout() -> i64 {
    120
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChatCompleted {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Value,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequest {
    pub operation: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResponse {
    pub operation: String,
    pub ok: bool,
    pub models: Vec<String>,
    pub data: Value,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecuteRequest {
    pub tool_id: String,
    #[serde(default = "default_version")]
    pub version: i32,
    pub arguments: Value,
    pub workspace_id: Uuid,
    pub workspace_root: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i64,
    #[serde(default = "default_output")]
    pub max_output_bytes: i64,
    #[serde(default)]
    pub workspace_quota_bytes: i64,
    #[serde(default)]
    pub workspace_max_files: i64,
    #[serde(default)]
    pub workspace_max_file_bytes: i64,
    #[serde(default)]
    pub workspace_usage_bytes: i64,
    #[serde(default)]
    pub workspace_usage_files: i64,
    #[serde(default = "default_network_policy")]
    pub network_policy: String,
    #[serde(default)]
    pub max_memory_bytes: i64,
}

fn default_version() -> i32 {
    1
}
fn default_output() -> i64 {
    262_144
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUsage {
    #[serde(default)]
    pub bytes: i64,
    #[serde(default)]
    pub files: i64,
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
}

/// Снимок файла до/после изменяющей операции Runtime.
///
/// Содержимое ограничено по размеру конфигурацией Runtime. При превышении
/// `truncated=true`, и откат этого файла запрещён на backend.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnapshot {
    pub path: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub before_base64: Option<String>,
    #[serde(default)]
    pub before_sha256: Option<String>,
    #[serde(default)]
    pub after_base64: Option<String>,
    #[serde(default)]
    pub after_sha256: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

/// `workspace.provision`: backend просит Runtime создать каталог нового
/// workspace. Имя каталога — только UUID, никаких пользовательских строк.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProvisionRequest {
    pub workspace_id: Uuid,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProvisionResult {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub ok: bool,
    /// Абсолютный путь на компьютере Runtime. Передаётся только backend.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// `workspace.delete`: backend просит Runtime физически удалить workspace
/// после истечения срока хранения. Runtime сам подтверждает принадлежность
/// каталога: путь должен быть `<provisioning_root>/<workspaceId>` и совпадать
/// с маркером, созданным при provisioning.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDeleteRequest {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub workspace_root: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDeleteResult {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub ok: bool,
    /// Объём и число файлов, измеренные перед удалением (для отчёта/метрик).
    #[serde(default)]
    pub bytes: i64,
    #[serde(default)]
    pub files: i64,
    #[serde(default)]
    pub already_absent: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    pub result: Value,
    pub result_text: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: i64,
    #[serde(default)]
    pub usage: Option<WorkspaceUsage>,
    #[serde(default)]
    pub changed_files: Vec<ChangedFile>,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub snapshots: Vec<FileSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionConfig {
    pub id: Uuid,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub allow_private_network: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiscoveredTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiscoveryResult {
    pub connection_id: Uuid,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub tools: Vec<McpDiscoveredTool>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub protocol_version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStatusResult {
    pub message_id: Uuid,
    pub status: String,
    #[serde(default)]
    pub outcome: Option<ToolOutcome>,
    #[serde(default)]
    pub error: Option<String>,
}
