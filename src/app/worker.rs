//! The worker thread: everything that blocks. It owns the model client, the
//! connected sources and a copy of the settings, and reports back to the
//! controller through events.

use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};

use super::{Event, Problem};
use crate::Result;
use crate::capture;
use crate::model::{Answer, Assistant, Fix, Request, needs};
use crate::settings::{Provider, Settings};
use crate::sources::catalog::{self, Added, Entry as CatalogEntry};
use crate::sources::{SourceStatus, Sources};

/// Blocking work for the worker thread.
pub enum Job {
    CheckAccount,
    /// Capture and ask, as the user asked.
    Analyze,
    /// New on-screen text from automatic cards: ask the cheap gate model,
    /// then run a quiet full check if it says yes.
    AutoCandidate(String),
    SignIn,
    SignOut,
    ReloadSources,
    /// Signs in to a source in the browser, then reconnects.
    SourceSignIn(String),
    SourceSignOut(String),
    /// Adds a catalog entry with the values from its form.
    AddSource(
        Box<CatalogEntry>,
        serde_json::Map<String, serde_json::Value>,
    ),
    RemoveSource(String),
    SearchCatalog(String),
    Configure(Settings),
    SetApiKey(Provider, Option<String>),
    ListModels(Provider),
}

/// What the worker reports back.
pub enum Report {
    Account(Option<String>),
    /// The screen was captured and is about to be sent. `quiet` marks an
    /// automatic check.
    Captured {
        quiet: bool,
    },
    /// A short status while the model works, such as "Checking calendar…".
    Progress(String),
    Analysis {
        quiet: bool,
        result: std::result::Result<Answer, Problem>,
        /// The JPEG that was analyzed, kept with a saved result.
        screenshot: Option<Vec<u8>>,
    },
    SignedIn(std::result::Result<String, Problem>),
    SignedOut(String),
    Sources(Vec<SourceStatus>, Option<String>),
    /// A source action finished; the message is shown to the user.
    SourceDone(std::result::Result<String, String>),
    /// A device sign-in code to keep visible until the source action finishes.
    SourcePrompt(String),
    /// A built-in connector was added and should be saved in settings.
    ConnectorAdded(String),
    Catalog(String, std::result::Result<Vec<CatalogEntry>, String>),
    ApiKeySaved(Provider, std::result::Result<(), String>),
    Models(Provider, std::result::Result<Vec<String>, String>),
}

pub fn start(
    mut assistant: Assistant,
    mut sources: Sources,
    mut settings: Settings,
    jobs: Receiver<Job>,
    events: Sender<Event>,
) -> Result<()> {
    thread::Builder::new()
        .name("context-worker".into())
        .spawn(move || {
            let send = |report| events.send_blocking(Event::Worker(report)).is_ok();
            // Full automatic checks in the last hour, to cap their usage.
            let mut automatic: Vec<Instant> = Vec::new();
            while let Ok(job) = jobs.recv_blocking() {
                let delivered = match job {
                    Job::CheckAccount => {
                        send(Report::Account(assistant.chatgpt_email().ok().flatten()))
                    }
                    Job::SignIn => {
                        send(Report::SignedIn(assistant.sign_in().map_err(Problem::from)))
                    }
                    Job::SignOut => send(Report::SignedOut(
                        assistant
                            .sign_out()
                            .unwrap_or_else(|error| error.to_string()),
                    )),
                    Job::Analyze => analyze(&mut assistant, &sources, &settings, None, &send),
                    Job::AutoCandidate(text) => {
                        automatic.retain(|at| at.elapsed() < Duration::from_secs(3600));
                        let worth = automatic.len() < AUTOMATIC_PER_HOUR
                            && assistant
                                .worth_a_look(&text, &settings)
                                .unwrap_or_else(|error| {
                                    crate::debug(format_args!(
                                        "automatic cards: gate failed: {error}"
                                    ));
                                    false
                                });
                        if worth {
                            automatic.push(Instant::now());
                            analyze(&mut assistant, &sources, &settings, Some(&text), &send)
                        } else {
                            true
                        }
                    }
                    Job::ReloadSources => {
                        sources.reload(&settings.connectors);
                        send(sources_report(&sources))
                    }
                    Job::SourceSignIn(name) => {
                        let result = sources
                            .sign_in(&name, &|message| {
                                send(Report::SourcePrompt(message));
                            })
                            .map(|()| format!("{name} is connected."))
                            .map_err(|error| error.to_string());
                        sources.reload(&settings.connectors);
                        send(Report::SourceDone(result)) && send(sources_report(&sources))
                    }
                    Job::SourceSignOut(name) => {
                        let result = sources
                            .sign_out(&name)
                            .map(|()| format!("Signed out of {name}."))
                            .map_err(|error| error.to_string());
                        sources.reload(&settings.connectors);
                        send(Report::SourceDone(result)) && send(sources_report(&sources))
                    }
                    Job::AddSource(entry, values) => match catalog::add(&entry, &values) {
                        Ok(Added::Connector(id)) => send(Report::ConnectorAdded(id)),
                        Ok(Added::Server(name)) => {
                            sources.reload(&settings.connectors);
                            let result = if entry.sign_in {
                                sources
                                    .sign_in(&name, &|message| {
                                        send(Report::SourcePrompt(message));
                                    })
                                    .map(|()| format!("{} is connected.", entry.name))
                                    .map_err(|error| error.to_string())
                            } else {
                                Ok(format!("Added {}.", entry.name))
                            };
                            if entry.sign_in {
                                sources.reload(&settings.connectors);
                            }
                            send(Report::SourceDone(result)) && send(sources_report(&sources))
                        }
                        Err(error) => send(Report::SourceDone(Err(error.to_string()))),
                    },
                    Job::RemoveSource(name) => {
                        let result = catalog::remove(&name)
                            .map(|()| format!("Removed {name}."))
                            .map_err(|error| error.to_string());
                        sources.reload(&settings.connectors);
                        send(Report::SourceDone(result)) && send(sources_report(&sources))
                    }
                    Job::SearchCatalog(query) => {
                        let results = sources.search(&query).map_err(|error| error.to_string());
                        send(Report::Catalog(query, results))
                    }
                    Job::Configure(next) => {
                        settings = next;
                        true
                    }
                    Job::SetApiKey(provider, key) => send(Report::ApiKeySaved(
                        provider,
                        assistant
                            .set_api_key(provider, key)
                            .map_err(|error| error.to_string()),
                    )),
                    Job::ListModels(provider) => send(Report::Models(
                        provider,
                        assistant
                            .models(provider)
                            .map_err(|error| error.to_string()),
                    )),
                };
                if !delivered {
                    break;
                }
            }
        })?;
    Ok(())
}

fn sources_report(sources: &Sources) -> Report {
    let (servers, problem) = sources.status();
    Report::Sources(servers, problem)
}

/// Full automatic checks allowed per hour.
const AUTOMATIC_PER_HOUR: usize = 6;

/// Captures and asks. `trigger` is the new text behind an automatic check,
/// which runs quietly: it says it is searching, then speaks again only with an answer.
fn analyze(
    assistant: &mut Assistant,
    sources: &Sources,
    settings: &Settings,
    trigger: Option<&str>,
    send: &impl Fn(Report) -> bool,
) -> bool {
    let quiet = trigger.is_some();
    let failed = |error: Box<dyn std::error::Error + Send + Sync>| Report::Analysis {
        quiet,
        result: Err(error.into()),
        screenshot: None,
    };
    if let Err(error) = assistant.ensure_ready(settings) {
        return send(failed(error));
    }
    let snapshot = match capture::take() {
        Ok(snapshot) => snapshot,
        Err(error) => return send(failed(error)),
    };
    // The capture is dropped here, before anything leaves the computer.
    if let Some(app) = snapshot
        .focus
        .app
        .as_deref()
        .filter(|app| settings.excludes(app))
    {
        let message = format!("Context doesn't look at {app}. You excluded it in Settings.");
        return send(failed(needs(Fix::Excluded, message)));
    }
    if !send(Report::Captured { quiet }) {
        return false;
    }
    let progress = |status: String| {
        send(Report::Progress(status));
    };
    let request = Request::capture(&snapshot, trigger);
    let result = assistant
        .ask(&request, sources, &progress, settings)
        .map_err(Problem::from);
    send(Report::Analysis {
        quiet,
        result,
        screenshot: Some(snapshot.jpeg),
    })
}
