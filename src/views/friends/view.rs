use gpui::prelude::*;
use gpui::{Context, IntoElement, KeyDownEvent, MouseButton, Window, div, px, relative, rgb};

use crate::{
    app_state::{AppState, InputField, PendingConfirmation},
    components::{
        avatar, button, card, dialog, empty_state, form_field, icon::IconName, input_text,
        section_header,
    },
    theme::{colors, spacing},
};

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_2();
    for friend in &state.friends {
        let open_peer_id = friend.id.clone();
        let key_peer_id = open_peer_id.clone();
        let call_peer_id = open_peer_id.clone();
        let call_key_peer_id = open_peer_id.clone();
        let remove_peer_id = open_peer_id.clone();
        let remove_key_peer_id = remove_peer_id.clone();
        let verify_peer_id = open_peer_id.clone();
        let verify_key_peer_id = verify_peer_id.clone();
        let card_id = format!("friend-card-{open_peer_id}");
        list = list.child(
            div()
                .id(card_id)
                .aria_label(format!("Open conversation with {}", friend.name))
                .tab_index(0)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(spacing::RADIUS_XL))
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::PANEL))
                .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
                .on_click(cx.listener(move |state, _, _, cx| {
                    state.open_or_create_conversation(&open_peer_id, cx)
                }))
                .on_key_down(cx.listener(move |state, event: &KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    if key.eq_ignore_ascii_case("enter") || key == "space" {
                        state.open_or_create_conversation(&key_peer_id, cx);
                    }
                }))
                .child(avatar::render_user(friend, 42.0))
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
                                .child(friend.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("{} · {}", friend.username, friend.status)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            button::success_icon(IconName::Phone)
                                .id(format!("call-friend-{call_peer_id}"))
                                .aria_label(format!("Call {}", friend.name))
                                .tab_index(0)
                                .on_click(cx.listener(move |state, _, _, cx| {
                                    cx.stop_propagation();
                                    state.start_voice_call_with_friend(&call_peer_id, cx)
                                }))
                                .on_key_down(cx.listener(move |state, event: &KeyDownEvent, _, cx| {
                                    let key = event.keystroke.key.as_str();
                                    if key.eq_ignore_ascii_case("enter") || key == "space" {
                                        cx.stop_propagation();
                                        state.start_voice_call_with_friend(&call_key_peer_id, cx);
                                    }
                                })),
                        )
                        .child(
                            button::clear_icon(IconName::Identity)
                                .id(format!("verify-friend-{verify_peer_id}"))
                                .aria_label(format!(
                                    "Manage identity verification for {}",
                                    friend.name
                                ))
                                .tab_index(0)
                                .on_click(cx.listener(move |state, _, _, cx| {
                                    cx.stop_propagation();
                                    state.open_identity_verification(&verify_peer_id, cx)
                                }))
                                .on_key_down(cx.listener(move |state, event: &KeyDownEvent, _, cx| {
                                    let key = event.keystroke.key.as_str();
                                    if key.eq_ignore_ascii_case("enter") || key == "space" {
                                        cx.stop_propagation();
                                        state.open_identity_verification(&verify_key_peer_id, cx);
                                    }
                                })),
                        )
                        .child(
                            button::destructive_icon_button(IconName::Trash)
                                .id(format!("remove-peer-{remove_peer_id}"))
                                .aria_label(format!("Remove {}", friend.name))
                                .tab_index(0)
                                .on_click(cx.listener(move |state, _, _, cx| {
                                    cx.stop_propagation();
                                    state.request_confirmation(
                                        PendingConfirmation::RemoveFriend(remove_peer_id.clone()),
                                        cx,
                                    )
                                }))
                                .on_key_down(cx.listener(move |state, event: &KeyDownEvent, _, cx| {
                                    let key = event.keystroke.key.as_str();
                                    if key.eq_ignore_ascii_case("enter") || key == "space" {
                                        cx.stop_propagation();
                                        state.request_confirmation(
                                            PendingConfirmation::RemoveFriend(remove_key_peer_id.clone()),
                                            cx,
                                        );
                                    }
                                })),
                        ),
                ),
        );
    }

    let peers = if state.friends.is_empty() {
        empty_state::render(
            "No friends yet",
            "Send a friend request by username. You can chat once the other person accepts it.",
        )
        .into_any_element()
    } else {
        list.into_any_element()
    };
    let verification_requests = if state.incoming_identity_verification_requests.is_empty() {
        div().into_any_element()
    } else {
        let mut requests = div().flex().flex_col().gap_2();
        for request in &state.incoming_identity_verification_requests {
            let peer_id = request.friend.id.clone();
            let request_id = request.request_id.clone();
            let accept_peer_id = peer_id.clone();
            let accept_request_id = request_id.clone();
            let decline_peer_id = peer_id;
            let decline_request_id = request_id;
            requests = requests.child(
                card::surface()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_3()
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
                                    .child(format!(
                                        "@{} wants to verify your identity",
                                        request.friend.username
                                    )),
                            )
                            .child(div().text_xs().text_color(rgb(colors::MUTED)).child(
                                "They are online now. Accept to verify each other on this device.",
                            )),
                    )
                    .child(
                        button::secondary("Decline")
                            .id(format!("decline-identity-verification-{decline_peer_id}"))
                            .tab_index(0)
                            .on_click(cx.listener(move |state, _, _, cx| {
                                state.respond_to_identity_verification(
                                    &decline_peer_id,
                                    &decline_request_id,
                                    false,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        button::primary("Accept")
                            .id(format!("accept-identity-verification-{accept_peer_id}"))
                            .tab_index(0)
                            .on_click(cx.listener(move |state, _, _, cx| {
                                state.respond_to_identity_verification(
                                    &accept_peer_id,
                                    &accept_request_id,
                                    true,
                                    cx,
                                )
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors::MUTED))
                    .child("LIVE IDENTITY REQUESTS"),
            )
            .child(requests)
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
                if state.verifying_friend_id.is_some() {
                    state.close_identity_verification(cx);
                } else if state.add_friend_modal_open {
                    state.toggle_add_friend_modal(cx);
                }
            } else if state.verifying_friend_id.is_some() || state.add_friend_modal_open {
                state.handle_input_key(event, cx);
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_header::render(
                    "Friends",
                    &format!("{} friends · direct connections", state.friends.len(),),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            button::primary(format!(
                                "Add friend ({})",
                                state.incoming_friend_requests.len()
                                    + state.outgoing_friend_requests.len()
                            ))
                                .id("toggle-add-friend-modal")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| {
                                        state.toggle_add_friend_modal(cx)
                                    }),
                                ),
                        ),
                ),
        )
        .child(verification_requests)
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::MUTED))
                .child("FRIENDS"),
        )
        .child(peers)
        .when(state.add_friend_modal_open, |this| {
            this.child(
                dialog::overlay()
                    .child(
                        dialog::surface()
                            .w(px(480.0))
                            .max_h(relative(0.9))
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
                                            .child("Add friend"),
                                    )
                                    .child(
                                        button::icon_button(IconName::Close)
                                            .id("close-add-friend-modal")
                                            .aria_label("Close add friend dialog")
                                            .tab_index(0)
                                            .on_click(cx.listener(|state, _, _, cx| {
                                                state.toggle_add_friend_modal(cx)
                                            })),
                                    ),
                            )
                            .child(div().text_sm().text_color(rgb(colors::MUTED)).child(
                                "Send a request by username. They must accept before you can chat.",
                            ))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(div().flex_1().child(input_field(
                                        state,
                                        window,
                                        cx,
                                        &focus,
                                        "Username",
                                        &state.peer_username_input,
                                        InputField::PeerUsername,
                                    )))
                                    .child(
                                        button::primary("Send request")
                                            .id("add-peer")
                                            .tab_index(0)
                                            .on_click(
                                                cx.listener(|state, _, _, cx| state.add_friend(cx)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(colors::TEXT))
                                            .child(format!(
                                                "Friend requests ({})",
                                                state.incoming_friend_requests.len()
                                                    + state.outgoing_friend_requests.len()
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("friend-requests-list")
                                            .max_h(px(360.0))
                                            .flex()
                                            .flex_col()
                                            .overflow_y_scroll()
                                            .track_scroll(&state.friend_requests_scroll_handle)
                                            .child(friend_request_sections(state, cx)),
                                    ),
                            ),
                    ),
            )
        })
        .when_some(state.verifying_friend_id.clone(), |this, peer_id| {
            let Some(friend) = state.friend(&peer_id) else {
                return this;
            };
            let status = state.identity_status_label(friend);
            let is_verified = status == "Verified on this device";
            let revoke_peer_id = peer_id.clone();
            this.child(
                dialog::overlay()
                    .child(
                        dialog::surface()
                            .id("identity-verification-content")
                            .w(px(560.0))
                            .max_h(relative(0.9))
                            .overflow_y_scroll()
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
                                            .child(format!("Verify @{}", friend.username)),
                                    )
                                    .child(
                                        button::icon_button(IconName::Close)
                                            .id("close-identity-verification")
                                            .aria_label("Close identity verification")
                                            .tab_index(0)
                                            .on_click(cx.listener(|state, _, _, cx| {
                                                state.close_identity_verification(cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors::MUTED))
                                    .child(format!("Status: {status}.")),
                            )
                            .when(is_verified, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .child(
                                            button::secondary("Revoke verification")
                                                .id(format!(
                                                    "revoke-identity-{revoke_peer_id}"
                                                ))
                                                .tab_index(0)
                                                .on_click(cx.listener(move |state, _, _, cx| {
                                                    state.revoke_contact_identity(
                                                        &revoke_peer_id,
                                                        cx,
                                                    )
                                                })),
                                        ),
                                )
                            })
                            .when(!is_verified, |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(colors::MUTED))
                                        .child(format!(
                                            "Send a live request to @{}; they must accept while signed in. Acceptance confirms control of the account key, not a person's real-world identity.",
                                            friend.username
                                        )),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors::MUTED))
                                        .child("Requests are delivered directly and are not queued. Compare fingerprints out of band if you need to verify who controls the account."),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .child(
                                            button::primary("Send verification request")
                                                .id("send-contact-identity-request")
                                                .tab_index(0)
                                                .on_click(cx.listener(move |state, _, _, cx| {
                                                    state.request_contact_identity_verification(
                                                        &peer_id,
                                                        cx,
                                                    )
                                                })),
                                        ),
                                )
                            }),
                    ),
            )
        })
}

fn friend_request_sections(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    if state.incoming_friend_requests.is_empty() && state.outgoing_friend_requests.is_empty() {
        return empty_state::render(
            "No pending requests",
            "Incoming and outgoing friend requests will appear here.",
        )
        .into_any_element();
    }

    let mut sections = div().flex().flex_col().gap_4();
    let mut incoming = div().flex().flex_col().gap_2();
    for request in &state.incoming_friend_requests {
        let peer_id = request.id.clone();
        let accept_id = peer_id.clone();
        let decline_id = peer_id;
        incoming = incoming.child(
            card::surface()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .child(avatar::render_user(request, 40.0))
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
                                .child(request.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("@{}", request.username)),
                        ),
                )
                .child(
                    button::secondary("Decline")
                        .id(format!("decline-friend-request-{decline_id}"))
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.decline_friend_request(&decline_id, cx)
                        })),
                )
                .child(
                    button::primary("Accept")
                        .id(format!("accept-friend-request-{accept_id}"))
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.accept_friend_request(&accept_id, cx)
                        })),
                ),
        );
    }
    sections = sections.child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors::MUTED))
                    .child(format!(
                        "INCOMING ({})",
                        state.incoming_friend_requests.len()
                    )),
            )
            .child(incoming),
    );

    let mut outgoing = div().flex().flex_col().gap_2();
    for request in &state.outgoing_friend_requests {
        let cancel_id = request.id.clone();
        outgoing = outgoing.child(
            card::surface()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .child(avatar::render_user(request, 40.0))
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
                                .child(request.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("@{}", request.username)),
                        ),
                )
                .child(
                    button::secondary("Cancel")
                        .id(format!("cancel-friend-request-{cancel_id}"))
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.cancel_friend_request(&cancel_id, cx)
                        })),
                ),
        );
    }
    sections = sections.child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors::MUTED))
                    .child(format!(
                        "OUTGOING ({})",
                        state.outgoing_friend_requests.len()
                    )),
            )
            .child(outgoing),
    );

    sections.into_any_element()
}

fn input_field(
    state: &AppState,
    window: &Window,
    cx: &mut Context<AppState>,
    focus: &gpui::FocusHandle,
    label: &'static str,
    value: &str,
    field: InputField,
) -> impl IntoElement {
    let focus = focus.clone();
    let focused = state.active_input == field && focus.is_focused(window);
    form_field::input_frame(focused)
        .track_focus(&focus)
        .id(format!("peer-field-{label}"))
        .child(input_text::render(
            value,
            label,
            state.selection_range(),
            focused,
            false,
        ))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |state, _, window, cx| {
                state.activate_input(field);
                let focus = state.input_focus.clone();
                window.focus(&focus, cx);
                cx.notify();
            }),
        )
}
