use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn render(label: impl Into<gpui::SharedString>, selected: bool) -> Div {
    div()
        .min_h(px(spacing::ACTION_TARGET))
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(if selected {
            colors::FOCUS
        } else {
            colors::BORDER
        }))
        .bg(rgb(if selected {
            colors::PANEL_ACTIVE
        } else {
            colors::CANVAS
        }))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(colors::TEXT))
        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
        .focus_visible(|style| style.border_color(rgb(colors::FOCUS)))
        .child(label.into())
}
