use gpui::prelude::*;
use gpui::{IntoElement, div, px, rgb};

use crate::theme::{colors, typography};

pub fn render(title: &str, description: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_color(rgb(colors::TEXT))
                .text_size(px(typography::PAGE_TITLE))
                .line_height(px(typography::PAGE_TITLE_LINE_HEIGHT))
                .font_weight(gpui::FontWeight::BOLD)
                .font_family(typography::HEADLINE_FONT)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_color(rgb(colors::MUTED))
                .text_size(px(typography::BODY))
                .child(description.to_string()),
        )
}
