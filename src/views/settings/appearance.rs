use gpui::prelude::*;
use gpui::{IntoElement, div, rgb};

use crate::{components::card, theme::colors};

pub fn render() -> impl IntoElement {
    div().flex_1().min_w_0().flex().flex_col().gap_6().child(
        card::surface()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(colors::TEXT))
                    .child("Frequency"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::MUTED))
                    .child("The default theme made by the Resonance team."),
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
