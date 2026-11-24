//! Общие доменные структуры AI-модуля (SQLx rows и безопасные DTO).

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

pub fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut result: String = value.chars().take(max_chars).collect();
    result.push('…');
    result
}

/// Маскирует чувствительные поля в аргументах инструмента для UI и аудита.
pub fn redact_arguments(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (key, item) in map {
                let lower = key.to_ascii_lowercase();
                let sensitive = lower.contains("password")
                    || lower.contains("secret")
                    || lower.contains("token")
                    || lower.contains("api_key")
                    || lower.contains("apikey")
                    || lower == "key";
                result.insert(
                    key.clone(),
                    if sensitive {
                        Value::String("***".to_owned())
                    } else {
                        redact_arguments(item)
                    },
                );
            }
            Value::Object(result)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_arguments).collect()),
        other => other.clone(),
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ProviderRow {
    pub id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key_cipher: Option<String>,
    pub default_model: String,
    pub generation: Value,
    pub limits: Value,
    pub runtime_node_id: Option<Uuid>,
    pub enabled: bool,
    pub status: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_models: Value,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderRow {
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "type": self.provider_type,
            "baseUrl": self.base_url,
            "defaultModel": self.default_model,
            "generation": self.generation,
            "limits": self.limits,
            "runtimeNodeId": self.runtime_node_id,
            "enabled": self.enabled,
            "status": self.status,
            "lastCheckedAt": self.last_checked_at,
            "lastError": self.last_error,
            "models": self.last_models,
            "hasApiKey": self.api_key_cipher.is_some(),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub title: String,
    pub mode: String,
    pub provider_id: Option<Uuid>,
    pub model: String,
    pub workspace_id: Option<Uuid>,
    pub runtime_node_id: Option<Uuid>,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub sequence: i32,
    pub role: String,
    pub content: String,
    pub parts: Value,
    pub model: String,
    pub status: String,
    pub usage: Value,
    pub tool_calls: Value,
    pub tool_call_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct TaskRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub owner_id: Uuid,
    pub runtime_node_id: Uuid,
    pub provider_id: Option<Uuid>,
    pub model: String,
    pub workspace_id: Option<Uuid>,
    pub status: String,
    pub stage: String,
    pub status_text: String,
    pub progress: f64,
    pub iterations: i32,
    pub max_iterations: i32,
    pub tool_calls_count: i32,
    pub timeout_seconds: i32,
    pub human_input: String,
    pub context: Value,
    pub cancel_requested: bool,
    pub cancel_status: String,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_confirmed_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WorkspaceRow {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub runtime_node_id: Uuid,
    pub name: String,
    pub root_path: String,
    pub quota_bytes: i64,
    pub max_files: i32,
    pub max_file_bytes: i64,
    pub max_parallel_tasks: i32,
    pub max_task_seconds: i32,
    pub allowed_tools: Vec<String>,
    pub allowed_programs: Vec<String>,
    pub status: String,
    pub usage_bytes: i64,
    pub usage_files: i64,
    pub usage_checked_at: Option<DateTime<Utc>>,
    pub network_policy: String,
    pub max_process_memory_bytes: i64,
    pub usage_reported_by: Option<Uuid>,
    pub usage_error: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkspaceRow {
    /// Безопасное представление: абсолютный путь видят только администраторы.
    pub fn view(&self, include_root: bool) -> Value {
        serde_json::json!({
            "id": self.id,
            "ownerId": self.owner_id,
            "runtimeNodeId": self.runtime_node_id,
            "name": self.name,
            "rootPath": if include_root { Some(self.root_path.clone()) } else { None },
            "quotaBytes": self.quota_bytes,
            "maxFiles": self.max_files,
            "maxFileBytes": self.max_file_bytes,
            "maxParallelTasks": self.max_parallel_tasks,
            "maxTaskSeconds": self.max_task_seconds,
            "allowedTools": self.allowed_tools,
            "allowedPrograms": self.allowed_programs,
            "status": self.status,
            "usageBytes": self.usage_bytes,
            "usageFiles": self.usage_files,
            "usageCheckedAt": self.usage_checked_at,
            "networkPolicy": self.network_policy,
            "maxProcessMemoryBytes": self.max_process_memory_bytes,
            "usageReportedBy": self.usage_reported_by,
            "usageError": self.usage_error,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct SkillRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub version: i32,
    pub skill_md: String,
    pub checksum: String,
    pub size_bytes: i64,
    pub file_count: i32,
    pub dependencies: Vec<String>,
    pub enabled: bool,
    pub imported_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SkillRow {
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "slug": self.slug,
            "name": self.name,
            "description": self.description,
            "version": self.version,
            "checksum": self.checksum,
            "sizeBytes": self.size_bytes,
            "fileCount": self.file_count,
            "dependencies": self.dependencies,
            "enabled": self.enabled,
            "importedBy": self.imported_by,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct QuotaRow {
    pub user_id: Uuid,
    pub max_concurrent_tasks: i32,
    pub max_tasks_per_day: i32,
    pub max_tool_calls_per_task: i32,
    pub max_task_seconds: i32,
    pub max_storage_bytes: i64,
    pub max_files: i32,
    pub max_file_bytes: i64,
    pub updated_at: DateTime<Utc>,
}

pub async fn load_quota(db: &sqlx::PgPool, user_id: Uuid) -> Option<QuotaRow> {
    sqlx::query_as::<_, QuotaRow>(
        "select user_id, max_concurrent_tasks, max_tasks_per_day, max_tool_calls_per_task,
                max_task_seconds, max_storage_bytes, max_files, max_file_bytes, updated_at
           from ai_user_quotas where user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSettings {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_secret_keys() {
        let value = serde_json::json!({
            "path": "src/main.rs",
            "apiKey": "sk-secret",
            "nested": { "password": "p", "token": "t" },
            "list": [{ "secret": 1 }, { "ok": 2 }]
        });
        let redacted = redact_arguments(&value);
        assert_eq!(redacted["path"], "src/main.rs");
        assert_eq!(redacted["apiKey"], "***");
        assert_eq!(redacted["nested"]["password"], "***");
        assert_eq!(redacted["list"][0]["secret"], "***");
        assert_eq!(redacted["list"][1]["ok"], 2);
    }
}
