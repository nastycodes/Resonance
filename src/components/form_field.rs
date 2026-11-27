use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn wrapper() -> Div {
    div().flex().flex_col().gap_2()
}

pub fn label(label: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(colors::MUTED))
        .child(label.into())
}

pub fn input_frame(focused: bool) -> Div {
    div()
        .w_full()
        .min_h(px(spacing::CONTROL_HEIGHT))
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(if focused {
            colors::FOCUS
        } else {
            colors::BORDER
        }))
        .bg(rgb(colors::CANVAS))
        .text_sm()
        .text_color(rgb(colors::TEXT))
        .focus(|style| style.border_color(rgb(colors::FOCUS)))
}
