//! The worker thread: everything that blocks. It owns the model client, the
//! connected sources and a copy of the settings, and reports back to the
//! controller through events.

use std::thread;

use async_channel::{Receiver, Sender};
use uuid::Uuid;

use super::{Event, Problem};
use crate::Result;
use crate::capture::{self, Focus};
use crate::history::{self, Entry, FollowUp};
use crate::model::{Answer, Assistant, Fix, Request, needs};
use crate::settings::{Provider, Settings};
use crate::sources::{SourceStatus, Sources};

/// Blocking work for the worker thread.
pub enum Job {
    CheckAccount,
    /// Capture and ask. `quiet` checks run without the user asking.
    Analyze {
        quiet: bool,
    },
    FollowUp {
        entry: Entry,
        question: String,
    },
    SignIn,
    SignOut,
    ReloadSources,
    Configure(Settings),
    SetApiKey(Provider, Option<String>),
    ListModels(Provider),
}

/// What the worker reports back.
pub enum Report {
    Account(Option<String>),
    Captured(Focus),
    /// A short status for the card while the model works, such as "Checking calendar…".
    Progress(String),
    Analysis {
        focus: Focus,
        quiet: bool,
        result: std::result::Result<Answer, Problem>,
        /// The JPEG that was analyzed, kept with a saved result.
        screenshot: Option<Vec<u8>>,
    },
    FollowUp {
        id: Uuid,
        result: std::result::Result<FollowUp, Problem>,
    },
    SignedIn(std::result::Result<String, Problem>),
    SignedOut(String),
    Sources(Vec<SourceStatus>, Option<String>),
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
                    Job::Analyze { quiet } => {
                        analyze(&mut assistant, &sources, &settings, quiet, &send)
                    }
                    Job::FollowUp { entry, question } => {
                        let result =
                            follow_up(&mut assistant, &sources, &settings, &entry, &question);
                        send(Report::FollowUp {
                            id: entry.id,
                            result,
                        })
                    }
                    Job::ReloadSources => {
                        sources.reload();
                        let (servers, problem) = sources.status();
                        send(Report::Sources(servers, problem))
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

fn analyze(
    assistant: &mut Assistant,
    sources: &Sources,
    settings: &Settings,
    quiet: bool,
    send: &impl Fn(Report) -> bool,
) -> bool {
    let failed = |focus, error: Box<dyn std::error::Error + Send + Sync>| Report::Analysis {
        focus,
        quiet,
        result: Err(error.into()),
        screenshot: None,
    };
    if let Err(error) = assistant.ensure_ready(settings) {
        return send(failed(Focus::default(), error));
    }
    let snapshot = match capture::take() {
        Ok(snapshot) => snapshot,
        Err(error) => return send(failed(Focus::default(), error)),
    };
    let focus = snapshot.focus.clone();
    // The capture is dropped here, before anything leaves the computer.
    if let Some(app) = focus.app.as_deref().filter(|app| settings.excludes(app)) {
        let message = format!("Context doesn't look at {app}. You excluded it in Settings.");
        return send(failed(focus, needs(Fix::Excluded, message)));
    }
    if !quiet && !send(Report::Captured(focus.clone())) {
        return false;
    }
    let progress = |status: String| {
        if !quiet {
            send(Report::Progress(status));
        }
    };
    let request = Request::capture(&snapshot, quiet);
    let result = assistant
        .ask(&request, sources, &progress, settings)
        .map_err(Problem::from);
    send(Report::Analysis {
        focus,
        quiet,
        result,
        screenshot: Some(snapshot.jpeg),
    })
}

fn follow_up(
    assistant: &mut Assistant,
    sources: &Sources,
    settings: &Settings,
    entry: &Entry,
    question: &str,
) -> std::result::Result<FollowUp, Problem> {
    assistant.ensure_ready(settings).map_err(Problem::from)?;
    let screenshot = history::screenshot(entry);
    let request = Request::follow_up(&entry.conversation(), question, screenshot.as_deref());
    let progress = |_: String| {};
    let answer = assistant
        .ask(&request, sources, &progress, settings)
        .map_err(Problem::from)?;
    Ok(FollowUp {
        question: question.to_owned(),
        answer: answer
            .text
            .unwrap_or_else(|| "I don't have anything to add on that.".into()),
        sources: answer.sources,
    })
}
