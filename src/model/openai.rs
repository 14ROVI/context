//! The OpenAI Responses API, used both by a ChatGPT plan (with the plan's
//! access token) and by an OpenAI API key. Runs the tool-calling loop.

use std::io::{BufRead, BufReader};

use reqwest::blocking::Client;
use serde_json::{Value, json};

use super::prompt::{self, MAX_TEXT_CHARS, MAX_TOOL_CALLS, MAX_TOOL_ROUNDS};
use super::{Answer, Fix, Request, needs};
use crate::Result;
use crate::sources::Sources;

const RESOURCE: &str = "https://api.openai.com/v1";
/// Image-capable models to prefer when Settings names none.
const VISION_MODELS: [&str; 3] = ["gpt-6-luna", "gpt-6.1-sol", "gpt-6-sol"];
const PLAN_USAGE: &str = "https://chatgpt.com/settings/usage";
const API_USAGE: &str = "https://platform.openai.com/usage";

/// Who is paying for the request, which changes the model list and errors.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// A ChatGPT plan through Sign in with ChatGPT.
    Plan,
    /// The user's OpenAI API key.
    ApiKey,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Service::Plan => "ChatGPT",
            Service::ApiKey => "OpenAI",
        }
    }
}

/// Models the account can use. A plan lists `models[].slug`; the public API
/// lists `data[].id`, which includes non-chat models that are left out here.
pub(super) fn list_models(http: &Client, token: &str, service: Service) -> Result<Vec<String>> {
    let response = http
        .get(format!("{RESOURCE}/models"))
        .bearer_auth(token)
        .send()?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let data = response.json().unwrap_or_default();
        return Err(request_error(&data, Some(status), service));
    }
    let catalog: Value = response.json()?;
    let models = match service {
        Service::Plan => catalog["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["visibility"] == "list")
            .filter_map(|model| model["slug"].as_str().map(str::to_owned))
            .collect(),
        Service::ApiKey => {
            const SKIP: [&str; 7] = [
                "audio",
                "realtime",
                "tts",
                "transcribe",
                "embedding",
                "image",
                "search",
            ];
            let mut ids: Vec<String> = catalog["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|model| model["id"].as_str())
                .filter(|id| id.starts_with("gpt-") && !SKIP.iter().any(|skip| id.contains(skip)))
                .map(str::to_owned)
                .collect();
            ids.sort();
            ids
        }
    };
    Ok(models)
}

/// The model from Settings when listed, else the first preferred vision model.
pub(super) fn choose_model(
    models: &[String],
    preferred: Option<&str>,
    service: Service,
) -> Result<String> {
    let preferred = preferred
        .map(str::to_owned)
        .or_else(|| std::env::var("CONTEXT_MODEL").ok());
    if let Some(preferred) = preferred {
        return models
            .iter()
            .find(|item| **item == preferred)
            .cloned()
            .ok_or_else(|| {
                needs(
                    Fix::Settings,
                    format!("Your {} account does not list {preferred}. Pick another model in Settings.", service.name()),
                )
            });
    }
    VISION_MODELS
        .iter()
        .find_map(|model| models.iter().find(|item| item.as_str() == *model))
        .cloned()
        .ok_or_else(|| {
            needs(
                Fix::Settings,
                format!("Context could not find a default image-capable {} model. Pick one in Settings.", service.name()),
            )
        })
}

/// Asks the model about one capture, running the tools it calls in between.
pub(super) fn analyze(
    http: &Client,
    access_token: &str,
    model: &str,
    request: &Request,
    sources: &Sources,
    progress: &dyn Fn(String),
    service: Service,
) -> Result<Answer> {
    let mut content = vec![json!({"type": "input_text", "text": request.text})];
    if let Some(jpeg) = request.image {
        content.push(json!({
            "type": "input_image",
            "image_url": data_url(jpeg),
            "detail": "high",
        }));
    }
    let mut input = vec![json!({"role": "user", "content": content})];
    let mut tools = sources.definitions();
    let mut used = Vec::new();
    let mut calls_left = MAX_TOOL_CALLS;

    for round in 0..=MAX_TOOL_ROUNDS {
        // The last round offers no tools, so the model has to answer.
        let offer_tools = !tools.is_empty() && round < MAX_TOOL_ROUNDS && calls_left > 0;
        let mut body = json!({
            "model": model,
            "instructions": request.instructions,
            "reasoning": {"effort": "low"},
            "store": false,
            "stream": true,
            "input": input,
        });
        if !tools.is_empty() {
            // With `store: false`, reasoning is carried between rounds in the input.
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        if offer_tools {
            body["tools"] = json!(tools);
        }
        crate::debug(format_args!(
            "round {round}: {} input items, {} tools offered",
            input.len(),
            if offer_tools { tools.len() } else { 0 }
        ));
        let turn = match send(http, access_token, &body) {
            Ok(turn) => turn,
            // Some plans or models refuse tools. Answer from the screen alone.
            Err(Failure::Api {
                status,
                body: error,
            }) if round == 0 && !tools.is_empty() && rejects_tools(&error, status) => {
                crate::debug(format_args!(
                    "tools refused (HTTP {status:?}): {}; retrying without tools",
                    short(&error)
                ));
                tools.clear();
                continue;
            }
            Err(failure) => {
                if let Failure::Api { status, body } = &failure {
                    crate::debug(format_args!(
                        "request failed (HTTP {status:?}): {}",
                        short(body)
                    ));
                }
                return Err(failure.into_error(service));
            }
        };
        crate::debug(format_args!(
            "round {round} done: {} chars of text, item types {:?}",
            turn.text.chars().count(),
            turn.items
                .iter()
                .map(|item| item["type"].as_str().unwrap_or("?"))
                .collect::<Vec<_>>()
        ));
        let calls: Vec<&Value> = turn
            .items
            .iter()
            .filter(|item| item["type"] == "function_call")
            .collect();
        if calls.is_empty() || !offer_tools {
            let (text, needs) = prompt::finish(&turn.text);
            return Ok(Answer {
                text,
                sources: used,
                needs,
            });
        }
        input.extend(turn.items.iter().cloned());
        for call in calls {
            let name = call["name"].as_str().unwrap_or_default();
            crate::debug(format_args!(
                "tool call {name} arguments {}",
                call["arguments"].as_str().unwrap_or("{}")
            ));
            let output = if calls_left == 0 {
                "Context's limit on lookups for one capture was reached.".to_owned()
            } else {
                calls_left -= 1;
                if let Some(label) = sources.label(name) {
                    progress(format!("Checking {label}…"));
                }
                let output = sources.call(name, call["arguments"].as_str().unwrap_or("{}"));
                if let Some(source) = output.source
                    && !used.contains(&source)
                {
                    used.push(source);
                }
                output.text
            };
            input.push(json!({
                "type": "function_call_output",
                "call_id": call["call_id"],
                "output": output,
            }));
        }
        progress("Writing the answer…".into());
    }
    Err(format!(
        "{} kept asking for lookups without answering.",
        service.name()
    )
    .into())
}

/// One streamed model response.
struct Turn {
    text: String,
    /// Every output item, including reasoning and function calls.
    items: Vec<Value>,
}

enum Failure {
    Api { status: Option<u16>, body: Value },
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl Failure {
    fn into_error(self, service: Service) -> Box<dyn std::error::Error + Send + Sync> {
        match self {
            Failure::Api { status, body } => request_error(&body, status, service),
            Failure::Other(error) => error,
        }
    }
}

impl<E: Into<Box<dyn std::error::Error + Send + Sync>>> From<E> for Failure {
    fn from(error: E) -> Self {
        Failure::Other(error.into())
    }
}

fn send(http: &Client, access_token: &str, body: &Value) -> std::result::Result<Turn, Failure> {
    let response = http
        .post(format!("{RESOURCE}/responses"))
        .bearer_auth(access_token)
        .json(body)
        .send()?;
    if !response.status().is_success() {
        let status = Some(response.status().as_u16());
        let body = response.json().unwrap_or_default();
        return Err(Failure::Api { status, body });
    }

    let mut event_data = String::new();
    let mut text = String::new();
    let mut text_chars = 0;
    let mut items = Vec::new();
    for line in BufReader::new(response).lines() {
        let line = line?;
        if let Some(data) = line.strip_prefix("data:") {
            if !event_data.is_empty() {
                event_data.push('\n');
            }
            event_data.push_str(data.trim_start());
        }
        if !line.is_empty() || event_data.is_empty() {
            continue;
        }
        let data = std::mem::take(&mut event_data);
        if data == "[DONE]" {
            break;
        }
        let event: Value = serde_json::from_str(&data)?;
        match event["type"].as_str() {
            Some("response.output_text.delta") => {
                if let Some(delta) = event["delta"].as_str() {
                    for ch in delta.chars().take(MAX_TEXT_CHARS - text_chars) {
                        text.push(ch);
                        text_chars += 1;
                    }
                }
            }
            Some("response.output_item.done") => items.push(event["item"].clone()),
            Some("response.completed") => return Ok(Turn { text, items }),
            Some("response.failed" | "error") => {
                return Err(Failure::Api {
                    status: None,
                    body: event,
                });
            }
            Some("response.incomplete") => {
                return Err("ChatGPT did not finish analyzing the screen.".into());
            }
            _ => {}
        }
    }
    Err("ChatGPT closed the response before finishing the screen analysis.".into())
}

/// Whether a failed first request looks like the plan or model refusing tools.
fn rejects_tools(body: &Value, status: Option<u16>) -> bool {
    let code = body
        .pointer("/error/code")
        .or_else(|| body.pointer("/response/error/code"))
        .and_then(Value::as_str);
    code == Some("subscription_sharing_unsupported_capability") || status == Some(400)
}

fn data_url(jpeg: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(jpeg);
    format!("data:image/jpeg;base64,{encoded}")
}

/// A JSON value cut to a readable length for diagnostics.
fn short(value: &Value) -> String {
    value.to_string().chars().take(2_000).collect()
}

fn request_error(
    data: &Value,
    status: Option<u16>,
    service: Service,
) -> Box<dyn std::error::Error + Send + Sync> {
    let code = data
        .pointer("/response/error/code")
        .or_else(|| data.pointer("/error/code"))
        .or_else(|| data.get("code"))
        .and_then(Value::as_str);
    let name = service.name();
    let usage = match service {
        Service::Plan => PLAN_USAGE,
        Service::ApiKey => API_USAGE,
    };
    match code {
        Some(
            "subscription_sharing_usage_limit_exceeded"
            | "rate_limit_exceeded"
            | "insufficient_quota",
        ) => needs(
            Fix::Usage(usage),
            format!("Your {name} usage limit was reached."),
        ),
        Some("subscription_sharing_usage_unavailable") => needs(
            Fix::Usage(usage),
            "ChatGPT plan usage is unavailable. Check your plan and the Context connection in ChatGPT settings.",
        ),
        Some("subscription_sharing_unsupported_capability") => {
            "This ChatGPT plan and model combination did not accept the screen request.".into()
        }
        _ if status == Some(401) => match service {
            Service::Plan => needs(
                Fix::SignIn,
                "ChatGPT ended this session. Connect again to continue.",
            ),
            Service::ApiKey => needs(
                Fix::Settings,
                "OpenAI rejected the API key. Check it in Settings.",
            ),
        },
        _ if status == Some(429) => needs(
            Fix::Usage(usage),
            format!("{name} is limiting requests. Try again in a moment."),
        ),
        _ => match status {
            Some(status) => format!("{name} could not analyze this screen (HTTP {status}).").into(),
            None => format!("{name} could not finish analyzing this screen.").into(),
        },
    }
}
