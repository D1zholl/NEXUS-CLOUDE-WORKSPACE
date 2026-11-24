//! Версионированный JSON-контракт Backend ↔ NEXUS AI Runtime.
//!
//! Каждое сообщение содержит `protocolVersion`, `messageId`, `type`,
//! `timestamp`, `nodeId`, `correlationId`, `sequence` и `payload`.
//! `nodeId` клиента всегда сверяется с аутентифицированным устройством.

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
    pub const TOOL_PROGRESS: &str = "tool.progress";
    pub const TOOL_COMPLETED: &str = "tool.completed";
    pub const TOOL_FAILED: &str = "tool.failed";

    pub const TASK_CANCEL: &str = "task.cancel";
    pub const TASK_CANCEL_ACK: &str = "task.cancel_ack";
    pub const TASK_CANCELLED: &str = "task.cancelled";

    pub const WORKSPACE_USAGE: &str = "workspace.usage";
    pub const WORKSPACE_USAGE_RESULT: &str = "workspace.usage_result";

    pub const EXECUTION_STATUS: &str = "execution.status";
    pub const EXECUTION_STATUS_RESULT: &str = "execution.status_result";

    pub const MCP_CONFIGURE: &str = "mcp.configure";
    pub const MCP_CONFIGURE_ACK: &str = "mcp.configure_ack";
    pub const MCP_DISCOVER: &str = "mcp.discover";
    pub const MCP_TOOLS: &str = "mcp.tools";
}

/// Общий конверт сообщения.
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
    pub fn server(
        message_id: Uuid,
        kind: &str,
        node_id: Uuid,
        correlation_id: Option<Uuid>,
        task_id: Option<Uuid>,
        sequence: Option<i64>,
        payload: Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id,
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

/// Возможности Runtime, присылаемые в hello/capabilities.
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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkIsolationStatus {
    /// true — Runtime умеет применять firewall-изоляцию для программ.
    #[serde(default)]
    pub supported: bool,
    /// true — процесс Runtime имеет права на изменение firewall.
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
    #[serde(default = "default_version")]
    pub version: i32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default = "default_risk")]
    pub risk: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i64,
    #[serde(default = "default_output")]
    pub max_output_bytes: i64,
    #[serde(default)]
    pub requires_permission: String,
    #[serde(default)]
    pub idempotent: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_version() -> i32 {
    1
}
fn default_risk() -> String {
    "low".to_owned()
}
fn default_timeout() -> i64 {
    120
}
fn default_output() -> i64 {
    262_144
}
fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramDescriptor {
    pub program_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub executable: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default = "default_risk")]
    pub risk: String,
    #[serde(default)]
    pub allowed_args: Value,
    #[serde(default = "default_network_policy")]
    pub network_policy: String,
}

fn default_network_policy() -> String {
    "deny".to_owned()
}

/// Сообщение в формате OpenAI Chat Completions.
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChatCompleted {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub usage: Value,
    #[serde(default)]
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
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecuteRequest {
    pub tool_id: String,
    #[serde(default = "default_version")]
    pub version: i32,
    #[serde(default)]
    pub arguments: Value,
    pub workspace_id: Uuid,
    /// Корень workspace на диске Runtime. Передаётся только по защищённому
    /// каналу; Runtime обязан проверить его по своему allowlist.
    pub workspace_root: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i64,
    #[serde(default = "default_output")]
    pub max_output_bytes: i64,
    /// Лимиты конкретного workspace. 0 — использовать глобальные limits Runtime.
    #[serde(default)]
    pub workspace_quota_bytes: i64,
    #[serde(default)]
    pub workspace_max_files: i64,
    #[serde(default)]
    pub workspace_max_file_bytes: i64,
    /// Последний известный backend usage (для диагностики и быстрого отказа).
    #[serde(default)]
    pub workspace_usage_bytes: i64,
    #[serde(default)]
    pub workspace_usage_files: i64,
    /// Политика сети для program.run: deny (по умолчанию) или allow.
    #[serde(default = "default_network_policy")]
    pub network_policy: String,
    /// Лимит памяти процесса (0 — не задан).
    #[serde(default)]
    pub max_memory_bytes: i64,
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
pub struct ToolOutcome {
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub result_text: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub duration_ms: i64,
    /// Актуальный usage workspace после операции (если Runtime его измерил).
    #[serde(default)]
    pub usage: Option<WorkspaceUsage>,
}

/// Описание MCP-соединения, которое backend передаёт Runtime.
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

/// Результат проверки статуса ранее выданного execution по message_id.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStatusResult {
    pub message_id: Uuid,
    /// completed | failed | cancelled | unknown | running
    pub status: String,
    #[serde(default)]
    pub outcome: Option<ToolOutcome>,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_uses_documented_field_names() {
        let envelope = Envelope::server(
            Uuid::nil(),
            kind::HELLO_ACK,
            Uuid::nil(),
            None,
            None,
            Some(1),
            serde_json::json!({}),
        );
        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["messageId"], Uuid::nil().to_string());
        assert_eq!(value["type"], "runtime.hello_ack");
        assert_eq!(value["nodeId"], Uuid::nil().to_string());
        assert!(value.get("correlationId").is_some());
    }
}
