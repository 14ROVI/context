//! Connected sources: MCP servers the model can query while answering.
//!
//! Servers are listed in `mcp.json` in Context's config folder, in the same
//! `mcpServers` shape other MCP clients use. Context decides which tools the
//! model may call. A server's tools are offered only when the server marks them
//! read-only, or when the user names them in that server's `tools` list.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion, Tool,
};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Result;

const CONFIG_FILE: &str = "mcp.json";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Tool output beyond this is cut so one result cannot crowd out the rest.
const MAX_OUTPUT_CHARS: usize = 12_000;
/// The Responses API limits function names to this length.
const MAX_TOOL_NAME: usize = 64;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    mcp_servers: BTreeMap<String, ServerConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfig {
    /// Program for a local server that speaks MCP over stdio.
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Address of a remote server that speaks streamable HTTP.
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    /// Tools the model may call. When absent, only tools the server marks
    /// read-only are offered.
    tools: Option<Vec<String>>,
    #[serde(default)]
    disabled: bool,
}

/// What the main window shows for each configured server.
#[derive(Clone)]
pub struct SourceStatus {
    pub name: String,
    /// Tools the model may call.
    pub enabled: usize,
    /// Tools left out because they are not marked read-only.
    pub hidden: usize,
    pub problem: Option<String>,
}

/// A tool the model sees, with the name it uses to call it.
struct Offer {
    model_name: String,
    server: usize,
    tool: Tool,
}

struct Server {
    name: String,
    client: Option<RunningService<RoleClient, ClientConfig>>,
    hidden: usize,
    problem: Option<String>,
}

/// The result of one tool call, ready to hand back to the model.
pub struct ToolOutput {
    pub text: String,
    /// A short label for the card, such as `calendar`. `None` when the call failed.
    pub source: Option<String>,
}

pub struct Sources {
    runtime: tokio::runtime::Runtime,
    servers: Vec<Server>,
    offers: Vec<Offer>,
    /// Problems reading `mcp.json` itself.
    config_problem: Option<String>,
}

impl Sources {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("context-sources")
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            servers: Vec::new(),
            offers: Vec::new(),
            config_problem: None,
        })
    }

    /// Disconnects every server, then reads `mcp.json` and connects again.
    pub fn reload(&mut self) {
        self.disconnect();
        let config = match read_config() {
            Ok(config) => config,
            Err(error) => {
                self.config_problem = Some(error.to_string());
                return;
            }
        };
        self.config_problem = None;
        crate::debug(format_args!(
            "sources: {} configured",
            config.mcp_servers.len()
        ));
        for (name, server) in config.mcp_servers {
            if server.disabled {
                continue;
            }
            let index = self.servers.len();
            let connected = self
                .runtime
                .block_on(async { tokio::time::timeout(CONNECT_TIMEOUT, connect(&server)).await });
            let (client, tools) = match connected {
                Ok(Ok(connected)) => connected,
                Ok(Err(error)) => {
                    crate::debug(format_args!("source {name} failed: {error:?}"));
                    self.servers.push(Server::failed(name, error.to_string()));
                    continue;
                }
                Err(_) => {
                    self.servers.push(Server::failed(
                        name,
                        "The server did not respond in time.".into(),
                    ));
                    continue;
                }
            };
            let mut hidden = 0;
            for tool in tools {
                if allowed(&tool, server.tools.as_deref()) {
                    let model_name = self.unique_name(&name, &tool.name);
                    self.offers.push(Offer {
                        model_name,
                        server: index,
                        tool,
                    });
                } else {
                    hidden += 1;
                }
            }
            self.servers.push(Server {
                name,
                client: Some(client),
                hidden,
                problem: None,
            });
        }
    }

    pub fn status(&self) -> (Vec<SourceStatus>, Option<String>) {
        let servers = self
            .servers
            .iter()
            .enumerate()
            .map(|(index, server)| SourceStatus {
                name: server.name.clone(),
                enabled: self.offers.iter().filter(|o| o.server == index).count(),
                hidden: server.hidden,
                problem: server.problem.clone(),
            })
            .collect();
        (servers, self.config_problem.clone())
    }

    /// Function definitions for the Responses API.
    pub fn definitions(&self) -> Vec<Value> {
        self.offers
            .iter()
            .map(|offer| {
                let server = &self.servers[offer.server].name;
                let description = offer
                    .tool
                    .description
                    .as_deref()
                    .unwrap_or("No description.");
                json!({
                    "type": "function",
                    "name": offer.model_name,
                    "description": format!("From the {server} source. {description}"),
                    "parameters": Value::Object((*offer.tool.input_schema).clone()),
                    "strict": false,
                })
            })
            .collect()
    }

    /// A friendly label for progress, such as `calendar`.
    pub fn label(&self, model_name: &str) -> Option<String> {
        let offer = self.offers.iter().find(|o| o.model_name == model_name)?;
        Some(self.servers[offer.server].name.clone())
    }

    /// Calls a tool the model chose. Failures become text the model can read.
    pub fn call(&self, model_name: &str, arguments: &str) -> ToolOutput {
        let failed = |text: String| ToolOutput { text, source: None };
        let Some(offer) = self.offers.iter().find(|o| o.model_name == model_name) else {
            return failed(format!("No tool named {model_name} is available."));
        };
        let server = &self.servers[offer.server];
        let Some(client) = &server.client else {
            return failed(format!("The {} source is not connected.", server.name));
        };
        let arguments = match serde_json::from_str::<Value>(arguments) {
            Ok(Value::Object(map)) => map,
            Ok(Value::Null) | Err(_) if arguments.trim().is_empty() => Default::default(),
            _ => return failed("The tool arguments were not a JSON object.".into()),
        };
        let request = CallToolRequestParams::new(offer.tool.name.clone()).with_arguments(arguments);
        let result = self.runtime.block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, client.call_tool(request)).await
        });
        match result {
            Ok(Ok(result)) => {
                let text = result_text(&result);
                if result.is_error == Some(true) {
                    failed(format!("The tool reported an error: {text}"))
                } else {
                    ToolOutput {
                        text,
                        source: Some(server.name.clone()),
                    }
                }
            }
            Ok(Err(error)) => failed(format!("The tool call failed: {error}")),
            Err(_) => failed("The tool did not respond in time.".into()),
        }
    }

    fn unique_name(&self, server: &str, tool: &str) -> String {
        let clean = |text: &str| -> String {
            text.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        let base: String = format!("{}__{}", clean(server), clean(tool))
            .chars()
            .take(MAX_TOOL_NAME - 3)
            .collect();
        let mut name = base.clone();
        let mut suffix = 2;
        while self.offers.iter().any(|offer| offer.model_name == name) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        name
    }

    fn disconnect(&mut self) {
        self.offers.clear();
        let clients: Vec<_> = self
            .servers
            .drain(..)
            .filter_map(|server| server.client)
            .collect();
        // Cancelling closes each server's stdin, which ends stdio servers.
        self.runtime.block_on(async {
            for client in clients {
                let _ = tokio::time::timeout(Duration::from_secs(2), client.cancel()).await;
            }
        });
    }
}

impl Drop for Sources {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl Server {
    fn failed(name: String, problem: String) -> Self {
        Self {
            name,
            client: None,
            hidden: 0,
            problem: Some(problem),
        }
    }
}

/// Where `mcp.json` lives. Creates an empty one on first use so it can be edited.
pub fn config_path() -> Result<PathBuf> {
    let path = crate::storage::config_dir()?.join(CONFIG_FILE);
    if !path.exists() {
        std::fs::write(&path, "{\n  \"mcpServers\": {}\n}\n")?;
    }
    Ok(path)
}

fn read_config() -> Result<Config> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{CONFIG_FILE} could not be read: {error}").into())
}

fn allowed(tool: &Tool, listed: Option<&[String]>) -> bool {
    match listed {
        Some(names) => names.iter().any(|name| *name == tool.name),
        None => tool
            .annotations
            .as_ref()
            .is_some_and(|notes| notes.read_only_hint == Some(true)),
    }
}

async fn connect(
    config: &ServerConfig,
) -> Result<(RunningService<RoleClient, ClientConfig>, Vec<Tool>)> {
    // Offer the newest protocol first. Some servers, including GitHub's hosted
    // one, negotiate down from it and then reject their own session, so retry
    // once offering the last version that still uses `initialize`.
    let client = match handshake(config, None).await {
        Ok(client) => client,
        Err(first) => {
            crate::debug(format_args!(
                "handshake with the newest protocol failed ({first}); retrying with {}",
                ProtocolVersion::LATEST_WITH_INITIALIZE
            ));
            handshake(config, Some(ProtocolVersion::LATEST_WITH_INITIALIZE))
                .await
                .map_err(|_| first)?
        }
    };
    let tools = client.list_all_tools().await?;
    Ok((client, tools))
}

async fn handshake(
    config: &ServerConfig,
    version: Option<ProtocolVersion>,
) -> Result<RunningService<RoleClient, ClientConfig>> {
    let mut identity = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("Context", env!("CARGO_PKG_VERSION")),
    );
    if let Some(version) = version {
        identity = identity.with_protocol_version(version);
    }
    Ok(match (&config.command, &config.url) {
        (Some(command), None) => {
            let transport = TokioChildProcess::builder(local_command(&expand(command)?, config)?)
                .stderr(std::process::Stdio::null())
                .spawn()?
                .0;
            identity.serve(transport).await?
        }
        (None, Some(url)) => {
            let mut transport = StreamableHttpClientTransportConfig::with_uri(expand(url)?);
            for (name, value) in &config.headers {
                transport
                    .custom_headers
                    .insert(name.parse()?, expand(value)?.parse()?);
            }
            identity
                .serve(StreamableHttpClientTransport::from_config(transport))
                .await?
        }
        _ => return Err("Set either `command` or `url` for this server.".into()),
    })
}

fn local_command(command: &str, config: &ServerConfig) -> Result<tokio::process::Command> {
    // `which` finds `npx.cmd` and similar launchers on Windows, as a shell would.
    let program = which::which(command)
        .map_err(|_| format!("`{command}` was not found. Check the command or your PATH."))?;
    let mut process = tokio::process::Command::new(program);
    for arg in &config.args {
        process.arg(expand(arg)?);
    }
    for (name, value) in &config.env {
        process.env(name, expand(value)?);
    }
    // Release builds on Windows have no console, so a console server would
    // otherwise open its own window. This flag has no equivalent elsewhere.
    #[cfg(windows)]
    process.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    Ok(process)
}

/// Replaces each `${NAME}` with that environment variable, so secrets can stay
/// out of `mcp.json`. A missing variable is an error, never an empty value.
fn expand(value: &str) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("`{value}` has an unclosed `${{`."))?;
        let name = &after[..end];
        let resolved = std::env::var(name).map_err(|_| {
            format!("The environment variable `{name}` is not set. Set it, then restart Context.")
        })?;
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Joins a tool result's text into one string for the model.
fn result_text(result: &rmcp::model::CallToolResult) -> String {
    let mut parts: Vec<String> = serde_json::to_value(&result.content)
        .ok()
        .and_then(|content| content.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|block| match block["type"].as_str() {
            Some("text") => block["text"].as_str().map(str::to_owned),
            Some("resource") => block["resource"]["text"].as_str().map(str::to_owned),
            _ => None,
        })
        .collect();
    if parts.is_empty()
        && let Some(structured) = &result.structured_content
    {
        parts.push(structured.to_string());
    }
    let text = parts.join("\n\n");
    let mut chars = text.chars();
    let mut cut: String = chars.by_ref().take(MAX_OUTPUT_CHARS).collect();
    if chars.next().is_some() {
        cut.push_str("\n[Output shortened by Context.]");
    }
    if cut.is_empty() {
        "The tool returned no text.".into()
    } else {
        cut
    }
}
