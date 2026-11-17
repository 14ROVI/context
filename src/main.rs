// Release builds on Windows run without a console window. Debug builds keep one for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod capture;
mod history;
mod model;
mod ocr;
mod settings;
mod sources;
mod storage;

use std::process::ExitCode;

use app::Launch;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Sends the MCP library's own diagnostics to stderr alongside Context's.
fn log_mcp_transport() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter =
        tracing_subscriber::filter::Targets::new().with_target("rmcp", tracing::Level::DEBUG);
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false),
        )
        .with(filter)
        .try_init();
}

/// Prints a diagnostic line to stderr when `CONTEXT_DEBUG` is set.
/// Never pass tokens, screenshots or tool output.
pub fn debug(message: impl std::fmt::Display) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("CONTEXT_DEBUG").is_some()) {
        eprintln!(
            "[{}] {message}",
            chrono::Local::now().format("%H:%M:%S%.3f")
        );
    }
}

const USAGE: &str = "Usage: context [open | capture]

  open      Start Context, or bring the running window forward (default).
  capture   Ask the running Context to look at the screen. Bind this to a
            desktop shortcut where Context cannot register one itself.";

fn main() -> ExitCode {
    if std::env::var_os("CONTEXT_DEBUG").is_some() {
        log_mcp_transport();
    }
    let command = match std::env::args().nth(1).as_deref() {
        None => Launch::Open,
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(word) => match Launch::parse(word) {
            Some(command) => command,
            None => {
                eprintln!("context: unknown command `{word}`.\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
        },
    };
    // When Context is already running, the single-instance plugin hands this
    // command to it and this process exits.
    match app::run(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("context: {error}");
            ExitCode::FAILURE
        }
    }
}
