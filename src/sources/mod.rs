//! Connected sources the model can query while answering: MCP servers from
//! `mcp.json` and connectors built into Context, such as Google Calendar.
//!
//! Context decides which tools the model may call. An MCP server's tools are
//! offered only when the server marks them read-only, or when the user names
//! them in that server's `tools` list. Built-in connectors are read-only.

pub mod catalog;
mod google;
mod mcp;
mod oauth;

use std::time::Duration;

use reqwest::blocking::Client;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};

use crate::Result;

pub use mcp::config_path;

/// The display name of a built-in connector, by its ID in settings.
pub fn connector_name(id: &str) -> Option<&'static str> {
    (id == google::ID).then_some(google::NAME)
}

/// The settings ID of a built-in connector, by its display name.
pub fn connector_id(name: &str) -> Option<&'static str> {
    (name == google::NAME).then_some(google::ID)
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// The Responses API limits function names to this length.
const MAX_TOOL_NAME: usize = 64;

/// A tool as the model sees it.
#[derive(Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// What the main window shows for each source.
#[derive(Clone)]
pub struct SourceStatus {
    pub name: String,
    /// Tools the model may call.
    pub enabled: usize,
    /// Tools left out because they are not marked read-only.
    pub hidden: usize,
    pub problem: Option<String>,
    /// The source answered that it needs the user to sign in.
    pub needs_sign_in: bool,
    /// Context holds a sign-in for this source.
    pub signed_in: bool,
    /// A connector built into Context rather than an MCP server.
    pub builtin: bool,
}

/// A tool the model sees, with the name it uses to call it.
struct Offer {
    model_name: String,
    server: usize,
    tool: ToolDef,
}

enum Kind {
    Mcp {
        config: Box<mcp::ServerConfig>,
        connection: Option<mcp::Connection>,
        /// What the server's 401 asked for, when it needs a sign-in.
        auth: Option<mcp::Challenge>,
    },
    Google,
}

struct Server {
    name: String,
    kind: Kind,
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
    http: Client,
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
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            runtime,
            http,
            servers: Vec::new(),
            offers: Vec::new(),
            config_problem: None,
        })
    }

    /// Disconnects everything, then connects the servers in `mcp.json` and the
    /// built-in `connectors` the user added.
    pub fn reload(&mut self, connectors: &[String]) {
        self.disconnect();
        if connectors.iter().any(|id| id == google::ID) {
            self.add_google();
        }
        let config = match mcp::read_config() {
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
            if !server.disabled {
                self.add_mcp(name, server);
            }
        }
    }

    fn add_google(&mut self) {
        let index = self.servers.len();
        let problem = (!google::signed_in()).then(|| "Not signed in".to_owned());
        if problem.is_none() {
            for tool in google::tools() {
                let model_name = self.unique_name("calendar", &tool.name);
                self.offers.push(Offer {
                    model_name,
                    server: index,
                    tool,
                });
            }
        }
        self.servers.push(Server {
            name: google::NAME.into(),
            kind: Kind::Google,
            hidden: 0,
            problem,
        });
    }

    fn add_mcp(&mut self, name: String, config: mcp::ServerConfig) {
        let index = self.servers.len();
        let url = config.url.clone();
        // Use a stored sign-in for remote servers, refreshed if needed. This
        // blocks, so it happens before entering the async runtime.
        let bearer = match url.as_deref().filter(|_| !config.has_authorization()) {
            Some(url) => match oauth::access_token(&self.http, url) {
                Ok(token) => token,
                Err(error) => {
                    crate::debug(format_args!("source {name}: {error}"));
                    None
                }
            },
            None => None,
        };
        let connected = self.runtime.block_on(async {
            tokio::time::timeout(CONNECT_TIMEOUT, mcp::connect(&config, bearer.as_deref())).await
        });
        let (connection, tools) = match connected {
            Ok(Ok(connected)) => connected,
            failure => {
                let problem = match failure {
                    Ok(Err(error)) => {
                        crate::debug(format_args!("source {name} failed: {error:?}"));
                        error.to_string()
                    }
                    _ => "The server did not respond in time.".into(),
                };
                // A remote server that rejects us may just need a sign-in.
                let auth = url
                    .as_deref()
                    .filter(|_| !config.has_authorization())
                    .and_then(|url| mcp::needs_sign_in(&self.http, url));
                let problem = if auth.is_some() {
                    "Sign in to connect".to_owned()
                } else {
                    problem
                };
                self.servers.push(Server {
                    name,
                    kind: Kind::Mcp {
                        config: Box::new(config),
                        connection: None,
                        auth,
                    },
                    hidden: 0,
                    problem: Some(problem),
                });
                return;
            }
        };
        let mut hidden = 0;
        for tool in tools {
            if mcp::allowed(&tool, config.tools.as_deref()) {
                let model_name = self.unique_name(&name, &tool.name);
                self.offers.push(Offer {
                    model_name,
                    server: index,
                    tool: ToolDef {
                        name: tool.name.to_string(),
                        description: tool
                            .description
                            .as_deref()
                            .unwrap_or("No description.")
                            .to_owned(),
                        schema: Value::Object((*tool.input_schema).clone()),
                    },
                });
            } else {
                hidden += 1;
            }
        }
        self.servers.push(Server {
            name,
            kind: Kind::Mcp {
                config: Box::new(config),
                connection: Some(connection),
                auth: None,
            },
            hidden,
            problem: None,
        });
    }

    pub fn status(&self) -> (Vec<SourceStatus>, Option<String>) {
        let servers = self
            .servers
            .iter()
            .enumerate()
            .map(|(index, server)| {
                let (needs_sign_in, signed_in, builtin) = match &server.kind {
                    Kind::Mcp { config, auth, .. } => (
                        auth.is_some(),
                        config.url.as_deref().is_some_and(oauth::signed_in),
                        false,
                    ),
                    Kind::Google => (!google::signed_in(), google::signed_in(), true),
                };
                SourceStatus {
                    name: server.name.clone(),
                    enabled: self.offers.iter().filter(|o| o.server == index).count(),
                    hidden: server.hidden,
                    problem: server.problem.clone(),
                    needs_sign_in,
                    signed_in,
                    builtin,
                }
            })
            .collect();
        (servers, self.config_problem.clone())
    }

    /// Signs in to the named source in the browser. Blocks until done.
    pub fn sign_in(&mut self, name: &str, prompt: &impl Fn(String)) -> Result<()> {
        let server = self
            .servers
            .iter()
            .find(|server| server.name == name)
            .ok_or("That source is no longer configured.")?;
        let (config, challenge) = match &server.kind {
            Kind::Google => return google::sign_in(&self.http),
            Kind::Mcp { config, auth, .. } => (config, auth.clone().unwrap_or_default()),
        };
        let url = config
            .url
            .as_deref()
            .ok_or("Only remote servers sign in.")?;
        let (endpoints, _supported) =
            oauth::discover(&self.http, url, challenge.metadata.as_deref())?;
        let builtin = mcp::builtin_client(&endpoints.issuer);
        let (client, scopes) = match (&config.oauth, &builtin) {
            (Some(own), _) => (
                Some(oauth::ClientId {
                    id: own.client_id.clone(),
                    secret: own.client_secret.clone(),
                }),
                own.scopes.clone(),
            ),
            (None, Some((client, scopes))) => (Some(client.clone()), Some((*scopes).to_owned())),
            // Otherwise ask for what the server's 401 asked for.
            (None, None) => (None, challenge.scope.clone()),
        };
        if endpoints.issuer.trim_end_matches('/') == "https://github.com/login/oauth" {
            let client = client
                .ok_or("This build has no GitHub client ID. Add one under oauth in mcp.json.")?;
            let grant = oauth::github_sign_in(&self.http, &client, scopes.as_deref(), prompt)?;
            return oauth::save(url, &grant);
        }
        crate::debug(format_args!(
            "signing in to {name} at {} ({})",
            endpoints.issuer,
            if client.is_some() {
                "registered app"
            } else {
                "registering"
            }
        ));
        let grant = oauth::sign_in(
            &self.http,
            oauth::SignIn {
                endpoints: &endpoints,
                client,
                scopes: scopes.as_deref(),
                resource: Some(url),
                extra: &[],
                name,
            },
        )?;
        oauth::save(url, &grant)
    }

    /// Searches the MCP Registry for sources to add.
    pub fn search(&self, query: &str) -> Result<Vec<catalog::Entry>> {
        catalog::search(&self.http, query)
    }

    /// Forgets the sign-in for the named source.
    pub fn sign_out(&mut self, name: &str) -> Result<()> {
        match self
            .servers
            .iter()
            .find(|server| server.name == name)
            .map(|s| &s.kind)
        {
            Some(Kind::Google) => google::sign_out(),
            Some(Kind::Mcp { config, .. }) => match config.url.as_deref() {
                Some(url) => oauth::forget(url),
                None => Ok(()),
            },
            None => Ok(()),
        }
    }

    /// Function definitions for the Responses API.
    pub fn definitions(&self) -> Vec<Value> {
        self.offers
            .iter()
            .map(|offer| {
                let server = &self.servers[offer.server].name;
                json!({
                    "type": "function",
                    "name": offer.model_name,
                    "description": format!("From the {server} source. {}", offer.tool.description),
                    "parameters": offer.tool.schema,
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
        let arguments = match serde_json::from_str::<Value>(arguments) {
            Ok(Value::Object(map)) => map,
            Ok(Value::Null) | Err(_) if arguments.trim().is_empty() => Default::default(),
            _ => return failed("The tool arguments were not a JSON object.".into()),
        };
        let connection = match &server.kind {
            Kind::Google => {
                return match google::call(&self.http, &offer.tool.name, &Value::Object(arguments)) {
                    Ok(text) => ToolOutput {
                        text: mcp::cut(&text),
                        source: Some(server.name.clone()),
                    },
                    Err(error) => failed(format!("The tool call failed: {error}")),
                };
            }
            Kind::Mcp { connection, .. } => connection,
        };
        let Some(connection) = connection else {
            return failed(format!("The {} source is not connected.", server.name));
        };
        let request = CallToolRequestParams::new(offer.tool.name.clone()).with_arguments(arguments);
        let result = self.runtime.block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, connection.call_tool(request)).await
        });
        match result {
            Ok(Ok(result)) => {
                let text = mcp::result_text(&result);
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
        let connections: Vec<_> = self
            .servers
            .drain(..)
            .filter_map(|server| match server.kind {
                Kind::Mcp { connection, .. } => connection,
                Kind::Google => None,
            })
            .collect();
        // Cancelling closes each server's stdin, which ends stdio servers.
        self.runtime.block_on(async {
            for connection in connections {
                let _ = tokio::time::timeout(Duration::from_secs(2), connection.cancel()).await;
            }
        });
    }
}

impl Drop for Sources {
    fn drop(&mut self) {
        self.disconnect();
    }
}
