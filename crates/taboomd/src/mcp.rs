use crate::handler::{ToolHandler, ToolResult};
use crate::recording;
use crate::tools::{self, ToolCall};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct McpConfig {
    pub port: u16,
    pub bind_addr: String,
    pub tokens: HashMap<String, String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            port: std::env::var("TABOOM_MCP_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3456),
            bind_addr: std::env::var("TABOOM_MCP_BIND")
                .unwrap_or_else(|_| "127.0.0.1".into()),
            tokens: parse_tokens(&std::env::var("TABOOM_MCP_TOKENS").unwrap_or_default()),
        }
    }
}

/// `TABOOM_MCP_TOKENS=agent-a=tok1,agent-b=tok2`: each name is a separate lease holder.
fn parse_tokens(spec: &str) -> HashMap<String, String> {
    spec.split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, tok)| (name.trim().to_string(), tok.trim().to_string()))
        .filter(|(name, tok)| !name.is_empty() && !tok.is_empty())
        .collect()
}

const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone)]
struct McpState {
    handler: Arc<ToolHandler>,
    tokens: Arc<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}

fn extract_client_id(headers: &HeaderMap, tokens: &HashMap<String, String>) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth.strip_prefix("Bearer ")?;
    tokens
        .iter()
        .find(|(_, v)| v.as_str() == token)
        .map(|(k, _)| k.clone())
}

async fn handle_mcp(
    State(state): State<McpState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<JsonRpcRequest>,
) -> Response {
    let client_id = if state.tokens.is_empty() {
        "anonymous".to_string()
    } else {
        match extract_client_id(&headers, &state.tokens) {
            Some(id) => id,
            None => {
                let id = req.id.unwrap_or(Value::Null);
                let resp = JsonRpcResponse::error(id, -32000, "unauthorized: invalid or missing bearer token");
                return (StatusCode::UNAUTHORIZED, axum::Json(resp)).into_response();
            }
        }
    };

    // Notifications (no id) get no JSON-RPC reply, per the streamable HTTP transport.
    let Some(id) = req.id else {
        return StatusCode::ACCEPTED.into_response();
    };

    dispatch(state, id, req.method, req.params, &client_id).await.into_response()
}

async fn dispatch(
    state: McpState,
    id: Value,
    method: String,
    params: Option<Value>,
    client_id: &str,
) -> (StatusCode, axum::Json<JsonRpcResponse>) {
    match method.as_str() {
        "initialize" => {
            let requested = params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let version = SUPPORTED_PROTOCOL_VERSIONS
                .into_iter()
                .find(|v| *v == requested)
                .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
            let resp = JsonRpcResponse::success(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "taboom",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            );
            (StatusCode::OK, axum::Json(resp))
        }
        "ping" => (StatusCode::OK, axum::Json(JsonRpcResponse::success(id, json!({})))),
        "tools/list" => {
            let defs = tools::all_tool_defs();
            let tools_json: Vec<Value> = defs
                .into_iter()
                .map(|d| {
                    json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.input_schema,
                    })
                })
                .collect();
            let resp = JsonRpcResponse::success(id, json!({ "tools": tools_json }));
            (StatusCode::OK, axum::Json(resp))
        }
        "tools/call" => {
            let params = match params {
                Some(p) => p,
                None => {
                    let resp = JsonRpcResponse::error(id, -32602, "missing params");
                    return (StatusCode::BAD_REQUEST, axum::Json(resp));
                }
            };

            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));

            let mut call_value = arguments.clone();
            if let Some(obj) = call_value.as_object_mut() {
                obj.insert("tool".to_string(), Value::String(tool_name.to_string()));
            }

            let call: ToolCall = match serde_json::from_value(call_value) {
                Ok(c) => c,
                Err(e) => {
                    let resp = JsonRpcResponse::error(
                        id,
                        -32602,
                        &format!("invalid tool call: {e}"),
                    );
                    return (StatusCode::BAD_REQUEST, axum::Json(resp));
                }
            };

            // Tools sleep, poll the screen and spawn processes; keep that off the async workers.
            let handler = Arc::clone(&state.handler);
            let client = client_id.to_string();
            let result = match tokio::task::spawn_blocking(move || handler.handle(call, &client)).await {
                Ok(r) => r,
                Err(e) => ToolResult::err(&format!("tool crashed: {e}")),
            };
            let content = mcp_content(&result);
            let resp = JsonRpcResponse::success(
                id,
                json!({
                    "content": content,
                    "isError": result.is_error,
                }),
            );
            (StatusCode::OK, axum::Json(resp))
        }
        _ => {
            let resp = JsonRpcResponse::error(id, -32601, &format!("method not found: {method}"));
            (StatusCode::OK, axum::Json(resp))
        }
    }
}

/// Images first, so the model sees pixels, then the JSON details as text.
fn mcp_content(result: &ToolResult) -> Value {
    let mut blocks: Vec<Value> = result
        .images
        .iter()
        .map(|i| json!({ "type": "image", "data": i.base64, "mimeType": i.mime }))
        .collect();
    blocks.push(json!({ "type": "text", "text": result.content.to_string() }));
    Value::Array(blocks)
}

#[derive(Deserialize)]
struct ShareQuery {
    exp: u64,
    sig: String,
}

fn share_ok(state: &McpState, id: &str, q: &ShareQuery) -> bool {
    state.handler.recorder().verify(id, q.exp, &q.sig)
}

async fn replay_page(
    State(state): State<McpState>,
    Path(id): Path<String>,
    Query(q): Query<ShareQuery>,
) -> Response {
    if !share_ok(&state, &id, &q) {
        return (StatusCode::FORBIDDEN, "link is invalid or expired").into_response();
    }
    let handler = Arc::clone(&state.handler);
    let page = tokio::task::spawn_blocking(move || {
        let rec = handler.recorder();
        let meta = rec.meta(&id)?;
        let (events, _) = rec.events(&id, 1, usize::MAX)?;
        let query = format!("exp={}&sig={}", q.exp, q.sig);
        anyhow::Ok(recording::replay_html(&meta, &events, rec.video(&id).as_ref(), &query))
    })
    .await;
    match page {
        Ok(Ok(html)) => axum::response::Html(html).into_response(),
        _ => (StatusCode::NOT_FOUND, "recording not found").into_response(),
    }
}

async fn replay_frame(
    State(state): State<McpState>,
    Path((id, file)): Path<(String, String)>,
    Query(q): Query<ShareQuery>,
) -> Response {
    if !share_ok(&state, &id, &q) {
        return (StatusCode::FORBIDDEN, "link is invalid or expired").into_response();
    }
    let Ok(path) = state.handler.recorder().frame_path(&id, &file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = if file.ends_with(".png") { "image/png" } else { "image/jpeg" };
            ([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serves the session video with Range support, so players can seek.
async fn replay_video(
    State(state): State<McpState>,
    Path((id, file)): Path<(String, String)>,
    Query(q): Query<ShareQuery>,
    req: axum::extract::Request,
) -> Response {
    use tower::ServiceExt;
    if !share_ok(&state, &id, &q) {
        return (StatusCode::FORBIDDEN, "link is invalid or expired").into_response();
    }
    let rec = state.handler.recorder();
    let (Some(video), Ok(dir)) = (rec.video(&id), rec.session_path(&id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if video.file != file {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tower_http::services::ServeFile::new(dir.join(video.file)).oneshot(req).await {
        Ok(resp) => resp.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Front page: live view links, how to connect, and (without tokens) recent recordings.
async fn home(State(state): State<McpState>) -> Response {
    let view = crate::liveview::watch_url();
    let takeover = crate::liveview::takeover_url();
    let handler = Arc::clone(&state.handler);
    let open = state.tokens.is_empty();
    let html = tokio::task::spawn_blocking(move || {
        let rec = handler.recorder();
        let mcp = format!("{}/mcp", rec.public_url());
        let recent = if !open {
            "<p class=\"sub\">Tokens are on, so recordings are not listed here. Ask your agent for <code>recording_share</code>.</p>".to_string()
        } else {
            let rows: String = rec
                .list(None, 20)
                .into_iter()
                .filter_map(|(m, steps)| {
                    let (page, video) = rec.share_url(&m.id, 24 * 3600).ok()?;
                    let video = video.map(|v| format!(r#" &middot; <a href="{}">video</a>"#, recording::esc(&v))).unwrap_or_default();
                    Some(format!(
                        r#"<li><a href="{}">{}</a> <span class="sub">{} &middot; {} steps{}</span>{}</li>"#,
                        recording::esc(&page),
                        recording::esc(&m.persona),
                        m.started_at.format("%Y-%m-%d %H:%M UTC"),
                        steps,
                        if m.ended_at.is_none() { " &middot; live" } else { "" },
                        video,
                    ))
                })
                .collect();
            if rows.is_empty() { "<p class=\"sub\">No sessions yet.</p>".into() } else { format!("<ul>{rows}</ul>") }
        };
        recording::home_html(&view, &takeover, &mcp, &recent)
    })
    .await
    .unwrap_or_default();
    axum::response::Html(html).into_response()
}

pub async fn start_server(
    config: McpConfig,
    handler: Arc<ToolHandler>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let state = McpState {
        handler,
        tokens: Arc::new(config.tokens),
    };

    let app = Router::new()
        .route("/mcp", post(handle_mcp))
        .route("/recordings/:id", get(replay_page))
        .route("/recordings/:id/frames/:file", get(replay_frame))
        .route("/recordings/:id/video/:file", get(replay_video))
        .route("/", get(home))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.port).parse()?;
    info!(addr = %addr, "MCP server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("MCP server error: {e}");
        }
    });

    Ok(handle)
}

pub fn generate_mcp_config_json(host: &str, port: u16, token: Option<&str>) -> Value {
    let mut config = json!({
        "mcpServers": {
            "taboom": {
                "url": format!("http://{host}:{port}/mcp"),
            }
        }
    });

    if let Some(tok) = token {
        config["mcpServers"]["taboom"]["headers"] = json!({
            "Authorization": format!("Bearer {tok}")
        });
    }

    config
}

pub fn generate_claude_code_config(host: &str, port: u16, token: Option<&str>) -> Value {
    let mut server = json!({
        "type": "http",
        "url": format!("http://{host}:{port}/mcp"),
    });
    if let Some(tok) = token {
        server["headers"] = json!({
            "Authorization": format!("Bearer {tok}")
        });
    }
    json!({
        "mcpServers": {
            "taboom": server
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_token_extraction() {
        let tokens: HashMap<String, String> = [
            ("agent-1".to_string(), "secret-token-123".to_string()),
        ]
        .into();

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer secret-token-123".parse().unwrap());
        assert_eq!(
            extract_client_id(&headers, &tokens),
            Some("agent-1".to_string())
        );
    }

    #[test]
    fn auth_token_missing() {
        let tokens: HashMap<String, String> = [
            ("agent-1".to_string(), "secret-token-123".to_string()),
        ]
        .into();

        let headers = HeaderMap::new();
        assert_eq!(extract_client_id(&headers, &tokens), None);
    }

    #[test]
    fn auth_token_wrong() {
        let tokens: HashMap<String, String> = [
            ("agent-1".to_string(), "secret-token-123".to_string()),
        ]
        .into();

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer wrong-token".parse().unwrap());
        assert_eq!(extract_client_id(&headers, &tokens), None);
    }

    #[test]
    fn tokens_from_env_spec() {
        let tokens = parse_tokens("agent-a=tok1, agent-b=tok2,broken,=x");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens["agent-b"], "tok2");
        assert!(parse_tokens("").is_empty());
    }

    #[test]
    fn images_become_image_blocks() {
        let mut result = ToolResult::ok(json!({ "width": 10 }));
        result.images.push(crate::handler::Image { base64: "AAAA".into(), mime: "image/png" });
        let content = mcp_content(&result);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["mimeType"], "image/png");
        assert_eq!(content[0]["data"], "AAAA");
        assert_eq!(content[1]["text"], "{\"width\":10}");
        assert_eq!(mcp_content(&ToolResult::ok(json!({ "ok": true })))[0]["type"], "text");
    }

    #[test]
    fn tokens_from_env_spec() {
        let tokens = parse_tokens("agent-a=tok1, agent-b=tok2,broken,=x");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens["agent-b"], "tok2");
        assert!(parse_tokens("").is_empty());
    }

    #[test]
    fn screenshot_becomes_image_block() {
        let content = mcp_content(&json!({
            "type": "image", "media_type": "image/png", "data": "AAAA", "width": 10, "height": 5,
        }));
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["mimeType"], "image/png");
        assert_eq!(content[0]["data"], "AAAA");
        assert!(content[1]["text"].as_str().unwrap().contains("\"width\":10"));
        assert_eq!(mcp_content(&json!({ "ok": true }))[0]["type"], "text");
    }

    #[test]
    fn mcp_config_json_format() {
        let config = generate_mcp_config_json("localhost", 3456, Some("tok"));
        let servers = config.get("mcpServers").unwrap();
        let taboom = servers.get("taboom").unwrap();
        assert_eq!(taboom.get("url").unwrap(), "http://localhost:3456/mcp");
    }

    #[test]
    fn claude_code_config_format() {
        let config = generate_claude_code_config("localhost", 3456, None);
        let servers = config.get("mcpServers").unwrap();
        let taboom = servers.get("taboom").unwrap();
        assert_eq!(taboom.get("type").unwrap(), "http");
        assert!(taboom.get("headers").is_none());
    }
}
