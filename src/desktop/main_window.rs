//! The main window: history on the left, the selected result on the right,
//! and onboarding until the first capture.

use std::time::Duration;

use async_channel::Sender;
use chrono::{DateTime, Local, NaiveDate};
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClipboardItem, Context, Div, FontWeight, IntoElement, Render, RenderImage,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, img,
    prelude::*, px, size,
};
use uuid::Uuid;

use super::theme::{self, Theme, Variant};
use super::tray::display_keys;
use super::{Account, Activity, Command, Event, Shortcut, SourcesState};
use crate::history::{self, Entry, History};
use crate::storage::APP_ID;

const NOTICE_LIFETIME: Duration = Duration::from_secs(8);
const SIDEBAR_WIDTH: f32 = 296.;
/// Text width inside a history row: the sidebar minus list and row padding.
/// GPUI measures text in a scroll view without a width, so it is set here.
const ROW_TEXT_WIDTH: f32 = SIDEBAR_WIDTH - 32.;

pub struct MainView {
    entries: Vec<Entry>,
    selected: Option<Uuid>,
    account: Account,
    activity: Activity,
    shortcut: Option<Shortcut>,
    notice: Option<Notice>,
    notice_generation: u64,
    history_problem: Option<&'static str>,
    confirm_clear: bool,
    copied: Option<Uuid>,
    sources: SourcesState,
    /// The selected entry's screenshot, decoded once per selection.
    shot: Option<Shot>,
    events: Sender<Event>,
}

struct Shot {
    id: Uuid,
    /// The decoded image, cut into tiles. `None` when the entry has no
    /// screenshot or it could not be read.
    image: Option<Tiles>,
}

struct Notice {
    message: SharedString,
    error: bool,
}

pub fn open(
    cx: &mut App,
    history: &History,
    events: Sender<Event>,
) -> gpui::Result<WindowHandle<MainView>> {
    let view = MainView {
        selected: history.entries.first().map(|entry| entry.id),
        entries: history.entries.clone(),
        account: Account::Checking,
        activity: Activity::Idle,
        shortcut: None,
        notice: None,
        notice_generation: 0,
        history_problem: history.problem,
        confirm_clear: false,
        copied: None,
        sources: SourcesState::Connecting,
        shot: None,
        events,
    };
    cx.open_window(
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Context".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::centered(size(px(980.), px(640.)), cx)),
            window_min_size: Some(size(px(720.), px(460.))),
            app_id: Some(APP_ID.into()),
            ..Default::default()
        },
        |window, cx| {
            // Closing the window minimizes it so Context keeps running.
            window.on_window_should_close(cx, |window, _| {
                window.minimize_window();
                false
            });
            cx.new(|cx| {
                cx.observe_window_appearance(window, |_, _, cx| cx.notify())
                    .detach();
                view
            })
        },
    )
}

impl MainView {
    pub fn select(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.confirm_clear = false;
        cx.notify();
    }

    pub fn set_history(
        &mut self,
        entries: Vec<Entry>,
        problem: Option<&'static str>,
        select: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        self.entries = entries;
        self.history_problem = problem;
        self.confirm_clear = false;
        if select.is_some() {
            self.selected = select;
        }
        if !self
            .entries
            .iter()
            .any(|entry| Some(entry.id) == self.selected)
        {
            self.selected = self.entries.first().map(|entry| entry.id);
        }
        cx.notify();
    }

    pub fn set_account(&mut self, account: Account, cx: &mut Context<Self>) {
        self.account = account;
        cx.notify();
    }

    pub fn set_activity(&mut self, activity: Activity, cx: &mut Context<Self>) {
        self.activity = activity;
        cx.notify();
    }

    pub fn set_sources(&mut self, sources: SourcesState, cx: &mut Context<Self>) {
        self.sources = sources;
        cx.notify();
    }

    pub fn set_shortcut(&mut self, shortcut: Shortcut, cx: &mut Context<Self>) {
        self.shortcut = Some(shortcut);
        cx.notify();
    }

    /// Shows a banner. Errors stay until dismissed; other notices fade.
    pub fn set_notice(&mut self, message: String, error: bool, cx: &mut Context<Self>) {
        self.notice = Some(Notice {
            message: message.into(),
            error,
        });
        self.notice_generation += 1;
        if !error {
            let generation = self.notice_generation;
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(NOTICE_LIFETIME).await;
                let _ = view.update(cx, |view, cx| {
                    if view.notice_generation == generation {
                        view.notice = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Reads the selected entry's screenshot from disk when the selection changes.
    fn load_screenshot(&mut self) {
        let Some(entry) = self
            .selected
            .and_then(|id| self.entries.iter().find(|entry| entry.id == id))
        else {
            self.shot = None;
            return;
        };
        if self.shot.as_ref().is_some_and(|shot| shot.id == entry.id) {
            return;
        }
        let image = history::screenshot(entry).and_then(|jpeg| Tiles::decode(&jpeg));
        self.shot = Some(Shot {
            id: entry.id,
            image,
        });
    }

    fn send(&self, command: Command) {
        let _ = self.events.try_send(Event::Command(command));
    }

    fn signed_in(&self) -> bool {
        matches!(self.account, Account::SignedIn(_))
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window);
        self.load_screenshot();
        div()
            .size_full()
            .flex()
            .bg(theme.canvas)
            .text_color(theme.text)
            .text_size(px(14.))
            .child(self.sidebar(theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when_some(self.banner(theme, cx), |main, banner| main.child(banner))
                    .child(self.content(theme, cx)),
            )
    }
}

// Sidebar
impl MainView {
    fn sidebar(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .child(theme::logo(22., theme))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Context"),
                    ),
            )
            .child(self.capture_panel(theme, cx))
            .child(self.history_header(theme, cx))
            .child(self.history_list(theme, cx))
            .child(self.sources_panel(theme, cx))
            .child(self.account_panel(theme, cx))
    }

    fn capture_panel(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let capturing = self.activity == Activity::Capturing;
        let button = if capturing {
            theme::button("capture", "", Variant::Primary, theme)
                .opacity(0.85)
                .cursor_default()
                .child(theme::working_dots(
                    "capture-dots",
                    Theme {
                        accent: theme.on_accent,
                        ..theme
                    },
                ))
                .child("Looking at your screen")
        } else {
            theme::button("capture", "Capture screen", Variant::Primary, theme)
                .on_click(cx.listener(|view, _, _, _| view.send(Command::Capture)))
        };
        div()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(button.w_full().h(px(36.)))
            .child(self.shortcut_hint(theme))
    }

    fn shortcut_hint(&self, theme: Theme) -> Div {
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_1p5()
            .text_xs()
            .text_color(theme.subtle);
        match &self.shortcut {
            Some(Shortcut::Registered(keys)) => row
                .child("or press")
                .child(theme::keycaps(&display_keys(keys), theme)),
            Some(Shortcut::External) => row.child("Bind “context capture” to a shortcut"),
            Some(Shortcut::Unavailable) => row.child("Global shortcut unavailable"),
            None => row,
        }
    }

    fn history_header(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(28.))
            .px_4()
            .mb_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.subtle)
            .child("HISTORY");
        if self.entries.is_empty() {
            return header;
        }
        let small = |button: gpui::Stateful<Div>| button.h(px(22.)).px_2().text_xs();
        if self.confirm_clear {
            header.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        small(theme::button(
                            "cancel-clear",
                            "Cancel",
                            Variant::Ghost,
                            theme,
                        ))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.confirm_clear = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        small(theme::button(
                            "confirm-clear",
                            "Clear all",
                            Variant::Danger,
                            theme,
                        ))
                        .on_click(cx.listener(|view, _, _, _| view.send(Command::ClearHistory))),
                    ),
            )
        } else {
            header.child(
                small(theme::button("clear", "Clear", Variant::Ghost, theme)).on_click(
                    cx.listener(|view, _, _, cx| {
                        view.confirm_clear = true;
                        cx.notify();
                    }),
                ),
            )
        }
    }

    fn history_list(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("history")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2();
        if self.entries.is_empty() {
            return list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.subtle)
                    .child("Results you capture will collect here."),
            );
        }
        let today = Local::now().date_naive();
        let mut current_day = None;
        for entry in &self.entries {
            let day = local_time(entry.created_at).map(|time| time.date_naive());
            if day != current_day {
                current_day = day;
                list = list.child(
                    div()
                        .px_2()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.subtle)
                        .child(day_label(day, today)),
                );
            }
            let id = entry.id;
            let selected = self.selected == Some(id);
            let meta = match &entry.app {
                Some(app) => format!("{app} · {}", clock(entry.created_at)),
                None => clock(entry.created_at),
            };
            list = list.child(
                div()
                    .id(SharedString::from(id.to_string()))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1p5()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme.selected))
                    .when(!selected, |row| row.hover(|style| style.bg(theme.hover)))
                    .child(
                        div()
                            .w(px(ROW_TEXT_WIDTH))
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(entry.text.headline.clone()),
                    )
                    .child(
                        div()
                            .w(px(ROW_TEXT_WIDTH))
                            .truncate()
                            .text_xs()
                            .text_color(theme.subtle)
                            .child(meta),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| view.select(id, cx))),
            );
        }
        list
    }

    fn sources_panel(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let small = |button: gpui::Stateful<Div>| button.h(px(22.)).px_2().text_xs();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(28.))
            .px_4()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.subtle)
            .child("SOURCES")
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        small(theme::button("edit-sources", "Edit", Variant::Ghost, theme))
                            .on_click(cx.listener(|view, _, _, _| view.send(Command::EditSources))),
                    )
                    .child(
                        small(theme::button(
                            "reload-sources",
                            "Reload",
                            Variant::Ghost,
                            theme,
                        ))
                        .on_click(cx.listener(|view, _, _, _| view.send(Command::ReloadSources))),
                    ),
            );
        let mut body = div().flex().flex_col().gap_1().px_4().pb_3();
        match &self.sources {
            SourcesState::Connecting => {
                body = body.child(status_line("Connecting sources…", theme));
            }
            SourcesState::Ready { servers, problem } => {
                if let Some(problem) = problem {
                    body = body.child(
                        div()
                            .text_xs()
                            .line_clamp(3)
                            .text_color(theme.danger)
                            .child(problem.clone()),
                    );
                }
                if servers.is_empty() && problem.is_none() {
                    body = body.child(div().text_xs().text_color(theme.subtle).child(
                        "Add MCP servers so answers can check your calendar, docs and more.",
                    ));
                }
                for server in servers {
                    let (dot, detail) = match &server.problem {
                        Some(problem) => (theme.danger, problem.clone()),
                        None if server.enabled == 0 => (
                            theme.subtle,
                            format!("No read-only tools, {} hidden", server.hidden),
                        ),
                        None if server.hidden > 0 => (
                            theme.accent,
                            format!(
                                "{}, {} hidden",
                                plural(server.enabled, "tool"),
                                server.hidden
                            ),
                        ),
                        None => (theme.accent, plural(server.enabled, "tool")),
                    };
                    body = body.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .mt(px(5.))
                                    .size(px(7.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(dot),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .w(px(ROW_TEXT_WIDTH - 16.))
                                            .truncate()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(server.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(ROW_TEXT_WIDTH - 16.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.subtle)
                                            .child(detail),
                                    ),
                            ),
                    );
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border)
            .pt_1()
            .child(header)
            .child(body)
    }

    fn account_panel(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let panel = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2p5()
            .border_t_1()
            .border_color(theme.border);
        let quit = theme::button("quit", "Quit", Variant::Ghost, theme)
            .h(px(26.))
            .px_2()
            .text_xs()
            .on_click(cx.listener(|view, _, _, _| view.send(Command::Quit)));
        let account = match &self.account {
            Account::SignedIn(email) => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(avatar(email, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .child(email.clone()),
                        )
                        .child(
                            div()
                                .id("sign-out")
                                .text_xs()
                                .text_color(theme.subtle)
                                .cursor_pointer()
                                .hover(|style| style.text_color(theme.text))
                                .child("Sign out")
                                .on_click(cx.listener(|view, _, _, _| view.send(Command::SignOut))),
                        ),
                ),
            Account::SignedOut => div().flex_1().child(
                theme::button("connect", "Connect ChatGPT", Variant::Secondary, theme)
                    .w_full()
                    .on_click(cx.listener(|view, _, _, _| view.send(Command::SignIn))),
            ),
            Account::Connecting => status_line("Waiting for your browser…", theme),
            Account::Checking => status_line("Checking your account…", theme),
        };
        panel.child(account).child(quit)
    }
}

// Main area
impl MainView {
    fn banner(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Div> {
        let (message, error) = match (&self.notice, self.history_problem) {
            (Some(notice), _) => (notice.message.clone(), notice.error),
            (None, Some(problem)) => (problem.into(), false),
            (None, None) => return None,
        };
        let dismissible = self.notice.is_some();
        Some(
            div().px_6().pt_4().child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .bg(if error {
                        theme.danger_soft
                    } else {
                        theme.accent_soft
                    })
                    .text_sm()
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(if error {
                        theme.danger
                    } else {
                        theme.accent
                    }))
                    .child(div().flex_1().min_w_0().child(message))
                    .when(dismissible, |banner| {
                        banner.child(
                            theme::button("dismiss-notice", "✕", Variant::Ghost, theme)
                                .h(px(24.))
                                .w(px(24.))
                                .px_0()
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.notice = None;
                                    cx.notify();
                                })),
                        )
                    }),
            ),
        )
    }

    fn content(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let entry = self
            .selected
            .and_then(|id| self.entries.iter().find(|entry| entry.id == id));
        match entry {
            Some(entry) => self.detail(entry, theme, cx).into_any_element(),
            None => self.welcome(theme, cx).into_any_element(),
        }
    }

    fn detail(&self, entry: &Entry, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let id = entry.id;
        let text = entry.text.plain_text();
        let when = local_time(entry.created_at)
            .map(|time| time.format("%A %e %B, %H:%M").to_string())
            .unwrap_or_default();
        div()
            .id("detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_10()
            .py_8()
            .child(
                div()
                    .max_w(px(640.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mb_3()
                            .when_some(entry.app.clone(), |row, app| {
                                row.child(theme::chip(app, theme))
                            })
                            .child(div().text_sm().text_color(theme.subtle).child(when)),
                    )
                    .child(
                        div()
                            .text_size(px(26.))
                            .line_height(px(33.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(entry.text.headline.clone()),
                    )
                    .when(!entry.text.summary.is_empty(), |page| {
                        page.child(
                            div()
                                .mt_3()
                                .text_size(px(16.))
                                .line_height(px(25.))
                                .child(entry.text.summary.clone()),
                        )
                    })
                    .when(!entry.text.details.is_empty(), |page| {
                        page.child(
                            div()
                                .mt_5()
                                .pt_5()
                                .border_t_1()
                                .border_color(theme.border)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .text_size(px(14.))
                                .line_height(px(23.))
                                .text_color(theme.muted)
                                .children(
                                    entry
                                        .text
                                        .details
                                        .split("\n\n")
                                        .map(|paragraph| div().child(paragraph.to_owned())),
                                ),
                        )
                    })
                    .child(
                        div()
                            .mt_8()
                            .flex()
                            .gap_2()
                            .child(
                                theme::button(
                                    "copy",
                                    if self.copied == Some(id) {
                                        "Copied"
                                    } else {
                                        "Copy"
                                    },
                                    Variant::Secondary,
                                    theme,
                                )
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            text.clone(),
                                        ));
                                        view.copied = Some(id);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                theme::button("delete", "Delete", Variant::Danger, theme).on_click(
                                    cx.listener(move |view, _, _, _| {
                                        view.send(Command::Delete(id))
                                    }),
                                ),
                            ),
                    )
                    .when_some(
                        self.shot
                            .as_ref()
                            .filter(|shot| shot.id == id)
                            .and_then(|shot| shot.image.as_ref()),
                        |page, tiles| {
                            page.child(
                                div()
                                    .mt_8()
                                    .mb_2()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.subtle)
                                    .child("WHAT CONTEXT SAW"),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .rounded(px(8.))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(theme.border)
                                    .child(tiles.render()),
                            )
                        },
                    )
                    .child(
                        div()
                            .mt_6()
                            .text_xs()
                            .text_color(theme.subtle)
                            .child(provenance(&entry.sources)),
                    ),
            )
    }

    fn welcome(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let signed_in = self.signed_in();
        let connect: AnyElement = match self.account {
            Account::SignedIn(_) => step_done("ChatGPT connected", theme).into_any_element(),
            Account::Connecting => {
                status_line("Waiting for your browser…", theme).into_any_element()
            }
            _ => theme::button(
                "welcome-connect",
                "Connect ChatGPT",
                Variant::Primary,
                theme,
            )
            .on_click(cx.listener(|view, _, _, _| view.send(Command::SignIn)))
            .into_any_element(),
        };
        let shortcut: AnyElement = match &self.shortcut {
            Some(Shortcut::Registered(keys)) => {
                theme::keycaps(&display_keys(keys), theme).into_any_element()
            }
            Some(Shortcut::External) => div()
                .text_sm()
                .text_color(theme.muted)
                .child("Add a desktop shortcut that runs “context capture”.")
                .into_any_element(),
            _ => div()
                .text_sm()
                .text_color(theme.muted)
                .child("Use Capture screen in the sidebar or tray.")
                .into_any_element(),
        };
        div()
            .id("welcome")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px_10()
            .py_12()
            .child(
                div()
                    .max_w(px(560.))
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(theme::logo(44., theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .line_height(px(32.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if signed_in {
                                        "Ready when you are"
                                    } else {
                                        "Answers for what's on your screen"
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .line_height(px(23.))
                                    .text_color(theme.muted)
                                    .child(
                                        "Press the shortcut in any app. Context reads the screen, \
                                         works out what you're doing and shows a short answer in \
                                         the corner, without pulling you out of your work.",
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(theme.border)
                            .child(step(1, "Connect your ChatGPT plan", connect, theme, false))
                            .child(step(
                                2,
                                "Press the shortcut in any app",
                                shortcut,
                                theme,
                                true,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.subtle)
                                    .child("TRY IT ON"),
                            )
                            .child(example(
                                "A long chat thread",
                                "Catch up on what was decided and what's still open.",
                                theme,
                            ))
                            .child(example(
                                "An error message",
                                "See what it means and what to try next.",
                                theme,
                            ))
                            .child(example(
                                "A question someone asked you",
                                "Get the facts from the conversation in one line.",
                                theme,
                            )),
                    ),
            )
    }
}

fn step(number: u8, title: &'static str, action: AnyElement, theme: Theme, border: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .when(border, |row| row.border_t_1().border_color(theme.border))
        .child(
            div()
                .size(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.accent_soft)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.accent)
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(action)
}

fn step_done(label: &'static str, theme: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_sm()
        .text_color(theme.accent)
        .child("✓")
        .child(label)
}

fn example(title: &'static str, body: &'static str, theme: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .px_4()
        .py_3()
        .rounded(px(10.))
        .bg(theme.sidebar)
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().text_sm().text_color(theme.muted).child(body))
}

fn status_line(label: &'static str, theme: Theme) -> Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(theme.muted)
        .child(theme::working_dots("status-dots", theme))
        .child(label)
}

fn avatar(email: &str, theme: Theme) -> Div {
    let initial = email
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.accent_soft)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.accent)
        .child(initial)
}

/// Screenshots are decoded at this size, sharp enough for the detail view.
const SCREENSHOT_EDGE: u32 = 1280;
/// GPUI 0.2 on Windows draws images larger than roughly 640px incompletely or
/// not at all, so screenshots are drawn as a grid of tiles no larger than this.
const TILE: u32 = 512;

/// A screenshot cut into rows of tiles, ready to draw.
struct Tiles {
    width: u32,
    rows: Vec<Vec<(Arc<RenderImage>, u32, u32)>>,
}

impl Tiles {
    fn decode(jpeg: &[u8]) -> Option<Self> {
        let picture = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
        let picture = if picture.width().max(picture.height()) > SCREENSHOT_EDGE {
            picture.thumbnail(SCREENSHOT_EDGE, SCREENSHOT_EDGE)
        } else {
            picture
        };
        let mut pixels = picture.into_rgba8();
        // GPUI expects BGRA.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let (width, height) = pixels.dimensions();
        let rows = (0..height)
            .step_by(TILE as usize)
            .map(|top| {
                (0..width)
                    .step_by(TILE as usize)
                    .map(|left| {
                        let tile_width = TILE.min(width - left);
                        let tile_height = TILE.min(height - top);
                        let tile =
                            image::imageops::crop_imm(&pixels, left, top, tile_width, tile_height)
                                .to_image();
                        let render = RenderImage::new(vec![image::Frame::new(tile)]);
                        (Arc::new(render), tile_width, tile_height)
                    })
                    .collect()
            })
            .collect();
        Some(Self { width, rows })
    }

    /// Lays the tiles out at the container's width, keeping the aspect ratio.
    fn render(&self) -> Div {
        div()
            .w_full()
            .flex()
            .flex_col()
            .children(self.rows.iter().map(|row| {
                div()
                    .w_full()
                    .flex()
                    .children(row.iter().map(|(tile, width, height)| {
                        img(tile.clone())
                            .flex_none()
                            .w(gpui::relative(*width as f32 / self.width as f32))
                            .map(|mut picture| {
                                picture.style().aspect_ratio = Some(*width as f32 / *height as f32);
                                picture
                            })
                    }))
            }))
    }
}

/// Says where an answer came from, so the reader knows what to double-check.
fn provenance(sources: &[String]) -> String {
    if sources.is_empty() {
        "Based on a screenshot of your display. Check important details.".into()
    } else {
        format!(
            "Based on a screenshot of your display and {}. Check important details.",
            sources.join(", ")
        )
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn local_time(timestamp: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp(timestamp, 0).map(|time| time.with_timezone(&Local))
}

fn clock(timestamp: i64) -> String {
    local_time(timestamp)
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn day_label(day: Option<NaiveDate>, today: NaiveDate) -> String {
    match day {
        Some(day) if day == today => "Today".into(),
        Some(day) if today.pred_opt() == Some(day) => "Yesterday".into(),
        Some(day) => day.format("%A %e %B").to_string(),
        None => "Earlier".into(),
    }
}
