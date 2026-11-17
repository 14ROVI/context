//! The built-in Google Calendar connector. It signs in with Google, asks only
//! for read access to events, and offers the model one tool: list events in a
//! date range. To the model it looks like any other source's tool.

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use reqwest::blocking::Client;
use serde_json::{Value, json};

use super::ToolDef;
use super::oauth::{self, ClientId, Endpoints, SignIn};
use crate::Result;

pub const ID: &str = "google-calendar";
pub const NAME: &str = "Google Calendar";
/// The grant is stored under this key.
const GRANT: &str = "builtin:google-calendar";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
const EVENTS: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
/// The longest range one call may cover.
const MAX_DAYS: i64 = 31;

/// Context's Google OAuth client, set when Context is built, or at run time
/// for development. A desktop client's secret is not confidential; Google
/// requires it anyway.
fn client() -> Option<ClientId> {
    let id = option_env!("CONTEXT_GOOGLE_CLIENT_ID")
        .map(str::to_owned)
        .or_else(|| std::env::var("CONTEXT_GOOGLE_CLIENT_ID").ok())
        .filter(|value| !value.is_empty())?;
    let secret = option_env!("CONTEXT_GOOGLE_CLIENT_SECRET")
        .map(str::to_owned)
        .or_else(|| std::env::var("CONTEXT_GOOGLE_CLIENT_SECRET").ok())
        .filter(|value| !value.is_empty());
    Some(ClientId { id, secret })
}

/// Whether this build of Context can sign in to Google.
pub fn available() -> bool {
    client().is_some()
}

pub fn signed_in() -> bool {
    oauth::signed_in(GRANT)
}

pub fn sign_in(http: &Client) -> Result<()> {
    let client = client().ok_or(
        "This build of Context has no Google sign-in. Build it with CONTEXT_GOOGLE_CLIENT_ID set.",
    )?;
    let endpoints = Endpoints {
        issuer: "https://accounts.google.com".into(),
        authorization: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token: "https://oauth2.googleapis.com/token".into(),
        registration: None,
    };
    let grant = oauth::sign_in(
        http,
        SignIn {
            endpoints: &endpoints,
            client: Some(client),
            scopes: Some(SCOPE),
            resource: None,
            // Ask for a refresh token so Context stays signed in.
            extra: &[("access_type", "offline"), ("prompt", "consent")],
            name: NAME,
        },
    )?;
    oauth::save(GRANT, &grant)
}

pub fn sign_out() -> Result<()> {
    oauth::forget(GRANT)
}

pub fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "list_events".into(),
        description: "List the user's Google Calendar events between two dates, inclusive, in \
                      their local time. Use it to check availability or what is planned."
            .into(),
        schema: json!({
            "type": "object",
            "properties": {
                "start_date": {"type": "string", "description": "First day, YYYY-MM-DD"},
                "end_date": {"type": "string", "description": "Last day, YYYY-MM-DD. Defaults to start_date."},
            },
            "required": ["start_date"],
        }),
    }]
}

pub fn call(http: &Client, tool: &str, arguments: &Value) -> Result<String> {
    if tool != "list_events" {
        return Err(format!("Google Calendar has no tool named {tool}.").into());
    }
    let date = |field: &str| -> Result<Option<NaiveDate>> {
        arguments[field]
            .as_str()
            .map(|text| {
                NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .map_err(|_| format!("{field} must be a date like 2026-11-23.").into())
            })
            .transpose()
    };
    let start = date("start_date")?.ok_or("start_date is required.")?;
    let end = date("end_date")?.unwrap_or(start).max(start);
    if (end - start).num_days() >= MAX_DAYS {
        return Err(format!("Ask for at most {MAX_DAYS} days at a time.").into());
    }
    let token = oauth::access_token(http, GRANT)?
        .ok_or("Google Calendar is not signed in. Sign in from Context's Settings.")?;
    let from = local_midnight(start)?;
    let to = local_midnight(end + Duration::days(1))?;
    let response = http
        .get(EVENTS)
        .bearer_auth(token)
        .query(&[
            ("timeMin", from.to_rfc3339()),
            ("timeMax", to.to_rfc3339()),
            ("singleEvents", "true".into()),
            ("orderBy", "startTime".into()),
            ("maxResults", "100".into()),
        ])
        .send()?;
    if response.status().as_u16() == 401 {
        return Err("Google Calendar's sign-in expired. Sign in again in Settings.".into());
    }
    let body: Value = response.error_for_status()?.json()?;
    Ok(format_events(&body, start, end))
}

fn local_midnight(date: NaiveDate) -> Result<DateTime<Local>> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight is valid"))
        .earliest()
        .ok_or_else(|| "That date does not exist in your time zone.".into())
}

/// Lists events day by day in local time, noting all-day events, events the
/// user declined and events marked as free.
fn format_events(body: &Value, start: NaiveDate, end: NaiveDate) -> String {
    let events = body["items"].as_array().cloned().unwrap_or_default();
    let mut lines = vec![format!(
        "Calendar for {} to {} (local time, zone {}):",
        start.format("%A %-d %B %Y"),
        end.format("%A %-d %B %Y"),
        Local::now().format("UTC%:z")
    )];
    let mut day = start;
    while day <= end {
        let mut entries = Vec::new();
        for event in &events {
            if event["status"] == "cancelled" {
                continue;
            }
            let title = event["summary"].as_str().unwrap_or("(no title)");
            let mut notes = Vec::new();
            if event["transparency"] == "transparent" {
                notes.push("marked free");
            }
            let declined = event["attendees"].as_array().is_some_and(|people| {
                people
                    .iter()
                    .any(|person| person["self"] == true && person["responseStatus"] == "declined")
            });
            if declined {
                notes.push("you declined");
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            if let Some(all_day) = event["start"]["date"].as_str() {
                let first = NaiveDate::parse_from_str(all_day, "%Y-%m-%d").ok();
                let last = event["end"]["date"]
                    .as_str()
                    .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
                    .map(|exclusive| exclusive - Duration::days(1));
                if first.is_some_and(|first| first <= day) && last.is_none_or(|last| day <= last) {
                    entries.push(format!("- All day: {title}{notes}"));
                }
                continue;
            }
            let time = |side: &str| {
                event[side]["dateTime"]
                    .as_str()
                    .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                    .map(|time| time.with_timezone(&Local))
            };
            if let (Some(from), Some(to)) = (time("start"), time("end"))
                && from.date_naive() <= day
                && day <= to.date_naive()
            {
                entries.push(format!(
                    "- {}–{} {title}{notes}",
                    from.format("%H:%M"),
                    to.format("%H:%M")
                ));
            }
        }
        lines.push(format!("{}:", day.format("%A %-d %B")));
        if entries.is_empty() {
            lines.push("- No events".into());
        } else {
            lines.extend(entries);
        }
        day += Duration::days(1);
    }
    lines.join("\n")
}
