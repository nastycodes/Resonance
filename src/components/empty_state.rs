use gpui::prelude::*;
use gpui::{IntoElement, div, px, rgb};

use crate::{components::card, theme::colors};

pub fn render(title: &str, description: &str) -> impl IntoElement {
    card::surface()
        .flex()
        .w_full()
        .min_h(px(160.0))
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .px_6()
        .py_10()
        .child(
            div()
                .text_color(rgb(colors::TEXT))
                .text_lg()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(400.0))
                .text_center()
                .text_color(rgb(colors::MUTED))
                .text_sm()
                .child(description.to_string()),
        )
}
