use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::{components::dialog, theme::colors};

pub fn surface(
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
) -> Div {
    dialog::surface().w(px(420.0)).child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors::TEXT))
                    .child(title.into()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::MUTED))
                    .child(description.into()),
            ),
    )
}
