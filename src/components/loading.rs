use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn inline(label: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(px(spacing::ACTION_TARGET))
        .child(
            div()
                .size(px(12.0))
                .rounded_full()
                .border_1()
                .border_color(rgb(colors::ACCENT)),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(colors::MUTED))
                .child(label.into()),
        )
}
