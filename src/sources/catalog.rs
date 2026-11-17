//! Finding and adding sources: a short featured list, search through the
//! public MCP Registry, and edits to `mcp.json` that keep whatever else the
//! user wrote there. Secrets typed into the add form are stored encrypted, and
//! `mcp.json` refers to them as `${secret:NAME}`.

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{google, mcp, oauth};
use crate::Result;

const REGISTRY: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub install: Install,
    /// Settings to ask for before adding.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Whether the user signs in with the provider after adding.
    #[serde(default)]
    pub sign_in: bool,
    /// False when this build cannot add the entry; `note` says why.
    #[serde(default = "yes")]
    pub available: bool,
    pub note: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Install {
    /// A connector built into Context, such as Google Calendar.
    Builtin { connector: String },
    /// A hosted server reached over streamable HTTP.
    Remote { url: String },
    /// A local server started with a command.
    Package { command: String, args: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    /// An HTTP header for remote servers, or an environment variable for packages.
    #[serde(default)]
    pub header: bool,
    /// Text placed before the value, such as `Bearer `.
    #[serde(default)]
    pub prefix: String,
}

/// Hosted servers that let Context register itself, so adding one is a
/// sign-in and nothing else, plus the built-in connectors.
pub fn featured() -> Vec<Entry> {
    let remote = |id: &str, name: &str, description: &str, url: &str| Entry {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        install: Install::Remote { url: url.into() },
        fields: Vec::new(),
        sign_in: true,
        available: true,
        note: None,
    };
    let google = Entry {
        id: "builtin:google-calendar".into(),
        name: google::NAME.into(),
        description: "Check your availability and what's planned. Read-only.".into(),
        install: Install::Builtin {
            connector: google::ID.into(),
        },
        fields: Vec::new(),
        sign_in: true,
        available: google::available(),
        note: (!google::available())
            .then(|| "This build of Context has no Google sign-in yet.".into()),
    };
    let github_oauth = mcp::builtin_client("https://github.com/login/oauth").is_some();
    let github = Entry {
        id: "remote:github".into(),
        name: "GitHub".into(),
        description: "Issues, pull requests and code in your repositories. Read-only.".into(),
        install: Install::Remote {
            url: "https://api.githubcopilot.com/mcp/readonly".into(),
        },
        // Without Context's GitHub app, sign in with a personal access token.
        fields: if github_oauth {
            Vec::new()
        } else {
            vec![Field {
                name: "Authorization".into(),
                description: "A GitHub personal access token with read access".into(),
                secret: true,
                required: true,
                header: true,
                prefix: "Bearer ".into(),
            }]
        },
        sign_in: github_oauth,
        available: true,
        note: None,
    };
    vec![
        google,
        github,
        remote(
            "remote:notion",
            "Notion",
            "Pages and databases in your Notion workspace.",
            "https://mcp.notion.com/mcp",
        ),
        remote(
            "remote:linear",
            "Linear",
            "Issues, projects and cycles in Linear.",
            "https://mcp.linear.app/mcp",
        ),
        remote(
            "remote:sentry",
            "Sentry",
            "Errors and issues from your Sentry projects.",
            "https://mcp.sentry.dev/mcp",
        ),
        remote(
            "remote:figma",
            "Figma",
            "Designs and comments in your Figma files.",
            "https://mcp.figma.com/mcp",
        ),
    ]
}

/// Searches the MCP Registry. Results are data from third parties; adding one
/// is the user's choice, and its command or address is shown before adding.
pub fn search(http: &Client, query: &str) -> Result<Vec<Entry>> {
    let body: Value = http
        .get(REGISTRY)
        .query(&[("search", query), ("limit", "30")])
        .send()?
        .error_for_status()?
        .json()?;
    let mut entries = Vec::new();
    for item in body["servers"].as_array().into_iter().flatten() {
        let server = &item["server"];
        if item["_meta"]["io.modelcontextprotocol.registry/official"]["isLatest"] == false {
            continue;
        }
        if let Some(entry) = from_registry(server) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn from_registry(server: &Value) -> Option<Entry> {
    let id = server["name"].as_str()?.to_owned();
    let name = server["title"]
        .as_str()
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id))
        .to_owned();
    let description = server["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let field = |item: &Value, header: bool| Field {
        name: item["name"].as_str().unwrap_or_default().to_owned(),
        description: item["description"].as_str().unwrap_or_default().to_owned(),
        secret: item["isSecret"] == true,
        required: item["isRequired"] == true,
        header,
        prefix: String::new(),
    };
    if let Some(remote) = server["remotes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|remote| remote["type"] == "streamable-http")
    {
        let fields: Vec<Field> = remote["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|header| field(header, true))
            .collect();
        return Some(Entry {
            id: format!("registry:{id}"),
            name,
            description,
            install: Install::Remote {
                url: remote["url"].as_str()?.to_owned(),
            },
            // Servers that need no headers usually sign in with OAuth.
            sign_in: fields.is_empty(),
            fields,
            available: true,
            note: None,
        });
    }
    let package = server["packages"].as_array()?.iter().find(|package| {
        package["transport"]["type"] == "stdio"
            && matches!(package["registryType"].as_str(), Some("npm" | "pypi"))
    });
    let Some(package) = package else {
        return Some(Entry {
            id: format!("registry:{id}"),
            name,
            description,
            install: Install::Remote { url: String::new() },
            fields: Vec::new(),
            sign_in: false,
            available: false,
            note: Some("Context can't install this kind of server yet.".into()),
        });
    };
    let identifier = package["identifier"].as_str()?;
    let version = package["version"].as_str();
    let (command, args) = match package["registryType"].as_str()? {
        "npm" => (
            "npx",
            vec![
                "-y".to_owned(),
                version.map_or(identifier.to_owned(), |v| format!("{identifier}@{v}")),
            ],
        ),
        _ => (
            "uvx",
            vec![version.map_or(identifier.to_owned(), |v| format!("{identifier}=={v}"))],
        ),
    };
    let runtime = if command == "npx" {
        "Node.js"
    } else {
        "uv (Python)"
    };
    let available = which::which(command).is_ok();
    Some(Entry {
        id: format!("registry:{id}"),
        name,
        description,
        install: Install::Package {
            command: command.into(),
            args,
        },
        fields: package["environmentVariables"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|variable| field(variable, false))
            .collect(),
        sign_in: false,
        available,
        note: (!available).then(|| format!("Needs {runtime} installed.")),
    })
}

/// Adds a catalog entry. Built-in connectors are returned for the caller to
/// enable; servers are written to `mcp.json`. Returns the source's name.
pub fn add(entry: &Entry, values: &Map<String, Value>) -> Result<Added> {
    if let Install::Builtin { connector } = &entry.install {
        return Ok(Added::Connector(connector.clone()));
    }
    let mut config = read_raw()?;
    let servers = config
        .as_object_mut()
        .ok_or("mcp.json is not a JSON object.")?
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("mcpServers in mcp.json is not an object.")?;
    let base = slug(&entry.name);
    let mut name = base.clone();
    let mut suffix = 2;
    while servers.contains_key(&name) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    let mut settings = Map::new();
    for field in &entry.fields {
        let value = values
            .get(&field.name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if value.is_empty() {
            if field.required {
                return Err(format!("{} is required.", field.name).into());
            }
            continue;
        }
        let stored = if field.secret {
            let secret = format!("{name}/{}", field.name);
            mcp::save_secret(&secret, value)?;
            format!("{}${{secret:{secret}}}", field.prefix)
        } else {
            format!("{}{value}", field.prefix)
        };
        settings.insert(field.name.clone(), Value::String(stored));
    }
    let server = match &entry.install {
        Install::Remote { url } => json!({"url": url, "headers": settings}),
        Install::Package { command, args } => {
            json!({"command": command, "args": args, "env": settings})
        }
        Install::Builtin { .. } => unreachable!("handled above"),
    };
    servers.insert(name.clone(), server);
    write_raw(&config)?;
    Ok(Added::Server(name))
}

pub enum Added {
    Connector(String),
    Server(String),
}

/// Removes a server from `mcp.json`, with its sign-in and stored secrets.
pub fn remove(name: &str) -> Result<()> {
    let mut config = read_raw()?;
    let Some(server) = config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(name))
    else {
        return Ok(());
    };
    if let Some(url) = server["url"].as_str() {
        let _ = oauth::forget(url);
    }
    for secret in secrets_in(&server) {
        let _ = mcp::forget_secret(&secret);
    }
    write_raw(&config)
}

/// The `${secret:NAME}` names a server's settings refer to.
fn secrets_in(server: &Value) -> Vec<String> {
    let text = server.to_string();
    let mut names = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("${secret:") {
        let after = &rest[start + "${secret:".len()..];
        let Some(end) = after.find('}') else { break };
        names.push(after[..end].to_owned());
        rest = &after[end..];
    }
    names
}

fn read_raw() -> Result<Value> {
    let text = std::fs::read_to_string(mcp::config_path()?)?;
    serde_json::from_str(&text)
        .map_err(|error| format!("mcp.json could not be read: {error}").into())
}

fn write_raw(config: &Value) -> Result<()> {
    std::fs::write(
        mcp::config_path()?,
        serde_json::to_string_pretty(config)? + "\n",
    )?;
    Ok(())
}

fn slug(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "source".into()
    } else {
        slug
    }
}
