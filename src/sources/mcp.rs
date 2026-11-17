//! MCP servers listed in `mcp.json`: reading the config, connecting over stdio
//! or streamable HTTP, and noticing when a server needs the user to sign in.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use reqwest::blocking::Client;
use rmcp::RoleClient;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion, Tool};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde::Deserialize;
use serde_json::json;

use super::oauth::ClientId;
use crate::Result;
use crate::storage::{self, Record};

const CONFIG_FILE: &str = "mcp.json";
/// Tool output beyond this is cut so one result cannot crowd out the rest.
const MAX_OUTPUT_CHARS: usize = 12_000;

pub type Connection = RunningService<RoleClient, ClientConfig>;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, ServerConfig>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Program for a local server that speaks MCP over stdio.
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Address of a remote server that speaks streamable HTTP.
    pub url: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Tools the model may call. When absent, only tools the server marks
    /// read-only are offered.
    pub tools: Option<Vec<String>>,
    /// A registered OAuth app for servers that cannot register Context.
    pub oauth: Option<OAuthConfig>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
}

impl ServerConfig {
    /// Whether `mcp.json` sets its own Authorization header for this server.
    pub fn has_authorization(&self) -> bool {
        self.headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
    }
}

/// Where `mcp.json` lives. Creates an empty one on first use so it can be edited.
pub fn config_path() -> Result<PathBuf> {
    let path = storage::config_dir()?.join(CONFIG_FILE);
    if !path.exists() {
        std::fs::write(&path, "{\n  \"mcpServers\": {}\n}\n")?;
    }
    Ok(path)
}

pub fn read_config() -> Result<Config> {
    let text = std::fs::read_to_string(config_path()?)?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{CONFIG_FILE} could not be read: {error}").into())
}

/// Context's GitHub OAuth app. Device flow needs no secret, and a client ID is
/// public, so it ships in the code.
const GITHUB_CLIENT_ID: &str = "Ov23lim0dhPPM7VljAnZ";

/// The registered app Context uses for well-known providers that cannot
/// register clients themselves, with the scopes it asks for.
/// `CONTEXT_GITHUB_CLIENT_ID`, at build or run time, replaces the default.
pub fn builtin_client(issuer: &str) -> Option<(ClientId, &'static str)> {
    if issuer.trim_end_matches('/') != "https://github.com/login/oauth" {
        return None;
    }
    let id = option_env!("CONTEXT_GITHUB_CLIENT_ID")
        .map(str::to_owned)
        .or_else(|| std::env::var("CONTEXT_GITHUB_CLIENT_ID").ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| GITHUB_CLIENT_ID.to_owned());
    Some((ClientId { id, secret: None }, "repo read:org"))
}

pub fn allowed(tool: &Tool, listed: Option<&[String]>) -> bool {
    match listed {
        Some(names) => names.iter().any(|name| *name == tool.name),
        None => tool
            .annotations
            .as_ref()
            .is_some_and(|notes| notes.read_only_hint == Some(true)),
    }
}

/// Connects and lists tools. `bearer` is an OAuth access token to send when
/// `mcp.json` sets no Authorization header of its own.
pub async fn connect(
    config: &ServerConfig,
    bearer: Option<&str>,
) -> Result<(Connection, Vec<Tool>)> {
    // Offer the newest protocol first. Some servers, including GitHub's hosted
    // one, negotiate down from it and then reject their own session, so retry
    // once offering the last version that still uses `initialize`.
    let client = match handshake(config, bearer, None).await {
        Ok(client) => client,
        Err(first) => {
            crate::debug(format_args!(
                "handshake with the newest protocol failed ({first}); retrying with {}",
                ProtocolVersion::LATEST_WITH_INITIALIZE
            ));
            handshake(
                config,
                bearer,
                Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
            )
            .await
            .map_err(|_| first)?
        }
    };
    let tools = client.list_all_tools().await?;
    Ok((client, tools))
}

async fn handshake(
    config: &ServerConfig,
    bearer: Option<&str>,
    version: Option<ProtocolVersion>,
) -> Result<Connection> {
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
            if let Some(token) = bearer.filter(|_| !config.has_authorization()) {
                transport
                    .custom_headers
                    .insert("authorization".parse()?, format!("Bearer {token}").parse()?);
            }
            identity
                .serve(StreamableHttpClientTransport::from_config(transport))
                .await?
        }
        _ => return Err("Set either `command` or `url` for this server.".into()),
    })
}

/// What a server's 401 said about signing in.
#[derive(Clone, Default)]
pub struct Challenge {
    /// The `resource_metadata` address, if the server gave one.
    pub metadata: Option<String>,
    /// The scopes the server asked for, if any.
    pub scope: Option<String>,
}

/// Asks a remote server whether it needs sign-in, and what it asked for.
pub fn needs_sign_in(http: &Client, url: &str) -> Option<Challenge> {
    let response = http
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": ProtocolVersion::LATEST_WITH_INITIALIZE.to_string(),
                "capabilities": {},
                "clientInfo": {"name": "Context", "version": env!("CARGO_PKG_VERSION")},
            },
        }))
        .send()
        .ok()?;
    if response.status().as_u16() != 401 {
        return None;
    }
    let header = response
        .headers()
        .get("www-authenticate")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    Some(Challenge {
        metadata: quoted(&header, "resource_metadata"),
        scope: quoted(&header, "scope"),
    })
}

/// A quoted parameter from a `WWW-Authenticate` header, such as `scope="read"`.
fn quoted(header: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = header
        .match_indices(&marker)
        // Skip longer names that end the same way, such as `xscope`.
        .find(|(at, _)| *at == 0 || matches!(header.as_bytes()[at - 1], b' ' | b','))?
        .0
        + marker.len();
    let end = header[start..].find('"')? + start;
    Some(header[start..end].to_owned()).filter(|value| !value.is_empty())
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

/// Replaces each `${NAME}` with that environment variable and each
/// `${secret:NAME}` with a secret Context stores encrypted, so secrets stay out
/// of `mcp.json`. Anything missing is an error, never an empty value.
pub fn expand(value: &str) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("`{value}` has an unclosed `${{`."))?;
        let name = &after[..end];
        let resolved = match name.strip_prefix("secret:") {
            Some(secret) => load_secret(secret)?.ok_or_else(|| {
                format!("The setting `{secret}` is missing. Add it again in Settings.")
            })?,
            None => std::env::var(name).map_err(|_| {
                format!(
                    "The environment variable `{name}` is not set. Set it, then restart Context."
                )
            })?,
        };
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn save_secret(name: &str, value: &str) -> Result<()> {
    storage::save(Record::SourceSecret(storage::key(name)), &value.to_owned())
}

pub fn load_secret(name: &str) -> Result<Option<String>> {
    storage::load(Record::SourceSecret(storage::key(name)))
}

pub fn forget_secret(name: &str) -> Result<()> {
    storage::remove(Record::SourceSecret(storage::key(name)))
}

/// Joins a tool result's text into one string for the model.
pub fn result_text(result: &rmcp::model::CallToolResult) -> String {
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
    cut(&parts.join("\n\n"))
}

/// Cuts tool output to a size the model can afford.
pub fn cut(text: &str) -> String {
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
