use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn render(
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(colors::DANGER))
        .bg(rgb(colors::PANEL))
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::DANGER))
                .child(title.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(colors::MUTED))
                .child(description.into()),
        )
}
