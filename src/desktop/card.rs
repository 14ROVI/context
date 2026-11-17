//! The corner card. It opens as soon as the screen is captured, shows progress
//! while the model works, then shows the answer, nothing, or what went wrong.
//! It never takes focus, pauses while hovered and closes itself.
//!
//! GPUI windows have a fixed size, so the card measures its text before it
//! opens and sizes itself to fit. When the answer arrives, the controller
//! opens a new card sized for it.

use std::time::{Duration, Instant};

use async_channel::Sender;
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, Div, FontWeight, IntoElement,
    LineFragment, Render, Rgba, SharedString, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, size,
};

use super::theme::{self, Theme, Variant};
use super::{Command, Event, Problem, native};
use crate::Result;
use crate::history::Entry;
use crate::storage::APP_ID;

const WIDTH: f32 = 384.;
const PADDING: f32 = 16.;
const BORDER: f32 = 1.;
/// The width every text block wraps at.
const TEXT_WIDTH: f32 = WIDTH - 2. * (PADDING + BORDER);
const HEADER: f32 = 26.;
const ACTIONS: f32 = 30.;
/// Space between the header, body and actions.
const GAP: f32 = 12.;
/// Space between a title and the text under it.
const TITLE_GAP: f32 = 4.;
const TITLE_SIZE: f32 = 15.;
const BODY_SIZE: f32 = 14.;
const LINE: f32 = 20.;
/// Longest texts the card shows before cutting them with an ellipsis.
const MAX_TITLE_LINES: usize = 3;
const MAX_BODY_LINES: usize = 6;
/// Matches the corner radius desktops give small windows, so the card's own
/// border and the system's window outline line up.
const RADIUS: f32 = 8.;
/// How long a hovered card stays after the pointer leaves.
const HOVER_GRACE: Duration = Duration::from_secs(3);

const THINKING_TITLE: &str = "Reading your screen…";
const THINKING_BODY: &str = "Keep working. The answer will appear here.";
const EMPTY_TITLE: &str = "Nothing to add here";
const EMPTY_BODY: &str = "Context didn't find anything useful on this screen.";

#[derive(Clone)]
pub enum Phase {
    Thinking,
    Ready(Entry),
    Empty,
    Problem(Problem),
}

impl Phase {
    fn lifetime(&self) -> Duration {
        Duration::from_secs(match self {
            // The model normally answers well within this. The worker reports
            // a timeout as a problem, which replaces this phase.
            Phase::Thinking => 120,
            Phase::Ready(_) => 20,
            Phase::Empty => 5,
            Phase::Problem(_) => 12,
        })
    }

    /// The title and body text, in the order the card shows them.
    fn texts(&self) -> (SharedString, SharedString) {
        match self {
            Phase::Thinking => (THINKING_TITLE.into(), THINKING_BODY.into()),
            Phase::Ready(entry) => (
                entry.text.headline.clone().into(),
                entry.text.summary.clone().into(),
            ),
            Phase::Empty => (EMPTY_TITLE.into(), EMPTY_BODY.into()),
            Phase::Problem(problem) => (
                if problem.sign_in {
                    "Connect ChatGPT to continue"
                } else {
                    "Context couldn't help with this one"
                }
                .into(),
                problem.message.clone().into(),
            ),
        }
    }

    fn has_actions(&self) -> bool {
        matches!(self, Phase::Ready(_) | Phase::Problem(_))
    }
}

/// Line counts measured for one phase.
#[derive(Clone, Copy)]
struct Layout {
    title_lines: usize,
    body_lines: usize,
}

impl Layout {
    fn measure(phase: &Phase, cx: &App) -> Self {
        let (title, body) = phase.texts();
        Self {
            title_lines: match phase {
                // The status line changes while thinking, so it stays on one line.
                Phase::Thinking => 1,
                _ => count_lines(
                    cx,
                    &title,
                    TITLE_SIZE,
                    FontWeight::SEMIBOLD,
                    MAX_TITLE_LINES,
                ),
            },
            body_lines: count_lines(cx, &body, BODY_SIZE, FontWeight::NORMAL, MAX_BODY_LINES),
        }
    }

    fn height(self, phase: &Phase) -> f32 {
        let mut body = self.title_lines as f32 * LINE;
        if self.body_lines > 0 {
            body += TITLE_GAP + self.body_lines as f32 * LINE;
        }
        if matches!(phase, Phase::Thinking) {
            body += SKELETON;
        }
        let actions = if phase.has_actions() {
            GAP + ACTIONS
        } else {
            0.
        };
        2. * (BORDER + PADDING) + HEADER + GAP + body + actions
    }
}

/// Two shimmering placeholder lines under the thinking text: the gap above
/// them, their top padding, two bars and the space between.
const SKELETON: f32 = TITLE_GAP + 8. + 8. + 8. + 8.;

/// Counts the lines `text` wraps to at the card's text width, the way GPUI
/// will lay it out, capped at `max`.
fn count_lines(cx: &App, text: &str, size: f32, weight: FontWeight, max: usize) -> usize {
    if text.trim().is_empty() {
        return 0;
    }
    let mut font = gpui::font(".SystemUIFont");
    font.weight = weight;
    let mut wrapper = cx.text_system().line_wrapper(font, px(size));
    // Measure a little narrower than the text box so rounding never adds a line.
    let width = px(TEXT_WIDTH - 2.);
    let lines: usize = text
        .split('\n')
        .map(|paragraph| {
            wrapper
                .wrap_line(&[LineFragment::text(paragraph)], width)
                .count()
                + 1
        })
        .sum();
    lines.min(max)
}

pub struct Card {
    phase: Phase,
    layout: Layout,
    /// What the model is doing right now, while thinking.
    status: Option<String>,
    app: Option<String>,
    closes_at: Instant,
    hovered: bool,
    copied: bool,
    events: Sender<Event>,
}

impl Card {
    pub fn set_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
    }

    fn expired(&mut self) -> bool {
        let now = Instant::now();
        if self.hovered {
            self.closes_at = self.closes_at.max(now + HOVER_GRACE);
        }
        now >= self.closes_at
    }

    fn send(&self, command: Command) {
        let _ = self.events.try_send(Event::Command(command));
    }
}

pub fn open(
    cx: &mut App,
    phase: Phase,
    app: Option<String>,
    display_id: Option<u32>,
    events: Sender<Event>,
) -> Result<WindowHandle<Card>> {
    let layout = Layout::measure(&phase, cx);
    let window_size = size(px(WIDTH), px(layout.height(&phase).ceil()));
    let display = cx
        .displays()
        .into_iter()
        .find(|display| Some(u32::from(display.id())) == display_id)
        .or_else(|| cx.primary_display());
    let bounds = match &display {
        Some(display) => {
            let screen = display.bounds();
            let x = screen.origin.x + screen.size.width - window_size.width - px(16.);
            // macOS shows notifications under the menu bar. Elsewhere the card
            // sits above the taskbar.
            let y = if std::env::consts::OS == "macos" {
                screen.origin.y + px(30.)
            } else {
                screen.origin.y + screen.size.height - window_size.height - px(56.)
            };
            Bounds::new(point(x, y), window_size)
        }
        None => Bounds::centered(None, window_size, cx),
    };
    let handle = cx.open_window(
        WindowOptions {
            kind: WindowKind::PopUp,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id: display.map(|display| display.id()),
            titlebar: None,
            focus: false,
            show: native::SHOW_CARD_ON_OPEN,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(APP_ID.into()),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let closes_at = Instant::now() + phase.lifetime();
                cx.spawn_in(window, async move |card, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        let closed =
                            card.update_in(cx, |card: &mut Card, window: &mut Window, _| {
                                let expired = card.expired();
                                if expired {
                                    window.remove_window();
                                }
                                expired
                            });
                        if closed.unwrap_or(true) {
                            break;
                        }
                    }
                })
                .detach();
                Card {
                    phase,
                    layout,
                    status: None,
                    app,
                    closes_at,
                    hovered: false,
                    copied: false,
                    events,
                }
            })
        },
    )?;
    let float = handle
        .update(cx, |_, window, _| native::prepare_float(window, bounds))
        .map_err(|error| error.to_string())
        .and_then(|float| float.map_err(|error| error.to_string()));
    let float = match float {
        Ok(float) => float,
        Err(error) => {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            return Err(error.into());
        }
    };
    // Runs after the current update returns, so GPUI sees the native resize.
    cx.spawn(async move |cx| {
        if let Err(error) = float.apply() {
            eprintln!("context: could not show the card. {error}");
            let _ = cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()));
        }
    })
    .detach();
    Ok(handle)
}

/// Cuts `text` to `max` characters with an ellipsis. GPUI can only ellipsize
/// text whose width it knows up front, so short labels are cut here.
fn clip(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut clipped: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        clipped = clipped.trim_end().to_owned();
        clipped.push('…');
    }
    clipped
}

/// "via github", or "via github +2" when several sources were used.
fn sources_label(sources: &[String]) -> String {
    let first = clip(sources.first().map_or("", String::as_str), 16);
    match sources.len() {
        0 | 1 => format!("via {first}"),
        count => format!("via {first} +{}", count - 1),
    }
}

/// A block of card text at the measured width, clamped to `lines`.
fn text_block(
    text: impl Into<SharedString>,
    font_size: f32,
    weight: FontWeight,
    color: Rgba,
    lines: usize,
) -> Div {
    div()
        .w(px(TEXT_WIDTH))
        .flex_none()
        .overflow_hidden()
        .text_size(px(font_size))
        .line_height(px(LINE))
        .font_weight(weight)
        .text_color(color)
        .line_clamp(lines.max(1))
        .text_ellipsis()
        .child(text.into())
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window);
        let header = div()
            .h(px(HEADER))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(theme::logo(16., theme))
            .child(
                div()
                    .flex_none()
                    .text_size(px(BODY_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Context"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(BODY_SIZE))
                    .text_color(theme.subtle)
                    .children(
                        self.app
                            .as_deref()
                            .map(|app| format!("· {}", clip(app, 28))),
                    ),
            )
            .child(
                theme::button("dismiss", "✕", Variant::Ghost, theme)
                    .w(px(26.))
                    .h(px(26.))
                    .px_0()
                    .on_click(|_, window, _| window.remove_window()),
            );

        let (title, body_text) = self.phase.texts();
        let title_color = match &self.phase {
            Phase::Problem(problem) if !problem.sign_in => theme.danger,
            _ => theme.text,
        };
        let mut body = div().flex_none().flex().flex_col().gap(px(TITLE_GAP));
        body = match &self.phase {
            Phase::Thinking => body.child(
                div()
                    .h(px(LINE))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(theme::working_dots("card-dots", theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(TITLE_SIZE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.status.clone().unwrap_or_else(|| title.to_string())),
                    ),
            ),
            _ => body.child(text_block(
                title,
                TITLE_SIZE,
                FontWeight::SEMIBOLD,
                title_color,
                self.layout.title_lines,
            )),
        };
        if self.layout.body_lines > 0 {
            body = body.child(text_block(
                body_text,
                BODY_SIZE,
                FontWeight::NORMAL,
                theme.muted,
                self.layout.body_lines,
            ));
        }
        if matches!(self.phase, Phase::Thinking) {
            body = body.child(
                div()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(theme::skeleton("skeleton-1", 0.9, theme))
                    .child(theme::skeleton("skeleton-2", 0.6, theme)),
            );
        }

        let mut actions = div()
            .h(px(ACTIONS))
            .flex_none()
            .flex()
            .items_center()
            .gap_2();
        match &self.phase {
            Phase::Ready(entry) => {
                let id = entry.id;
                let text = entry.text.plain_text();
                actions = actions
                    .child(
                        theme::button("details", "Open details", Variant::Primary, theme).on_click(
                            cx.listener(move |card, _, _, _| {
                                card.send(Command::Open(Some(id)));
                            }),
                        ),
                    )
                    .child(
                        theme::button(
                            "copy",
                            if self.copied { "Copied" } else { "Copy" },
                            Variant::Secondary,
                            theme,
                        )
                        .on_click(cx.listener(move |card, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            card.copied = true;
                            cx.notify();
                        })),
                    )
                    .when(!entry.sources.is_empty(), |row| {
                        row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .justify_end()
                                .child(theme::chip(sources_label(&entry.sources), theme)),
                        )
                    });
            }
            Phase::Problem(problem) if problem.sign_in => {
                actions = actions.child(
                    theme::button("connect", "Connect ChatGPT", Variant::Primary, theme)
                        .on_click(cx.listener(|card, _, _, _| card.send(Command::SignIn))),
                );
            }
            Phase::Problem(_) => {
                actions = actions.child(
                    theme::button("open", "Open Context", Variant::Secondary, theme)
                        .on_click(cx.listener(|card, _, _, _| card.send(Command::Open(None)))),
                );
            }
            Phase::Thinking | Phase::Empty => {}
        }

        div().size_full().child(
            div()
                .id("card")
                .size_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .p(px(PADDING))
                .rounded(px(RADIUS))
                .border(px(BORDER))
                .border_color(theme.border)
                .bg(theme.surface)
                .text_color(theme.text)
                .on_hover(cx.listener(|card, hovered: &bool, _, _| card.hovered = *hovered))
                .child(header)
                .child(body)
                .when(self.phase.has_actions(), |card| card.child(actions))
                .with_animation(
                    "card-in",
                    Animation::new(Duration::from_millis(180)),
                    |card, delta| card.opacity(delta),
                ),
        )
    }
}
