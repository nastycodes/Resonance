use gpui::prelude::*;
use gpui::{IntoElement, div, rgb};

use crate::{components::card, theme::colors};

pub fn render() -> impl IntoElement {
    div().flex_1().flex().flex_col().gap_4().child(
        card::surface()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(rgb(colors::PANEL))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(colors::TEXT))
                    .child("Resonance"),
            )
            .child(
                div().text_sm().text_color(rgb(colors::MUTED)).child(
                    "A desktop messaging app centered around your account and peer identity.",
                ),
            )
            .child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(rgb(colors::SUBTLE))
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            ),
    )
}
