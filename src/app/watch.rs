//! Automatic cards. When enabled in Settings, this thread watches the focused
//! window for new text, entirely on this computer:
//!
//! 1. Every couple of seconds it grabs the focused window and compares a tiny
//!    greyscale thumbnail with the last one. Usually nothing changed.
//! 2. When the window changed and has settled, it reads the text with local
//!    OCR and keeps only lines it has not seen in that window before.
//! 3. A quick filter keeps lines that look like a question or request.
//!
//! What passes goes to the controller as a candidate. The worker then asks a
//! small, cheap model whether it is something the user needs help with, and
//! only then runs a full check.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Sender;
use image::{DynamicImage, GrayImage};

use super::Event;
use crate::capture;
use crate::ocr::Reader;
use crate::settings::Settings;

/// How often the focused window is checked for changes.
const POLL: Duration = Duration::from_secs(2);
/// Thumbnail size. Big enough that one new chat line shows up.
const THUMB_WIDTH: u32 = 192;
const THUMB_HEIGHT: u32 = 128;
/// A thumbnail pixel this much brighter or darker counts as changed.
const PIXEL_CHANGED: u8 = 24;
/// This many changed pixels count as a change to the window.
const CHANGED_PIXELS: usize = 6;
/// Lines kept from a window that just came into focus, newest last.
const ON_FOCUS_LINES: usize = 8;
/// The same candidate text is not sent again within this time.
const REPEAT_AFTER: Duration = Duration::from_secs(60 * 60);
/// Candidates sent per hour, each costing one small text-only model request.
const CANDIDATES_PER_HOUR: usize = 30;
/// Candidate text is cut to this many characters.
const MAX_CANDIDATE: usize = 800;

/// Phrases that usually ask the reader for something.
const ASKS: [&str; 16] = [
    "can you",
    "could you",
    "are you",
    "do you",
    "will you",
    "would you",
    "did you",
    "have you",
    "let me know",
    "lmk",
    "any chance",
    "what time",
    "when are",
    "free on",
    "available",
    "thoughts on",
];

pub fn start(settings: Arc<RwLock<Settings>>, models: Option<PathBuf>, events: Sender<Event>) {
    let _ = thread::Builder::new()
        .name("context-watch".into())
        .spawn(move || Watcher::new(settings, models, events).run());
}

struct Watcher {
    settings: Arc<RwLock<Settings>>,
    models: Option<PathBuf>,
    reader: Option<Reader>,
    /// Loading the models failed; don't retry until automatic cards restart.
    failed: bool,
    events: Sender<Event>,
    /// The focused window, as app and title.
    window: Option<String>,
    thumbnail: Option<GrayImage>,
    /// The window changed and has not been read since.
    changed: bool,
    /// Normalized lines already seen in the focused window.
    seen: HashSet<String>,
    sent: HashMap<String, Instant>,
    recent: Vec<Instant>,
}

impl Watcher {
    fn new(
        settings: Arc<RwLock<Settings>>,
        models: Option<PathBuf>,
        events: Sender<Event>,
    ) -> Self {
        Self {
            settings,
            models,
            reader: None,
            failed: false,
            events,
            window: None,
            thumbnail: None,
            changed: false,
            seen: HashSet::new(),
            sent: HashMap::new(),
            recent: Vec::new(),
        }
    }

    fn run(mut self) {
        loop {
            thread::sleep(POLL);
            if !self.tick() {
                break;
            }
        }
    }

    /// One poll. Returns false when the app is gone.
    fn tick(&mut self) -> bool {
        let (enabled, settings) = match self.settings.read() {
            Ok(settings) => (settings.auto_cards, settings.clone()),
            Err(_) => return false,
        };
        if !enabled {
            self.window = None;
            self.failed = false;
            return true;
        }
        if self.failed || (self.reader.is_none() && !self.load_reader()) {
            return true;
        }
        // Excluded apps and games are never captured, not even locally.
        let skip = |app: &str| settings.excludes(app) || is_game(app);
        let Some(shot) = capture::focused_window_image(skip) else {
            self.window = None;
            return true;
        };
        let window = format!("{}\u{1f}{}", shot.app, shot.title);
        let thumbnail = thumbnail(&shot.image);
        if self.window.as_ref() != Some(&window) {
            let _ = self.events.try_send(Event::Watching(Ok(shot.app.clone())));
            // A newly focused window: read it now, and treat its last few lines
            // as new, so a question already waiting there is noticed.
            self.window = Some(window);
            self.thumbnail = Some(thumbnail);
            self.changed = false;
            self.seen.clear();
            let lines = self.read(&shot.image);
            let start = lines.len().saturating_sub(ON_FOCUS_LINES);
            let fresh = lines[start..].to_vec();
            self.remember(&lines);
            return self.consider(&shot.app, &fresh);
        }
        let changed = self
            .thumbnail
            .as_ref()
            .map_or(usize::MAX, |last| changed_pixels(last, &thumbnail));
        if changed > 0 && changed != usize::MAX {
            crate::debug(format_args!("automatic cards: {changed} pixels changed"));
        }
        let moved = changed >= CHANGED_PIXELS;
        self.thumbnail = Some(thumbnail);
        if moved {
            // Still changing, perhaps scrolling or typing. Read once it settles.
            self.changed = true;
            return true;
        }
        if !self.changed {
            return true;
        }
        self.changed = false;
        let lines = self.read(&shot.image);
        let fresh: Vec<String> = lines
            .iter()
            .filter(|line| !self.seen.contains(&normalize(line)))
            .cloned()
            .collect();
        self.remember(&lines);
        self.consider(&shot.app, &fresh)
    }

    fn load_reader(&mut self) -> bool {
        let loaded = match &self.models {
            Some(dir) => Reader::load(dir).map_err(|error| {
                format!("Automatic cards could not load text recognition. {error}")
            }),
            None => Err("Automatic cards need Context's text recognition files, which are                          missing. Reinstall Context."
                .into()),
        };
        match loaded {
            Ok(reader) => {
                self.reader = Some(reader);
                true
            }
            Err(message) => {
                self.failed = true;
                let _ = self.events.try_send(Event::Watching(Err(message)));
                false
            }
        }
    }

    fn read(&self, image: &image::RgbaImage) -> Vec<String> {
        let started = Instant::now();
        let lines = self
            .reader
            .as_ref()
            .map(|reader| reader.lines(image).unwrap_or_default())
            .unwrap_or_default();
        crate::debug(format_args!(
            "automatic cards: read {} lines in {} ms",
            lines.len(),
            started.elapsed().as_millis()
        ));
        lines
    }

    fn remember(&mut self, lines: &[String]) {
        self.seen.extend(lines.iter().map(|line| normalize(line)));
    }

    /// Sends new lines that look like a question or request. Returns false
    /// when the app is gone.
    fn consider(&mut self, app: &str, fresh: &[String]) -> bool {
        let asks: Vec<&String> = fresh.iter().filter(|line| asks_something(line)).collect();
        if asks.is_empty() {
            return true;
        }
        let text: String = asks
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(MAX_CANDIDATE)
            .collect();
        let key = normalize(&text);
        let now = Instant::now();
        self.sent
            .retain(|_, at| now.duration_since(*at) < REPEAT_AFTER);
        self.recent
            .retain(|at| now.duration_since(*at) < Duration::from_secs(3600));
        if self.sent.contains_key(&key) || self.recent.len() >= CANDIDATES_PER_HOUR {
            return true;
        }
        self.sent.insert(key, now);
        self.recent.push(now);
        crate::debug(format_args!(
            "automatic cards: candidate in {app} ({} lines)",
            asks.len()
        ));
        self.events
            .send_blocking(Event::AutoCandidate {
                app: app.to_owned(),
                text,
            })
            .is_ok()
    }
}

/// A small greyscale thumbnail, enough to notice a new line of text.
fn thumbnail(image: &image::RgbaImage) -> GrayImage {
    DynamicImage::ImageRgba8(image.clone())
        .resize_exact(
            THUMB_WIDTH,
            THUMB_HEIGHT,
            image::imageops::FilterType::Triangle,
        )
        .to_luma8()
}

/// How many thumbnail pixels changed noticeably. Counting pixels, rather than
/// averaging, keeps one new line in a large window from being diluted.
fn changed_pixels(a: &GrayImage, b: &GrayImage) -> usize {
    if a.dimensions() != b.dimensions() {
        return usize::MAX;
    }
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .filter(|(x, y)| x.abs_diff(**y) > PIXEL_CHANGED)
        .count()
}

fn normalize(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Well-known games and their launchers, in lower case. The desktop reports a
/// program's description where it has one, such as "League of Legends", and
/// its file name without `.exe` otherwise, so both are listed. In-game chat is
/// full of questions that are not for Context, and a card over a game
/// interrupts it. Names match exactly, so "rust" would not match "rustrover".
const GAMES: &[&str] = &[
    // Riot
    "league of legends",
    "leagueclient",
    "leagueclientux",
    "leagueclientuxrender",
    "riot client",
    "riotclientux",
    "riotclientservices",
    "valorant",
    "valorant-win64-shipping",
    // Shooters
    "counter-strike 2",
    "cs2",
    "apex legends",
    "r5apex",
    "r5apex_dx12",
    "fortnite",
    "fortniteclient-win64-shipping",
    "overwatch",
    "call of duty",
    "cod",
    "pubg: battlegrounds",
    "tslgame",
    "rainbow six siege",
    "rainbowsix",
    "rainbowsix_vulkan",
    "destiny 2",
    "destiny2",
    "escape from tarkov",
    "escapefromtarkov",
    "marvel rivals",
    "marvel-win64-shipping",
    // Online, sandbox and others
    "dota 2",
    "dota2",
    "rocket league",
    "rocketleague",
    "minecraft",
    "minecraft.windows",
    "roblox",
    "robloxplayerbeta",
    "world of warcraft",
    "wow",
    "hearthstone",
    "diablo iv",
    "path of exile",
    "pathofexile",
    "pathofexile_x64steam",
    "genshin impact",
    "genshinimpact",
    "honkai: star rail",
    "starrail",
    "zenless zone zero",
    "zenlesszonezero",
    "osu!",
    "grand theft auto v",
    "gta5",
    "gta5_enhanced",
    "elden ring",
    "eldenring",
    "baldur's gate 3",
    "bg3",
    "bg3_dx11",
];

fn is_game(app: &str) -> bool {
    let app = app.to_lowercase();
    GAMES.contains(&app.as_str())
}

/// Whether a line looks like it asks the reader for something.
fn asks_something(line: &str) -> bool {
    let line = line.to_lowercase();
    if line.split_whitespace().count() < 3 {
        return false;
    }
    line.contains('?') || ASKS.iter().any(|phrase| line.contains(phrase))
}
