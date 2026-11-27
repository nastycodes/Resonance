use gpui::prelude::*;
use gpui::{Context, IntoElement, div, rgb};

use crate::{
    app_state::AppState,
    components::{avatar, list_item},
    navigation::route::SettingsPage,
    theme::colors,
};

pub fn render(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    let user = state
        .current_user
        .as_ref()
        .expect("authenticated shell requires an identity");
    list_item::render()
        .id("open-profile-settings")
        .mt_auto()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .cursor_pointer()
        .border_t_1()
        .border_color(rgb(colors::BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_2()
                .child(avatar::render_user(user, 34.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(colors::TEXT))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(user.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("@{} · {}", user.username, user.status)),
                        ),
                ),
        )
        .on_click(cx.listener(|state, _, _, cx| {
            state.select_settings_page(SettingsPage::Account, cx);
        }))
}
