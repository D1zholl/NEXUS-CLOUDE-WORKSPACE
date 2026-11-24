//! OpenAI-compatible клиент локальной PolyStrata.
//!
//! Поддерживает `/models`, `/health`, `/status` и `/chat/completions` с SSE.
//! Форматы проверяются, а не предполагаются «полностью OpenAI-совместимыми».

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::config::ProviderConfig;
use crate::protocol::{
    ChatMessage, ProviderChatCompleted, ProviderChatRequest, ToolCall, ToolCallFunction,
};

pub struct ProviderClient {
    client: Client,
    base_url: String,
    api_key: String,
    timeout_seconds: u64,
}

impl ProviderClient {
    pub fn new(config: &ProviderConfig) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(config.connect_timeout_seconds.max(1)))
            .timeout(Duration::from_secs(config.timeout_seconds.max(5)))
            .build()
            .context("не удалось создать HTTP-клиент PolyStrata")?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            api_key: config.api_key.clone(),
            timeout_seconds: config.timeout_seconds,
        })
    }

    fn endpoint(&self, base_url: &str, path: &str) -> String {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    pub async fn models_at(&self, base_url: &str, api_key: Option<&str>) -> Result<Vec<String>> {
        let key = api_key
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.api_key);
        let response = self
            .client
            .get(self.endpoint(base_url, "models"))
            .timeout(Duration::from_secs(20))
            .bearer_auth_optional(key)
            .send()
            .await
            .context("PolyStrata /models недоступен")?;
        if !response.status().is_success() {
            bail!("PolyStrata /models вернул HTTP {}", response.status());
        }
        let value: Value = response.json().await.context("некорректный JSON /models")?;
        Ok(parse_models(&value))
    }

    pub async fn status(&self) -> Result<(bool, Value)> {
        self.status_at(&self.base_url, None).await
    }

    pub async fn status_at(&self, base_url: &str, api_key: Option<&str>) -> Result<(bool, Value)> {
        let key = api_key
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.api_key);
        let mut data = json!({});
        let mut reachable = false;
        for path in ["health", "status"] {
            match self
                .client
                .get(self.endpoint(base_url, path))
                .timeout(Duration::from_secs(10))
                .bearer_auth_optional(key)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    data[path] = json!({
                        "httpStatus": status.as_u16(),
                        "body": truncate_value(&parsed, 2000),
                    });
                    if status.is_success() {
                        reachable = true;
                    }
                }
                Err(error) => {
                    data[path] = json!({ "error": error.to_string() });
                }
            }
        }
        let models = self.models_at(base_url, api_key).await.unwrap_or_default();
        if !models.is_empty() {
            reachable = true;
        }
        data["models"] = json!(models);
        Ok((reachable, data))
    }

    pub async fn chat<F>(
        &self,
        request: &ProviderChatRequest,
        api_key: Option<&str>,
        cancel: &CancellationToken,
        mut on_delta: F,
    ) -> Result<ProviderChatCompleted>
    where
        F: FnMut(&str),
    {
        let base_url = if request.base_url.trim().is_empty() {
            self.base_url.clone()
        } else {
            request.base_url.trim_end_matches('/').to_owned()
        };
        let key = api_key
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.api_key.clone());
        let timeout = if request.timeout_seconds > 0 {
            request.timeout_seconds.clamp(5, 1800) as u64
        } else {
            self.timeout_seconds.max(5)
        };
        let mut body = json!({
            "model": request.model,
            "messages": messages_json(&request.messages),
            "stream": request.stream,
        });
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if !request.tools.is_empty() {
            body["tools"] = tools_json(request);
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => bail!("генерация отменена"),
            response = self
                .client
                .post(self.endpoint(&base_url, "chat/completions"))
                .timeout(if request.stream {
                    // SSE-поток контролируется внутренним deadline; общий timeout
                    // запроса должен быть заведомо больше, иначе долгая генерация
                    // обрывается на полуслове.
                    Duration::from_secs(timeout.saturating_add(30))
                } else {
                    Duration::from_secs(timeout)
                })
                .bearer_auth_optional(&key)
                .header(
                    "Accept",
                    if request.stream {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .json(&body)
                .send() => response,
        }
        .context("PolyStrata /chat/completions недоступен")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let body = body.chars().take(300).collect::<String>();
            if body.trim().is_empty() {
                bail!("PolyStrata /chat/completions вернул HTTP {status}");
            }
            bail!("PolyStrata /chat/completions вернул HTTP {status}: {body}");
        }
        if !request.stream {
            let value: Value = tokio::select! {
                _ = cancel.cancelled() => bail!("генерация отменена"),
                value = response.json() => value.context("некорректный JSON Chat Completions")?,
            };
            return parse_completion(&value);
        }
        stream_completion(response, cancel, &mut on_delta, timeout).await
    }
}

trait BearerAuthOptional {
    fn bearer_auth_optional(self, key: &str) -> Self;
}

impl BearerAuthOptional for reqwest::RequestBuilder {
    fn bearer_auth_optional(self, key: &str) -> Self {
        if key.trim().is_empty() {
            self
        } else {
            self.bearer_auth(key.trim())
        }
    }
}

fn truncate_value(value: &Value, max: usize) -> Value {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.len() <= max {
        return value.clone();
    }
    Value::String(format!("{}…", &text[..max]))
}

pub fn parse_models(value: &Value) -> Vec<String> {
    let mut models = Vec::new();
    if let Some(items) = value.get("data").and_then(|value| value.as_array()) {
        for item in items {
            if let Some(id) = item.get("id").and_then(|value| value.as_str()) {
                models.push(id.to_owned());
            } else if let Some(id) = item.as_str() {
                models.push(id.to_owned());
            }
        }
    } else if let Some(items) = value.get("models").and_then(|value| value.as_array()) {
        for item in items {
            if let Some(name) = item.get("name").and_then(|value| value.as_str()) {
                models.push(name.to_owned());
            } else if let Some(id) = item.get("id").and_then(|value| value.as_str()) {
                models.push(id.to_owned());
            }
        }
    } else if let Some(items) = value.as_array() {
        for item in items {
            if let Some(id) = item.as_str() {
                models.push(id.to_owned());
            } else if let Some(id) = item.get("id").and_then(|value| value.as_str()) {
                models.push(id.to_owned());
            }
        }
    }
    models.sort();
    models.dedup();
    models
}

fn messages_json(messages: &[ChatMessage]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                let mut object = serde_json::Map::new();
                object.insert("role".to_owned(), json!(message.role));
                if let Some(content) = &message.content {
                    object.insert("content".to_owned(), json!(content));
                }
                if let Some(name) = &message.name {
                    object.insert("name".to_owned(), json!(name));
                }
                if let Some(tool_call_id) = &message.tool_call_id {
                    object.insert("tool_call_id".to_owned(), json!(tool_call_id));
                }
                if !message.tool_calls.is_empty() {
                    object.insert(
                        "tool_calls".to_owned(),
                        tool_calls_json(&message.tool_calls),
                    );
                }
                Value::Object(object)
            })
            .collect(),
    )
}

fn tools_json(request: &ProviderChatRequest) -> Value {
    Value::Array(
        request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.function.name,
                        "description": tool.function.description,
                        "parameters": tool.function.parameters,
                    }
                })
            })
            .collect(),
    )
}

fn tool_calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    }
                })
            })
            .collect(),
    )
}

fn parse_completion(value: &Value) -> Result<ProviderChatCompleted> {
    let choice = value
        .get("choices")
        .and_then(|value| value.as_array())
        .and_then(|choices| choices.first())
        .ok_or_else(|| anyhow::anyhow!("PolyStrata вернула ответ без choices"))?;
    let message = choice
        .get("message")
        .ok_or_else(|| anyhow::anyhow!("PolyStrata вернула ответ без message"))?;
    let content = message
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|value| value.as_array()) {
        for (index, call) in calls.iter().enumerate() {
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let name = function
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_owned();
            let arguments = function
                .get("arguments")
                .and_then(|value| value.as_str())
                .unwrap_or("{}")
                .to_owned();
            tool_calls.push(ToolCall {
                id: call
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("call_{index}")),
                kind: "function".to_owned(),
                function: ToolCallFunction { name, arguments },
            });
        }
    }
    Ok(ProviderChatCompleted {
        content,
        tool_calls,
        usage: value.get("usage").cloned().unwrap_or_else(|| json!({})),
        finish_reason: choice
            .get("finish_reason")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
    })
}

async fn stream_completion<F>(
    response: reqwest::Response,
    cancel: &CancellationToken,
    on_delta: &mut F,
    timeout_seconds: u64,
) -> Result<ProviderChatCompleted>
where
    F: FnMut(&str),
{
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut content = String::new();
    let mut tool_calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    let mut usage = json!({});
    let mut finish_reason: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds + 10);

    loop {
        if cancel.is_cancelled() {
            bail!("генерация отменена");
        }
        let chunk = tokio::select! {
            _ = cancel.cancelled() => bail!("генерация отменена"),
            _ = tokio::time::sleep_until(deadline) => bail!("SSE-поток PolyStrata превысил timeout"),
            chunk = stream.next() => match chunk {
                Some(chunk) => chunk.context("ошибка чтения SSE-потока PolyStrata")?,
                None => break,
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(position) = buffer.find('\n') {
            let line: String = buffer.drain(..=position).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                return Ok(ProviderChatCompleted {
                    content,
                    tool_calls: collected_calls(tool_calls),
                    usage,
                    finish_reason,
                });
            }
            let value: Value = match serde_json::from_str(data) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if let Some(error) = value.get("error") {
                bail!("PolyStrata error: {}", truncate_value(error, 300));
            }
            if let Some(used) = value.get("usage") {
                if !used.is_null() {
                    usage = used.clone();
                }
            }
            let Some(choice) = value
                .get("choices")
                .and_then(|value| value.as_array())
                .and_then(|choices| choices.first())
            else {
                continue;
            };
            if let Some(reason) = choice.get("finish_reason").and_then(|value| value.as_str()) {
                finish_reason = Some(reason.to_owned());
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(text) = delta.get("content").and_then(|value| value.as_str()) {
                if !text.is_empty() {
                    content.push_str(text);
                    on_delta(text);
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|value| value.as_array()) {
                for call in calls {
                    let index = call
                        .get("index")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as usize;
                    let entry = tool_calls
                        .entry(index)
                        .or_insert_with(|| (String::new(), String::new(), String::new()));
                    if let Some(id) = call.get("id").and_then(|value| value.as_str()) {
                        entry.0.push_str(id);
                    }
                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(|value| value.as_str()) {
                            entry.1.push_str(name);
                        }
                        if let Some(arguments) =
                            function.get("arguments").and_then(|value| value.as_str())
                        {
                            entry.2.push_str(arguments);
                        }
                    }
                }
            }
        }
    }
    Ok(ProviderChatCompleted {
        content,
        tool_calls: collected_calls(tool_calls),
        usage,
        finish_reason,
    })
}

fn collected_calls(collected: BTreeMap<usize, (String, String, String)>) -> Vec<ToolCall> {
    collected
        .into_iter()
        .map(|(index, (id, name, arguments))| ToolCall {
            id: if id.is_empty() {
                format!("call_{index}")
            } else {
                id
            },
            kind: "function".to_owned(),
            function: ToolCallFunction {
                name,
                arguments: if arguments.is_empty() {
                    "{}".to_owned()
                } else {
                    arguments
                },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_lists_in_several_shapes() {
        assert_eq!(
            parse_models(&json!({"data":[{"id":"a"},{"id":"b"},{"id":"a"}]})),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert_eq!(parse_models(&json!([{"id":"x"}])), vec!["x".to_owned()]);
        assert_eq!(
            parse_models(&json!({"models":[{"name":"n"}]})),
            vec!["n".to_owned()]
        );
    }

    #[test]
    fn parses_completion_with_tool_calls() {
        let value = json!({
            "choices": [{
                "message": {
                    "content": "hi",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "fs_read", "arguments": "{\"path\":\"a\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"total_tokens": 10}
        });
        let parsed = parse_completion(&value).unwrap();
        assert_eq!(parsed.content, "hi");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].function.name, "fs_read");
        assert_eq!(parsed.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn serializes_openai_snake_case() {
        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: None,
            name: None,
            tool_call_id: Some("call_1".into()),
            tool_calls: vec![],
        }];
        let value = messages_json(&messages);
        assert_eq!(value[0]["tool_call_id"], "call_1");
        assert_eq!(value[0]["role"], "assistant");
    }

    #[tokio::test]
    async fn parses_sse_stream_with_tool_calls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 8192];
            let _ = stream.read(&mut buffer).await;
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"fs_read\",\"arguments\":\"{\\\"path\\\":\"}}]},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"total_tokens\":7}}\n\n",
                "data: [DONE]\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });

        let config = ProviderConfig {
            base_url: format!("http://{addr}/v1"),
            api_key: String::new(),
            timeout_seconds: 10,
            connect_timeout_seconds: 2,
        };
        let client = ProviderClient::new(&config).unwrap();
        let request = ProviderChatRequest {
            provider_id: None,
            base_url: config.base_url.clone(),
            api_key: None,
            model: "test".to_owned(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("hi".into()),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
            }],
            tools: Vec::new(),
            stream: true,
            temperature: None,
            top_p: None,
            max_tokens: Some(32),
            timeout_seconds: 10,
        };
        let mut deltas = Vec::new();
        let cancel = CancellationToken::new();
        let completed = client
            .chat(&request, None, &cancel, |delta| {
                deltas.push(delta.to_owned())
            })
            .await
            .unwrap();
        assert_eq!(deltas.concat(), "Hello");
        assert_eq!(completed.content, "Hello");
        assert_eq!(completed.tool_calls.len(), 1);
        assert_eq!(completed.tool_calls[0].function.name, "fs_read");
        assert_eq!(
            completed.tool_calls[0].function.arguments,
            "{\"path\":\"a.txt\"}"
        );
        assert_eq!(completed.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(completed.usage["total_tokens"], 7);
        let _ = server.await;
    }

    /// Реальная проверка против установленной PolyStrata.
    /// Запуск: `NEXUS_POLYSTRATA_URL=http://127.0.0.1:8080/v1
    /// NEXUS_POLYSTRATA_MODEL=<model> cargo test real_polystrata_smoke -- --ignored`.
    #[tokio::test]
    #[ignore = "требует запущенную PolyStrata и выбранную модель"]
    async fn real_polystrata_smoke() {
        let config = ProviderConfig {
            base_url: std::env::var("NEXUS_POLYSTRATA_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_owned()),
            api_key: std::env::var("NEXUS_POLYSTRATA_API_KEY").unwrap_or_default(),
            timeout_seconds: 120,
            connect_timeout_seconds: 10,
        };
        let client = ProviderClient::new(&config).unwrap();
        let (reachable, data) = client.status().await.expect("PolyStrata status");
        assert!(reachable, "PolyStrata недоступна: {data}");
        let models = client
            .models_at(&config.base_url, None)
            .await
            .expect("PolyStrata models");
        assert!(!models.is_empty(), "PolyStrata не вернула моделей");
        let model = std::env::var("NEXUS_POLYSTRATA_MODEL").unwrap_or_else(|_| models[0].clone());
        let request = ProviderChatRequest {
            provider_id: None,
            base_url: config.base_url.clone(),
            api_key: None,
            model,
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("Ответь одним словом: ping".into()),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
            }],
            tools: Vec::new(),
            stream: false,
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(16),
            timeout_seconds: 120,
        };
        let cancel = CancellationToken::new();
        let completed = client
            .chat(&request, None, &cancel, |_| {})
            .await
            .expect("PolyStrata chat");
        assert!(
            !completed.content.trim().is_empty() || !completed.tool_calls.is_empty(),
            "PolyStrata вернула пустой ответ"
        );
    }
}
