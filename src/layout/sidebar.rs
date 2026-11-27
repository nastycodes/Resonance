use gpui::prelude::*;
use gpui::{Context, IntoElement, MouseButton, deferred, div, rgb};

use crate::{
    app_state::{AppState, PendingConfirmation},
    components::{button, conversation_item, menu},
    navigation::route::Route,
    theme::{colors, spacing},
};

pub fn render(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    let mut sidebar = div()
        .h_full()
        .w(gpui::px(spacing::SIDEBAR_WIDTH))
        .flex()
        .flex_col()
        .bg(rgb(colors::SIDEBAR))
        .border_r_1()
        .border_color(rgb(colors::BORDER))
        .pt_3()
        .on_key_down(cx.listener(|state, event, window, cx| {
            state.handle_menu_key(event, window, cx);
        }));

    sidebar = sidebar
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::MUTED))
                        .child("MENU"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(nav_item(
                            crate::components::icon::IconName::Friends,
                            "Friends",
                            Route::Friends,
                            state.navigation.route,
                            cx,
                        ))
                        .child(nav_item(
                            crate::components::icon::IconName::Friends,
                            "Groups",
                            Route::Groups,
                            state.navigation.route,
                            cx,
                        )),
                ),
        )
        .child(conversation_section(state, cx));

    sidebar.child(super::account_bar::render(state, cx))
}

fn conversation_section(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    let mut section = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .pt_6()
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::MUTED))
                .child("CONVERSATIONS"),
        );

    let has_open_conversations = state.conversations.iter().any(|item| !item.is_closed);
    for conversation in state.conversations.iter().filter(|item| !item.is_closed) {
        let id = conversation.id.clone();
        let menu_id = format!("sidebar-conversation:{id}");
        let selected = state.navigation.route == Route::Messages
            && state.selected_conversation.as_deref() == Some(conversation.id.as_str());
        let item_background = if selected {
            colors::PANEL_ACTIVE
        } else {
            colors::SIDEBAR
        };
        let click_id = id.clone();
        let menu_key = menu_id.clone();
        let timestamp = format_conversation_time(&conversation.updated_at);
        let preview = if conversation.preview.is_empty() {
            "No messages yet".to_string()
        } else {
            conversation
                .preview
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let call_status = if let Some(call) = state
            .active_voice_call
            .as_ref()
            .filter(|call| call.conversation_id == conversation.id)
        {
            Some(if call.connected { "IN CALL" } else { "CALLING" })
        } else if state
            .incoming_voice_call
            .as_ref()
            .is_some_and(|call| call.conversation_id == conversation.id)
        {
            Some("CALL")
        } else {
            None
        };
        let mut item =
            div().relative().w_full().child(
                conversation_item::render()
                    .id(format!("open-conversation-{id}"))
                    .tab_index(0)
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(item_background))
                    .cursor_pointer()
                    .bg(rgb(item_background))
                    .focus_visible(|style| style.border_color(rgb(colors::ACCENT)))
                    .on_click(cx.listener(move |state, _, _, cx| {
                        state.open_conversation_by_id(&click_id, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .w_full()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT))
                                    .child(conversation.name.clone()),
                            )
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(rgb(colors::MUTED))
                                    .child(timestamp),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .w_full()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(colors::MUTED))
                                    .child(preview),
                            )
                            .when_some(call_status, |this, status| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors::ACCENT))
                                        .child(status),
                                )
                            }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |state, _, _, cx| state.toggle_menu(&menu_key, cx)),
                    ),
            );
        if state.active_menu.as_deref() == Some(menu_id.as_str()) {
            item = item.child(deferred(conversation_menu(&id, cx)));
        }
        section = section.child(item);
    }

    if !has_open_conversations {
        section = section.child(
            div()
                .px_2()
                .py_2()
                .text_xs()
                .text_color(rgb(colors::MUTED))
                .child("No recent conversations"),
        );
    }

    section
}

fn format_conversation_time(timestamp: &str) -> String {
    let parsed = timestamp
        .parse::<i64>()
        .ok()
        .map(|numeric| {
            let seconds = if numeric.abs() >= 100_000_000_000 {
                numeric.div_euclid(1000)
            } else {
                numeric
            };
            chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        })
        .flatten()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(timestamp)
                .ok()
                .map(|datetime| datetime.with_timezone(&chrono::Utc))
        });

    parsed
        .map(|datetime| datetime.format("%H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn conversation_menu(conversation_id: &str, cx: &mut Context<AppState>) -> gpui::Div {
    let close_id = conversation_id.to_string();
    let delete_id = conversation_id.to_string();
    let menu_id = format!("sidebar-conversation:{conversation_id}");
    button::menu_surface(&menu_id, cx)
        .child(menu::item(
            format!("close-conversation-{conversation_id}"),
            "Close conversation",
            false,
            cx,
            move |state, _, cx| state.close_conversation(&close_id, cx),
        ))
        .child(menu::item(
            format!("delete-conversation-{conversation_id}"),
            "Delete conversation",
            true,
            cx,
            move |state, _, cx| {
                state.request_confirmation(
                    PendingConfirmation::DeleteConversation(delete_id.clone()),
                    cx,
                )
            },
        ))
}

fn nav_item(
    icon: crate::components::icon::IconName,
    label: &'static str,
    route: Route,
    active_route: Route,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let active = route == active_route;
    crate::components::navigation_item::render_item(format!("nav-{label}"), icon, label, active)
        .on_click(cx.listener(move |state, _, _, cx| state.navigate(route, cx)))
}
