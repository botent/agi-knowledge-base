pub mod config;
pub mod oauth;

use anyhow::{anyhow, Context, Result};
use mcp_protocol_sdk::client::McpClient;
use mcp_protocol_sdk::protocol::types::Tool as McpTool;
use mcp_protocol_sdk::transport::http::HttpClientTransport;
use mcp_protocol_sdk::transport::traits::TransportConfig;
use serde_json::{json, Value};
use url::Url;

use crate::constants::{APP_NAME, APP_VERSION};
use crate::mcp::config::McpServer;
use crate::util::normalize_url;

pub struct McpConnection {
    pub server: McpServer,
    pub client: McpClient,
    pub tool_cache: Vec<McpTool>,
}

pub async fn connect_http(server: &McpServer, bearer: Option<String>) -> Result<McpConnection> {
    let (base_url, sse_url) = derive_http_urls(server)?;

    let mut config = TransportConfig::default();
    if let Some(headers) = &server.headers {
        for (key, value) in headers {
            config.headers.insert(key.clone(), value.clone());
        }
    }
    if let Some(token) = bearer {
        config
            .headers
            .insert("Authorization".to_string(), format!("Bearer {token}"));
    }

    let transport = HttpClientTransport::with_config(
        base_url.as_str(),
        sse_url.as_deref(),
        config,
    )
    .await
    .with_context(|| format!("create HTTP transport (base: {base_url}, sse: {sse_url:?})"))?;
    let mut client = McpClient::new(APP_NAME.to_string(), APP_VERSION.to_string());
    client
        .connect(transport)
        .await
        .with_context(|| format!("connect MCP at {base_url}"))?;

    Ok(McpConnection {
        server: server.clone(),
        client,
        tool_cache: Vec::new(),
    })
}

fn derive_http_urls(server: &McpServer) -> Result<(String, Option<String>)> {
    let raw = normalize_url(&server.url);
    let url = Url::parse(&raw).context("parse MCP server url")?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/mcp") {
        path = path.trim_end_matches("/mcp").to_string();
    }
    if path == "/" {
        path.clear();
    }

    let mut base = url.clone();
    base.set_path(if path.is_empty() { "" } else { &path });
    base.set_query(None);
    base.set_fragment(None);
    let base_url = base.to_string().trim_end_matches('/').to_string();

    let sse_url = if let Some(sse) = &server.sse_url {
        Some(normalize_url(sse))
    } else {
        let mut sse = url.clone();
        let sse_path = if path.is_empty() {
            "/mcp/events".to_string()
        } else {
            format!("{path}/mcp/events")
        };
        sse.set_path(&sse_path);
        sse.set_query(None);
        sse.set_fragment(None);
        Some(sse.to_string())
    };

    Ok((base_url, sse_url))
}

pub async fn refresh_tools(connection: &mut McpConnection) -> Result<Vec<McpTool>> {
    let tools = connection
        .client
        .list_tools(None)
        .await
        .context("list MCP tools")?;
    connection.tool_cache = tools.tools.clone();
    Ok(tools.tools)
}

pub async fn call_tool(connection: &McpConnection, tool: &str, args: Value) -> Result<Value> {
    let arg_map = match args {
        Value::Null => None,
        Value::Object(map) => Some(map.into_iter().collect()),
        other => return Err(anyhow!("Tool args must be JSON object, got {other}")),
    };

    let result = connection
        .client
        .call_tool(tool.to_string(), arg_map)
        .await
        .context("call MCP tool")?;

    let value = serde_json::to_value(result).context("serialize tool result")?;
    Ok(value)
}

pub fn tools_to_openai(tools: &[McpTool]) -> Result<Vec<Value>> {
    let mut openai_tools = Vec::new();
    for tool in tools {
        let parameters =
            serde_json::to_value(&tool.input_schema).context("serialize tool schema")?;
        openai_tools.push(json!({
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": parameters
        }));
    }
    Ok(openai_tools)
}
