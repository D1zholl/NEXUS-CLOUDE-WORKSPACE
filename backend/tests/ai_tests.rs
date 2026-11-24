//! AI Workspace integration tests: реальный HTTP, PostgreSQL и WebSocket Runtime.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::TestEnv;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use nexus::ai::runtime::protocol::Envelope;

struct Api {
    base: String,
    client: reqwest::Client,
    cookie: Option<String>,
    csrf: Option<String>,
}

impl Api {
    fn new(base: &str) -> Self {
        Self {
            base: base.to_owned(),
            client: reqwest::Client::new(),
            cookie: None,
            csrf: None,
        }
    }

    async fn login(&mut self, username: &str, password: &str) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}/api/v1/auth/login", self.base))
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if let Some(cookie) = response.headers().get("set-cookie") {
            let cookie = cookie
                .to_str()
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("");
            self.cookie = Some(cookie.to_owned());
        }
        let payload: Value = response.json().await.map_err(|error| error.to_string())?;
        if payload.get("success") != Some(&Value::Bool(true)) {
            return Err(format!("login failed: {payload}"));
        }
        let data = payload.get("data").cloned().unwrap_or(Value::Null);
        self.csrf = data
            .get("csrfToken")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned);
        Ok(data)
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        self.call_with_headers(method, path, body, &[]).await
    }

    async fn call_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> Result<(u16, Value), String> {
        let mut request = self
            .client
            .request(
                method.parse().map_err(|_| "method".to_owned())?,
                format!("{}{}", self.base, path),
            )
            .header("Accept", "application/json");
        if let Some(cookie) = &self.cookie {
            request = request.header("Cookie", cookie);
        }
        if let Some(csrf) = &self.csrf {
            request = request.header("X-CSRF-Token", csrf);
        }
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        Ok((status, payload))
    }

    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, payload) = self.call(method, path, body).await.unwrap();
        assert!(
            (200..300).contains(&status) && payload["success"] == true,
            "{method} {path}: {status} {payload}"
        );
        payload["data"].clone()
    }

    async fn get(&self, path: &str) -> Value {
        self.ok("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        self.ok("POST", path, Some(body)).await
    }

    async fn post_multipart(
        &self,
        path: &str,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<(u16, Value), String> {
        let mut request = self
            .client
            .post(format!("{}{}", self.base, path))
            .header("Accept", "application/json");
        if let Some(cookie) = &self.cookie {
            request = request.header("Cookie", cookie);
        }
        if let Some(csrf) = &self.csrf {
            request = request.header("X-CSRF-Token", csrf);
        }
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_owned()),
        );
        let response = request
            .multipart(form)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        Ok((status, payload))
    }
}

async fn start_server(env: &TestEnv) -> (String, tokio::task::JoinHandle<()>) {
    let app = nexus::build_router(env.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{addr}"), handle)
}

struct FakeRuntime {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    node_id: Uuid,
    sequence: i64,
}

impl FakeRuntime {
    async fn connect(base: &str, token: &str, node_id: Uuid) -> Self {
        let url = format!(
            "{}/api/v1/ai/runtime/connect",
            base.replace("http://", "ws://")
        );
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("WebSocket Runtime подключается");
        let mut runtime = Self {
            socket,
            node_id,
            sequence: 0,
        };
        runtime
            .send(
                "runtime.hello",
                None,
                None,
                json!({
                    "runtimeVersion": "0.1.0-test",
                    "platform": "windows/x86_64",
                    "protocolVersion": 1,
                    "models": ["test-model"],
                    "polystrata": { "reachable": true, "version": "test", "modelCount": 1 },
                    "tools": [],
                    "programs": []
                }),
            )
            .await;
        let ack = runtime
            .wait_for("runtime.hello_ack", Duration::from_secs(5))
            .await;
        assert_eq!(ack.kind, "runtime.hello_ack");
        runtime
    }

    async fn send(
        &mut self,
        kind: &str,
        correlation: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: Value,
    ) {
        self.sequence += 1;
        let envelope = Envelope {
            protocol_version: 1,
            message_id: Uuid::new_v4(),
            kind: kind.to_owned(),
            timestamp: chrono::Utc::now(),
            node_id: self.node_id,
            correlation_id: correlation,
            task_id,
            sequence: Some(self.sequence),
            payload,
        };
        self.socket
            .send(Message::Text(
                serde_json::to_string(&envelope).unwrap().into(),
            ))
            .await
            .unwrap();
    }

    async fn next(&mut self, wait: Duration) -> Option<Envelope> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let message = timeout(remaining, self.socket.next()).await.ok()??;
            match message {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).ok();
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                Ok(Message::Close(_)) | Err(_) => return None,
                _ => continue,
            }
        }
    }

    async fn wait_for(&mut self, kind: &str, wait: Duration) -> Envelope {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "не дождались {kind}");
            let envelope = self
                .next(remaining)
                .await
                .unwrap_or_else(|| panic!("соединение закрыто до {kind}"));
            if envelope.kind == kind {
                return envelope;
            }
            if envelope.kind == "runtime.error" {
                panic!("Runtime получил ошибку: {}", envelope.payload);
            }
        }
    }

    async fn assert_no_message(&mut self, wait: Duration) {
        if let Some(envelope) = self.next(wait).await {
            panic!("неожиданное сообщение Runtime: {}", envelope.kind);
        }
    }

    async fn completed(&mut self, command: &Envelope, content: &str) {
        self.send(
            "provider.chat.completed",
            Some(command.message_id),
            command.task_id,
            json!({
                "content": content,
                "toolCalls": [],
                "usage": { "total_tokens": 10 },
                "finishReason": "stop"
            }),
        )
        .await;
    }

    async fn completed_with_tool(
        &mut self,
        command: &Envelope,
        tool_call_id: &str,
        name: &str,
        arguments: Value,
    ) {
        self.send(
            "provider.chat.completed",
            Some(command.message_id),
            command.task_id,
            json!({
                "content": "",
                "toolCalls": [{
                    "id": tool_call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(&arguments).unwrap()
                    }
                }],
                "usage": { "total_tokens": 20 },
                "finishReason": "tool_calls"
            }),
        )
        .await;
    }
}

async fn setup_runtime(_env: &TestEnv, api: &Api) -> (FakeRuntime, Uuid) {
    let code = api
        .post(
            "/api/v1/ai/admin/enrollment-codes",
            json!({ "label": "test", "ttlSeconds": 600 }),
        )
        .await;
    let code_value = code["code"].as_str().unwrap().to_owned();
    let enroll = api
        .client
        .post(format!("{}/api/v1/ai/runtime/enroll", api.base))
        .json(&json!({
            "code": code_value,
            "name": "TEST-RUNTIME",
            "platform": "windows/x86_64",
            "runtimeVersion": "0.1.0-test",
            "protocolVersion": 1
        }))
        .send()
        .await
        .unwrap();
    let status = enroll.status();
    let payload: Value = enroll.json().await.unwrap();
    assert!(status.is_success(), "enroll: {payload}");
    let data = &payload["data"];
    let node_id = Uuid::parse_str(data["nodeId"].as_str().unwrap()).unwrap();
    let token = data["deviceToken"].as_str().unwrap().to_owned();
    let runtime = FakeRuntime::connect(&api.base, &token, node_id).await;
    (runtime, node_id)
}

async fn setup_provider(api: &Api, node_id: Uuid) -> Uuid {
    let provider = api
        .post(
            "/api/v1/ai/admin/providers",
            json!({
                "name": "PolyStrata Test",
                "baseUrl": "http://127.0.0.1:8080/v1",
                "defaultModel": "test-model",
                "runtimeNodeId": node_id,
                "generation": { "temperature": 0.2 }
            }),
        )
        .await;
    Uuid::parse_str(provider["id"].as_str().unwrap()).unwrap()
}

async fn setup_workspace(
    env: &TestEnv,
    api: &Api,
    node_id: Uuid,
    owner_id: Uuid,
    tools: Vec<&str>,
) -> Uuid {
    let root = env
        .root
        .path()
        .join(format!("ws-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&root).unwrap();
    let workspace = api
        .post(
            "/api/v1/ai/admin/workspaces",
            json!({
                "ownerId": owner_id,
                "runtimeNodeId": node_id,
                "name": format!("WS {}", Uuid::new_v4().simple()),
                "rootPath": root.to_string_lossy(),
                "allowedTools": tools,
                "allowedPrograms": [],
                "maxParallelTasks": 2,
                "maxTaskSeconds": 120
            }),
        )
        .await;
    Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap()
}

async fn wait_task(api: &Api, task_id: Uuid, status: &str) -> Value {
    for _ in 0..100 {
        let task = api.get(&format!("/api/v1/ai/tasks/{task_id}")).await;
        if task["status"] == status {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("задача {task_id} не перешла в {status}");
}

async fn wait_message(api: &Api, conversation_id: Uuid, status: &str) -> Value {
    for _ in 0..100 {
        let page = api
            .get(&format!(
                "/api/v1/ai/conversations/{conversation_id}/messages"
            ))
            .await;
        if let Some(message) = page["items"].as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["status"] == status && item["role"] == "assistant")
        }) {
            return message.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("ассистент не перешёл в {status}");
}

async fn setup_delete_approval(
    env: &TestEnv,
    api: &Api,
    admin_id: Uuid,
) -> (FakeRuntime, Uuid, Uuid) {
    let (mut runtime, node_id) = setup_runtime(env, api).await;
    setup_provider(api, node_id).await;
    let workspace_id = setup_workspace(
        env,
        api,
        node_id,
        admin_id,
        vec!["fs.delete", "fs.list", "fs.read", "skill.load"],
    )
    .await;
    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            json!({ "content": "Удали старый файл" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();
    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    runtime
        .completed_with_tool(
            &command,
            "call_delete_1",
            "fs_delete",
            json!({ "path": "old.txt" }),
        )
        .await;
    wait_task(api, task_id, "waiting_approval").await;
    let pending = api.get("/api/v1/ai/approvals/pending").await;
    let approval_id = Uuid::parse_str(
        pending["items"].as_array().unwrap().first().unwrap()["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    (runtime, task_id, approval_id)
}

#[tokio::test]
async fn enrollment_is_single_use_and_token_can_be_revoked() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin = env.create_user("aiadmin", "SecretPass1!").await;
    api.login("aiadmin", "SecretPass1!").await.unwrap();

    let code = api
        .post(
            "/api/v1/ai/admin/enrollment-codes",
            json!({ "label": "one-time", "ttlSeconds": 600 }),
        )
        .await;
    let code_value = code["code"].as_str().unwrap().to_owned();

    let first = api
        .client
        .post(format!("{base}/api/v1/ai/runtime/enroll"))
        .json(&json!({
            "code": code_value,
            "name": "PC-1",
            "platform": "windows/x86_64",
            "runtimeVersion": "0.1.0",
            "protocolVersion": 1
        }))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success());
    let payload: Value = first.json().await.unwrap();
    let node_id = Uuid::parse_str(payload["data"]["nodeId"].as_str().unwrap()).unwrap();
    let token = payload["data"]["deviceToken"].as_str().unwrap().to_owned();
    let stored_hash: String =
        sqlx::query_scalar("select token_hash from ai_runtime_nodes where id = $1")
            .bind(node_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_ne!(stored_hash, token);
    assert!(!stored_hash.starts_with("nxa_"));
    assert_eq!(stored_hash.len(), 64);

    let second = api
        .client
        .post(format!("{base}/api/v1/ai/runtime/enroll"))
        .json(&json!({
            "code": code_value,
            "name": "PC-2",
            "platform": "windows/x86_64",
            "runtimeVersion": "0.1.0",
            "protocolVersion": 1
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), reqwest::StatusCode::UNAUTHORIZED);

    let mut runtime = FakeRuntime::connect(&base, &token, node_id).await;
    let _ = &mut runtime;
    api.post(
        &format!("/api/v1/ai/admin/runtime-nodes/{node_id}/revoke"),
        json!({}),
    )
    .await;
    let status = api
        .get(&format!("/api/v1/ai/admin/runtime-nodes/{node_id}"))
        .await;
    assert_eq!(status["node"]["status"], "revoked");

    // После отзыва device token не может открыть новое соединение.
    let result = tokio_tungstenite::connect_async({
        let mut request = format!(
            "{}/api/v1/ai/runtime/connect",
            base.replace("http://", "ws://")
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    })
    .await;
    assert!(result.is_err(), "revoked Runtime не должен подключаться");

    let _ = admin;
    server.abort();
}

#[tokio::test]
async fn chat_streams_without_tools_and_persists_history() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("chatadmin", "SecretPass1!").await;
    api.login("chatadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    let _provider = setup_provider(&api, node_id).await;

    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({ "mode": "chat", "model": "test-model" }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();

    api.post(
        &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
        json!({ "content": "Привет, NEXUS" }),
    )
    .await;

    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    assert_eq!(
        command.payload["tools"].as_array().unwrap().len(),
        0,
        "Chat Mode не должен передавать tools"
    );
    assert_eq!(command.payload["stream"], true);
    runtime
        .send(
            "provider.chat.delta",
            Some(command.message_id),
            None,
            json!({ "delta": "Привет" }),
        )
        .await;
    runtime
        .send(
            "provider.chat.delta",
            Some(command.message_id),
            None,
            json!({ "delta": ", мир" }),
        )
        .await;
    runtime.completed(&command, "Привет, мир").await;

    let message = wait_message(&api, conversation_id, "completed").await;
    assert_eq!(message["content"], "Привет, мир");

    // История доступна после «перезагрузки» (новый HTTP-запрос).
    let page = api
        .get(&format!(
            "/api/v1/ai/conversations/{conversation_id}/messages"
        ))
        .await;
    let items = page["items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["role"] == "user"));
    assert!(
        items
            .iter()
            .any(|item| item["role"] == "assistant" && item["status"] == "completed")
    );

    let title = api
        .get(&format!("/api/v1/ai/conversations/{conversation_id}"))
        .await;
    assert_ne!(title["title"], "Новый диалог");

    let _ = admin_id;
    server.abort();
}

#[tokio::test]
async fn agent_executes_tool_cycle_and_records_execution() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("agentadmin", "SecretPass1!").await;
    api.login("agentadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;
    let workspace_id = setup_workspace(
        &env,
        &api,
        node_id,
        admin_id,
        vec!["fs.read", "fs.list", "fs.search", "skill.load"],
    )
    .await;

    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            json!({ "content": "Прочитай файл a.txt" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();
    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    let tools = command.payload["tools"].as_array().unwrap();
    assert!(!tools.is_empty(), "Agent Mode передаёт разрешённые tools");
    assert!(
        tools
            .iter()
            .all(|tool| tool["function"]["name"] != "fs_delete"),
        "неразрешённый инструмент не должен попадать в запрос"
    );
    runtime
        .completed_with_tool(
            &command,
            "call_read_1",
            "fs_read",
            json!({ "path": "a.txt" }),
        )
        .await;

    let tool_command = runtime
        .wait_for("tool.execute", Duration::from_secs(5))
        .await;
    assert_eq!(tool_command.payload["toolId"], "fs.read");
    assert_eq!(tool_command.payload["arguments"]["path"], "a.txt");
    runtime
        .send(
            "tool.completed",
            Some(tool_command.message_id),
            Some(task_id),
            json!({
                "result": { "path": "a.txt", "content": "hello", "truncated": false },
                "resultText": "{\"path\":\"a.txt\",\"content\":\"hello\"}",
                "exitCode": null,
                "durationMs": 3
            }),
        )
        .await;

    let next = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    let messages = next.payload["messages"].as_array().unwrap();
    assert!(
        messages.iter().any(|message| message["role"] == "tool"
            && message["tool_calls"]
                .as_array()
                .map(|calls| calls.is_empty())
                .unwrap_or(true)
            && message["content"].as_str().unwrap_or("").contains("hello")),
        "результат инструмента возвращается модели: {messages:?}"
    );
    runtime.completed(&next, "Файл прочитан: hello").await;

    let task = wait_task(&api, task_id, "completed").await;
    assert_eq!(task["status"], "completed");
    let executions = api
        .get(&format!("/api/v1/ai/tasks/{task_id}/executions"))
        .await;
    let executions = executions["items"].as_array().unwrap();
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0]["toolId"], "fs.read");
    assert_eq!(executions[0]["status"], "completed");

    let message = wait_message(&api, conversation_id, "completed").await;
    assert_eq!(message["content"], "Файл прочитан: hello");

    server.abort();
}

#[tokio::test]
async fn destructive_tool_requires_approval_and_is_bound_to_arguments() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("approveadmin", "SecretPass1!").await;
    api.login("approveadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;
    let workspace_id = setup_workspace(
        &env,
        &api,
        node_id,
        admin_id,
        vec!["fs.delete", "fs.list", "fs.read", "skill.load"],
    )
    .await;

    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            json!({ "content": "Удали старый файл" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();
    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    runtime
        .completed_with_tool(
            &command,
            "call_delete_1",
            "fs_delete",
            json!({ "path": "old.txt" }),
        )
        .await;

    let task = wait_task(&api, task_id, "waiting_approval").await;
    assert_eq!(task["status"], "waiting_approval");
    runtime.assert_no_message(Duration::from_millis(400)).await;

    let pending = api.get("/api/v1/ai/approvals/pending").await;
    let approvals = pending["items"].as_array().unwrap();
    assert_eq!(approvals.len(), 1);
    let approval_id = Uuid::parse_str(approvals[0]["id"].as_str().unwrap()).unwrap();
    assert_eq!(approvals[0]["toolId"], "fs.delete");
    assert_eq!(approvals[0]["arguments"]["path"], "old.txt");

    let (status, _) = api
        .call(
            "POST",
            &format!("/api/v1/ai/approvals/{approval_id}/approve"),
            Some(json!({ "reason": "test" })),
        )
        .await
        .unwrap();
    assert_eq!(status, 200);

    let tool_command = runtime
        .wait_for("tool.execute", Duration::from_secs(5))
        .await;
    assert_eq!(tool_command.payload["toolId"], "fs.delete");
    assert_eq!(tool_command.payload["arguments"]["path"], "old.txt");
    runtime
        .send(
            "tool.completed",
            Some(tool_command.message_id),
            Some(task_id),
            json!({
                "result": { "path": "old.txt", "deleted": true },
                "resultText": "deleted",
                "durationMs": 1
            }),
        )
        .await;
    let next = runtime
        .wait_for("provider.chat.start", Duration::from_secs(5))
        .await;
    runtime.completed(&next, "Файл удалён").await;
    wait_task(&api, task_id, "completed").await;

    // Повторное подтверждение невозможно: approval потреблён.
    let (status, _) = api
        .call(
            "POST",
            &format!("/api/v1/ai/approvals/{approval_id}/approve"),
            Some(json!({})),
        )
        .await
        .unwrap();
    assert_eq!(status, 409);

    let executions = api
        .get(&format!("/api/v1/ai/tasks/{task_id}/executions"))
        .await;
    let executions = executions["items"].as_array().unwrap();
    assert_eq!(executions.len(), 1);
    assert!(executions[0]["approvalId"].as_str().is_some());

    server.abort();
}

#[tokio::test]
async fn disabled_ai_module_blocks_api_immediately() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    env.create_user("modadmin", "SecretPass1!").await;
    api.login("modadmin", "SecretPass1!").await.unwrap();

    sqlx::query("update module_states set enabled = false where key = 'ai'")
        .execute(&env.pool)
        .await
        .unwrap();
    env.state.modules.invalidate().await;

    let (status, payload) = api.call("GET", "/api/v1/ai/providers", None).await.unwrap();
    assert_eq!(status, 403);
    assert_eq!(payload["error"]["code"], "MODULE_DISABLED");

    sqlx::query("update module_states set enabled = true where key = 'ai'")
        .execute(&env.pool)
        .await
        .unwrap();
    env.state.modules.invalidate().await;
    let (status, _) = api.call("GET", "/api/v1/ai/providers", None).await.unwrap();
    assert_eq!(status, 200);

    server.abort();
}

#[tokio::test]
async fn provider_api_key_is_encrypted_and_never_returned() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    env.create_user("secretadmin", "SecretPass1!").await;
    api.login("secretadmin", "SecretPass1!").await.unwrap();
    let (runtime, node_id) = setup_runtime(&env, &api).await;
    let _ = runtime;
    let provider = api
        .post(
            "/api/v1/ai/admin/providers",
            json!({
                "name": "Secret Provider",
                "baseUrl": "http://127.0.0.1:8080/v1",
                "apiKey": "sk-super-secret-key",
                "defaultModel": "test-model",
                "runtimeNodeId": node_id
            }),
        )
        .await;
    let provider_id = Uuid::parse_str(provider["id"].as_str().unwrap()).unwrap();
    assert_eq!(provider["hasApiKey"], true);
    assert!(!provider.to_string().contains("sk-super-secret-key"));
    let cipher: Option<String> =
        sqlx::query_scalar("select api_key_cipher from ai_provider_configs where id = $1")
            .bind(provider_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    let cipher = cipher.unwrap();
    assert!(!cipher.contains("sk-super-secret-key"));
    assert!(cipher.starts_with("v1:"));
    let decrypted = nexus::ai::providers::decrypted_key(&env.state, provider_id)
        .await
        .unwrap();
    assert_eq!(decrypted.as_deref(), Some("sk-super-secret-key"));

    let db_audit: String = sqlx::query_scalar(
        "select coalesce(string_agg(details::text, ' '), '') from audit_events where event_type like 'ai.provider%'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(!db_audit.contains("sk-super-secret-key"));

    server.abort();
}

#[tokio::test]
async fn rejected_approval_never_executes() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("rejectadmin", "SecretPass1!").await;
    api.login("rejectadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, task_id, approval_id) = setup_delete_approval(&env, &api, admin_id).await;

    let (status, _) = api
        .call(
            "POST",
            &format!("/api/v1/ai/approvals/{approval_id}/reject"),
            Some(json!({ "reason": "not allowed" })),
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    wait_task(&api, task_id, "cancelled").await;
    runtime.assert_no_message(Duration::from_millis(500)).await;
    let executions = api
        .get(&format!("/api/v1/ai/tasks/{task_id}/executions"))
        .await;
    let executions = executions["items"].as_array().unwrap();
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0]["status"], "cancelled");
    let pending = api.get("/api/v1/ai/approvals/pending").await;
    assert_eq!(pending["items"].as_array().unwrap().len(), 0);

    server.abort();
}

#[tokio::test]
async fn expired_approval_cannot_execute() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("expireadmin", "SecretPass1!").await;
    api.login("expireadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, task_id, approval_id) = setup_delete_approval(&env, &api, admin_id).await;

    sqlx::query(
        "update ai_tool_approvals set expires_at = now() - interval '1 minute' where id = $1",
    )
    .bind(approval_id)
    .execute(&env.pool)
    .await
    .unwrap();
    let (status, payload) = api
        .call(
            "POST",
            &format!("/api/v1/ai/approvals/{approval_id}/approve"),
            Some(json!({})),
        )
        .await
        .unwrap();
    assert_eq!(status, 409, "{payload}");
    runtime.assert_no_message(Duration::from_millis(500)).await;
    let pending = api.get("/api/v1/ai/approvals/pending").await;
    assert_eq!(pending["items"].as_array().unwrap().len(), 0);
    let task = wait_task(&api, task_id, "waiting_approval").await;
    assert_eq!(task["status"], "waiting_approval");

    server.abort();
}

#[tokio::test]
async fn skill_import_assign_and_load_on_demand() {
    use std::io::Write;

    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("skilladmin", "SecretPass1!").await;
    api.login("skilladmin", "SecretPass1!").await.unwrap();

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("demo-skill/SKILL.md", options).unwrap();
        zip.write_all(
            b"---\nname: Demo Skill\ndescription: Test skill\n---\n# Demo Skill\nUse references/info.md.\n",
        )
        .unwrap();
        zip.start_file("demo-skill/references/info.md", options)
            .unwrap();
        zip.write_all(b"reference content").unwrap();
        zip.start_file("demo-skill/scripts/run.ps1", options)
            .unwrap();
        zip.write_all(b"Write-Output 'must not run'").unwrap();
        zip.finish().unwrap();
    }
    let (status, payload) = api
        .post_multipart(
            "/api/v1/ai/admin/skills/import",
            "demo-skill.zip",
            buffer.into_inner(),
        )
        .await
        .unwrap();
    assert_eq!(status, 200, "{payload}");
    let skill_id = Uuid::parse_str(payload["data"]["skill"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(payload["data"]["skill"]["fileCount"], 3);

    api.post(
        &format!("/api/v1/ai/admin/skills/{skill_id}/assignments"),
        json!({ "scope": "user", "userId": admin_id }),
    )
    .await;
    let skills = api.get("/api/v1/ai/skills").await;
    assert!(
        skills["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == format!("{skill_id}"))
    );
    let content = api.get(&format!("/api/v1/ai/skills/{skill_id}")).await;
    assert!(
        content["content"]
            .as_str()
            .unwrap()
            .contains("# Demo Skill")
    );
    assert_eq!(content["untrusted"], true);

    let reference = nexus::ai::skills::load_content(
        &env.state,
        admin_id,
        None,
        "demo-skill",
        Some("references/info.md"),
    )
    .await
    .unwrap();
    assert_eq!(reference["content"], "reference content");
    // Скрипт не исполняется при импорте; он существует только как файл Skill.
    let traversal = nexus::ai::skills::load_content(
        &env.state,
        admin_id,
        None,
        "demo-skill",
        Some("../../secret"),
    )
    .await;
    assert!(traversal.is_err());

    let (status, _) = api
        .call("GET", &format!("/api/v1/ai/skills/{skill_id}"), None)
        .await
        .unwrap();
    assert_eq!(status, 200);

    server.abort();
}

#[tokio::test]
async fn user_cannot_read_another_users_conversation() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut owner = Api::new(&base);
    env.create_user("owner", "SecretPass1!").await;
    owner.login("owner", "SecretPass1!").await.unwrap();
    let (runtime, node_id) = setup_runtime(&env, &owner).await;
    let _ = runtime;
    setup_provider(&owner, node_id).await;
    let conversation = owner
        .post(
            "/api/v1/ai/conversations",
            json!({ "mode": "chat", "model": "test-model" }),
        )
        .await;
    let conversation_id = conversation["id"].as_str().unwrap().to_owned();

    let hash = nexus::auth::password::hash_password("SecretPass2!".to_owned())
        .await
        .unwrap();
    sqlx::query(
        "insert into users (username, display_name, password_hash, status)
         values ('intruder', 'intruder', $1, 'active')",
    )
    .bind(hash)
    .execute(&env.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into user_roles (user_id, role_id)
         select u.id, r.id from users u, roles r
          where u.username = 'intruder' and r.key = 'user'",
    )
    .execute(&env.pool)
    .await
    .unwrap();
    let mut intruder = Api::new(&base);
    intruder.login("intruder", "SecretPass2!").await.unwrap();
    let (status, payload) = intruder
        .call(
            "GET",
            &format!("/api/v1/ai/conversations/{conversation_id}"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        status, 404,
        "чужой диалог не должен раскрываться: {payload}"
    );
    let (status, _) = intruder
        .call(
            "GET",
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(status, 404);

    server.abort();
}

#[tokio::test]
async fn mcp_registry_discovers_tools_and_agent_executes_them() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("mcpadmin", "SecretPass1!").await;
    api.login("mcpadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;

    let connection = api
        .post(
            "/api/v1/ai/admin/mcp-connections",
            json!({
                "name": "Local MCP",
                "runtimeNodeId": node_id,
                "endpoint": "http://127.0.0.1:3999/mcp",
                "secret": "mcp-super-secret",
                "enabled": true,
                "allowPrivateNetwork": true
            }),
        )
        .await;
    let connection_id = connection["id"].as_str().unwrap().to_owned();

    let discover_path = format!("/api/v1/ai/admin/mcp-connections/{connection_id}/discover");
    let discover = api.post(&discover_path, json!({}));
    let serve = async {
        let command = runtime
            .wait_for("mcp.discover", Duration::from_secs(10))
            .await;
        runtime
            .send(
                "mcp.tools",
                Some(command.message_id),
                None,
                json!({
                    "connectionId": connection_id,
                    "ok": true,
                    "serverName": "test-server",
                    "protocolVersion": "2024-11-05",
                    "tools": [{
                        "name": "echo_text",
                        "description": "Echo text",
                        "inputSchema": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": { "text": { "type": "string" } },
                            "required": ["text"]
                        }
                    }]
                }),
            )
            .await;
    };
    let (discovered, ()) = tokio::join!(discover, serve);
    assert_eq!(discovered["tools"].as_array().unwrap().len(), 1);
    assert_eq!(discovered["connection"]["status"], "ok");

    let stored: String = sqlx::query_scalar(
        "select coalesce(string_agg(payload::text, ' '), '') from ai_runtime_commands
          where kind = 'mcp.configure'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(!stored.contains("mcp-super-secret"));

    let (tool_row_id, tool_id, model_name): (Uuid, String, String) =
        sqlx::query_as("select id, tool_id, model_name from ai_mcp_tools where connection_id = $1")
            .bind(Uuid::parse_str(&connection_id).unwrap())
            .fetch_one(&env.pool)
            .await
            .unwrap();
    // Для проверки самого цикла выполнения отключаем approval у этого tool;
    // approval-политика MCP отдельно проверяется при discovery.
    api.call(
        "PATCH",
        &format!("/api/v1/ai/admin/mcp-tools/{tool_row_id}"),
        Some(json!({ "approvalPolicy": "never", "risk": "low" })),
    )
    .await
    .unwrap();

    api.post(
        &format!("/api/v1/ai/admin/mcp-connections/{connection_id}/assignments"),
        json!({ "scope": "user", "userId": admin_id }),
    )
    .await;

    let workspace_id = setup_workspace(&env, &api, node_id, admin_id, vec![tool_id.as_str()]).await;

    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            json!({ "content": "Вызови MCP echo" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();

    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(10))
        .await;
    let tools = command.payload["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == model_name)
    );
    runtime
        .completed_with_tool(
            &command,
            "call_mcp_1",
            &model_name,
            json!({ "text": "hello" }),
        )
        .await;
    let tool_command = runtime
        .wait_for("tool.execute", Duration::from_secs(10))
        .await;
    assert_eq!(tool_command.payload["toolId"], tool_id);
    runtime
        .send(
            "tool.completed",
            Some(tool_command.message_id),
            Some(task_id),
            json!({
                "result": { "content": [{ "type": "text", "text": "hello" }] },
                "resultText": "hello",
                "durationMs": 1
            }),
        )
        .await;
    let next = runtime
        .wait_for("provider.chat.start", Duration::from_secs(10))
        .await;
    runtime.completed(&next, "MCP tool выполнен").await;
    wait_task(&api, task_id, "completed").await;
    let executions = api
        .get(&format!("/api/v1/ai/tasks/{task_id}/executions"))
        .await;
    assert_eq!(executions["items"][0]["toolId"], tool_id);
    server.abort();
}

#[tokio::test]
async fn workspace_usage_is_reported_by_runtime() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("usageadmin", "SecretPass1!").await;
    api.login("usageadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;
    let workspace_id = setup_workspace(&env, &api, node_id, admin_id, vec!["fs.read"]).await;

    let refresh_path = format!("/api/v1/ai/workspaces/{workspace_id}/usage/refresh");
    let refresh = api.post(&refresh_path, json!({}));
    let serve = async {
        let command = runtime
            .wait_for("workspace.usage", Duration::from_secs(10))
            .await;
        runtime
            .send(
                "workspace.usage_result",
                Some(command.message_id),
                None,
                json!({
                    "workspaceId": workspace_id,
                    "bytes": 4096,
                    "files": 3,
                    "checkedAt": chrono::Utc::now(),
                    "error": null
                }),
            )
            .await;
    };
    let (usage, ()) = tokio::join!(refresh, serve);
    assert_eq!(usage["usedBytes"], 4096);
    assert_eq!(usage["usedFiles"], 3);
    assert_eq!(usage["source"], "runtime");
    let stored: (i64, i64) =
        sqlx::query_as("select usage_bytes, usage_files from ai_workspaces where id = $1")
            .bind(workspace_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(stored, (4096, 3));
    server.abort();
}

#[tokio::test]
async fn task_cancel_is_not_successful_without_runtime_confirmation() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("canceladmin", "SecretPass1!").await;
    api.login("canceladmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;
    let workspace_id = setup_workspace(&env, &api, node_id, admin_id, vec!["fs.read"]).await;
    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let sent = api
        .post(
            &format!("/api/v1/ai/conversations/{conversation_id}/messages"),
            json!({ "content": "Долгая задача" }),
        )
        .await;
    let task_id = Uuid::parse_str(sent["task"]["id"].as_str().unwrap()).unwrap();
    let _provider = runtime
        .wait_for("provider.chat.start", Duration::from_secs(10))
        .await;

    let cancel = api
        .post(&format!("/api/v1/ai/tasks/{task_id}/cancel"), json!({}))
        .await;
    assert_eq!(cancel["requested"], true);
    let cancel_command = runtime
        .wait_for("task.cancel", Duration::from_secs(10))
        .await;
    let cancel_request = cancel_command
        .payload
        .get("cancelRequestMessageId")
        .and_then(|value| value.as_str())
        .unwrap()
        .to_owned();

    let current = api.get(&format!("/api/v1/ai/tasks/{task_id}")).await;
    assert_ne!(current["status"], "cancelled");
    assert_eq!(current["cancelStatus"], "requested");

    runtime
        .send(
            "task.cancel_ack",
            Some(cancel_command.message_id),
            Some(task_id),
            json!({
                "cancelRequestMessageId": cancel_request,
                "accepted": true,
                "activeExecutions": 1
            }),
        )
        .await;
    runtime
        .send(
            "task.cancelled",
            Some(cancel_command.message_id),
            Some(task_id),
            json!({
                "taskId": task_id,
                "confirmed": true,
                "cancelRequestMessageId": cancel_request,
                "activeExecutions": 0,
                "terminatedProcesses": 1,
                "reason": "test"
            }),
        )
        .await;
    let task = wait_task(&api, task_id, "cancelled").await;
    assert_eq!(task["cancelStatus"], "confirmed");
    server.abort();
}

#[tokio::test]
async fn send_message_honours_idempotency_key() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    env.create_user("idemadmin", "SecretPass1!").await;
    api.login("idemadmin", "SecretPass1!").await.unwrap();
    let (mut runtime, node_id) = setup_runtime(&env, &api).await;
    setup_provider(&api, node_id).await;
    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({ "mode": "chat", "model": "test-model" }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let path = format!("/api/v1/ai/conversations/{conversation_id}/messages");
    let body = json!({ "content": "Один и тот же запрос" });
    let (status1, first) = api
        .call_with_headers(
            "POST",
            &path,
            Some(body.clone()),
            &[("Idempotency-Key", "chat-retry-1")],
        )
        .await
        .unwrap();
    assert_eq!(status1, 200, "{first}");
    let command = runtime
        .wait_for("provider.chat.start", Duration::from_secs(10))
        .await;
    runtime.completed(&command, "ответ").await;
    wait_message(&api, conversation_id, "completed").await;
    let (status2, replay) = api
        .call_with_headers(
            "POST",
            &path,
            Some(body),
            &[("Idempotency-Key", "chat-retry-1")],
        )
        .await
        .unwrap();
    assert_eq!(status2, 200);
    assert_eq!(replay["data"], first["data"]);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from ai_messages where conversation_id = $1 and role = 'user'",
    )
    .bind(conversation_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "повтор с Idempotency-Key не должен создавать сообщение"
    );
    server.abort();
}

#[tokio::test]
async fn skill_versions_keep_full_file_snapshots_and_can_be_restored() {
    use std::io::Write;

    fn zip_skill(reference: &str, extra: &str) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("versioned-skill/SKILL.md", options).unwrap();
            zip.write_all(
                b"---\nname: Versioned Skill\ndescription: v skill\n---\n# Versioned Skill\n",
            )
            .unwrap();
            zip.start_file("versioned-skill/references/info.md", options)
                .unwrap();
            zip.write_all(reference.as_bytes()).unwrap();
            zip.start_file("versioned-skill/assets/data.txt", options)
                .unwrap();
            zip.write_all(extra.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    env.create_user("veradmin", "SecretPass1!").await;
    api.login("veradmin", "SecretPass1!").await.unwrap();

    let (status, payload) = api
        .post_multipart(
            "/api/v1/ai/admin/skills/import",
            "v1.zip",
            zip_skill("old reference", "old asset"),
        )
        .await
        .unwrap();
    assert_eq!(status, 200, "{payload}");
    let skill_id = payload["data"]["skill"]["id"].as_str().unwrap().to_owned();
    let (status, payload) = api
        .post_multipart(
            "/api/v1/ai/admin/skills/import",
            "v2.zip",
            zip_skill("new reference", "new asset"),
        )
        .await
        .unwrap();
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["data"]["version"], 2);

    let versions = api
        .get(&format!("/api/v1/ai/admin/skills/{skill_id}/versions"))
        .await;
    let items = versions["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["fileCount"], 3);
    let v1 = api
        .get(&format!("/api/v1/ai/admin/skills/{skill_id}/versions/1"))
        .await;
    let files = v1["files"].as_array().unwrap();
    let reference = files
        .iter()
        .find(|file| file["path"] == "references/info.md")
        .unwrap();
    assert_eq!(reference["content"], "old reference");

    let restored = api
        .post(
            &format!("/api/v1/ai/admin/skills/{skill_id}/versions/1/restore"),
            json!({}),
        )
        .await;
    assert_eq!(restored["version"], 3);
    let v3 = api
        .get(&format!("/api/v1/ai/admin/skills/{skill_id}/versions/3"))
        .await;
    let content = v3["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "references/info.md")
        .unwrap()["content"]
        .clone();
    assert_eq!(content, "old reference");
    server.abort();
}

#[tokio::test]
async fn recovery_requeues_safe_tasks_and_keeps_pending_receipts() {
    let env = TestEnv::new().await;
    let (base, server) = start_server(&env).await;
    let mut api = Api::new(&base);
    let admin_id = env.create_user("recoveradmin", "SecretPass1!").await;
    api.login("recoveradmin", "SecretPass1!").await.unwrap();
    let (runtime, node_id) = setup_runtime(&env, &api).await;
    let provider_id = setup_provider(&api, node_id).await;
    let workspace_id = setup_workspace(&env, &api, node_id, admin_id, vec!["fs.read"]).await;
    let conversation = api
        .post(
            "/api/v1/ai/conversations",
            json!({
                "mode": "agent",
                "model": "test-model",
                "workspaceId": workspace_id
            }),
        )
        .await;
    let conversation_id = Uuid::parse_str(conversation["id"].as_str().unwrap()).unwrap();
    let context = json!({ "messages": [], "pending": null });

    let queued_id: Uuid = sqlx::query_scalar(
        "insert into ai_tasks
            (conversation_id, owner_id, runtime_node_id, provider_id, model, workspace_id,
             status, stage, context)
         values ($1, $2, $3, $4, 'test-model', $5, 'queued', 'queued', $6)
         returning id",
    )
    .bind(conversation_id)
    .bind(admin_id)
    .bind(node_id)
    .bind(provider_id)
    .bind(workspace_id)
    .bind(&context)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    let running_id: Uuid = sqlx::query_scalar(
        "insert into ai_tasks
            (conversation_id, owner_id, runtime_node_id, provider_id, model, workspace_id,
             status, stage, context)
         values ($1, $2, $3, $4, 'test-model', $5, 'running', 'model', $6)
         returning id",
    )
    .bind(conversation_id)
    .bind(admin_id)
    .bind(node_id)
    .bind(provider_id)
    .bind(workspace_id)
    .bind(&context)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    let execution_id: Uuid = sqlx::query_scalar(
        "insert into ai_tool_executions
            (task_id, tool_call_id, tool_id, arguments, args_hash, status,
             idempotency_key, dispatch_message_id)
         values ($1, 'call_recover', 'fs.read', '{\"path\":\"a.txt\"}', 'hash',
                 'running', $2, $3)
         returning id",
    )
    .bind(running_id)
    .bind(format!("{running_id}:call_recover"))
    .bind(Uuid::new_v4())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(!execution_id.is_nil());

    let report = nexus::ai::recover_interrupted(&env.pool).await;
    assert!(report.resume.contains(&queued_id));
    assert!(report.reconcile.iter().any(|(id, _)| *id == running_id));
    let queued_state: (String, Option<String>) =
        sqlx::query_as("select status, recovery_state from ai_tasks where id = $1")
            .bind(queued_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(queued_state.0, "queued");
    assert_eq!(queued_state.1.as_deref(), Some("requeued"));
    let running_state: (String, Option<String>) =
        sqlx::query_as("select status, recovery_state from ai_tasks where id = $1")
            .bind(running_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(running_state.0, "running");
    assert_eq!(running_state.1.as_deref(), Some("awaiting_receipt"));
    let _ = runtime;
    server.abort();
}
