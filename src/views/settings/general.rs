use gpui::prelude::*;
use gpui::{Context, IntoElement, div, rgb};

use crate::{
    app_state::AppState,
    components::{card, switch},
    theme::colors,
};

pub fn render(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    div().flex_1().min_w_0().flex().flex_col().gap_6().child(
        card::surface()
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors::TEXT))
                            .child("Notification preference"),
                    )
                    .child(
                        div().text_xs().text_color(rgb(colors::MUTED)).child(
                            "Stored per account. Desktop notifications are not implemented.",
                        ),
                    ),
            )
            .child(
                div()
                    .id("toggle-notifications")
                    .cursor_pointer()
                    .tab_index(0)
                    .child(switch::render(state.notifications_enabled))
                    .on_click(cx.listener(|state, _, _, cx| state.toggle_notifications(cx))),
            ),
    )
}
