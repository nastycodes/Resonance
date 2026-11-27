use gpui::prelude::*;
use gpui::{Context, IntoElement, Window, div, rgb};

use crate::{app_state::AppState, navigation::route::Route, theme::colors};

pub fn render(
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let content = match state.navigation.route {
        Route::Friends => crate::views::friends::view::render(state, window, cx).into_any_element(),
        Route::Groups => crate::views::groups::view::render(state, window, cx).into_any_element(),
        Route::Messages => {
            crate::views::messages::view::render(state, window, cx).into_any_element()
        }
    };
    div()
        .h_full()
        .flex_1()
        .min_w_0()
        .bg(rgb(colors::CANVAS))
        .child(content)
}
