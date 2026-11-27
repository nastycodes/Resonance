use gpui::prelude::*;
use gpui::{IntoElement, div, rgb};

use crate::{
    app_state::AppState,
    components::{avatar, card},
    navigation::route::Route,
    theme::{colors, spacing},
};

pub fn render(state: &AppState) -> impl IntoElement {
    let heading = match state.navigation.route {
        _ => "PROFILE",
    };
    let mut panel = div()
        .h_full()
        .w(gpui::px(spacing::CONTEXT_WIDTH))
        .flex()
        .flex_col()
        .gap_5()
        .p_5()
        .bg(rgb(colors::SIDEBAR))
        .border_l_1()
        .border_color(rgb(colors::BORDER))
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::SUBTLE))
                .child(heading),
        );

    match state.navigation.route {
        Route::Messages => {
            let selected_conversation = state
                .selected_conversation
                .as_deref()
                .and_then(|id| state.conversation(id));
            if let Some(conversation) = selected_conversation.filter(|item| item.is_group()) {
                panel = panel.child(
                    card::surface()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_color(rgb(colors::TEXT))
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(conversation.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("{} members", conversation.members.len())),
                        )
                        .children(conversation.members.iter().map(|member| {
                            div()
                                .text_sm()
                                .text_color(rgb(colors::TEXT))
                                .child(member.name.clone())
                        })),
                );
            } else if let Some(user) =
                selected_conversation.and_then(|conversation| state.friend(&conversation.user_id))
            {
                panel = panel.child(
                    card::surface()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .p_5()
                        .child(avatar::render_user(user, 68.0))
                        .child(
                            div()
                                .text_color(rgb(colors::TEXT))
                                .text_base()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(user.name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size_2().rounded_full().bg(rgb(
                                    if user.status == "Online" {
                                        colors::GREEN
                                    } else {
                                        colors::SUBTLE
                                    },
                                )))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(colors::MUTED))
                                        .child(user.status.clone()),
                                ),
                        ),
                );
            } else {
                panel = panel.child(generic_note(
                    "Choose a conversation",
                    "Select a person to see their profile here.",
                ));
            }
        }
        _ => {}
    }
    panel
}

fn generic_note(title: &str, description: &str) -> impl IntoElement {
    card::surface()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .child(
            div()
                .text_color(rgb(colors::TEXT))
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_color(rgb(colors::MUTED))
                .text_sm()
                .child(description.to_string()),
        )
}
