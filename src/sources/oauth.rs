//! OAuth sign-in for sources: MCP authorization (discovery, dynamic client
//! registration, PKCE) and built-in connectors such as Google Calendar. The
//! browser opens on the provider's page and returns to a one-time listener on
//! 127.0.0.1. Grants are stored encrypted, one record per source.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openidconnect::{CsrfToken, PkceCodeChallenge};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::Result;
use crate::callback::{Callback, parameter};
use crate::storage::{self, Record};

/// Refresh a token this long before it expires.
const EXPIRY_MARGIN: u64 = 60;

/// Where a provider signs people in and hands out tokens.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub issuer: String,
    pub authorization: String,
    pub token: String,
    pub registration: Option<String>,
}

/// An OAuth client. Desktop clients cannot keep a secret, so a secret here is
/// one the provider requires anyway, as Google does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientId {
    pub id: String,
    pub secret: Option<String>,
}

/// What a source signs in with: the client, scopes and extra parameters.
pub struct SignIn<'a> {
    pub endpoints: &'a Endpoints,
    /// `None` registers a client with the provider first.
    pub client: Option<ClientId>,
    pub scopes: Option<&'a str>,
    /// The protected resource, sent as RFC 8707 `resource`.
    pub resource: Option<&'a str>,
    pub extra: &'a [(&'a str, &'a str)],
    /// Shown in the browser tab when sign-in finishes, such as "GitHub".
    pub name: &'a str,
}

/// A stored sign-in.
#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client: ClientId,
    pub resource: Option<String>,
}

/// Finds the authorization server for a protected MCP server. `metadata` is
/// the `resource_metadata` address from the server's 401, when it sent one.
pub fn discover(http: &Client, server: &str, metadata: Option<&str>) -> Result<Endpoints> {
    let metadata = match metadata {
        Some(metadata) => metadata.to_owned(),
        None => well_known(server, "oauth-protected-resource")?,
    };
    let resource: Value = http.get(&metadata).send()?.error_for_status()?.json()?;
    let issuer = resource["authorization_servers"][0]
        .as_str()
        .ok_or("The server did not say where to sign in.")?;
    authorization_server(http, issuer)
}

/// Reads authorization server metadata, trying the RFC 8414 and OpenID
/// locations in the order the MCP specification gives.
pub fn authorization_server(http: &Client, issuer: &str) -> Result<Endpoints> {
    let candidates = [
        well_known(issuer, "oauth-authorization-server")?,
        well_known(issuer, "openid-configuration")?,
        format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ),
    ];
    for candidate in candidates {
        let Ok(response) = http.get(&candidate).send() else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(meta) = response.json::<Value>() else {
            continue;
        };
        let (Some(authorization), Some(token)) = (
            meta["authorization_endpoint"].as_str(),
            meta["token_endpoint"].as_str(),
        ) else {
            continue;
        };
        return Ok(Endpoints {
            issuer: meta["issuer"].as_str().unwrap_or(issuer).to_owned(),
            authorization: authorization.to_owned(),
            token: token.to_owned(),
            registration: meta["registration_endpoint"].as_str().map(str::to_owned),
        });
    }
    Err("Context could not find where this source signs in.".into())
}

/// `https://host/.well-known/{name}{path}`, the path-inserted form.
fn well_known(address: &str, name: &str) -> Result<String> {
    let url = Url::parse(address)?;
    let path = url.path().trim_end_matches('/');
    Ok(format!(
        "{}/.well-known/{name}{path}",
        url.origin().ascii_serialization()
    ))
}

/// Runs the browser sign-in and returns the grant. Blocks until the browser
/// returns or the sign-in times out.
pub fn sign_in(http: &Client, request: SignIn) -> Result<Grant> {
    let callback = Callback::listen("/callback")?;
    let redirect = callback.redirect.clone();
    let client = match request.client {
        Some(client) => client,
        None => register(http, request.endpoints, &redirect)?,
    };
    let state = CsrfToken::new_random();
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorize = Url::parse(&request.endpoints.authorization)?;
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("state", state.secret())
            .append_pair("code_challenge", challenge.as_str())
            .append_pair("code_challenge_method", "S256");
        if let Some(scopes) = request.scopes {
            query.append_pair("scope", scopes);
        }
        if let Some(resource) = request.resource {
            query.append_pair("resource", resource);
        }
        for (name, value) in request.extra {
            query.append_pair(name, value);
        }
    }
    callback.wait(
        &authorize,
        state.secret(),
        request.name,
        |url| match parameter(url, "code") {
            Some(code) => exchange(
                http,
                &request.endpoints.token,
                &client,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", &redirect),
                    ("code_verifier", verifier.secret()),
                ],
                request.resource,
            ),
            None => Err(format!("{} sign-in was cancelled or declined.", request.name).into()),
        },
    )
}

/// GitHub's device flow needs only a public client ID, never a client secret.
/// Keep the user code visible in Context while the browser asks for it.
pub fn github_sign_in(
    http: &Client,
    client: &ClientId,
    scopes: Option<&str>,
    prompt: &impl Fn(String),
) -> Result<Grant> {
    #[derive(Deserialize)]
    struct Device {
        device_code: String,
        user_code: String,
        verification_uri: String,
        expires_in: u64,
        #[serde(default = "default_interval")]
        interval: u64,
    }
    fn default_interval() -> u64 {
        5
    }
    let device: Device = http
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client.id.as_str()),
            ("scope", scopes.unwrap_or_default()),
        ])
        .send()?
        .error_for_status()?
        .json()
        .map_err(|_| "GitHub could not start sign-in. Check that the app enables Device Flow.")?;
    // Open only GitHub's known verification page, even if a response is malformed.
    if device.verification_uri != "https://github.com/login/device" {
        return Err("GitHub returned an unexpected sign-in address.".into());
    }
    prompt(format!(
        "Enter {} at github.com/login/device to connect GitHub.",
        device.user_code
    ));
    open::that_detached(&device.verification_uri)
        .map_err(|_| "Context could not open GitHub sign-in in your browser.")?;
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.min(900));
    let mut interval = Duration::from_secs(device.interval.clamp(5, 900));
    let token_endpoint = "https://github.com/login/oauth/access_token";
    // Device grants never use the secret from an optional custom client.
    let client = ClientId {
        id: client.id.clone(),
        secret: None,
    };
    loop {
        if deadline.saturating_duration_since(Instant::now()) <= interval {
            return Err("GitHub sign-in timed out. Try again from Context.".into());
        }
        std::thread::sleep(interval);
        let response = http
            .post(token_endpoint)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client.id.as_str()),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send();
        let response = match response {
            Err(error) if error.is_timeout() => {
                interval = interval.saturating_mul(2);
                continue;
            }
            response => response?,
        };
        let status = response.status();
        let body: Value = response.json()?;
        match body["error"].as_str() {
            Some("authorization_pending") if status.is_success() => continue,
            Some("slow_down") if status.is_success() => {
                interval += Duration::from_secs(5);
                continue;
            }
            Some("access_denied") => return Err("GitHub sign-in was declined.".into()),
            Some("expired_token") => {
                return Err("GitHub sign-in expired. Try again from Context.".into());
            }
            Some(_) => return Err("GitHub did not accept the sign-in. Try again.".into()),
            None if status.is_success() => {}
            None => return Err("GitHub could not finish signing in. Try again.".into()),
        }
        return grant(&body, token_endpoint, &client, None);
    }
}

/// Registers Context as a client with the authorization server (RFC 7591).
fn register(http: &Client, endpoints: &Endpoints, redirect: &str) -> Result<ClientId> {
    let endpoint = endpoints.registration.as_deref().ok_or(
        "This source needs an app registered with it before Context can sign in. Add its client ID under `oauth` in mcp.json.",
    )?;
    let response = http
        .post(endpoint)
        .json(&json!({
            "client_name": "Context",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()?;
    let status = response.status();
    let body: Value = response.json().unwrap_or_default();
    if !status.is_success() {
        crate::debug(format_args!(
            "client registration failed ({status}): {body}"
        ));
        return Err("The source refused to register Context as a client.".into());
    }
    Ok(ClientId {
        id: body["client_id"]
            .as_str()
            .ok_or("The source did not return a client ID.")?
            .to_owned(),
        secret: body["client_secret"].as_str().map(str::to_owned),
    })
}

fn exchange(
    http: &Client,
    token_endpoint: &str,
    client: &ClientId,
    form: &[(&str, &str)],
    resource: Option<&str>,
) -> Result<Grant> {
    let mut fields: Vec<(&str, &str)> = form.to_vec();
    fields.push(("client_id", &client.id));
    if let Some(secret) = &client.secret {
        fields.push(("client_secret", secret));
    }
    if let Some(resource) = resource {
        fields.push(("resource", resource));
    }
    let response = http
        .post(token_endpoint)
        .header("Accept", "application/json")
        .form(&fields)
        .send()?;
    let status = response.status();
    let body: Value = response.json().unwrap_or_default();
    if !status.is_success() || body.get("error").is_some() {
        crate::debug(format_args!(
            "token request failed ({status}): {}",
            body["error"].as_str().unwrap_or_default()
        ));
        return Err("The source did not accept the sign-in. Try signing in again.".into());
    }
    grant(&body, token_endpoint, client, resource)
}

fn grant(
    body: &Value,
    token_endpoint: &str,
    client: &ClientId,
    resource: Option<&str>,
) -> Result<Grant> {
    let access_token = body["access_token"]
        .as_str()
        .ok_or("The source did not return an access token.")?
        .to_owned();
    Ok(Grant {
        access_token,
        refresh_token: body["refresh_token"].as_str().map(str::to_owned),
        expires_at: body["expires_in"].as_u64().map(|seconds| now() + seconds),
        token_endpoint: token_endpoint.to_owned(),
        client: client.clone(),
        resource: resource.map(str::to_owned),
    })
}

/// A current access token for the source stored under `key`, refreshed when
/// it is about to expire. `Ok(None)` when the source has never signed in.
pub fn access_token(http: &Client, key: &str) -> Result<Option<String>> {
    let record = Record::SourceGrant(storage::key(key));
    let Some(mut grant) = storage::load::<Grant>(record)? else {
        return Ok(None);
    };
    let expiring = grant
        .expires_at
        .is_some_and(|expires| expires <= now() + EXPIRY_MARGIN);
    if expiring {
        let refresh_token = grant
            .refresh_token
            .clone()
            .ok_or("This source's sign-in expired. Sign in again.")?;
        let mut refreshed = exchange(
            http,
            &grant.token_endpoint,
            &grant.client,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
            ],
            grant.resource.as_deref(),
        )?;
        // Some providers return the refresh token only once.
        refreshed.refresh_token = refreshed.refresh_token.or(grant.refresh_token);
        grant = refreshed;
        storage::save(record, &grant)?;
    }
    Ok(Some(grant.access_token))
}

pub fn save(key: &str, grant: &Grant) -> Result<()> {
    storage::save(Record::SourceGrant(storage::key(key)), grant)
}

pub fn forget(key: &str) -> Result<()> {
    storage::remove(Record::SourceGrant(storage::key(key)))
}

pub fn signed_in(key: &str) -> bool {
    storage::load::<Grant>(Record::SourceGrant(storage::key(key)))
        .ok()
        .flatten()
        .is_some()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}
