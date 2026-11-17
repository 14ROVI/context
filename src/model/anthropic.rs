//! Claude through the Anthropic Messages API with the user's API key. Rust has
//! no official Anthropic SDK, so this sends raw HTTP requests and runs the
//! tool-use loop itself.

use reqwest::blocking::{Client, RequestBuilder};
use serde_json::{Value, json};

use super::prompt::{self, MAX_TOOL_ROUNDS};
use super::{Answer, Fix, Lookups, Request, base64, needs, short};
use crate::Result;
use crate::sources::Sources;

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2026-10-21";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const MAX_TOKENS: u32 = 16_000;
/// Cards should arrive in seconds, so the model thinks briefly.
const EFFORT: &str = "low";
/// Models that accept server-side refusal fallback. When a safety classifier
/// declines, the API retries on a fallback model instead of returning nothing.
const FALLBACK_MODELS: [&str; 5] = [
    "claude-fable-5-2",
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const CONSOLE: &str = "https://platform.claude.com/";

fn api(http: &Client, key: &str, method: reqwest::Method, path: &str) -> RequestBuilder {
    http.request(method, format!("{API}{path}"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
}

/// Model IDs the key can use.
pub fn list_models(http: &Client, key: &str) -> Result<Vec<String>> {
    let response = api(http, key, reqwest::Method::GET, "/models?limit=100").send()?;
    let status = response.status().as_u16();
    let body: Value = response.json().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(api_error(&body, status));
    }
    Ok(body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect())
}

/// Asks Claude about one capture, running the tools it calls in between.
pub fn analyze(
    http: &Client,
    key: &str,
    model: &str,
    request: &Request,
    sources: &Sources,
    progress: &dyn Fn(String),
) -> Result<Answer> {
    let first = json!([
        {"type": "text", "text": request.text},
        {
            "type": "image",
            "source": {"type": "base64", "media_type": "image/jpeg", "data": base64(request.image)},
        },
    ]);
    let mut messages = vec![json!({"role": "user", "content": first})];
    let tools: Vec<Value> = sources
        .definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool["name"],
                "description": tool["description"],
                "input_schema": tool["parameters"],
            })
        })
        .collect();
    let fallback = FALLBACK_MODELS.contains(&model);
    let mut lookups = Lookups::new(sources, progress);

    for round in 0..=MAX_TOOL_ROUNDS {
        let mut body = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": request.instructions,
            "messages": messages,
            "output_config": {"effort": EFFORT},
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            // The last round asks for the answer, with no more lookups.
            if round == MAX_TOOL_ROUNDS || !lookups.left() {
                body["tool_choice"] = json!({"type": "none"});
            }
        }
        let mut send = api(http, key, reqwest::Method::POST, "/messages");
        if fallback {
            body["fallbacks"] = json!("default");
            send = send.header("anthropic-beta", FALLBACK_BETA);
        }
        crate::debug(format_args!(
            "Anthropic round {round}: {} messages, {} tools",
            messages.len(),
            tools.len()
        ));
        let response = send.json(&body).send()?;
        let status = response.status().as_u16();
        let reply: Value = response.json().unwrap_or_default();
        if !(200..300).contains(&status) {
            crate::debug(format_args!(
                "Anthropic request failed (HTTP {status}): {}",
                short(&reply)
            ));
            return Err(api_error(&reply, status));
        }
        let stop = reply["stop_reason"].as_str().unwrap_or_default();
        crate::debug(format_args!("Anthropic round {round} stopped: {stop}"));
        if stop == "refusal" {
            return Err("Claude declined to answer about this screen.".into());
        }
        let content = reply["content"].as_array().cloned().unwrap_or_default();
        let calls: Vec<&Value> = content
            .iter()
            .filter(|block| block["type"] == "tool_use")
            .collect();
        if stop != "tool_use" || calls.is_empty() {
            let text: String = content
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            let (text, needs) = prompt::finish(&text);
            return Ok(Answer {
                text,
                sources: lookups.used,
                needs,
            });
        }
        // Keep the assistant turn exactly as returned, including thinking and
        // fallback blocks, then answer every tool call in one user message.
        messages.push(json!({"role": "assistant", "content": content.clone()}));
        let mut results = Vec::new();
        for call in calls {
            let (output, failed) = lookups.call(
                call["name"].as_str().unwrap_or_default(),
                &call["input"].to_string(),
            );
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": call["id"],
                "content": output,
                "is_error": failed,
            }));
        }
        messages.push(json!({"role": "user", "content": results}));
        progress("Writing the answer…".into());
    }
    Err("Claude kept asking for lookups without answering.".into())
}

/// The model for quick decisions: the current Haiku.
pub const QUICK_MODEL: &str = "claude-haiku-5-5";

/// A small text-only request that returns Claude's reply, used for quick
/// yes-or-no decisions.
pub fn quick(http: &Client, key: &str, system: &str, text: &str) -> Result<String> {
    let body = json!({
        "model": QUICK_MODEL,
        "max_tokens": 1024,
        "system": system,
        "messages": [{"role": "user", "content": text}],
        "output_config": {"effort": "low"},
    });
    let response = api(http, key, reqwest::Method::POST, "/messages")
        .json(&body)
        .send()?;
    let status = response.status().as_u16();
    let reply: Value = response.json().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(api_error(&reply, status));
    }
    Ok(reply["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join(""))
}

fn api_error(body: &Value, status: u16) -> Box<dyn std::error::Error + Send + Sync> {
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match status {
        401 | 403 => needs(
            Fix::Settings,
            "Anthropic rejected the API key. Check it in Settings.",
        ),
        429 => needs(
            Fix::Usage(CONSOLE),
            "Anthropic is limiting requests on this key. Try again in a moment.",
        ),
        529 => "Claude is busy right now. Try again in a moment.".into(),
        404 if message.contains("model") => needs(
            Fix::Settings,
            format!("{message} Pick another model in Settings."),
        ),
        _ if message.is_empty() => {
            format!("Claude could not analyze this screen (HTTP {status}).").into()
        }
        _ => format!("Claude could not analyze this screen: {message}").into(),
    }
}
