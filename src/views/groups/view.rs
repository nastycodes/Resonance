use gpui::prelude::*;
use gpui::{Context, IntoElement, KeyDownEvent, MouseButton, Window, div, px, rgb};

use crate::{
    app_state::{AppState, InputField},
    components::{
        avatar, button, card, dialog, empty_state, icon, icon::IconName, input_text, section_header,
    },
    theme::{colors, spacing},
};

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let mut groups = div().flex().flex_col().gap_2();
    for conversation in state.conversations.iter().filter(|item| item.is_group()) {
        let id = conversation.id.clone();
        let call_id = id.clone();
        let call_key_id = call_id.clone();
        let members = conversation.members.len();
        let is_owner = state.current_user.as_ref().is_some_and(|user| {
            conversation
                .members
                .first()
                .is_some_and(|owner| owner.id == user.id)
        });
        let manage_id = id.clone();
        groups = groups.child(
            card::surface()
                .id(format!("group-card-{id}"))
                .tab_index(0)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
                .on_click(
                    cx.listener(move |state, _, _, cx| state.open_conversation_by_id(&id, cx)),
                )
                .child(icon::render(IconName::Friends, 30.0, colors::ACCENT))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(rgb(colors::TEXT))
                                .child(conversation.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("{members} members")),
                        )
                        .child(div().text_xs().text_color(rgb(colors::MUTED)).child(
                            if conversation.preview.is_empty() {
                                "No messages yet".to_string()
                            } else {
                                conversation.preview.clone()
                            },
                        )),
                )
                .child(
                    button::primary_icon(IconName::Phone)
                        .id(format!("call-group-{call_id}"))
                        .aria_label(format!("Call {}", conversation.name))
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            cx.stop_propagation();
                            state.start_voice_call_in_conversation(&call_id, cx)
                        }))
                        .on_key_down(cx.listener(move |state, event: &KeyDownEvent, _, cx| {
                            let key = event.keystroke.key.as_str();
                            if key.eq_ignore_ascii_case("enter") || key == "space" {
                                cx.stop_propagation();
                                state.start_voice_call_in_conversation(&call_key_id, cx);
                            }
                        })),
                )
                .when(is_owner, |this| {
                    this.child(
                        button::secondary("Manage")
                            .id(format!("manage-group-{manage_id}"))
                            .tab_index(0)
                            .on_click(cx.listener(move |state, _, _, cx| {
                                cx.stop_propagation();
                                state.open_group_membership(&manage_id, cx);
                            })),
                    )
                }),
        );
    }

    let group_list = if state
        .conversations
        .iter()
        .any(|conversation| conversation.is_group())
    {
        groups.into_any_element()
    } else {
        empty_state::render(
            "No groups yet",
            "Create a group with at least two friends to start a shared conversation.",
        )
        .into_any_element()
    };

    let focus = state.input_focus.clone();
    div()
        .h_full()
        .relative()
        .flex()
        .flex_col()
        .gap_6()
        .p(px(spacing::PAGE_GUTTER))
        .track_focus(&state.input_focus)
        .on_key_down(cx.listener(|state, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key.eq_ignore_ascii_case("escape") {
                if state.create_group_modal_open {
                    state.toggle_create_group_modal(cx);
                } else if state.manage_group_id.is_some() {
                    state.close_group_membership(cx);
                }
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_header::render(
                    "Groups",
                    &format!(
                        "{} group conversations · encrypted delivery to members",
                        state
                            .conversations
                            .iter()
                            .filter(|conversation| conversation.is_group())
                            .count()
                    ),
                ))
                .child(
                    button::icon_button(IconName::Plus)
                        .id("toggle-create-group-modal")
                        .aria_label(if state.create_group_modal_open {
                            "Close create group dialog"
                        } else {
                            "Create a group"
                        })
                        .tab_index(0)
                        .on_click(
                            cx.listener(|state, _, _, cx| state.toggle_create_group_modal(cx)),
                        ),
                ),
        )
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::MUTED))
                .child("YOUR GROUPS"),
        )
        .child(group_list)
        .when(state.create_group_modal_open, |this| {
            this.child(
                dialog::overlay().child(
                    dialog::surface()
                        .w(px(480.0))
                        .max_h(px(640.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors::TEXT))
                                        .child("Create group"),
                                )
                                .child(
                                    button::icon_button(IconName::Close)
                                        .id("close-create-group-modal")
                                        .aria_label("Close create group dialog")
                                        .tab_index(0)
                                        .on_click(cx.listener(|state, _, _, cx| {
                                            state.toggle_create_group_modal(cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(colors::MUTED))
                                .child("Choose a name and at least two friends."),
                        )
                        .child(group_name_input(state, window, cx, &focus))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(colors::MUTED))
                                .child(format!("FRIENDS ({})", state.friends.len())),
                        )
                        .child(
                            div()
                                .id("group-member-list")
                                .max_h(px(300.0))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .overflow_y_scroll()
                                .track_scroll(&state.group_member_scroll_handle)
                                .children(state.friends.iter().map(|friend| {
                                    let id = friend.id.clone();
                                    let key_id = id.clone();
                                    let selected =
                                        state.selected_group_members.contains(&friend.id);
                                    div()
                                        .id(format!("group-member-{id}"))
                                        .tab_index(0)
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .p_2()
                                        .rounded(px(spacing::RADIUS_MD))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
                                        .on_click(cx.listener(move |state, _, _, cx| {
                                            state.toggle_group_member(&id, cx)
                                        }))
                                        .on_key_down(cx.listener(
                                            move |state, event: &KeyDownEvent, _, cx| {
                                                let key = event.keystroke.key.as_str();
                                                if key.eq_ignore_ascii_case("enter")
                                                    || key == "space"
                                                {
                                                    cx.stop_propagation();
                                                    state.toggle_group_member(&key_id, cx);
                                                }
                                            },
                                        ))
                                        .child(icon::checkbox(selected))
                                        .child(avatar::render_user(friend, 32.0))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(colors::TEXT))
                                                        .child(friend.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(colors::MUTED))
                                                        .child(format!("@{}", friend.username)),
                                                ),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_xs().text_color(rgb(colors::MUTED)).child(
                                    format!("{} selected", state.selected_group_members.len()),
                                ))
                                .child(
                                    button::primary("Create group")
                                        .id("create-group")
                                        .tab_index(0)
                                        .on_click(
                                            cx.listener(|state, _, _, cx| state.create_group(cx)),
                                        ),
                                ),
                        ),
                ),
            )
        })
        .when_some(state.manage_group_id.clone(), |this, group_id| {
            if let Some(conversation) = state
                .conversations
                .iter()
                .find(|conversation| conversation.id == group_id && conversation.is_group())
            {
                this.child(manage_group_dialog(conversation, state, cx))
            } else {
                this
            }
        })
}

fn manage_group_dialog(
    conversation: &crate::models::conversation::Conversation,
    state: &AppState,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let group_id = conversation.id.clone();
    let addable_friends = state
        .friends
        .iter()
        .filter(|friend| {
            !conversation
                .members
                .iter()
                .any(|member| member.id == friend.id)
        })
        .collect::<Vec<_>>();
    dialog::overlay().child(
        dialog::surface()
            .w(px(480.0))
            .max_h(px(640.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(colors::TEXT))
                            .child(format!("Manage {}", conversation.name)),
                    )
                    .child(
                        button::icon_button(IconName::Close)
                            .id("close-group-membership")
                            .aria_label("Close group membership")
                            .tab_index(0)
                            .on_click(
                                cx.listener(|state, _, _, cx| state.close_group_membership(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors::MUTED))
                    .child(format!(
                        "MLS roster version {} · {} of 12 members",
                        conversation.membership_version,
                        conversation.members.len()
                    )),
            )
            .child(
                div()
                    .id("manage-group-members")
                    .max_h(px(240.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .overflow_y_scroll()
                    .track_scroll(&state.group_member_scroll_handle)
                    .children(conversation.members.iter().map(|member| {
                        let member_id = member.id.clone();
                        let action_group_id = group_id.clone();
                        let is_owner = conversation
                            .members
                            .first()
                            .is_some_and(|owner| owner.id == member.id);
                        let can_remove = !is_owner && conversation.members.len() > 3;
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT))
                                    .child(format!(
                                        "{}{} · {}",
                                        member.name,
                                        if is_owner { " · Owner" } else { "" },
                                        state.group_identity_status_label(&group_id, member)
                                    )),
                            )
                            .when(can_remove, |this| {
                                this.child(
                                    button::secondary("Remove")
                                        .id(format!("remove-group-member-{}", member.id))
                                        .on_click(cx.listener(move |state, _, _, cx| {
                                            cx.stop_propagation();
                                            state.remove_group_member(
                                                &action_group_id,
                                                &member_id,
                                                cx,
                                            );
                                        })),
                                )
                            })
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors::MUTED))
                    .child("ADD A FRIEND"),
            )
            .child(
                div()
                    .id("manage-group-friends")
                    .max_h(px(180.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .overflow_y_scroll()
                    .track_scroll(&state.group_member_scroll_handle)
                    .children(addable_friends.iter().map(|friend| {
                        let member_id = friend.id.clone();
                        let action_group_id = group_id.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT))
                                    .child(format!("{} (@{})", friend.name, friend.username)),
                            )
                            .child(
                                button::secondary("Add")
                                    .id(format!("add-group-member-{}", friend.id))
                                    .on_click(cx.listener(move |state, _, _, cx| {
                                        cx.stop_propagation();
                                        state.add_group_member(&action_group_id, &member_id, cx);
                                    })),
                            )
                    })),
            ),
    )
}

fn group_name_input(
    state: &AppState,
    window: &Window,
    cx: &mut Context<AppState>,
    focus: &gpui::FocusHandle,
) -> impl IntoElement {
    let focus = focus.clone();
    let focused = state.active_input == InputField::GroupName && focus.is_focused(window);
    div()
        .track_focus(&focus)
        .id("group-name-field")
        .min_h(px(spacing::CONTROL_HEIGHT))
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if focused {
            colors::ACCENT
        } else {
            colors::BORDER
        }))
        .bg(rgb(colors::CANVAS))
        .focus(|style| style.border_color(rgb(colors::ACCENT)))
        .focus_visible(|style| style.shadow_sm())
        .text_sm()
        .text_color(rgb(colors::TEXT))
        .child(input_text::render(
            &state.group_name_input,
            "Group name",
            state.selection_range(),
            focused,
            false,
        ))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |state, _, window, cx| {
                state.activate_input(InputField::GroupName);
                let focus = state.input_focus.clone();
                window.focus(&focus, cx);
                cx.notify();
            }),
        )
        .on_key_down(cx.listener(|state, event: &KeyDownEvent, _, cx| {
            state.handle_input_key(event, cx);
        }))
}
