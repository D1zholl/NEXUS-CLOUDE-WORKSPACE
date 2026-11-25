//! Клиент MCP (Streamable HTTP / JSON-RPC 2.0) на стороне Runtime.
//!
//! Runtime получает список разрешённых соединений только от аутентифицированного
//! Backend. Endpoint повторно проверяется на SSRF при configure и discovery.
//! MCP-результаты считаются недоверенными и ограничиваются по размеру.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::protocol::{McpConnectionConfig, McpDiscoveredTool, McpDiscoveryResult, ToolOutcome};

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RESULT_BYTES: usize = 262_144;

#[derive(Clone)]
struct Connection {
    config: McpConnectionConfig,
    client: reqwest::Client,
}

#[derive(Default, Clone)]
struct Session {
    initialized: bool,
    session_id: Option<String>,
    protocol_version: String,
    server_name: String,
}

pub struct McpRegistry {
    connections: RwLock<HashMap<Uuid, Connection>>,
    sessions: Mutex<HashMap<Uuid, Session>>,
    tools: RwLock<HashMap<String, (Uuid, String, Value)>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            tools: RwLock::new(HashMap::new()),
        }
    }

    pub async fn configure(&self, configs: Vec<McpConnectionConfig>) {
        let mut map = HashMap::new();
        for config in configs {
            match Url::parse(&config.endpoint) {
                Ok(url) => {
                    if let Err(error) = validate_target(&url, config.allow_private_network).await {
                        tracing::warn!(
                            connection = %config.id,
                            %error,
                            "MCP endpoint отклонён Runtime (SSRF)"
                        );
                        continue;
                    }
                    let client = match reqwest::Client::builder()
                        .connect_timeout(Duration::from_secs(10))
                        .timeout(CALL_TIMEOUT)
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                    {
                        Ok(client) => client,
                        Err(error) => {
                            tracing::warn!(%error, "не удалось создать MCP HTTP client");
                            continue;
                        }
                    };
                    map.insert(config.id, Connection { config, client });
                }
                Err(error) => {
                    tracing::warn!(connection = %config.id, %error, "некорректный MCP endpoint");
                }
            }
        }
        *self.connections.write().await = map;
        self.sessions.lock().await.clear();
        // Найденные schema сохраняем: backend повторно присылает configure на
        // каждом reconnect, и повторный discovery не должен быть обязательным
        // для продолжения уже разрешённых вызовов.
    }

    pub async fn discover(&self, connection_id: Uuid) -> McpDiscoveryResult {
        let mut result = McpDiscoveryResult {
            connection_id,
            ok: false,
            tools: Vec::new(),
            error: None,
            server_name: String::new(),
            protocol_version: String::new(),
        };
        let Some(connection) = self.connection(connection_id).await else {
            result.error = Some("MCP-соединение не настроено на Runtime".to_owned());
            return result;
        };
        match self.initialize(connection_id, &connection).await {
            Ok(session) => {
                result.server_name = session.server_name.clone();
                result.protocol_version = session.protocol_version.clone();
            }
            Err(error) => {
                result.error = Some(truncate(&error.to_string()));
                return result;
            }
        }
        match self.list_tools(connection_id, &connection).await {
            Ok(tools) => {
                let mut registry = self.tools.write().await;
                for tool in &tools {
                    registry.insert(
                        format!("{connection_id}:{}", tool.name),
                        (connection_id, tool.name.clone(), tool.input_schema.clone()),
                    );
                }
                result.tools = tools;
                result.ok = true;
            }
            Err(error) => {
                result.error = Some(truncate(&error.to_string()));
            }
        }
        result
    }

    pub async fn call(
        &self,
        tool_id: &str,
        arguments: &Value,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ToolOutcome> {
        let mut parts = tool_id.splitn(3, '.');
        if parts.next() != Some("mcp") {
            bail!("некорректный MCP tool id");
        }
        let connection_id = parts
            .next()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| anyhow::anyhow!("некорректный MCP connection id"))?;
        let tool_name = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("некорректное имя MCP tool"))?;
        let Some(connection) = self.connection(connection_id).await else {
            bail!("MCP-соединение не настроено на Runtime");
        };
        if !self
            .sessions
            .lock()
            .await
            .get(&connection_id)
            .map(|s| s.initialized)
            .unwrap_or(false)
        {
            self.initialize(connection_id, &connection).await?;
        }
        if let Some((_, _, schema)) = self
            .tools
            .read()
            .await
            .get(&format!("{connection_id}:{tool_name}"))
            .cloned()
        {
            let validator = jsonschema::validator_for(&schema)
                .map_err(|error| anyhow::anyhow!("некорректная MCP JSON Schema: {error}"))?;
            if let Some(error) = validator.iter_errors(arguments).next() {
                bail!("аргументы MCP tool не соответствуют схеме: {error}");
            }
        }
        let session_id = self
            .sessions
            .lock()
            .await
            .get(&connection_id)
            .and_then(|session| session.session_id.clone());
        let body = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": tool_name, "arguments": arguments },
        });
        let (_, value) = tokio::select! {
            _ = cancel.cancelled() => bail!("выполнение MCP tool отменено"),
            result = self.post(&connection, session_id.as_deref(), body, CALL_TIMEOUT) => result?,
        };
        if let Some(error) = value.get("error") {
            bail!("MCP error: {}", truncate(&error.to_string()));
        }
        let result = value.get("result").cloned().unwrap_or(Value::Null);
        let is_error = result
            .get("isError")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let text = mcp_content_text(&result);
        if is_error {
            bail!("MCP tool вернул ошибку: {}", truncate(&text));
        }
        let serialized = serde_json::to_string(&result).unwrap_or_default();
        Ok(ToolOutcome {
            result,
            result_text: Some(if text.is_empty() {
                serialized.chars().take(MAX_RESULT_BYTES).collect()
            } else {
                text.chars().take(MAX_RESULT_BYTES).collect()
            }),
            exit_code: None,
            duration_ms: 0,
            usage: None,
            changed_files: Vec::new(),
            diff: None,
            snapshots: Vec::new(),
        })
    }

    async fn connection(&self, id: Uuid) -> Option<Connection> {
        self.connections.read().await.get(&id).cloned()
    }

    async fn initialize(&self, id: Uuid, connection: &Connection) -> Result<Session> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "nexus-ai-runtime",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });
        let (session_id, value) = self.post(connection, None, body, INIT_TIMEOUT).await?;
        if let Some(error) = value.get("error") {
            bail!("MCP initialize error: {}", truncate(&error.to_string()));
        }
        let result = value.get("result").cloned().unwrap_or(Value::Null);
        let protocol_version = result
            .get("protocolVersion")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();
        let server_name = result
            .get("serverInfo")
            .and_then(|value| value.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();
        let session = Session {
            initialized: true,
            session_id: session_id.clone(),
            protocol_version,
            server_name,
        };
        self.sessions.lock().await.insert(id, session.clone());
        // notifications/initialized не требует ответа.
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        let _ = self
            .post(
                connection,
                session_id.as_deref(),
                notification,
                INIT_TIMEOUT,
            )
            .await;
        Ok(session)
    }

    async fn list_tools(
        &self,
        id: Uuid,
        connection: &Connection,
    ) -> Result<Vec<McpDiscoveredTool>> {
        let session_id = self
            .sessions
            .lock()
            .await
            .get(&id)
            .and_then(|session| session.session_id.clone());
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let body = match &cursor {
                Some(cursor) => json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list",
                    "params": { "cursor": cursor },
                }),
                None => json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list",
                    "params": {},
                }),
            };
            let (_, value) = self
                .post(connection, session_id.as_deref(), body, INIT_TIMEOUT)
                .await?;
            if let Some(error) = value.get("error") {
                bail!("MCP tools/list error: {}", truncate(&error.to_string()));
            }
            let result = value.get("result").cloned().unwrap_or(Value::Null);
            if let Some(items) = result.get("tools").and_then(|value| value.as_array()) {
                for item in items {
                    let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    let description = item
                        .get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(1000)
                        .collect::<String>();
                    let schema = item
                        .get("inputSchema")
                        .cloned()
                        .filter(|value| value.is_object())
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                    tools.push(McpDiscoveredTool {
                        name: name.chars().take(120).collect(),
                        description,
                        input_schema: schema,
                    });
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    async fn post(
        &self,
        connection: &Connection,
        session_id: Option<&str>,
        body: Value,
        timeout: Duration,
    ) -> Result<(Option<String>, Value)> {
        let mut request = connection
            .client
            .post(&connection.config.endpoint)
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(secret) = connection
            .config
            .secret
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            request = request.bearer_auth(secret.trim());
        }
        if let Some(session_id) = session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        let response = request.send().await.context("MCP endpoint недоступен")?;
        let status = response.status();
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let text = response.text().await.unwrap_or_default();
        if text.len() > MAX_RESULT_BYTES * 4 {
            bail!("MCP response слишком большой");
        }
        if !status.is_success() && !text.trim().is_empty() {
            bail!("MCP HTTP {}: {}", status.as_u16(), truncate(&text));
        }
        if text.trim().is_empty() {
            // Уведомления могут получать 202 без тела.
            if status.is_success() {
                return Ok((session, Value::Null));
            }
            bail!("MCP HTTP {} без тела ответа", status.as_u16());
        }
        let value = if content_type.contains("text/event-stream") {
            parse_sse_json(&text)?
        } else {
            serde_json::from_str::<Value>(&text)
                .with_context(|| format!("некорректный JSON MCP: {}", truncate(&text)))?
        };
        Ok((session, value))
    }
}

fn parse_sse_json(text: &str) -> Result<Value> {
    let mut last: Option<Value> = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            last = Some(value);
        }
    }
    last.ok_or_else(|| anyhow::anyhow!("MCP SSE-ответ не содержит JSON"))
}

fn mcp_content_text(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(|value| value.as_array()) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for item in items {
        if item.get("type").and_then(|value| value.as_str()) == Some("text") {
            if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                parts.push(text.to_owned());
            }
        }
    }
    parts.join("\n").chars().take(MAX_RESULT_BYTES).collect()
}

fn truncate(text: &str) -> String {
    text.chars().take(500).collect()
}

fn is_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| {
                        v4.is_loopback()
                            || v4.is_private()
                            || v4.is_link_local()
                            || v4.is_unspecified()
                    })
                    .unwrap_or(false)
        }
    }
}

/// Повторная SSRF-проверка endpoint на стороне Runtime.
pub async fn validate_target(url: &Url, allow_private_network: bool) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("MCP endpoint должен использовать http или https");
    }
    if url.username() != "" || url.password().is_some() {
        bail!("MCP endpoint не должен содержать учётные данные");
    }
    let Some(host) = url.host_str() else {
        bail!("MCP endpoint без host");
    };
    let port = url.port_or_known_default().unwrap_or(80);
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if is_forbidden(ip) && !allow_private_network {
            bail!("MCP endpoint указывает на внутренний адрес");
        }
        return Ok(());
    }
    if host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host.eq_ignore_ascii_case("metadata.google.internal")
    {
        if !allow_private_network {
            bail!("MCP endpoint указывает на localhost/internal host");
        }
        return Ok(());
    }
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("не удалось разрешить MCP host {host}"))?
        .collect();
    if addresses.is_empty() {
        bail!("MCP host не разрешается в IP");
    }
    if !allow_private_network && addresses.iter().any(|addr| is_forbidden(addr.ip())) {
        bail!("MCP endpoint разрешается во внутренний адрес");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocks_internal_targets_by_default() {
        let url = Url::parse("http://127.0.0.1:3000/mcp").unwrap();
        assert!(validate_target(&url, false).await.is_err());
        assert!(validate_target(&url, true).await.is_ok());
        let url = Url::parse("http://169.254.169.254/latest").unwrap();
        assert!(validate_target(&url, false).await.is_err());
    }

    #[test]
    fn parses_sse_json_tool_result() {
        let text =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n\n";
        let value = parse_sse_json(text).unwrap();
        assert_eq!(value["id"], 2);
    }

    #[test]
    fn extracts_text_content() {
        let value = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" }
            ]
        });
        assert_eq!(mcp_content_text(&value), "hello\nworld");
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut chunk).await.unwrap();
            if read == 0 {
                return String::new();
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while buffer.len() < header_end + length {
            let read = stream.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8_lossy(&buffer[header_end..]).to_string()
    }

    #[tokio::test]
    async fn discovers_and_calls_tools_over_http_jsonrpc() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = read_http_request(&mut stream).await;
                let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let method = request
                    .get("method")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "serverInfo": { "name": "mock-mcp", "version": "1" },
                            "capabilities": { "tools": {} }
                        }
                    }),
                    "tools/list" => json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": {
                            "tools": [{
                                "name": "echo",
                                "description": "Echo text",
                                "inputSchema": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": { "text": { "type": "string" } },
                                    "required": ["text"]
                                }
                            }]
                        }
                    }),
                    "tools/call" => json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": {
                            "content": [{ "type": "text", "text": "echo: hello" }],
                            "isError": false
                        }
                    }),
                    _ => json!({ "jsonrpc": "2.0", "id": request["id"], "result": {} }),
                };
                let payload = serde_json::to_string(&response).unwrap();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: test-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let registry = McpRegistry::new();
        let connection_id = Uuid::new_v4();
        registry
            .configure(vec![McpConnectionConfig {
                id: connection_id,
                name: "mock".to_owned(),
                endpoint: format!("http://{address}/mcp"),
                secret: None,
                allow_private_network: true,
            }])
            .await;
        let discovery = registry.discover(connection_id).await;
        assert!(discovery.ok, "{discovery:?}");
        assert_eq!(discovery.tools.len(), 1);
        assert_eq!(discovery.tools[0].name, "echo");
        assert_eq!(discovery.server_name, "mock-mcp");

        let outcome = registry
            .call(
                &format!("mcp.{connection_id}.echo"),
                &json!({ "text": "hello" }),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome.result_text.as_deref(), Some("echo: hello"));
        let _ = server.await;
    }
}
