use std::time::{SystemTime, UNIX_EPOCH};

use openidconnect::core::{CoreIdToken, CoreIdTokenVerifier, CoreJsonWebKeySet};
use openidconnect::{ClientId, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use super::{AUTH_ISSUER, PLAN_SCOPE, RESOURCE, sign_in_required};
use crate::Result;
use crate::callback::{Callback, parameter};
use crate::model::short;
use crate::storage::{self, Record};

const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const CALLBACK: &str = "/auth/callback";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

#[derive(Serialize, Deserialize)]
pub(super) struct Settings {
    host_id: String,
    pub account: Option<Account>,
}

impl Settings {
    pub(super) fn load() -> Result<Self> {
        Ok(storage::load(Record::Account)?.unwrap_or_else(|| Self {
            host_id: format!("urn:uuid:{}", Uuid::new_v4()),
            account: None,
        }))
    }

    /// The signed-in email, when a session is saved.
    pub(super) fn email(&self) -> Option<String> {
        let account = self.account.as_ref()?;
        account.session.as_ref()?;
        account
            .identity
            .as_ref()
            .map(|identity| identity.email.clone())
    }

    fn save(&self) -> Result<()> {
        storage::save(Record::Account, self)
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Account {
    client_id: String,
    identity: Option<Identity>,
    pub session: Option<Session>,
}

impl Account {
    pub(super) fn check_plan(&self) -> Result<()> {
        let scope = &self
            .session
            .as_ref()
            .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?
            .scope;
        if !scope.split_whitespace().any(|value| value == PLAN_SCOPE) {
            return Err(sign_in_required(
                "Connect ChatGPT again and allow Context to use your plan.",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Identity {
    subject: String,
    email: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Session {
    pub access_token: String,
    refresh_token: Option<String>,
    scope: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    id_token: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
    token_type: String,
    expires_in: u64,
}

impl Tokens {
    fn into_session(self, existing_scope: &str) -> Result<Session> {
        if !self.token_type.eq_ignore_ascii_case("bearer") || self.access_token.is_empty() {
            return Err("ChatGPT did not return a usable access token.".into());
        }
        Ok(Session {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            scope: self.scope.unwrap_or_else(|| existing_scope.to_owned()),
            expires_at: unix_time()?.saturating_add(self.expires_in),
        })
    }
}

pub(super) fn sign_in(http: &Client, settings: &mut Settings) -> Result<String> {
    // Persist the installation identity before the first browser round trip.
    settings.save()?;
    let callback = Callback::listen(CALLBACK)?;
    let redirect = callback.redirect.clone();
    let state = CsrfToken::new_random();
    let nonce = Nonce::new_random();
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorize = Url::parse(AUTHORIZE)?;
    let existing_id = settings
        .account
        .as_ref()
        .map(|account| account.client_id.as_str());
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("client_id", existing_id.unwrap_or(REGISTRATION_CLIENT))
            .append_pair("ext_agent_host_id", &settings.host_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPE)
            .append_pair("resource", RESOURCE)
            .append_pair("state", state.secret())
            .append_pair("nonce", nonce.secret())
            .append_pair("code_challenge", challenge.as_str())
            .append_pair("code_challenge_method", "S256");
        if existing_id.is_none() {
            query.append_pair("agent_name_hint", "Context");
        } else {
            query.append_pair("prompt", "consent");
        }
    }
    callback.wait(&authorize, state.secret(), "ChatGPT", |url| {
        exchange_callback(http, settings, url, &redirect, verifier.secret(), &nonce)
    })
}

fn exchange_callback(
    http: &Client,
    settings: &mut Settings,
    callback: &Url,
    redirect: &str,
    verifier: &str,
    nonce: &Nonce,
) -> Result<String> {
    if let Some(error) = parameter(callback, "error") {
        crate::debug(format_args!(
            "sign-in callback error={error} description={:?}",
            parameter(callback, "error_description")
        ));
        return Err("ChatGPT sign-in was cancelled or declined.".into());
    }
    let code = parameter(callback, "code").ok_or("ChatGPT did not return a sign-in code.")?;
    let returned_id = parameter(callback, "client_id");
    let account = match settings.account.as_ref() {
        Some(account) => {
            if returned_id
                .as_ref()
                .is_some_and(|id| *id != account.client_id)
            {
                return Err("ChatGPT returned a different account registration.".into());
            }
            account
        }
        None => {
            let client_id = returned_id
                .filter(|id| id.starts_with("oaiapp_"))
                .ok_or("ChatGPT did not issue a client ID for Context.")?;
            settings.account = Some(Account {
                client_id,
                identity: None,
                session: None,
            });
            settings.save()?;
            settings.account.as_ref().expect("registration was saved")
        }
    };
    let tokens = token_request(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &account.client_id),
            ("resource", RESOURCE),
            ("code", &code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
        ],
    )?;
    let id_token = tokens
        .id_token
        .as_deref()
        .ok_or("ChatGPT did not return an identity token.")?;
    let id_token: CoreIdToken = id_token.parse()?;
    let keys: CoreJsonWebKeySet = http.get(JWKS).send()?.error_for_status()?.json()?;
    let verifier = CoreIdTokenVerifier::new_public_client(
        ClientId::new(account.client_id.clone()),
        IssuerUrl::new(AUTH_ISSUER.into())?,
        keys,
    );
    let claims = id_token.claims(&verifier, nonce)?;
    let identity = Identity {
        subject: claims.subject().as_str().to_owned(),
        email: claims
            .email()
            .ok_or("ChatGPT did not return an account email.")?
            .as_str()
            .to_owned(),
    };
    if account
        .identity
        .as_ref()
        .is_some_and(|saved| saved.subject != identity.subject)
    {
        return Err("Choose the original ChatGPT account for this Context registration.".into());
    }
    let email = identity.email.clone();
    settings.account = Some(Account {
        client_id: account.client_id.clone(),
        identity: Some(identity),
        session: Some(tokens.into_session("")?),
    });
    settings.save()?;
    settings
        .account
        .as_ref()
        .expect("account was saved")
        .check_plan()?;
    Ok(email)
}

pub(super) fn refresh_if_needed(http: &Client, settings: &mut Settings) -> Result<()> {
    let account = settings
        .account
        .as_mut()
        .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?;
    let session = account
        .session
        .as_ref()
        .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?;
    if session.expires_at > unix_time()?.saturating_add(60) {
        return Ok(());
    }
    let refresh_token = session.refresh_token.as_deref().ok_or_else(|| {
        sign_in_required("Your ChatGPT session ended. Connect again to continue.")
    })?;
    let tokens = token_request(
        http,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &account.client_id),
            ("resource", RESOURCE),
            ("refresh_token", refresh_token),
        ],
    )?;
    account.session = Some(tokens.into_session(&session.scope)?);
    settings.save()
}

pub(super) fn sign_out(http: &Client, settings: &mut Settings) -> Result<String> {
    let Some(account) = settings.account.as_mut() else {
        return Ok("Context was not connected to ChatGPT.".into());
    };
    let Some(session) = account.session.take() else {
        return Ok("Context was not connected to ChatGPT.".into());
    };
    let revoked = match session.refresh_token {
        Some(ref token) => revoke_session(http, &account.client_id, token).is_ok(),
        None => true,
    };
    settings.save()?;
    if revoked {
        Ok("Signed out of ChatGPT.".into())
    } else {
        Ok(
            "Signed out on this computer. To revoke access everywhere, disconnect Context \
             in ChatGPT settings."
                .into(),
        )
    }
}

fn revoke_session(http: &Client, client_id: &str, token: &str) -> Result<()> {
    let discovery: serde_json::Value = http.get(DISCOVERY).send()?.error_for_status()?.json()?;
    let endpoint = discovery["revocation_endpoint"]
        .as_str()
        .ok_or("ChatGPT did not publish a revocation endpoint.")?;
    let endpoint = Url::parse(endpoint)?;
    if endpoint.origin() != Url::parse(AUTH_ISSUER)?.origin() {
        return Err("ChatGPT returned an unexpected revocation endpoint.".into());
    }
    http.post(endpoint)
        .form(&[
            ("token", token),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ])
        .send()?
        .error_for_status()?;
    Ok(())
}

fn token_request(http: &Client, form: &[(&str, &str)]) -> Result<Tokens> {
    let response = http.post(TOKEN_ENDPOINT).form(form).send()?;
    if response.status().is_success() {
        return Ok(response.json()?);
    }
    Err(token_error(response))
}

fn token_error(response: Response) -> Box<dyn std::error::Error + Send + Sync> {
    let status = response.status().as_u16();
    let body: serde_json::Value = response.json().unwrap_or_default();
    crate::debug(format_args!(
        "token endpoint HTTP {status}: {}",
        short(&body)
    ));
    match body["error"]
        .as_str()
        .or_else(|| body["error"]["code"].as_str())
    {
        Some("invalid_grant") => {
            sign_in_required("Your ChatGPT session expired. Connect again to continue.")
        }
        Some("invalid_client") => "ChatGPT rejected this Context registration.".into(),
        _ => format!("ChatGPT could not complete sign-in (HTTP {status}).").into(),
    }
}

fn unix_time() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
