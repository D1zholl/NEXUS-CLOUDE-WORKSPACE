//! Исходящее WSS-соединение, heartbeat, исполнение команд и reconnect.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{Value, json};
use tokio::sync::{RwLock, Semaphore, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{DeviceCredentials, RuntimeConfig};
use crate::mcp::McpRegistry;
use crate::process::ProgramRegistry;
use crate::protocol::{
    Envelope, ExecutionStatusResult, McpConnectionConfig, PolystrataStatus, ProviderChatCompleted,
    ProviderRequest, ProviderResponse, ProvisioningStatus, RuntimeCapabilities, ToolExecuteRequest,
    ToolOutcome, WorkspaceDeleteRequest, WorkspaceDeleteResult, WorkspaceProvisionRequest,
    WorkspaceProvisionResult, WorkspaceUsage, kind,
};
use crate::provider::ProviderClient;
use crate::receipts::{PrunedMarker, Receipt, ReceiptFailure, ReceiptStore};
use crate::tools::ToolExecutor;
use crate::{filesystem, network};

/// Маркер принадлежности каталога workspace. Создаётся при provisioning и
/// проверяется при физическом удалении. Не содержит секретов.
const WORKSPACE_MARKER: &str = ".nexus-workspace.json";
/// Служебный каталог для атомарного переноса перед удалением.
const TRASH_DIR: &str = ".nexus-trash";

#[derive(Clone)]
pub struct RuntimeContext {
    pub config: Arc<RuntimeConfig>,
    pub credentials: DeviceCredentials,
    provider: Arc<ProviderClient>,
    tools: Arc<ToolExecutor>,
    mcp: Arc<McpRegistry>,
    roots: Arc<Vec<PathBuf>>,
    outbound: mpsc::UnboundedSender<Envelope>,
    sequence: Arc<AtomicI64>,
    active: Arc<DashMap<Uuid, ActiveTask>>,
    task_index: Arc<DashMap<Uuid, Vec<Uuid>>>,
    provider_cache: Arc<DashMap<Uuid, ProviderChatCompleted>>,
    tool_cache: Arc<DashMap<Uuid, ToolOutcome>>,
    failure_cache: Arc<DashMap<Uuid, ReceiptFailure>>,
    pruned_cache: Arc<DashMap<Uuid, String>>,
    receipts: Arc<ReceiptStore>,
    semaphore: Arc<Semaphore>,
    last_status: Arc<RwLock<PolystrataStatus>>,
}

struct ActiveTask {
    task_id: Option<Uuid>,
    cancel: CancellationToken,
}

pub async fn run(
    config: RuntimeConfig,
    credentials: DeviceCredentials,
    mut shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<()> {
    let roots = config.canonical_roots()?;
    let (receipt_store, loaded, pruned) = ReceiptStore::load(
        config.receipts_path(),
        config.receipts_max_bytes(),
        config.receipts_max_archives(),
        config.receipts_load_limit(),
    );
    let receipts = Arc::new(receipt_store);
    tracing::info!(
        count = loaded.len(),
        pruned = pruned.len(),
        path = %receipts.path().display(),
        "receipts загружены"
    );
    let provider_cache: Arc<DashMap<Uuid, ProviderChatCompleted>> = Arc::new(DashMap::new());
    let tool_cache: Arc<DashMap<Uuid, ToolOutcome>> = Arc::new(DashMap::new());
    let failure_cache: Arc<DashMap<Uuid, ReceiptFailure>> = Arc::new(DashMap::new());
    let pruned_cache: Arc<DashMap<Uuid, String>> = Arc::new(DashMap::new());
    for receipt in loaded {
        if let Some(failure) = receipt.failure {
            failure_cache.insert(receipt.message_id, failure);
            continue;
        }
        if let Some(completed) = receipt.provider {
            provider_cache.insert(receipt.message_id, completed);
        }
        if let Some(outcome) = receipt.tool {
            tool_cache.insert(receipt.message_id, outcome);
        }
    }
    for PrunedMarker { message_id, kind } in pruned {
        // Полный receipt имеет приоритет: маркер только запрещает повтор.
        if !provider_cache.contains_key(&message_id)
            && !tool_cache.contains_key(&message_id)
            && !failure_cache.contains_key(&message_id)
        {
            pruned_cache.insert(message_id, kind);
        }
    }

    let config = Arc::new(config);
    let max_parallel = config.limits.max_parallel_tasks.max(1);
    let mcp = Arc::new(McpRegistry::new());
    let mut attempt = 0u32;
    loop {
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let context = RuntimeContext {
            config: config.clone(),
            credentials: credentials.clone(),
            provider: Arc::new(ProviderClient::new(&config.provider)?),
            tools: Arc::new(ToolExecutor::new(
                &config.limits,
                roots.clone(),
                ProgramRegistry::new(&config.programs, &config.limits),
            )),
            mcp: mcp.clone(),
            roots: Arc::new(roots.clone()),
            outbound,
            sequence: Arc::new(AtomicI64::new(0)),
            active: Arc::new(DashMap::new()),
            task_index: Arc::new(DashMap::new()),
            provider_cache: provider_cache.clone(),
            tool_cache: tool_cache.clone(),
            failure_cache: failure_cache.clone(),
            pruned_cache: pruned_cache.clone(),
            receipts: receipts.clone(),
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            last_status: Arc::new(RwLock::new(PolystrataStatus::default())),
        };

        match connect_once(context, outbound_rx, shutdown.clone()).await {
            Ok(()) => tracing::info!("соединение с NEXUS закрыто"),
            Err(error) => {
                tracing::warn!(%error, "ошибка соединения с NEXUS");
            }
        }
        if shutdown.as_ref().map(|rx| *rx.borrow()).unwrap_or(false) {
            tracing::info!("получен сигнал остановки Runtime");
            return Ok(());
        }
        attempt = attempt.saturating_add(1);
        let min = config.server.reconnect_min_seconds.max(1);
        let max = config.server.reconnect_max_seconds.max(min);
        let base = (min * 2u64.saturating_pow(attempt.min(8))).min(max);
        let jitter = rand::thread_rng().gen_range(0..=base / 3 + 1);
        let delay = Duration::from_secs(base + jitter);
        tracing::info!(seconds = delay.as_secs(), "переподключение к NEXUS");
        match shutdown.as_mut() {
            Some(receiver) => {
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        changed = receiver.changed() => {
                            if changed.is_err() || *receiver.borrow() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
            None => tokio::time::sleep(delay).await,
        }
    }
}

async fn connect_once(
    context: RuntimeContext,
    mut outbound_rx: mpsc::UnboundedReceiver<Envelope>,
    mut shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<()> {
    let mut request = context
        .config
        .server
        .url
        .as_str()
        .into_client_request()
        .context("некорректный server.url")?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", context.credentials.device_token)
            .parse()
            .context("device token содержит недопустимые символы")?,
    );
    request.headers_mut().insert(
        "user-agent",
        format!("nexus-ai-runtime/{}", env!("CARGO_PKG_VERSION"))
            .parse()
            .context("user-agent")?,
    );

    tracing::info!(url = %context.config.server.url, "подключение к NEXUS");
    let (socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .context("не удалось установить WSS-соединение с NEXUS")?;
    tracing::info!(status = %response.status(), "WebSocket Runtime подключён");

    let (mut write, mut read) = socket.split();
    let mut last_incoming_sequence: i64 = 0;
    let writer = tokio::spawn(async move {
        while let Some(envelope) = outbound_rx.recv().await {
            let Ok(text) = serde_json::to_string(&envelope) else {
                continue;
            };
            if write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let capabilities = context.base_capabilities();
    context.send(
        kind::HELLO,
        None,
        None,
        serde_json::to_value(&capabilities)?,
    );
    {
        let probe = context.clone();
        tokio::spawn(async move {
            let capabilities = probe.probe_capabilities().await;
            {
                let mut status = probe.last_status.write().await;
                *status = capabilities.polystrata.clone();
            }
            probe.send(
                kind::CAPABILITIES,
                None,
                None,
                serde_json::to_value(capabilities).unwrap_or(Value::Null),
            );
        });
    }

    let heartbeat_context = context.clone();
    let heartbeat = tokio::spawn(async move {
        let interval = Duration::from_secs(heartbeat_context.config.server.heartbeat_seconds);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        let mut capability_ticker = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            ticker.tick().await;
            let status = heartbeat_context.last_status.read().await.clone();
            heartbeat_context.send(
                kind::HEARTBEAT,
                None,
                None,
                json!({
                    "status": if status.reachable { "ok" } else { "degraded" },
                    "polystrata": status,
                    "runtimeVersion": env!("CARGO_PKG_VERSION"),
                }),
            );
            if tokio::time::Instant::now() >= capability_ticker {
                let capabilities = heartbeat_context.probe_capabilities().await;
                {
                    let mut status = heartbeat_context.last_status.write().await;
                    *status = capabilities.polystrata.clone();
                }
                heartbeat_context.send(
                    kind::CAPABILITIES,
                    None,
                    None,
                    serde_json::to_value(capabilities).unwrap_or(Value::Null),
                );
                capability_ticker = tokio::time::Instant::now() + Duration::from_secs(300);
            }
        }
    });

    loop {
        let next = if let Some(receiver) = shutdown.as_mut() {
            tokio::select! {
                _ = receiver.changed() => {
                    heartbeat.abort();
                    writer.abort();
                    bail!("Runtime остановлен");
                }
                result = tokio::time::timeout(Duration::from_secs(120), read.next()) => result,
            }
        } else {
            tokio::time::timeout(Duration::from_secs(120), read.next()).await
        };
        let message = match next {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(error))) => {
                heartbeat.abort();
                writer.abort();
                return Err(error).context("WebSocket Runtime: ошибка чтения");
            }
            Ok(None) => {
                heartbeat.abort();
                writer.abort();
                bail!("NEXUS закрыл WebSocket Runtime");
            }
            Err(_) => {
                heartbeat.abort();
                writer.abort();
                bail!("нет данных от NEXUS более 120 секунд");
            }
        };
        match message {
            Message::Text(text) => {
                let envelope: Envelope = match serde_json::from_str(&text) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        tracing::warn!(%error, "некорректный кадр NEXUS");
                        continue;
                    }
                };
                if envelope.node_id != context.credentials.node_id {
                    tracing::warn!(
                        expected = %context.credentials.node_id,
                        received = %envelope.node_id,
                        "node_id не совпадает с device token; кадр отклонён"
                    );
                    continue;
                }
                if envelope.protocol_version != crate::protocol::PROTOCOL_VERSION {
                    context.send(
                        kind::RUNTIME_ERROR,
                        Some(envelope.message_id),
                        envelope.task_id,
                        json!({
                            "code": "AI_PROTOCOL_VERSION_UNSUPPORTED",
                            "message": "Версия протокола Runtime не поддерживается."
                        }),
                    );
                    continue;
                }
                if let Some(sequence) = envelope.sequence {
                    if sequence <= last_incoming_sequence {
                        let known = context.active.contains_key(&envelope.message_id)
                            || context.tool_cache.contains_key(&envelope.message_id)
                            || context.provider_cache.contains_key(&envelope.message_id);
                        if known {
                            // Повторная доставка команды: handle() вернёт
                            // сохранённый receipt и не выполнит side effect снова.
                            tracing::warn!(
                                sequence,
                                kind = %envelope.kind,
                                "повторная доставка команды NEXUS: будет возвращён receipt"
                            );
                        } else {
                            tracing::warn!(
                                sequence,
                                last = last_incoming_sequence,
                                kind = %envelope.kind,
                                "повторная/устаревшая команда NEXUS отклонена"
                            );
                            continue;
                        }
                    } else {
                        last_incoming_sequence = sequence;
                    }
                }
                let age = chrono::Utc::now()
                    .signed_duration_since(envelope.timestamp)
                    .num_seconds();
                if !(-15 * 60..=15 * 60).contains(&age) {
                    tracing::warn!(
                        age_seconds = age,
                        kind = %envelope.kind,
                        "команда NEXUS с недопустимым timestamp отклонена"
                    );
                    continue;
                }
                let ctx = context.clone();
                if matches!(
                    envelope.kind.as_str(),
                    kind::MCP_CONFIGURE | kind::MCP_DISCOVER
                ) {
                    // Конфигурация и discovery MCP должны выполняться строго
                    // последовательно: discovery не должен обогнать configure.
                    if let Err(error) = context.handle(envelope).await {
                        tracing::warn!(%error, "ошибка MCP-команды NEXUS");
                    }
                    continue;
                }
                tokio::spawn(async move {
                    if let Err(error) = ctx.handle(envelope).await {
                        tracing::warn!(%error, "ошибка обработки команды NEXUS");
                    }
                });
            }
            Message::Ping(data) => {
                // Ответ Pong отправляется протоколом при следующей записи;
                // heartbeat обеспечивает регулярный flush.
                let _ = data;
            }
            Message::Close(_) => {
                heartbeat.abort();
                writer.abort();
                bail!("NEXUS инициировал закрытие соединения");
            }
            _ => {}
        }
    }
}

impl RuntimeContext {
    fn send(
        &self,
        kind_value: &str,
        correlation: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: Value,
    ) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let envelope = Envelope::new(
            kind_value,
            self.credentials.node_id,
            correlation,
            task_id,
            Some(sequence),
            payload,
        );
        let _ = self.outbound.send(envelope);
    }

    fn base_capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            models: Vec::new(),
            polystrata: PolystrataStatus::default(),
            tools: self.tools.descriptors(),
            programs: self.tools.programs().descriptors(),
            network_isolation: network::status(),
            provisioning: self.provisioning_status(),
        }
    }

    /// Состояние `workspace.provision`: каталог должен существовать и входить
    /// в allowlist Runtime.
    fn provisioning_status(&self) -> ProvisioningStatus {
        match self.provisioning_root() {
            Some(_) => ProvisioningStatus {
                supported: true,
                configured: true,
            },
            None => ProvisioningStatus {
                supported: true,
                configured: false,
            },
        }
    }

    fn provisioning_root(&self) -> Option<PathBuf> {
        let configured = self.config.storage.provisioning_root.trim();
        let candidate = if configured.is_empty() {
            self.roots.first()?.clone()
        } else {
            let raw = PathBuf::from(configured);
            if !raw.is_absolute() {
                return None;
            }
            std::fs::canonicalize(raw).ok()?
        };
        if !candidate.is_dir() {
            return None;
        }
        // База может совпадать с allowed_root: Runtime создаёт workspace
        // дочерним каталогом, а containment дочернего пути проверяет
        // Workspace::new.
        if !self.roots.iter().any(|root| candidate.starts_with(root)) {
            return None;
        }
        Some(candidate)
    }

    async fn probe_capabilities(&self) -> RuntimeCapabilities {
        let (reachable, data) = match self.provider.status().await {
            Ok((reachable, data)) => (reachable, data),
            Err(error) => (
                false,
                json!({ "error": sanitize(&error.to_string(), &self.roots) }),
            ),
        };
        let models: Vec<String> = data
            .get("models")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let status = PolystrataStatus {
            reachable,
            version: data
                .get("status")
                .and_then(|value| value.get("body"))
                .and_then(|value| value.get("version"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_owned(),
            model_count: models.len() as i64,
            error: if reachable {
                None
            } else {
                Some("PolyStrata недоступна".to_owned())
            },
        };
        RuntimeCapabilities {
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            models,
            polystrata: status,
            tools: self.tools.descriptors(),
            programs: self.tools.programs().descriptors(),
            network_isolation: network::status(),
            provisioning: self.provisioning_status(),
        }
    }

    async fn handle(&self, envelope: Envelope) -> Result<()> {
        match envelope.kind.as_str() {
            kind::HELLO_ACK => {
                tracing::info!(
                    server_time = %envelope.payload.get("serverTime").and_then(|v| v.as_str()).unwrap_or(""),
                    "Runtime зарегистрирован в NEXUS"
                );
            }
            kind::HEARTBEAT_ACK | kind::CAPABILITIES_ACK => {}
            kind::PROVIDER_START => self.start_provider(envelope).await?,
            kind::PROVIDER_REQUEST => self.provider_operation(envelope).await?,
            kind::PROVIDER_CANCEL => {
                if let Some(correlation) = envelope.correlation_id {
                    self.cancel_message(correlation);
                }
            }
            kind::TASK_CANCEL => {
                if let Some(task_id) = envelope.task_id {
                    let cancel_request = envelope
                        .payload
                        .get("cancelRequestMessageId")
                        .and_then(|value| value.as_str())
                        .and_then(|value| Uuid::parse_str(value).ok())
                        .or(envelope.correlation_id)
                        .unwrap_or(envelope.message_id);
                    self.cancel_task(task_id, cancel_request).await;
                }
            }
            kind::WORKSPACE_USAGE => self.workspace_usage(envelope).await?,
            kind::WORKSPACE_PROVISION => self.workspace_provision(envelope).await?,
            kind::WORKSPACE_DELETE => self.workspace_delete(envelope).await?,
            kind::EXECUTION_STATUS => self.execution_status(envelope).await?,
            kind::MCP_CONFIGURE => {
                let connections: Vec<McpConnectionConfig> = envelope
                    .payload
                    .get("connections")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                self.mcp.configure(connections).await;
                self.send(
                    kind::MCP_CONFIGURE_ACK,
                    Some(envelope.message_id),
                    None,
                    json!({ "configured": true }),
                );
            }
            kind::MCP_DISCOVER => {
                let connection_id = envelope
                    .payload
                    .get("connectionId")
                    .and_then(|value| value.as_str())
                    .and_then(|value| Uuid::parse_str(value).ok());
                let Some(connection_id) = connection_id else {
                    self.send(
                        kind::RUNTIME_ERROR,
                        Some(envelope.message_id),
                        None,
                        json!({
                            "code": "AI_MCP_INVALID_REQUEST",
                            "message": "mcp.discover без connectionId"
                        }),
                    );
                    return Ok(());
                };
                let context = self.clone();
                let correlation = envelope.message_id;
                tokio::spawn(async move {
                    let result = context.mcp.discover(connection_id).await;
                    context.send(
                        kind::MCP_TOOLS,
                        Some(correlation),
                        None,
                        serde_json::to_value(result).unwrap_or(Value::Null),
                    );
                });
            }
            kind::TOOL_EXECUTE => self.start_tool(envelope).await?,
            kind::RUNTIME_ERROR => {
                tracing::warn!(
                    code = envelope
                        .payload
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    message = envelope
                        .payload
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    "NEXUS сообщил об ошибке Runtime"
                );
            }
            other => {
                tracing::debug!(kind = other, "неизвестная команда NEXUS");
            }
        }
        Ok(())
    }

    async fn start_provider(&self, envelope: Envelope) -> Result<()> {
        let request: crate::protocol::ProviderChatRequest =
            serde_json::from_value(envelope.payload.clone())
                .context("некорректный provider.chat.start")?;
        if let Some(cached) = self.provider_cache.get(&envelope.message_id) {
            self.send(
                kind::PROVIDER_COMPLETED,
                Some(envelope.message_id),
                envelope.task_id,
                serde_json::to_value(cached.clone())?,
            );
            return Ok(());
        }
        if let Some(failure) = self.failure_cache.get(&envelope.message_id) {
            self.send(
                kind::PROVIDER_FAILED,
                Some(envelope.message_id),
                envelope.task_id,
                json!({ "code": failure.code, "message": failure.message }),
            );
            return Ok(());
        }
        if self.pruned_cache.contains_key(&envelope.message_id) {
            self.send(
                kind::PROVIDER_FAILED,
                Some(envelope.message_id),
                envelope.task_id,
                json!({
                    "code": "AI_RECEIPT_ROTATED",
                    "message": "Операция уже исполнялась ранее; повтор запрещён после ротации receipts."
                }),
            );
            return Ok(());
        }
        if self.active.contains_key(&envelope.message_id) {
            return Ok(());
        }
        let cancel = CancellationToken::new();
        self.register(envelope.message_id, envelope.task_id, cancel.clone());
        self.send(
            kind::PROVIDER_ACK,
            Some(envelope.message_id),
            envelope.task_id,
            json!({ "accepted": true }),
        );

        let context = self.clone();
        let message_id = envelope.message_id;
        let task_id = envelope.task_id;
        tokio::spawn(async move {
            let permit = context.semaphore.clone().acquire_owned().await;
            let result = match permit {
                Ok(_permit) => {
                    let node_id = context.credentials.node_id;
                    let outbound = context.outbound.clone();
                    let mut on_delta = |delta: &str| {
                        let sequence = context.sequence.fetch_add(1, Ordering::SeqCst) + 1;
                        let envelope = Envelope::new(
                            kind::PROVIDER_DELTA,
                            node_id,
                            Some(message_id),
                            task_id,
                            Some(sequence),
                            json!({ "delta": delta }),
                        );
                        let _ = outbound.send(envelope);
                    };
                    context
                        .provider
                        .chat(&request, request.api_key.as_deref(), &cancel, &mut on_delta)
                        .await
                }
                Err(_) => Err(anyhow::anyhow!("Runtime остановлен")),
            };
            match result {
                Ok(completed) => {
                    context.provider_cache.insert(message_id, completed.clone());
                    context.receipts.append(&Receipt {
                        message_id,
                        kind: "provider".to_owned(),
                        provider: Some(completed.clone()),
                        tool: None,
                        failure: None,
                    });
                    context.send(
                        kind::PROVIDER_COMPLETED,
                        Some(message_id),
                        task_id,
                        serde_json::to_value(completed).unwrap_or(Value::Null),
                    );
                }
                Err(error) => {
                    let (code, message) = provider_error(&error);
                    let message = sanitize(&message, &context.roots);
                    // Фиксируем и неуспешный результат: повторная доставка того
                    // же messageId не должна заново вызывать модель.
                    let failure = ReceiptFailure {
                        kind: "provider".to_owned(),
                        code: code.to_owned(),
                        message: message.clone(),
                    };
                    context.failure_cache.insert(message_id, failure.clone());
                    context.receipts.append(&Receipt {
                        message_id,
                        kind: "provider".to_owned(),
                        provider: None,
                        tool: None,
                        failure: Some(failure),
                    });
                    context.send(
                        kind::PROVIDER_FAILED,
                        Some(message_id),
                        task_id,
                        json!({
                            "code": code,
                            "message": message,
                        }),
                    );
                }
            }
            context.finish(message_id);
        });
        Ok(())
    }

    async fn provider_operation(&self, envelope: Envelope) -> Result<()> {
        let request: ProviderRequest = serde_json::from_value(envelope.payload.clone())
            .context("некорректный provider.request")?;
        let correlation = envelope.message_id;
        let context = self.clone();
        tokio::spawn(async move {
            let response = match request.operation.as_str() {
                "models" => {
                    match context
                        .provider
                        .models_at(&request.base_url, request.api_key.as_deref())
                        .await
                    {
                        Ok(models) => ProviderResponse {
                            operation: "models".to_owned(),
                            ok: true,
                            models,
                            data: json!({}),
                            error: None,
                        },
                        Err(error) => ProviderResponse {
                            operation: "models".to_owned(),
                            ok: false,
                            models: Vec::new(),
                            data: json!({}),
                            error: Some(sanitize(&error.to_string(), &context.roots)),
                        },
                    }
                }
                "status" => {
                    match context
                        .provider
                        .status_at(&request.base_url, request.api_key.as_deref())
                        .await
                    {
                        Ok((reachable, data)) => {
                            let models: Vec<String> = data
                                .get("models")
                                .and_then(|value| value.as_array())
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                                        .collect()
                                })
                                .unwrap_or_default();
                            ProviderResponse {
                                operation: "status".to_owned(),
                                ok: reachable,
                                models,
                                data,
                                error: if reachable {
                                    None
                                } else {
                                    Some("PolyStrata недоступна".to_owned())
                                },
                            }
                        }
                        Err(error) => ProviderResponse {
                            operation: "status".to_owned(),
                            ok: false,
                            models: Vec::new(),
                            data: json!({}),
                            error: Some(sanitize(&error.to_string(), &context.roots)),
                        },
                    }
                }
                other => ProviderResponse {
                    operation: other.to_owned(),
                    ok: false,
                    models: Vec::new(),
                    data: json!({}),
                    error: Some("неизвестная операция провайдера".to_owned()),
                },
            };
            context.send(
                kind::PROVIDER_RESPONSE,
                Some(correlation),
                envelope.task_id,
                serde_json::to_value(response).unwrap_or(Value::Null),
            );
        });
        Ok(())
    }

    async fn start_tool(&self, envelope: Envelope) -> Result<()> {
        let request: ToolExecuteRequest = serde_json::from_value(envelope.payload.clone())
            .context("некорректный tool.execute")?;
        if let Some(cached) = self.tool_cache.get(&envelope.message_id) {
            self.send(
                kind::TOOL_COMPLETED,
                Some(envelope.message_id),
                envelope.task_id,
                serde_json::to_value(cached.clone())?,
            );
            return Ok(());
        }
        if let Some(failure) = self.failure_cache.get(&envelope.message_id) {
            self.send(
                kind::TOOL_FAILED,
                Some(envelope.message_id),
                envelope.task_id,
                json!({ "code": failure.code, "message": failure.message }),
            );
            return Ok(());
        }
        if self.pruned_cache.contains_key(&envelope.message_id) {
            self.send(
                kind::TOOL_FAILED,
                Some(envelope.message_id),
                envelope.task_id,
                json!({
                    "code": "AI_RECEIPT_ROTATED",
                    "message": "Операция уже исполнялась ранее; повтор запрещён после ротации receipts."
                }),
            );
            return Ok(());
        }
        if self.active.contains_key(&envelope.message_id) {
            return Ok(());
        }
        let cancel = CancellationToken::new();
        self.register(envelope.message_id, envelope.task_id, cancel.clone());
        self.send(
            kind::TOOL_ACCEPTED,
            Some(envelope.message_id),
            envelope.task_id,
            json!({ "toolId": request.tool_id }),
        );
        let context = self.clone();
        let message_id = envelope.message_id;
        let task_id = envelope.task_id;
        tokio::spawn(async move {
            let permit = context.semaphore.clone().acquire_owned().await;
            let result = match permit {
                Ok(_permit) => {
                    if request.tool_id.starts_with("mcp.") {
                        context
                            .mcp
                            .call(&request.tool_id, &request.arguments, &cancel)
                            .await
                    } else {
                        context.tools.execute(&request, &cancel).await
                    }
                }
                Err(_) => Err(anyhow::anyhow!("Runtime остановлен")),
            };
            match result {
                Ok(mut outcome) => {
                    // MCP-инструмент может менять workspace, но его протокол не
                    // возвращает usage. Измеряем сами, чтобы backend подтвердил
                    // резерв квоты фактическим состоянием.
                    if outcome.usage.is_none() && request.tool_id.starts_with("mcp.") {
                        if let Ok(workspace) =
                            filesystem::Workspace::new(&request.workspace_root, &context.roots)
                        {
                            let max_entries = context
                                .config
                                .limits
                                .max_directory_entries
                                .saturating_mul(100);
                            if let Ok((bytes, files)) =
                                filesystem::directory_usage(&workspace.root, max_entries)
                            {
                                outcome.usage = Some(WorkspaceUsage {
                                    bytes: bytes as i64,
                                    files: files as i64,
                                    checked_at: Some(chrono::Utc::now()),
                                    error: None,
                                });
                            }
                        }
                    }
                    let outcome = context.redact_outcome(outcome);
                    context.tool_cache.insert(message_id, outcome.clone());
                    context.receipts.append(&Receipt {
                        message_id,
                        kind: "tool".to_owned(),
                        provider: None,
                        tool: Some(outcome.clone()),
                        failure: None,
                    });
                    context.send(
                        kind::TOOL_COMPLETED,
                        Some(message_id),
                        task_id,
                        serde_json::to_value(outcome).unwrap_or(Value::Null),
                    );
                }
                Err(error) => {
                    let (code, message) = tool_error(&error, &cancel);
                    let message = sanitize(&message, &context.roots);
                    // Известная ошибка тоже receipt: повторная доставка не
                    // должна запускать инструмент повторно.
                    let failure = ReceiptFailure {
                        kind: "tool".to_owned(),
                        code: code.to_owned(),
                        message: message.clone(),
                    };
                    context.failure_cache.insert(message_id, failure.clone());
                    context.receipts.append(&Receipt {
                        message_id,
                        kind: "tool".to_owned(),
                        provider: None,
                        tool: None,
                        failure: Some(failure),
                    });
                    context.send(
                        kind::TOOL_FAILED,
                        Some(message_id),
                        task_id,
                        json!({
                            "code": code,
                            "message": message,
                        }),
                    );
                }
            }
            context.finish(message_id);
        });
        Ok(())
    }

    fn register(&self, message_id: Uuid, task_id: Option<Uuid>, cancel: CancellationToken) {
        self.active
            .insert(message_id, ActiveTask { task_id, cancel });
        if let Some(task_id) = task_id {
            self.task_index.entry(task_id).or_default().push(message_id);
        }
    }

    fn finish(&self, message_id: Uuid) {
        if let Some((_, active)) = self.active.remove(&message_id) {
            if let Some(task_id) = active.task_id {
                if let Some(mut entry) = self.task_index.get_mut(&task_id) {
                    entry.retain(|id| *id != message_id);
                }
            }
        }
    }

    fn cancel_message(&self, message_id: Uuid) {
        if let Some(active) = self.active.get(&message_id) {
            active.cancel.cancel();
        }
    }

    fn redact_outcome(&self, mut outcome: ToolOutcome) -> ToolOutcome {
        let secrets = [
            self.credentials.device_token.clone(),
            self.config.provider.api_key.clone(),
        ];
        let clean = |text: &str| -> String {
            let mut value = sanitize(text, &self.roots);
            for secret in &secrets {
                if secret.trim().len() >= 8 {
                    value = value.replace(secret, "<redacted>");
                }
            }
            value
        };
        if let Some(text) = &outcome.result_text {
            outcome.result_text = Some(clean(text));
        }
        let serialized = outcome.result.to_string();
        let cleaned = clean(&serialized);
        if cleaned != serialized {
            if let Ok(parsed) = serde_json::from_str::<Value>(&cleaned) {
                outcome.result = parsed;
            } else {
                outcome.result = Value::String(cleaned);
            }
        }
        outcome
    }

    async fn cancel_task(&self, task_id: Uuid, cancel_request_message_id: Uuid) {
        let ids = self
            .task_index
            .get(&task_id)
            .map(|entry| entry.clone())
            .unwrap_or_default();
        let total = ids.len();
        // Ack означает «запрос принят», а не «остановлено».
        self.send(
            kind::TASK_CANCEL_ACK,
            Some(cancel_request_message_id),
            Some(task_id),
            json!({
                "cancelRequestMessageId": cancel_request_message_id,
                "accepted": true,
                "activeExecutions": total,
            }),
        );
        for id in &ids {
            self.cancel_message(*id);
        }
        let context = self.clone();
        tokio::spawn(async move {
            // Ждём фактического завершения активных provider/tool задач.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                let remaining = context
                    .task_index
                    .get(&task_id)
                    .map(|entry| entry.len())
                    .unwrap_or(0);
                if remaining == 0 || tokio::time::Instant::now() >= deadline {
                    let confirmed = remaining == 0;
                    context.send(
                        kind::TASK_CANCELLED,
                        Some(cancel_request_message_id),
                        Some(task_id),
                        json!({
                            "taskId": task_id,
                            "confirmed": confirmed,
                            "cancelRequestMessageId": cancel_request_message_id,
                            "activeExecutions": remaining,
                            "terminatedProcesses": total.saturating_sub(remaining),
                            "reason": if confirmed {
                                "Все активные операции остановлены"
                            } else {
                                "Часть операций не завершилась за 30 секунд"
                            },
                        }),
                    );
                    if confirmed {
                        context.task_index.remove(&task_id);
                    }
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    async fn workspace_provision(&self, envelope: Envelope) -> Result<()> {
        let Some(correlation) = envelope.correlation_id else {
            return Ok(());
        };
        let request: WorkspaceProvisionRequest =
            serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
        let result = match self.provision_workspace_dir(request.workspace_id) {
            Ok(path) => WorkspaceProvisionResult {
                workspace_id: request.workspace_id,
                ok: true,
                path: Some(path),
                error: None,
            },
            Err(error) => WorkspaceProvisionResult {
                workspace_id: request.workspace_id,
                ok: false,
                path: None,
                error: Some(sanitize(&error.to_string(), &self.roots)),
            },
        };
        self.send(
            kind::WORKSPACE_PROVISION_RESULT,
            Some(correlation),
            envelope.task_id,
            serde_json::to_value(result)?,
        );
        Ok(())
    }

    /// Создаёт `<provisioningRoot>/<workspaceId>`. Имя каталога — только UUID:
    /// пользовательские строки в путь не попадают.
    fn provision_workspace_dir(&self, workspace_id: Uuid) -> Result<String> {
        if workspace_id.is_nil() {
            bail!("некорректный идентификатор workspace");
        }
        let root = self
            .provisioning_root()
            .ok_or_else(|| anyhow::anyhow!("storage.provisioning_root не настроен"))?;
        let target = root.join(workspace_id.to_string());
        if target.exists() {
            bail!("каталог workspace уже существует");
        }
        std::fs::create_dir(&target)
            .with_context(|| format!("не удалось создать {}", target.display()))?;
        let canonical = std::fs::canonicalize(&target)
            .with_context(|| format!("не удалось проверить {}", target.display()))?;
        if !canonical.starts_with(&root) {
            let _ = std::fs::remove_dir(&target);
            bail!("каталог workspace вышел за пределы provisioning root");
        }
        // Проверяем, что созданный каталог принимается как workspace.
        let workspace = match filesystem::Workspace::new(&canonical.to_string_lossy(), &self.roots)
        {
            Ok(workspace) => workspace,
            Err(error) => {
                let _ = std::fs::remove_dir(&canonical);
                return Err(error);
            }
        };
        if let Err(error) = write_workspace_marker(&workspace.root, workspace_id) {
            let _ = std::fs::remove_dir(&canonical);
            return Err(error);
        }
        Ok(canonical.to_string_lossy().to_string())
    }

    async fn workspace_delete(&self, envelope: Envelope) -> Result<()> {
        let Some(correlation) = envelope.correlation_id else {
            return Ok(());
        };
        let request: WorkspaceDeleteRequest =
            serde_json::from_value(envelope.payload.clone()).unwrap_or_default();
        let result = match self.delete_workspace_dir(request.workspace_id, request.workspace_root) {
            Ok(result) => result,
            Err(error) => WorkspaceDeleteResult {
                workspace_id: request.workspace_id,
                ok: false,
                bytes: 0,
                files: 0,
                already_absent: false,
                error: Some(sanitize(&error.to_string(), &self.roots)),
            },
        };
        self.send(
            kind::WORKSPACE_DELETE_RESULT,
            Some(correlation),
            envelope.task_id,
            serde_json::to_value(result)?,
        );
        Ok(())
    }

    /// Физическое удаление каталога workspace.
    ///
    /// Runtime не доверяет переданному пути: каталог обязан быть прямым
    /// дочерним каталогом `provisioning_root` с именем workspace UUID и
    /// маркером, созданным при provisioning. Удаление выполняется через
    /// атомарный rename в `.nexus-trash`; повторный вызов идемпотентен.
    fn delete_workspace_dir(
        &self,
        workspace_id: Uuid,
        workspace_root: Option<String>,
    ) -> Result<WorkspaceDeleteResult> {
        if workspace_id.is_nil() {
            bail!("некорректный идентификатор workspace");
        }
        let root = self
            .provisioning_root()
            .ok_or_else(|| anyhow::anyhow!("storage.provisioning_root не настроен"))?;
        let max_entries = self.config.limits.max_directory_entries.saturating_mul(100);
        delete_managed_workspace(&root, workspace_id, workspace_root.as_deref(), max_entries)
    }

    async fn workspace_usage(&self, envelope: Envelope) -> Result<()> {
        let Some(correlation) = envelope.correlation_id else {
            return Ok(());
        };
        let workspace_root = envelope
            .payload
            .get("workspaceRoot")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let workspace_id = envelope
            .payload
            .get("workspaceId")
            .and_then(|value| value.as_str())
            .and_then(|value| Uuid::parse_str(value).ok())
            .unwrap_or(Uuid::nil());
        let usage = match crate::filesystem::Workspace::new(workspace_root, &self.roots) {
            Ok(workspace) => {
                let max_entries = self.config.limits.max_directory_entries.saturating_mul(100);
                match filesystem::directory_usage(&workspace.root, max_entries) {
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
                        error: Some(sanitize(&error.to_string(), &self.roots)),
                    },
                }
            }
            Err(error) => WorkspaceUsage {
                bytes: 0,
                files: 0,
                checked_at: Some(chrono::Utc::now()),
                error: Some(sanitize(&error.to_string(), &self.roots)),
            },
        };
        self.send(
            kind::WORKSPACE_USAGE_RESULT,
            Some(correlation),
            None,
            json!({
                "workspaceId": workspace_id,
                "bytes": usage.bytes,
                "files": usage.files,
                "checkedAt": usage.checked_at,
                "error": usage.error,
            }),
        );
        Ok(())
    }

    async fn execution_status(&self, envelope: Envelope) -> Result<()> {
        let Some(correlation) = envelope.correlation_id else {
            return Ok(());
        };
        let execution_message_id = envelope
            .payload
            .get("executionMessageId")
            .and_then(|value| value.as_str())
            .and_then(|value| Uuid::parse_str(value).ok());
        let Some(execution_message_id) = execution_message_id else {
            self.send(
                kind::RUNTIME_ERROR,
                Some(envelope.message_id),
                None,
                json!({
                    "code": "AI_EXECUTION_STATUS_INVALID",
                    "message": "execution.status без executionMessageId"
                }),
            );
            return Ok(());
        };
        let result = if let Some(outcome) = self.tool_cache.get(&execution_message_id) {
            ExecutionStatusResult {
                message_id: execution_message_id,
                status: "completed".to_owned(),
                outcome: Some(outcome.clone()),
                error: None,
            }
        } else if let Some(failure) = self.failure_cache.get(&execution_message_id) {
            ExecutionStatusResult {
                message_id: execution_message_id,
                status: "failed".to_owned(),
                outcome: None,
                error: Some(format!("{}: {}", failure.code, failure.message)),
            }
        } else if self.pruned_cache.contains_key(&execution_message_id) {
            ExecutionStatusResult {
                message_id: execution_message_id,
                status: "failed".to_owned(),
                outcome: None,
                error: Some(
                    "AI_RECEIPT_ROTATED: операция уже исполнялась ранее; повтор запрещён."
                        .to_owned(),
                ),
            }
        } else if self.active.contains_key(&execution_message_id) {
            ExecutionStatusResult {
                message_id: execution_message_id,
                status: "running".to_owned(),
                outcome: None,
                error: None,
            }
        } else {
            ExecutionStatusResult {
                message_id: execution_message_id,
                status: "unknown".to_owned(),
                outcome: None,
                error: Some(
                    "Runtime не содержит receipt: результат операции неизвестен".to_owned(),
                ),
            }
        };
        self.send(
            kind::EXECUTION_STATUS_RESULT,
            Some(correlation),
            envelope.task_id,
            serde_json::to_value(result).unwrap_or(Value::Null),
        );
        Ok(())
    }
}

/// Ядро удаления, отделённое от RuntimeContext для тестируемости.
///
/// `root` — уже канонический `provisioning_root`. Runtime принимает только
/// прямой дочерний каталог с именем workspace UUID. Возвращает измеренный
/// размер до удаления и признак «уже отсутствует» для идемпотентного повтора.
fn delete_managed_workspace(
    root: &Path,
    workspace_id: Uuid,
    workspace_root: Option<&str>,
    max_entries: usize,
) -> Result<WorkspaceDeleteResult> {
    if workspace_id.is_nil() {
        bail!("некорректный идентификатор workspace");
    }
    let expected = root.join(workspace_id.to_string());
    let candidate = match workspace_root.map(str::trim) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => expected.clone(),
    };
    if !candidate.is_absolute() {
        bail!("путь workspace должен быть абсолютным");
    }
    let candidate_exists = candidate.exists();
    let canonical = if candidate_exists {
        std::fs::canonicalize(&candidate)
            .with_context(|| format!("не удалось проверить {}", candidate.display()))?
    } else {
        expected.clone()
    };
    if !same_path(&canonical, &expected) {
        bail!(
            "AI_WORKSPACE_OWNERSHIP_UNVERIFIED: каталог не является provisioning-каталогом этого workspace"
        );
    }
    if let Some(parent) = canonical.parent() {
        if !same_path(parent, root) {
            bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: каталог вне provisioning root");
        }
    }
    if candidate_exists && filesystem::is_reparse_point(&canonical)? {
        bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: workspace является symlink/junction");
    }
    if candidate_exists {
        verify_workspace_marker(&canonical, workspace_id)?;
    }

    let trash_root = root.join(TRASH_DIR);
    if !trash_root.exists() {
        std::fs::create_dir(&trash_root)
            .with_context(|| format!("не удалось создать {}", trash_root.display()))?;
    }
    let trash_canonical = std::fs::canonicalize(&trash_root)
        .with_context(|| format!("не удалось проверить {}", trash_root.display()))?;
    if !same_path(&trash_canonical, &trash_root)
        || !trash_canonical.starts_with(root)
        || same_path(&trash_canonical, root)
    {
        bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: некорректный каталог удаления");
    }
    if filesystem::is_reparse_point(&trash_canonical)? {
        bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: каталог удаления является symlink/junction");
    }
    let trash_target = trash_canonical.join(workspace_id.to_string());

    let (bytes, files) = if candidate_exists {
        filesystem::directory_usage(&canonical, max_entries).unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    let already_absent = !candidate_exists && !trash_target.exists();

    // Удаляем остаток предыдущей неудачной попытки, затем переносим текущий
    // каталог. Rename в пределах одного тома атомарен.
    if trash_target.exists() {
        if filesystem::is_reparse_point(&trash_target)? {
            bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: остаток удаления является symlink");
        }
        std::fs::remove_dir_all(&trash_target)
            .with_context(|| format!("не удалось очистить {}", trash_target.display()))?;
    }
    if candidate_exists {
        std::fs::rename(&canonical, &trash_target).with_context(|| {
            format!(
                "не удалось переместить workspace {} в каталог удаления",
                canonical.display()
            )
        })?;
    }
    if trash_target.exists() {
        std::fs::remove_dir_all(&trash_target)
            .with_context(|| format!("не удалось удалить {}", trash_target.display()))?;
    }
    if candidate.exists() {
        bail!("каталог workspace всё ещё существует после удаления");
    }
    Ok(WorkspaceDeleteResult {
        workspace_id,
        ok: true,
        bytes: bytes as i64,
        files: files as i64,
        already_absent,
        error: None,
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Создаёт маркер принадлежности workspace. Вызывается только после создания
/// пустого каталога `<provisioning_root>/<uuid>`.
fn write_workspace_marker(root: &Path, workspace_id: Uuid) -> Result<()> {
    let marker = root.join(WORKSPACE_MARKER);
    if marker.exists() {
        bail!("маркер workspace уже существует");
    }
    let payload = json!({
        "workspaceId": workspace_id,
        "createdAt": chrono::Utc::now(),
        "format": 1,
    });
    let bytes = serde_json::to_vec_pretty(&payload)?;
    std::fs::write(&marker, bytes)
        .with_context(|| format!("не удалось создать маркер {}", marker.display()))?;
    Ok(())
}

/// Проверяет маркер. Отсутствие маркера допускается только для каталогов,
/// созданных до появления маркеров: путь уже подтверждён как
/// `<provisioning_root>/<uuid>`, а маркер с чужим UUID всегда отклоняется.
fn verify_workspace_marker(root: &Path, workspace_id: Uuid) -> Result<()> {
    let marker = root.join(WORKSPACE_MARKER);
    let raw = match std::fs::read(&marker) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(
                workspace = %workspace_id,
                path = %root.display(),
                "маркер workspace отсутствует (legacy provisioning): принадлежность подтверждена путём"
            );
            return Ok(());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("не удалось прочитать маркер {}", marker.display()));
        }
    };
    let value: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("некорректный маркер {}", marker.display()))?;
    let marked = value
        .get("workspaceId")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok());
    if marked != Some(workspace_id) {
        bail!("AI_WORKSPACE_OWNERSHIP_UNVERIFIED: маркер принадлежит другому workspace");
    }
    Ok(())
}

fn provider_error(error: &anyhow::Error) -> (&'static str, String) {
    let message = error.to_string();
    if message.contains("отменена") {
        ("AI_PROVIDER_CANCELLED", message)
    } else if message.contains("timeout") || message.contains("превысил") {
        ("AI_PROVIDER_TIMEOUT", message)
    } else if message.contains("HTTP") {
        ("AI_PROVIDER_HTTP_ERROR", message)
    } else if message.contains("JSON") || message.contains("choices") {
        ("AI_PROVIDER_INVALID_RESPONSE", message)
    } else {
        ("AI_PROVIDER_ERROR", message)
    }
}

fn tool_error(error: &anyhow::Error, cancel: &CancellationToken) -> (&'static str, String) {
    let message = error.to_string();
    if cancel.is_cancelled() || message.contains("отменено") {
        ("AI_TOOL_CANCELLED", message)
    } else if message.contains("схем") {
        ("AI_TOOL_INVALID_ARGUMENTS", message)
    } else if message.contains("разрешён") || message.contains("зарегистрирован")
    {
        ("AI_TOOL_NOT_ALLOWED", message)
    } else if message.contains("symlink")
        || message.contains("workspace")
        || message.contains("путь")
    {
        ("AI_WORKSPACE_PATH_DENIED", message)
    } else {
        ("AI_TOOL_FAILED", message)
    }
}

fn sanitize(message: &str, roots: &[PathBuf]) -> String {
    let mut value = message.to_owned();
    for root in roots {
        value = value.replace(&root.to_string_lossy().to_string(), "<workspace>");
    }
    value.chars().take(1000).collect()
}

#[cfg(test)]
mod workspace_lifecycle_tests {
    use super::*;

    fn prepared() -> (tempfile::TempDir, PathBuf, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let workspace_id = Uuid::new_v4();
        let workspace = root.join(workspace_id.to_string());
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("index.txt"), b"hello").unwrap();
        write_workspace_marker(&workspace, workspace_id).unwrap();
        (dir, root, workspace_id)
    }

    #[test]
    fn deletes_only_managed_workspace_and_is_idempotent() {
        let (_dir, root, workspace_id) = prepared();
        let workspace = root.join(workspace_id.to_string());
        let first = delete_managed_workspace(
            &root,
            workspace_id,
            Some(&workspace.to_string_lossy()),
            10_000,
        )
        .unwrap();
        assert!(first.ok);
        assert!(!first.already_absent);
        assert!(first.files >= 2);
        assert!(!workspace.exists());
        let second = delete_managed_workspace(
            &root,
            workspace_id,
            Some(&workspace.to_string_lossy()),
            10_000,
        )
        .unwrap();
        assert!(second.ok);
        assert!(second.already_absent);
    }

    #[test]
    fn rejects_path_outside_provisioning_root() {
        let (dir, root, workspace_id) = prepared();
        let outside = dir.path().join("other");
        std::fs::create_dir(&outside).unwrap();
        let error = delete_managed_workspace(
            &root,
            workspace_id,
            Some(&outside.to_string_lossy()),
            10_000,
        )
        .unwrap_err();
        assert!(error.to_string().contains("OWNERSHIP_UNVERIFIED"));
        assert!(outside.exists());
    }

    #[test]
    fn rejects_foreign_marker() {
        let (_dir, root, workspace_id) = prepared();
        let workspace = root.join(workspace_id.to_string());
        std::fs::write(
            workspace.join(WORKSPACE_MARKER),
            format!(r#"{{"workspaceId":"{}"}}"#, Uuid::new_v4()),
        )
        .unwrap();
        let error = delete_managed_workspace(&root, workspace_id, None, 10_000).unwrap_err();
        assert!(error.to_string().contains("OWNERSHIP_UNVERIFIED"));
        assert!(workspace.exists());
    }

    #[test]
    fn accepts_legacy_workspace_without_marker_by_path_convention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let workspace_id = Uuid::new_v4();
        let workspace = root.join(workspace_id.to_string());
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), b"legacy").unwrap();
        delete_managed_workspace(&root, workspace_id, None, 10_000).unwrap();
        assert!(!workspace.exists());
    }

    #[test]
    fn marker_roundtrip() {
        let (_dir, root, workspace_id) = prepared();
        verify_workspace_marker(&root.join(workspace_id.to_string()), workspace_id).unwrap();
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::config::{LimitsConfig, ProviderConfig, ServerConfig, StorageConfig};
    use tokio_tungstenite::accept_async;

    fn test_config(port: u16, root: &std::path::Path, state: &std::path::Path) -> RuntimeConfig {
        RuntimeConfig {
            server: ServerConfig {
                url: format!("ws://127.0.0.1:{port}/api/v1/ai/runtime/connect"),
                enroll_url: String::new(),
                name: "TEST".into(),
                allow_insecure_ws: true,
                heartbeat_seconds: 20,
                reconnect_min_seconds: 1,
                reconnect_max_seconds: 2,
            },
            storage: StorageConfig {
                state_dir: state.to_string_lossy().to_string(),
                allowed_roots: vec![root.parent().unwrap().to_string_lossy().to_string()],
                ..Default::default()
            },
            provider: ProviderConfig {
                base_url: "http://127.0.0.1:9/v1".into(),
                timeout_seconds: 5,
                connect_timeout_seconds: 1,
                ..Default::default()
            },
            limits: LimitsConfig::default(),
            programs: Vec::new(),
            logging: Default::default(),
        }
    }

    #[tokio::test]
    async fn executes_tool_over_websocket_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = dir.path().canonicalize().unwrap();
        let workspace_root = allowed.join("workspace");
        std::fs::create_dir_all(&workspace_root).unwrap();
        let state = allowed.join("state");
        std::fs::create_dir_all(&state).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = test_config(port, &workspace_root, &state);
        let credentials = DeviceCredentials {
            node_id: Uuid::new_v4(),
            device_token: "nxa_test".to_owned(),
            server_url: config.server.url.clone(),
            enrolled_at: chrono::Utc::now(),
            runtime_name: "TEST".to_owned(),
        };
        let workspace_id = Uuid::new_v4();
        let (server_done_tx, server_done_rx) = tokio::sync::oneshot::channel();
        let root_for_server = workspace_root.clone();
        let node_id = credentials.node_id;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let hello = socket.next().await.unwrap().unwrap();
            let hello: Envelope = serde_json::from_str(hello.to_text().unwrap()).unwrap();
            assert_eq!(hello.kind, kind::HELLO);
            assert_eq!(hello.node_id, node_id);
            let message_id = Uuid::new_v4();
            let command = Envelope::new(
                kind::TOOL_EXECUTE,
                node_id,
                Some(message_id),
                None,
                Some(1),
                json!({
                    "toolId": "fs.write",
                    "version": 1,
                    "arguments": { "path": "a.txt", "content": "first", "overwrite": true },
                    "workspaceId": workspace_id,
                    "workspaceRoot": root_for_server.to_string_lossy(),
                    "timeoutSeconds": 10,
                    "maxOutputBytes": 65536,
                    "workspaceMaxFileBytes": 1024 * 1024,
                    "workspaceQuotaBytes": 10 * 1024 * 1024,
                    "workspaceMaxFiles": 100
                }),
            );
            socket
                .send(Message::Text(
                    serde_json::to_string(&command).unwrap().into(),
                ))
                .await
                .unwrap();
            let mut completed = None;
            for _ in 0..10 {
                let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Message::Text(text) = message {
                    let envelope: Envelope = serde_json::from_str(&text).unwrap();
                    if envelope.kind == kind::TOOL_COMPLETED {
                        completed = Some(envelope);
                        break;
                    }
                }
            }
            assert!(completed.is_some(), "tool.completed не получен");
            assert_eq!(
                std::fs::read_to_string(root_for_server.join("a.txt")).unwrap(),
                "first"
            );

            // Повторная доставка того же message_id не должна выполнять write.
            let mut duplicate = command.clone();
            duplicate.payload["arguments"]["content"] = json!("second");
            socket
                .send(Message::Text(
                    serde_json::to_string(&duplicate).unwrap().into(),
                ))
                .await
                .unwrap();
            let mut duplicate_completed = None;
            for _ in 0..10 {
                let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Message::Text(text) = message {
                    let envelope: Envelope = serde_json::from_str(&text).unwrap();
                    if envelope.kind == kind::TOOL_COMPLETED {
                        duplicate_completed = Some(envelope);
                        break;
                    }
                }
            }
            assert!(duplicate_completed.is_some(), "повторный ответ не получен");
            assert_eq!(
                std::fs::read_to_string(root_for_server.join("a.txt")).unwrap(),
                "first",
                "deduplication по messageId должна предотвратить повторный write"
            );
            let _ = server_done_tx.send(());
        });

        let config = Arc::new(config);
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let context = RuntimeContext {
            config: config.clone(),
            credentials,
            provider: Arc::new(ProviderClient::new(&config.provider).unwrap()),
            tools: Arc::new(ToolExecutor::new(
                &config.limits,
                vec![allowed.clone()],
                ProgramRegistry::new(&config.programs, &config.limits),
            )),
            mcp: Arc::new(McpRegistry::new()),
            roots: Arc::new(vec![allowed.clone()]),
            outbound,
            sequence: Arc::new(AtomicI64::new(0)),
            active: Arc::new(DashMap::new()),
            task_index: Arc::new(DashMap::new()),
            provider_cache: Arc::new(DashMap::new()),
            tool_cache: Arc::new(DashMap::new()),
            failure_cache: Arc::new(DashMap::new()),
            pruned_cache: Arc::new(DashMap::new()),
            receipts: Arc::new(ReceiptStore::new(
                state.join("receipts.jsonl"),
                config.receipts_max_bytes(),
                config.receipts_max_archives(),
                config.receipts_load_limit(),
            )),
            semaphore: Arc::new(Semaphore::new(2)),
            last_status: Arc::new(RwLock::new(PolystrataStatus::default())),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let runtime = tokio::spawn(async move {
            let _ = connect_once(context, outbound_rx, Some(shutdown_rx)).await;
        });

        tokio::time::timeout(Duration::from_secs(20), server_done_rx)
            .await
            .expect("серверный сценарий не завершился")
            .unwrap();
        let _ = shutdown_tx.send(true);
        runtime.abort();
        let _ = server.await;
    }
}
