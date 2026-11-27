use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

pub fn render(label: impl Into<gpui::SharedString>, tone: Tone) -> Div {
    let foreground = match tone {
        Tone::Neutral => colors::MUTED,
        Tone::Accent => colors::ACCENT,
        Tone::Success => colors::SUCCESS,
        Tone::Warning => colors::WARNING,
        Tone::Danger => colors::DANGER,
    };
    div()
        .min_h(px(spacing::SPACE_5))
        .px_2()
        .py_1()
        .rounded(px(spacing::RADIUS_MD))
        .bg(rgb(colors::PANEL_ACTIVE))
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(foreground))
        .child(label.into())
}
