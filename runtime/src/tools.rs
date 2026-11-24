//! Runtime Tool Registry: локальная проверка схемы, workspace и политики.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dashmap::DashMap;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::config::LimitsConfig;
use crate::filesystem::{
    Workspace, directory_usage, list_entries, normalize_relative, read_text, search, sha256_hex,
    write_atomic,
};
use crate::patch;
use crate::process::{ProcessOutcome, ProgramRegistry};
use crate::protocol::{ToolDescriptor, ToolExecuteRequest, ToolOutcome, WorkspaceUsage};

pub struct ToolExecutor {
    limits: LimitsConfig,
    allowed_roots: Vec<PathBuf>,
    programs: ProgramRegistry,
    locks: DashMap<uuid::Uuid, Arc<Mutex<()>>>,
}

impl ToolExecutor {
    pub fn new(
        limits: &LimitsConfig,
        allowed_roots: Vec<PathBuf>,
        programs: ProgramRegistry,
    ) -> Self {
        Self {
            limits: limits.clone(),
            allowed_roots,
            programs,
            locks: DashMap::new(),
        }
    }

    pub fn programs(&self) -> &ProgramRegistry {
        &self.programs
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![
            descriptor(
                "fs.list",
                "Список файлов",
                "Показывает файлы и каталоги workspace.",
                "filesystem",
                "low",
                "ai.workspace.read",
                true,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "path": { "type": "string", "maxLength": 500 },
                        "depth": { "type": "integer", "minimum": 1, "maximum": 6 }
                    }
                }),
            ),
            descriptor(
                "fs.read",
                "Чтение файла",
                "Читает UTF-8 файл.",
                "filesystem",
                "low",
                "ai.workspace.read",
                true,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "path": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "maxBytes": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["path"]
                }),
            ),
            descriptor(
                "fs.write",
                "Запись файла",
                "Создаёт или перезаписывает файл.",
                "filesystem",
                "medium",
                "ai.workspace.write",
                false,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "path": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "content": { "type": "string" },
                        "overwrite": { "type": "boolean" }
                    },
                    "required": ["path", "content"]
                }),
            ),
            descriptor(
                "fs.search",
                "Поиск",
                "Ищет строку в текстовых файлах.",
                "filesystem",
                "low",
                "ai.workspace.read",
                true,
                60,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "query": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "path": { "type": "string", "maxLength": 500 },
                        "maxResults": { "type": "integer", "minimum": 1, "maximum": 200 }
                    },
                    "required": ["query"]
                }),
            ),
            descriptor(
                "fs.mkdir",
                "Создать каталог",
                "Создаёт каталог.",
                "filesystem",
                "low",
                "ai.workspace.write",
                true,
                15,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": { "path": { "type": "string", "minLength": 1, "maxLength": 500 } },
                    "required": ["path"]
                }),
            ),
            descriptor(
                "fs.move",
                "Переместить",
                "Перемещает файл/каталог.",
                "filesystem",
                "high",
                "ai.workspace.write",
                false,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "from": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "to": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "overwrite": { "type": "boolean" }
                    },
                    "required": ["from", "to"]
                }),
            ),
            descriptor(
                "fs.copy",
                "Копировать",
                "Копирует файл.",
                "filesystem",
                "medium",
                "ai.workspace.write",
                false,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "from": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "to": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "overwrite": { "type": "boolean" }
                    },
                    "required": ["from", "to"]
                }),
            ),
            descriptor(
                "fs.delete",
                "Удалить",
                "Удаляет файл или каталог.",
                "filesystem",
                "high",
                "ai.workspace.write",
                false,
                30,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "path": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "recursive": { "type": "boolean" }
                    },
                    "required": ["path"]
                }),
            ),
            descriptor(
                "fs.apply_patch",
                "Патч",
                "Применяет unified diff.",
                "filesystem",
                "medium",
                "ai.workspace.write",
                false,
                60,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "patch": { "type": "string", "minLength": 1 },
                        "strip": { "type": "integer", "minimum": 0, "maximum": 3 }
                    },
                    "required": ["patch"]
                }),
            ),
            descriptor(
                "program.run",
                "Запуск программы",
                "Запускает зарегистрированную программу.",
                "process",
                "high",
                "ai.tools.use",
                false,
                300,
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "programId": { "type": "string", "minLength": 1, "maxLength": 80 },
                        "args": { "type": "array", "maxItems": 32, "items": { "type": "string", "maxLength": 500 } },
                        "cwd": { "type": "string", "maxLength": 500 }
                    },
                    "required": ["programId"]
                }),
            ),
        ]
    }

    pub async fn execute(
        &self,
        request: &ToolExecuteRequest,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ToolOutcome> {
        let descriptors = self.descriptors();
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.tool_id == request.tool_id)
            .ok_or_else(|| anyhow::anyhow!("tool_id не разрешён Runtime"))?;
        validate_schema(&descriptor.input_schema, &request.arguments)?;
        let workspace = Workspace::new(&request.workspace_root, &self.allowed_roots)?;
        let mutable = matches!(
            request.tool_id.as_str(),
            "fs.write" | "fs.copy" | "fs.move" | "fs.delete" | "fs.mkdir" | "fs.apply_patch"
        );
        let lock = self
            .locks
            .entry(request.workspace_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = if mutable {
            Some(lock.lock_owned().await)
        } else {
            None
        };
        let result = match request.tool_id.as_str() {
            "fs.list" => self.fs_list(&workspace, &request.arguments)?,
            "fs.read" => self.fs_read(&workspace, &request.arguments)?,
            "fs.write" => self.fs_write(&workspace, &request.arguments, request)?,
            "fs.search" => self.fs_search(&workspace, &request.arguments)?,
            "fs.mkdir" => self.fs_mkdir(&workspace, &request.arguments)?,
            "fs.move" => self.fs_move(&workspace, &request.arguments)?,
            "fs.copy" => self.fs_copy(&workspace, &request.arguments, request)?,
            "fs.delete" => self.fs_delete(&workspace, &request.arguments)?,
            "fs.apply_patch" => self.fs_apply_patch(&workspace, &request.arguments, request)?,
            "program.run" => {
                return self
                    .program_run(&workspace, &request.arguments, request, cancel)
                    .await;
            }
            _ => bail!("неизвестный инструмент"),
        };
        // Usage измеряется после изменяющих операций. Ошибка измерения не
        // отменяет уже выполненную операцию, но попадает в usage.error.
        let usage = if mutable {
            Some(usage_for(
                &workspace,
                self.limits.max_directory_entries.saturating_mul(100),
            ))
        } else {
            None
        };
        let result_text = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(ToolOutcome {
            result,
            result_text: Some(
                result_text
                    .chars()
                    .take(request.max_output_bytes.max(1024) as usize)
                    .collect(),
            ),
            exit_code: None,
            duration_ms: 0,
            usage,
        })
    }

    fn fs_list(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let path = args
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let depth = args
            .get("depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(1) as usize;
        list_entries(
            workspace,
            path,
            depth,
            self.limits.max_directory_entries,
            &self.limits.skip_directories,
        )
    }

    fn fs_read(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let relative = required_str(args, "path")?;
        let path = workspace.resolve_existing(relative)?;
        let max_bytes = args
            .get("maxBytes")
            .and_then(|value| value.as_u64())
            .unwrap_or(self.limits.max_tool_output_bytes as u64)
            .min(self.limits.max_file_bytes) as usize;
        let (text, truncated) = read_text(&path, max_bytes)?;
        Ok(json!({
            "path": workspace.relative(&path),
            "content": text.chars().take(self.limits.max_tool_output_bytes).collect::<String>(),
            "truncated": truncated,
            "sha256": sha256_hex(text.as_bytes()),
        }))
    }

    fn fs_write(
        &self,
        workspace: &Workspace,
        args: &Value,
        limits: &ToolExecuteRequest,
    ) -> Result<Value> {
        let relative = required_str(args, "path")?;
        let content = required_str(args, "content")?;
        let max_file = self.effective_max_file(limits);
        if content.len() as u64 > max_file {
            bail!("содержимое превышает лимит файла workspace");
        }
        let overwrite = args
            .get("overwrite")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let path = workspace.resolve(relative)?;
        if path == workspace.root {
            bail!("нельзя записать корень workspace");
        }
        let old_size = if path.is_file() {
            std::fs::metadata(&path)?.len()
        } else {
            0
        };
        if old_size == 0 && !path.exists() {
            self.ensure_quota(workspace, limits, content.len() as i64, 1)?;
        } else {
            self.ensure_quota(workspace, limits, content.len() as i64 - old_size as i64, 0)?;
        }
        write_atomic(&path, content.as_bytes(), overwrite)?;
        Ok(json!({
            "path": workspace.relative(&path),
            "bytes": content.len(),
            "sha256": sha256_hex(content.as_bytes()),
        }))
    }

    fn fs_search(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let query = required_str(args, "query")?;
        let path = args
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let max_results = args
            .get("maxResults")
            .and_then(|value| value.as_u64())
            .unwrap_or(self.limits.max_search_results as u64)
            .min(self.limits.max_search_results as u64) as usize;
        search(
            workspace,
            query,
            path,
            self.limits.max_search_files,
            max_results,
            &self.limits.skip_directories,
            self.limits.max_file_bytes.min(1024 * 1024) as usize,
        )
    }

    fn fs_mkdir(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let relative = required_str(args, "path")?;
        normalize_relative(relative)?;
        let path = workspace.resolve(relative)?;
        // Проверяем существующего предка ещё раз непосредственно перед созданием.
        if path.exists() {
            bail!("путь уже существует");
        }
        std::fs::create_dir_all(&path)?;
        Ok(json!({ "path": workspace.relative(&path), "created": true }))
    }

    fn fs_move(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let from = workspace.resolve_existing(required_str(args, "from")?)?;
        let to = workspace.resolve(required_str(args, "to")?)?;
        if to == workspace.root {
            bail!("нельзя переместить в корень workspace");
        }
        let overwrite = args
            .get("overwrite")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if to.exists() {
            if !overwrite {
                bail!("целевой путь уже существует");
            }
            if to.is_dir() {
                std::fs::remove_dir_all(&to)?;
            } else {
                std::fs::remove_file(&to)?;
            }
        }
        if let Some(parent) = to.parent() {
            if !parent.exists() {
                bail!("целевой каталог не существует");
            }
        }
        std::fs::rename(&from, &to)?;
        Ok(json!({
            "from": workspace.relative(&from),
            "to": workspace.relative(&to),
            "moved": true,
        }))
    }

    fn fs_copy(
        &self,
        workspace: &Workspace,
        args: &Value,
        limits: &ToolExecuteRequest,
    ) -> Result<Value> {
        let from = workspace.resolve_existing(required_str(args, "from")?)?;
        if !from.is_file() {
            bail!("копирование поддерживается только для файлов");
        }
        let metadata = std::fs::metadata(&from)?;
        if metadata.len() > self.effective_max_file(limits) {
            bail!("файл превышает лимит копирования workspace");
        }
        let to = workspace.resolve(required_str(args, "to")?)?;
        let overwrite = args
            .get("overwrite")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if to.exists() && !overwrite {
            bail!("целевой файл уже существует");
        }
        let old_size = if to.is_file() {
            std::fs::metadata(&to)?.len()
        } else {
            0
        };
        let delta = metadata.len() as i64 - old_size as i64;
        let new_files = if to.exists() { 0 } else { 1 };
        self.ensure_quota(workspace, limits, delta, new_files)?;
        std::fs::copy(&from, &to)?;
        Ok(json!({
            "from": workspace.relative(&from),
            "to": workspace.relative(&to),
            "bytes": metadata.len(),
            "copied": true,
        }))
    }

    fn fs_delete(&self, workspace: &Workspace, args: &Value) -> Result<Value> {
        let path = workspace.resolve_existing(required_str(args, "path")?)?;
        let recursive = args
            .get("recursive")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            if !recursive {
                bail!("для удаления каталога требуется recursive=true");
            }
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        Ok(json!({ "path": workspace.relative(&path), "deleted": true }))
    }

    fn fs_apply_patch(
        &self,
        workspace: &Workspace,
        args: &Value,
        request: &ToolExecuteRequest,
    ) -> Result<Value> {
        let patch_text = required_str(args, "patch")?;
        let strip = args
            .get("strip")
            .and_then(|value| value.as_u64())
            .unwrap_or(1) as usize;
        let quota = if request.workspace_quota_bytes > 0 || request.workspace_max_files > 0 {
            let max_entries = self.limits.max_directory_entries.saturating_mul(100);
            let (used_bytes, used_files) = directory_usage(&workspace.root, max_entries)?;
            Some(patch::Quota {
                used_bytes,
                used_files,
                quota_bytes: request.workspace_quota_bytes,
                max_files: request.workspace_max_files,
            })
        } else {
            None
        };
        patch::apply_checked(
            workspace,
            patch_text,
            strip,
            self.effective_max_file(request),
            quota.as_ref(),
        )
    }

    fn effective_max_file(&self, limits: &ToolExecuteRequest) -> u64 {
        if limits.workspace_max_file_bytes > 0 {
            (limits.workspace_max_file_bytes as u64).min(self.limits.max_file_bytes)
        } else {
            self.limits.max_file_bytes
        }
    }

    fn ensure_quota(
        &self,
        workspace: &Workspace,
        limits: &ToolExecuteRequest,
        delta_bytes: i64,
        delta_files: i64,
    ) -> Result<()> {
        if limits.workspace_quota_bytes <= 0 && limits.workspace_max_files <= 0 {
            return Ok(());
        }
        let (used_bytes, used_files) =
            directory_usage(&workspace.root, self.limits.max_directory_entries * 100)?;
        if limits.workspace_quota_bytes > 0
            && used_bytes as i64 + delta_bytes > limits.workspace_quota_bytes
        {
            bail!(
                "операция превышает квоту workspace: {} + {} > {} байт",
                used_bytes,
                delta_bytes,
                limits.workspace_quota_bytes
            );
        }
        if limits.workspace_max_files > 0
            && used_files as i64 + delta_files > limits.workspace_max_files
        {
            bail!(
                "операция превышает лимит файлов workspace: {} + {} > {}",
                used_files,
                delta_files,
                limits.workspace_max_files
            );
        }
        Ok(())
    }

    async fn program_run(
        &self,
        workspace: &Workspace,
        args: &Value,
        request: &ToolExecuteRequest,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ToolOutcome> {
        let program_id = required_str(args, "programId")?;
        let list: Vec<String> = args
            .get("args")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let cwd = args
            .get("cwd")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(|value| workspace.resolve_existing(value))
            .transpose()?;
        // Перед запуском проверяем, что текущий usage не превышает квоту.
        let max_entries = self.limits.max_directory_entries.saturating_mul(100);
        if request.workspace_quota_bytes > 0 || request.workspace_max_files > 0 {
            let (used_bytes, used_files) = directory_usage(&workspace.root, max_entries)?;
            self.check_quota_values(request, used_bytes, used_files, 0, 0)?;
        }
        // Во время работы программа может писать файлы. Мониторим usage и
        // принудительно завершаем дерево при превышении квоты.
        let run_cancel = cancel.child_token();
        let violation = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let monitor = if request.workspace_quota_bytes > 0 || request.workspace_max_files > 0 {
            let root = workspace.root.clone();
            let limits = request.workspace_quota_bytes;
            let max_files = request.workspace_max_files;
            let token = run_cancel.clone();
            let violation = violation.clone();
            Some(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_millis(750));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    if token.is_cancelled() {
                        break;
                    }
                    match directory_usage(&root, max_entries) {
                        Ok((bytes, files)) => {
                            if limits > 0 && bytes as i64 > limits {
                                *violation.lock().await = Some(format!(
                                    "программа превысила квоту workspace: {bytes} > {limits} байт"
                                ));
                                token.cancel();
                                break;
                            }
                            if max_files > 0 && files as i64 > max_files {
                                *violation.lock().await = Some(format!(
                                    "программа превысила лимит файлов workspace: {files} > {max_files}"
                                ));
                                token.cancel();
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "не удалось измерить usage при запуске программы");
                        }
                    }
                }
            }))
        } else {
            None
        };
        let result = self
            .programs
            .run(
                program_id,
                &list,
                &workspace.root,
                cwd.as_deref(),
                &request.network_policy,
                request.max_memory_bytes.max(0) as u64,
                &run_cancel,
            )
            .await;
        if let Some(monitor) = monitor {
            monitor.abort();
        }
        if let Some(message) = violation.lock().await.clone() {
            bail!("{message}");
        }
        let outcome = result.map(process_outcome)?;
        let usage = Some(usage_for(workspace, max_entries));
        let mut outcome = outcome;
        outcome.usage = usage;
        Ok(outcome)
    }

    fn check_quota_values(
        &self,
        request: &ToolExecuteRequest,
        used_bytes: u64,
        used_files: u64,
        delta_bytes: i64,
        delta_files: i64,
    ) -> Result<()> {
        if request.workspace_quota_bytes > 0
            && used_bytes as i64 + delta_bytes > request.workspace_quota_bytes
        {
            bail!(
                "операция превышает квоту workspace: {} + {} > {} байт",
                used_bytes,
                delta_bytes,
                request.workspace_quota_bytes
            );
        }
        if request.workspace_max_files > 0
            && used_files as i64 + delta_files > request.workspace_max_files
        {
            bail!(
                "операция превышает лимит файлов workspace: {} + {} > {}",
                used_files,
                delta_files,
                request.workspace_max_files
            );
        }
        Ok(())
    }
}

fn process_outcome(outcome: ProcessOutcome) -> ToolOutcome {
    ToolOutcome {
        result: outcome.result,
        result_text: Some(outcome.result_text),
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
        usage: None,
    }
}

fn usage_for(workspace: &Workspace, max_entries: usize) -> WorkspaceUsage {
    match directory_usage(&workspace.root, max_entries) {
        Ok((bytes, files)) => WorkspaceUsage {
            bytes: bytes as i64,
            files: files as i64,
            checked_at: Some(chrono::Utc::now()),
            error: None,
        },
        Err(error) => WorkspaceUsage {
            bytes: 0,
            files: 0,
            checked_at: Some(chrono::Utc::now()),
            error: Some(
                format!("usage недоступен: {error}")
                    .chars()
                    .take(300)
                    .collect(),
            ),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn descriptor(
    id: &str,
    name: &str,
    description: &str,
    category: &str,
    risk: &str,
    permission: &str,
    idempotent: bool,
    timeout: i64,
    schema: Value,
) -> ToolDescriptor {
    ToolDescriptor {
        tool_id: id.to_owned(),
        version: 1,
        name: name.to_owned(),
        description: description.to_owned(),
        category: category.to_owned(),
        risk: risk.to_owned(),
        input_schema: schema,
        timeout_seconds: timeout,
        max_output_bytes: 262_144,
        requires_permission: permission.to_owned(),
        idempotent,
        enabled: true,
    }
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .with_context(|| format!("отсутствует обязательное поле {key}"))
}

fn validate_schema(schema: &Value, arguments: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| anyhow::anyhow!("некорректная JSON Schema инструмента: {error}"))?;
    if let Some(error) = validator.iter_errors(arguments).next() {
        bail!("аргументы не соответствуют схеме: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_rejects_unknown_fields() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        });
        assert!(validate_schema(&schema, &json!({"path":"a"})).is_ok());
        assert!(validate_schema(&schema, &json!({"path":"a","extra":1})).is_err());
        assert!(validate_schema(&schema, &json!({})).is_err());
    }
}
