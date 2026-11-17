//! Colors and small building blocks shared by the main window and the card.
//! The palette follows the system light or dark appearance.

use gpui::{
    Animation, AnimationExt, Div, ElementId, FontWeight, Rgba, SharedString, Stateful, Window,
    WindowAppearance, div, prelude::*, pulsating_between, px, rgb, rgba,
};
use std::time::Duration;

#[derive(Clone, Copy)]
pub struct Theme {
    pub canvas: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub hover: Rgba,
    pub selected: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub subtle: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_soft: Rgba,
    pub on_accent: Rgba,
    pub danger: Rgba,
    pub danger_soft: Rgba,
}

impl Theme {
    pub fn of(window: &Window) -> Self {
        match window.appearance() {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }

    fn dark() -> Self {
        Self {
            canvas: rgb(0x111317),
            sidebar: rgb(0x16191e),
            surface: rgb(0x1b1f25),
            hover: rgb(0x222731),
            selected: rgb(0x252c3b),
            border: rgb(0x2a303a),
            text: rgb(0xeceff4),
            muted: rgb(0xa1aab8),
            subtle: rgb(0x6c7584),
            accent: rgb(0x8098ff),
            accent_hover: rgb(0x96aaff),
            accent_soft: rgba(0x8098ff26),
            on_accent: rgb(0x0b1020),
            danger: rgb(0xff8f87),
            danger_soft: rgba(0xff8f871f),
        }
    }

    fn light() -> Self {
        Self {
            canvas: rgb(0xffffff),
            sidebar: rgb(0xf6f7f9),
            surface: rgb(0xffffff),
            hover: rgb(0xeceef2),
            selected: rgb(0xe6ebfd),
            border: rgb(0xe2e5ea),
            text: rgb(0x15181d),
            muted: rgb(0x566070),
            subtle: rgb(0x8a93a1),
            accent: rgb(0x4d68e8),
            accent_hover: rgb(0x3f59d6),
            accent_soft: rgba(0x4d68e81f),
            on_accent: rgb(0xffffff),
            danger: rgb(0xc4372c),
            danger_soft: rgba(0xc4372c14),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// A clickable button. Attach behavior with `.on_click`.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    variant: Variant,
    theme: Theme,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap_1p5()
        .h(px(30.))
        .px_3()
        .rounded(px(7.))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label.into());
    match variant {
        Variant::Primary => base
            .bg(theme.accent)
            .text_color(theme.on_accent)
            .hover(move |style| style.bg(theme.accent_hover)),
        Variant::Secondary => base
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .text_color(theme.text)
            .hover(move |style| style.bg(theme.hover)),
        Variant::Ghost => base
            .text_color(theme.muted)
            .hover(move |style| style.bg(theme.hover).text_color(theme.text)),
        Variant::Danger => base
            .text_color(theme.danger)
            .hover(move |style| style.bg(theme.danger_soft)),
    }
}

/// Shows a shortcut such as `Ctrl+Alt+Space` as separate key caps.
pub fn keycaps(shortcut: &str, theme: Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .children(shortcut.split('+').map(|key| {
            div()
                .flex()
                .items_center()
                .h(px(20.))
                .px_1p5()
                .rounded(px(5.))
                .border_1()
                .border_b_2()
                .border_color(theme.border)
                .bg(theme.surface)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted)
                .child(key.trim().to_owned())
        }))
}

/// The Context mark: a small accent diamond.
pub fn logo(size: f32, theme: Theme) -> Div {
    div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 0.3))
        .bg(theme.accent)
        .child(
            div()
                .size(px(size * 0.34))
                .rounded_full()
                .bg(theme.on_accent),
        )
}

/// Three dots that pulse while Context is working.
pub fn working_dots(id: &'static str, theme: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .children((0..3usize).map(move |index| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(theme.accent)
                .with_animation(
                    (id, index),
                    Animation::new(Duration::from_millis(1200 + index as u64 * 180))
                        .repeat()
                        .with_easing(pulsating_between(0.25, 1.0)),
                    |dot, delta| dot.opacity(delta),
                )
        }))
}

/// A placeholder line that shimmers while content loads.
pub fn skeleton(id: &'static str, width: f32, theme: Theme) -> gpui::AnimationElement<Div> {
    div()
        .h(px(8.))
        .w(gpui::relative(width))
        .rounded_full()
        .bg(theme.hover)
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1600))
                .repeat()
                .with_easing(pulsating_between(0.4, 1.0)),
            |bar, delta| bar.opacity(delta),
        )
}

/// A pill such as the source app name.
pub fn chip(label: impl Into<SharedString>, theme: Theme) -> Div {
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(theme.accent_soft)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.accent)
        .child(label.into())
}
