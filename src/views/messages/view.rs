use gpui::prelude::*;
use gpui::{Context, IntoElement, Window, div};

use crate::app_state::AppState;

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let timeline = super::message_list::render(state, window, cx).into_any_element();
    div().h_full().flex().flex_col().child(
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .overflow_hidden()
            .child(timeline),
    )
}
