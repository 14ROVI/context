//! Experimental automatic cards. When enabled in Settings, this thread watches
//! which window is focused, without capturing anything, and asks for a quiet
//! check when the user settles on a new window or conversation.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;

use super::Event;
use crate::capture;

/// How often the focused window is read.
const POLL: Duration = Duration::from_secs(2);
/// How long a window must stay focused before it is checked.
const SETTLE: Duration = Duration::from_secs(4);
/// The same window and title is not checked again within this time.
const REPEAT_AFTER: Duration = Duration::from_secs(30 * 60);
/// At most this many automatic checks run per hour.
const PER_HOUR: usize = 6;

pub fn start(enabled: Arc<AtomicBool>, events: Sender<Event>) {
    let _ = thread::Builder::new()
        .name("context-watch".into())
        .spawn(move || {
            let mut current: Option<(String, Instant)> = None;
            let mut fired = false;
            let mut checked: HashMap<String, Instant> = HashMap::new();
            let mut recent: Vec<Instant> = Vec::new();
            loop {
                thread::sleep(POLL);
                if !enabled.load(Ordering::Relaxed) {
                    current = None;
                    continue;
                }
                let Some((app, title)) = capture::focused() else {
                    continue;
                };
                let key = format!("{app}\u{1f}{title}");
                let now = Instant::now();
                match &current {
                    Some((seen, _)) if *seen == key => {}
                    _ => {
                        current = Some((key.clone(), now));
                        fired = false;
                        continue;
                    }
                }
                let settled = current
                    .as_ref()
                    .is_some_and(|(_, since)| now.duration_since(*since) >= SETTLE);
                if fired || !settled {
                    continue;
                }
                fired = true;
                recent.retain(|at| now.duration_since(*at) < Duration::from_secs(3600));
                checked.retain(|_, at| now.duration_since(*at) < REPEAT_AFTER);
                if recent.len() >= PER_HOUR || checked.contains_key(&key) {
                    continue;
                }
                recent.push(now);
                checked.insert(key, now);
                crate::debug(format_args!("automatic check for {app}"));
                if events.send_blocking(Event::AutoCheck(app)).is_err() {
                    break;
                }
            }
        });
}
