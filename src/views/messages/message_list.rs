use crate::{
    app_state::{AppState, PendingConfirmation},
    components::{avatar, button, card, icon::IconName, menu, message},
    models::{
        conversation::Conversation,
        message::{Message, RecipientDeliveryState},
    },
    services::attachment::{self, AttachmentKind, AttachmentMetadata},
    theme::{colors, spacing},
};
use gpui::prelude::*;
use gpui::{
    Context, Div, IntoElement, MouseButton, TextRun, Window, deferred, div, font, px, relative, rgb,
};

fn parse_timestamp(timestamp: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(numeric) = timestamp.parse::<i64>() {
        let seconds = if numeric.abs() >= 100_000_000_000 {
            numeric.div_euclid(1000)
        } else {
            numeric
        };
        if let Some(datetime) = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0) {
            return Some(datetime);
        }
    }
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|datetime| datetime.with_timezone(&chrono::Utc))
}

fn message_order_cmp(left: &Message, right: &Message) -> std::cmp::Ordering {
    match (left.queue_sequence, right.queue_sequence) {
        (Some(left_queue), Some(right_queue)) => left_queue.cmp(&right_queue),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => parse_timestamp(&left.sent_at)
            .map(|value| value.timestamp())
            .unwrap_or(i64::MIN)
            .cmp(
                &parse_timestamp(&right.sent_at)
                    .map(|value| value.timestamp())
                    .unwrap_or(i64::MIN),
            ),
    }
}

#[cfg(test)]
mod message_order_tests {
    use super::{Message, message_order_cmp};

    fn message(id: &str, queue_sequence: u64) -> Message {
        let mut message = Message::new(id, "conversation", "author", "Author", id, "1", false);
        message.queue_sequence = Some(queue_sequence);
        message
    }

    #[test]
    fn alternating_sender_messages_follow_server_queue_sequence() {
        let mut messages = vec![
            message("b2", 4),
            message("a1", 1),
            message("b1", 3),
            message("a2", 2),
        ];
        messages.sort_by(message_order_cmp);
        let ids = messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["a1", "a2", "b1", "b2"]);
    }
}

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let selected = state
        .selected_conversation
        .as_deref()
        .and_then(|id| state.conversation(id));
    let Some(conversation) = selected else {
        return crate::components::empty_state::render(
            "No conversation selected",
            "Choose a recent conversation in the sidebar or start one from Friends.",
        )
        .into_any_element();
    };
    let conversation_id = conversation.id.clone();
    let group_conversation = conversation.is_group();
    let peer = if group_conversation {
        None
    } else {
        state.friend(&conversation.user_id)
    };
    if !group_conversation && peer.is_none() {
        return crate::components::empty_state::render(
            "Friend unavailable",
            "Open a conversation from your Friends list.",
        )
        .into_any_element();
    }
    let peer_initials = peer.map(|peer| peer.initials.as_str()).unwrap_or("");
    let peer_color = peer.map(|peer| peer.color).unwrap_or(colors::ACCENT);
    let peer_status = peer.map(|peer| peer.status.as_str()).unwrap_or("");
    let mut messages: Vec<_> = state
        .conversation_messages(&conversation.id)
        .cloned()
        .collect();
    let conversation_menu_id = format!("conversation:{}", conversation.id);
    let header_menu_key = conversation_menu_id.clone();
    let conversation_menu_open =
        state.active_menu.as_deref() == Some(conversation_menu_id.as_str());
    let mut conversation_actions = button::clear_icon(IconName::MoreVertical)
        .id("conversation-actions")
        .aria_label("Conversation actions")
        .tab_index(0)
        .on_click(cx.listener(move |state, _, _, cx| state.toggle_menu(&header_menu_key, cx)));

    if conversation_menu_open {
        let close_id = conversation.id.clone();
        let delete_id = conversation.id.clone();
        conversation_actions = conversation_actions.child(deferred(
            button::menu_surface(&conversation_menu_id, cx)
                .child(menu::item(
                    "close-current-conversation",
                    "Close conversation",
                    false,
                    cx,
                    move |state, _, cx| state.close_conversation(&close_id, cx),
                ))
                .child(menu::item(
                    "delete-current-conversation",
                    "Delete conversation",
                    true,
                    cx,
                    move |state, _, cx| {
                        state.request_confirmation(
                            PendingConfirmation::DeleteConversation(delete_id.clone()),
                            cx,
                        )
                    },
                )),
        ));
    }
    let mut conversation_call = div().flex().items_center().gap_2();
    if state.active_voice_call.is_none() && state.incoming_voice_call.is_none() {
        conversation_call = conversation_call.child(
            button::success_icon(IconName::Phone)
                .id("start-conversation-voice-call")
                .aria_label("Start voice call")
                .tab_index(0)
                .on_click(cx.listener(|state, _, _, cx| state.start_voice_call(cx))),
        );
    }

    let mut message_area = div()
        .h_full()
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .bg(rgb(colors::CANVAS))
        .on_key_down(cx.listener(|state, event, window, cx| {
            state.handle_menu_key(event, window, cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_6()
                .py_5()
                .border_b_1()
                .border_color(rgb(colors::BORDER))
                .child(if group_conversation {
                    crate::components::icon::render(IconName::Friends, 30.0, colors::ACCENT)
                        .into_any_element()
                } else {
                    avatar::render(peer_initials, peer_color, 34.0).into_any_element()
                })
                .bg(rgb(colors::SIDEBAR))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
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
                                .text_color(if group_conversation {
                                    rgb(colors::MUTED)
                                } else if peer_status == "Online" {
                                    rgb(colors::GREEN)
                                } else {
                                    rgb(colors::MUTED)
                                })
                                .child(if group_conversation {
                                    format!("Group · {} members", conversation.members.len())
                                } else if peer_status == "Online" {
                                    "Online".to_string()
                                } else {
                                    "Offline".to_string()
                                }),
                        ),
                )
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(conversation_call)
                        .child(conversation_actions),
                ),
        );

    if let Some(call_panel) = conversation_call_panel(state, conversation, cx) {
        message_area = message_area.child(call_panel);
    }

    let mut timeline = div()
        .id("message-timeline")
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .gap_5()
        .p(px(spacing::PANEL_GUTTER))
        .overflow_y_scroll()
        .track_scroll(&state.message_scroll_handle);
    fn format_timestamp(timestamp: &str) -> String {
        parse_timestamp(timestamp)
            .map(|datetime| datetime.format("%d.%m.%Y · %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| timestamp.to_string())
    }

    fn delivery_failure_label(code: &str) -> &str {
        match code {
            "retry_limit" => "device retry limit reached",
            "missing_mls_state" => "waiting for missing MLS state",
            "member_removed" => "member removed",
            "rejected" => "rejected by device",
            other => other,
        }
    }

    fn format_time(timestamp: &str) -> String {
        parse_timestamp(timestamp)
            .map(|datetime| datetime.format("%H:%M").to_string())
            .unwrap_or_else(|| timestamp.to_string())
    }

    fn message_day(timestamp: &str) -> (String, String) {
        parse_timestamp(timestamp)
            .map(|datetime| {
                (
                    datetime.format("%Y-%m-%d").to_string(),
                    datetime.format("%A, %B %-d, %Y").to_string(),
                )
            })
            .unwrap_or_else(|| ("unknown".into(), "Date unavailable".into()))
    }

    messages.sort_by(message_order_cmp);
    let mut current_day: Option<String> = None;
    for message in messages {
        let (day_key, day_label) = message_day(&message.sent_at);
        if current_day.as_deref() != Some(day_key.as_str()) {
            timeline = timeline.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(colors::MUTED))
                    .child(day_label),
            );
            current_day = Some(day_key);
        }
        let status_menu_id = format!("message-status:{}", message.id);
        let status_menu_key = status_menu_id.clone();
        let retry_message_id = message.id.clone();
        let message_menu_id = format!("message-actions:{}", message.id);
        let message_menu_key = message_menu_id.clone();
        let message_menu_open = state.active_menu.as_deref() == Some(message_menu_id.as_str());
        let status_menu_open = state.active_menu.as_deref() == Some(status_menu_id.as_str());
        let sent_at = format_time(&message.sent_at);
        let group_delivery_details = if message.is_mine && group_conversation {
            message
                .group_deliveries
                .iter()
                .map(|delivery| {
                    let name = conversation
                        .members
                        .iter()
                        .chain(state.friends.iter())
                        .find(|member| member.id == delivery.recipient_id)
                        .map(|member| member.username.as_str())
                        .unwrap_or("Former group member");
                    let detail = match delivery.state {
                        RecipientDeliveryState::Pending if !delivery.api_accepted => {
                            delivery.next_attempt_at.as_deref().map_or_else(
                                || "retry scheduled".to_string(),
                                |time| format!("retry at {}", format_timestamp(time)),
                            )
                        }
                        RecipientDeliveryState::Pending if !delivery.device_available => {
                            "waiting for a registered device".into()
                        }
                        RecipientDeliveryState::Pending if delivery.pending_devices > 0 => {
                            let status_prefix = delivery
                                .last_error
                                .as_deref()
                                .map(delivery_failure_label)
                                .filter(|label| *label != "temporary")
                                .map(|label| format!("{label} · "))
                                .unwrap_or_default();
                            delivery.next_attempt_at.as_deref().map_or_else(
                                || {
                                    format!(
                                        "{status_prefix}pending on {}/{} devices · attempt {}",
                                        delivery.pending_devices,
                                        delivery.active_devices,
                                        delivery.attempts
                                    )
                                },
                                |time| {
                                    format!(
                                        "{status_prefix}retry at {} · pending on {}/{} devices",
                                        format_timestamp(time),
                                        delivery.pending_devices,
                                        delivery.active_devices
                                    )
                                },
                            )
                        }
                        RecipientDeliveryState::Pending => {
                            format!("awaiting device delivery · attempt {}", delivery.attempts)
                        }
                        RecipientDeliveryState::Delivered
                            if !delivery.api_accepted && delivery.api_failure_permanent =>
                        {
                            delivery.last_error.as_deref().map_or_else(
                                || "delivered · API synchronization rejected".into(),
                                |error| {
                                    format!(
                                        "delivered · API synchronization rejected: {}",
                                        delivery_failure_label(error)
                                    )
                                },
                            )
                        }
                        RecipientDeliveryState::Delivered if !delivery.api_accepted => {
                            format!("delivered · device sync pending ({}/5)", delivery.attempts)
                        }
                        RecipientDeliveryState::Delivered if delivery.pending_devices > 0 => {
                            format!(
                                "delivered · {} other device(s) pending",
                                delivery.pending_devices
                            )
                        }
                        RecipientDeliveryState::Delivered if delivery.failed_devices > 0 => {
                            format!(
                                "delivered · {} other device(s) failed",
                                delivery.failed_devices
                            )
                        }
                        RecipientDeliveryState::Delivered => "delivered".into(),
                        RecipientDeliveryState::Failed => {
                            delivery.last_error.as_deref().map_or_else(
                                || {
                                    format!(
                                        "failed · {}/{} devices · {} attempts",
                                        delivery.failed_devices,
                                        delivery.active_devices,
                                        delivery.attempts
                                    )
                                },
                                |error| {
                                    format!(
                                        "failed · {}/{} devices: {}",
                                        delivery.failed_devices,
                                        delivery.active_devices,
                                        delivery_failure_label(error)
                                    )
                                },
                            )
                        }
                        RecipientDeliveryState::Expired => {
                            format!("expired · {} device(s)", delivery.expired_devices)
                        }
                        RecipientDeliveryState::Rejected => {
                            delivery.last_error.as_deref().map_or_else(
                                || format!("rejected · {} device(s)", delivery.rejected_devices),
                                |reason| {
                                    format!(
                                        "rejected · {} device(s): {}",
                                        delivery.rejected_devices,
                                        delivery_failure_label(reason)
                                    )
                                },
                            )
                        }
                    };
                    let detail = if delivery.read_at.is_some() {
                        format!("{detail} · read")
                    } else {
                        detail
                    };
                    format!("@{name} · {detail}")
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let retryable_group_delivery = message.group_deliveries.iter().any(|delivery| {
            delivery.state == RecipientDeliveryState::Failed
                || (delivery.state == RecipientDeliveryState::Delivered
                    && !delivery.api_accepted
                    && !delivery.api_failure_permanent
                    && delivery.attempts >= 5)
        });
        let show_retry = message.is_mine
            && if message.group_deliveries.is_empty() {
                message.delivery_state == "failed"
            } else {
                retryable_group_delivery
            };
        let mut timestamp = div().relative().h(px(20.0)).flex().items_center().child(
            div()
                .id(format!("message-status-button-{}", message.id))
                .h_full()
                .flex()
                .items_center()
                .text_sm()
                .text_color(rgb(colors::MUTED))
                .cursor_pointer()
                .child(sent_at)
                .on_click(
                    cx.listener(move |state, _, _, cx| state.toggle_menu(&status_menu_key, cx)),
                ),
        );
        if status_menu_open {
            let status_menu_dismiss_id = status_menu_id.clone();
            timestamp = timestamp.child(deferred(
                button::menu_surface(&status_menu_dismiss_id, cx)
                    .w(px(280.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(rgb(colors::TEXT))
                                    .child("Message activity"),
                            )
                            .children(
                                [
                                    ("Sent", Some(message.sent_at.as_str())),
                                    ("Delivered", message.delivered_at.as_deref()),
                                    ("Read", message.read_at.as_deref()),
                                ]
                                .into_iter()
                                .map(|(label, timestamp)| {
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .text_xs()
                                        .child(
                                            div()
                                                .w(px(64.0))
                                                .flex_shrink_0()
                                                .text_color(rgb(colors::MUTED))
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .justify_end()
                                                .text_color(rgb(colors::TEXT))
                                                .child(
                                                    timestamp
                                                        .map(format_timestamp)
                                                        .unwrap_or_else(|| "Not yet".into()),
                                                ),
                                        )
                                }),
                            )
                            .when(!group_delivery_details.is_empty(), |this| {
                                this.child(
                                    div()
                                        .mt_2()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(rgb(colors::MUTED))
                                        .children(
                                            group_delivery_details
                                                .into_iter()
                                                .map(|detail| div().child(detail)),
                                        ),
                                )
                            }),
                    ),
            ));
        }
        let mut message_row = message::bubble(message.is_mine);
        if !message.deleted {
            message_row = message_row.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |state, _, _, cx| state.toggle_menu(&message_menu_key, cx)),
            );
        }
        let message_sender = if message.is_mine {
            "You".to_string()
        } else {
            message.author_name.clone()
        };
        let body = if message.deleted {
            "You deleted this message"
        } else {
            message.body.as_str()
        };
        let content_body =
            if !message.deleted && message.attachment.is_some() && message.attachments.is_empty() {
                ""
            } else {
                body
            };
        let message_attachments = message.attachments().cloned().collect::<Vec<_>>();
        let message_content = if !message.deleted && !message_attachments.is_empty() {
            render_message_content(
                state,
                &message.id,
                content_body,
                &message_attachments,
                window,
                cx,
            )
            .into_any_element()
        } else {
            render_message_body(state, &message.id, body, window, cx).into_any_element()
        };
        message_row = message_row.child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .h(px(20.0))
                                .flex()
                                .items_center()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(rgb(colors::TEXT))
                                .child(message_sender),
                        )
                        .child(timestamp),
                )
                .child(message_content)
                .when(show_retry, |this| {
                    this.child(
                        div().w_full().flex().justify_end().text_xs().child(
                            div()
                                .id(format!("retry-message-{}", message.id))
                                .cursor_pointer()
                                .text_color(rgb(colors::ACCENT))
                                .child("Retry")
                                .on_click(cx.listener(move |state, _, _, cx| {
                                    state.retry_failed_message(&retry_message_id, cx)
                                })),
                        ),
                    )
                }),
        );
        if !message.deleted && message_menu_open {
            let copy_id = message.id.clone();
            let copy_body = if body.is_empty() {
                message.preview()
            } else {
                body.to_string()
            };
            let selection_id = message.id.clone();
            let selection_body = body.to_string();
            let edit_id = message.id.clone();
            let delete_id = message.id.clone();
            let mut surface = button::menu_surface(&message_menu_id, cx)
                .child(menu::item(
                    format!("copy-selection-{}", message.id),
                    "Copy selected text",
                    false,
                    cx,
                    move |state, window, cx| {
                        let editor =
                            state.message_body_editor(&selection_id, &selection_body, window, cx);
                        let selected = editor.read(cx).state().selected_text();
                        if !selected.is_empty() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                        }
                        state.active_menu = None;
                        cx.notify();
                    },
                ))
                .child(menu::item(
                    format!("copy-message-{}", message.id),
                    "Copy message",
                    false,
                    cx,
                    move |state, _, cx| state.copy_message(&copy_id, &copy_body, cx),
                ));
            if message.is_mine && message.attachments().next().is_none() {
                surface = surface
                    .child(menu::item(
                        format!("edit-message-{}", message.id),
                        "Edit message",
                        false,
                        cx,
                        move |state, window, cx| state.edit_message(&edit_id, window, cx),
                    ))
                    .child(menu::item(
                        format!("delete-message-{}", message.id),
                        "Delete message",
                        true,
                        cx,
                        move |state, _, cx| state.delete_message(&delete_id, cx),
                    ));
            }
            message_row = message_row.child(deferred(surface));
        }
        timeline = timeline.child(message_row);
    }
    if !state
        .selected_conversation
        .as_deref()
        .map(|id| state.conversation_messages(id).next().is_some())
        .unwrap_or(false)
    {
        timeline = timeline.child(crate::components::empty_state::render(
            "No messages",
            if group_conversation {
                "Send a message to share it securely with every group member."
            } else {
                "Messages are encrypted for your friend and delivered when they next connect."
            },
        ));
    }

    let draft_focused = state
        .message_input
        .read(cx)
        .focus_handle_ref()
        .is_focused(window);
    let mut pending_attachment_elements = Vec::new();
    for pending in &state.pending_attachments {
        pending_attachment_elements.push(render_pending_attachment(pending, cx));
    }
    let mut compose = div()
        .mx_5()
        .mb_5()
        .flex()
        .flex_col()
        .items_stretch()
        .flex_shrink_0()
        .gap_2();
    if state.editing_message_id.is_some() {
        compose = compose.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(rgb(colors::MUTED))
                .child("Editing message")
                .child(
                    button::quiet("Cancel")
                        .id("cancel-message-edit")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| state.cancel_message_edit(cx))),
                ),
        );
    }
    compose = compose.child(
        card::elevated()
            .id("message-draft")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            .key_context("TextBox")
            .px_3()
            .pt_2()
            .pb_2()
            .when(draft_focused, |this| this.shadow_sm())
            .child(state.message_input.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_start()
                            .gap_2()
                            .children(pending_attachment_elements),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                button::composer_clear_icon(IconName::Paperclip)
                                    .id("attach-message-file")
                                    .aria_label("Attach file")
                                    .tab_index(0)
                                    .on_click(
                                        cx.listener(|state, _, _, cx| state.choose_attachment(cx)),
                                    ),
                            )
                            .child(
                                button::composer_clear_icon(if state.is_recording_voice() {
                                    IconName::Stop
                                } else {
                                    IconName::Microphone
                                })
                                .id("record-voice-note")
                                .aria_label(if state.is_recording_voice() {
                                    "Stop voice recording"
                                } else {
                                    "Record voice note"
                                })
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.toggle_voice_recording(cx)),
                                ),
                            )
                            .child(
                                button::composer_icon(if state.editing_message_id.is_some() {
                                    IconName::Check
                                } else {
                                    IconName::ArrowUp
                                })
                                .id(if state.editing_message_id.is_some() {
                                    "save-message-edit"
                                } else {
                                    "send-message"
                                })
                                .aria_label(if state.editing_message_id.is_some() {
                                    "Save edit"
                                } else {
                                    "Send message"
                                })
                                .tab_index(0)
                                .on_click(cx.listener(|state, _, _, cx| state.send_message(cx))),
                            ),
                    ),
            ),
    );
    message_area = message_area.child(timeline).child(compose);

    if state.selected_conversation.as_deref() != Some(conversation_id.as_str()) {
        return crate::components::empty_state::render(
            "No conversation selected",
            "Choose a recent conversation in the sidebar or start one from Friends.",
        )
        .into_any_element();
    }
    message_area.into_any_element()
}

fn conversation_call_panel(
    state: &AppState,
    conversation: &Conversation,
    cx: &mut Context<AppState>,
) -> Option<impl IntoElement> {
    if let Some(incoming) = state
        .incoming_voice_call
        .as_ref()
        .filter(|call| call.conversation_id == conversation.id)
    {
        let caller_name = incoming.caller_name.clone();
        return Some(
            card::elevated()
                .id("conversation-incoming-voice-call")
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_5()
                .py_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(colors::TEXT))
                                .child("Incoming voice call"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!("{caller_name} is calling")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button::success_icon(IconName::Phone)
                                .id("conversation-accept-voice-call")
                                .aria_label("Accept call")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.accept_voice_call(cx)),
                                ),
                        )
                        .child(
                            button::destructive_icon_button(IconName::PhoneOff)
                                .id("conversation-decline-voice-call")
                                .aria_label("Decline call")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.decline_voice_call(cx)),
                                ),
                        ),
                ),
        );
    }

    let call = state
        .active_voice_call
        .as_ref()
        .filter(|call| call.conversation_id == conversation.id)?;
    let local_user_id = state.current_user.as_ref().map(|user| user.id.as_str());
    let status = if call.connected {
        "In call"
    } else if Some(call.initiator_id.as_str()) == local_user_id {
        "Outbound call"
    } else {
        "Joining call"
    };

    let mic_control = div()
        .id("conversation-toggle-voice-call-mute")
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .aria_label(if call.muted {
            "Unmute microphone"
        } else {
            "Mute microphone"
        })
        .tab_index(0)
        .child(button::clear_icon(if call.muted {
            IconName::MicrophoneOff
        } else {
            IconName::Microphone
        }))
        .on_click(cx.listener(|state, _, _, cx| state.toggle_voice_call_mute(cx)));
    let leave_control = div()
        .id("conversation-leave-voice-call")
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .aria_label("Leave call")
        .tab_index(0)
        .child(button::destructive_icon_button(IconName::PhoneOff))
        .on_click(cx.listener(|state, _, _, cx| state.end_voice_call(true, cx)));

    let controls = div()
        .flex()
        .items_center()
        .gap_1()
        .child(mic_control)
        .child(leave_control);

    let call_header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors::TEXT))
                .child(status.to_string() + if call.connected { "" } else { "…" }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::MUTED))
                        .child(format!("{} participants", call.participants.len())),
                )
                .child(controls),
        );

    let panel = card::unrounded()
        .id("conversation-active-voice-call")
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .px_5()
        .py_4()
        .child(call_header);

    Some(panel)
}

fn render_message_body(
    state: &AppState,
    message_id: &str,
    body: &str,
    window: &Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let text_width = body
        .split('\n')
        .map(|line| {
            window
                .text_system()
                .shape_line(
                    line.into(),
                    px(14.0),
                    &[TextRun {
                        len: line.len(),
                        font: font(""),
                        color: rgb(colors::MUTED).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width()
        })
        .fold(px(0.0), |wide, line_width| wide.max(line_width));
    let bubble_max_width = window.viewport_size().width * 0.4;
    let padding_width = px(spacing::PANEL_GUTTER);
    let content_width = (text_width + px(8.0))
        .min(bubble_max_width - padding_width)
        .max(px(1.0));

    div()
        .min_w_0()
        .w(content_width)
        .max_w(relative(1.0))
        .child(state.message_body_editor(message_id, body, window, cx))
}

fn render_message_content(
    state: &AppState,
    message_id: &str,
    body: &str,
    attachments: &[AttachmentMetadata],
    window: &Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let mut content = div().flex().flex_col().min_w_0().gap_2();
    if !body.trim().is_empty() {
        content = content.child(render_message_body(state, message_id, body, window, cx));
    }
    for (index, metadata) in attachments.iter().enumerate() {
        content = content.child(render_attachment_body(
            state,
            metadata,
            &format!("{message_id}-{index}"),
            cx,
        ));
    }
    content
}

fn render_attachment_body(
    state: &AppState,
    metadata: &AttachmentMetadata,
    message_id: &str,
    cx: &mut Context<AppState>,
) -> Div {
    let download_metadata = metadata.clone();
    let preview_metadata = metadata.clone();
    let playback_metadata = metadata.clone();
    let attachment_id = metadata.attachment_id.clone();
    let is_playing = state.is_audio_playing(&attachment_id);
    let is_loading = state.is_audio_loading(&attachment_id);
    let download_progress = state.attachment_download_percent(&attachment_id);
    let download_speed = state.attachment_download_speed(&attachment_id);
    let upload_progress = state.attachment_upload_progress(&attachment_id);
    let is_downloading = state.is_attachment_downloading(&attachment_id);
    let details = format_attachment_details(metadata);
    let details = if let Some(percent) = download_progress {
        let speed = download_speed
            .map(format_attachment_speed)
            .unwrap_or_else(|| "Starting".into());
        format!("Downloading {percent}% · {speed} · {details}")
    } else if is_loading {
        format!("Loading · {details}")
    } else {
        match state.attachment_transfer_status(&attachment_id) {
            Ok(crate::app_state::AttachmentTransferState::Pending) => {
                format!("Pending · {details}")
            }
            Ok(crate::app_state::AttachmentTransferState::Active) => match upload_progress {
                Ok(Some((percent, speed))) => format!(
                    "Uploading {percent}% · {} · {details}",
                    format_attachment_speed(speed)
                ),
                Ok(None) => format!("Connecting · {details}"),
                Err(error) => {
                    format!("Upload progress unavailable: {error} · {details}")
                }
            },
            Ok(crate::app_state::AttachmentTransferState::AwaitingConfirmation) => {
                match upload_progress {
                    Ok(Some((percent, speed))) => format!(
                        "Sent {percent}% · awaiting peer confirmation · {} · {details}",
                        format_attachment_speed(speed)
                    ),
                    Ok(None) => format!("Awaiting peer confirmation · {details}"),
                    Err(error) => {
                        format!("Confirmation status unavailable: {error} · {details}")
                    }
                }
            }
            Ok(crate::app_state::AttachmentTransferState::Completed) => {
                if state.attachment_is_outgoing(&attachment_id) {
                    format!("Completed by all recipients · {details}")
                } else {
                    format!("Completed on this device · {details}")
                }
            }
            Ok(crate::app_state::AttachmentTransferState::Failed) => {
                format!("Failed · retry to resume · {details}")
            }
            Ok(crate::app_state::AttachmentTransferState::Cancelled) => {
                format!("Cancelled · retry to resume · {details}")
            }
            Err(error) => format!("Transfer status unavailable: {error} · {details}"),
        }
    };
    let title = match metadata.kind {
        AttachmentKind::Voice => "Voice note".to_string(),
        AttachmentKind::File => metadata.file_name.clone(),
    };
    div()
        .w_full()
        .max_w(px(380.0))
        .flex()
        .items_center()
        .gap_3()
        .p_3()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::PANEL))
        .child(
            div()
                .size(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(spacing::RADIUS_SM))
                .bg(rgb(colors::RAIL))
                .child(crate::components::icon::render(
                    match metadata.kind {
                        AttachmentKind::Voice => IconName::Microphone,
                        AttachmentKind::File => IconName::File,
                    },
                    18.0,
                    colors::ACCENT,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::TEXT))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child(details),
                ),
        )
        .child({
            let mut actions = div().flex().items_center().gap_1();
            if attachment::is_image_previewable(metadata) {
                actions = actions.child(
                    button::clear_icon(IconName::Eye)
                        .id(format!("preview-image-{message_id}"))
                        .aria_label("Preview image")
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.preview_image_attachment(preview_metadata.clone(), cx)
                        })),
                );
            }
            if attachment::is_audio_playable(metadata) {
                actions = actions.child(
                    button::clear_icon(if is_playing {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .id(format!("play-audio-{message_id}"))
                    .aria_label(if is_loading {
                        "Loading audio"
                    } else if is_playing {
                        "Pause audio"
                    } else {
                        "Play audio"
                    })
                    .tab_index(0)
                    .on_click(cx.listener(move |state, _, _, cx| {
                        state.toggle_audio_playback(playback_metadata.clone(), cx)
                    })),
                );
            }
            if is_downloading {
                actions.child(
                    button::clear_icon(IconName::Close)
                        .id(format!("cancel-attachment-{message_id}"))
                        .aria_label("Cancel attachment transfer")
                        .tab_index(0)
                        .on_click(
                            cx.listener(move |state, _, _, cx| {
                                state.cancel_attachment_download(cx)
                            }),
                        ),
                )
            } else {
                actions.child(
                    button::clear_icon(IconName::Download)
                        .id(format!("download-attachment-{message_id}"))
                        .aria_label("Download attachment")
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.download_attachment(download_metadata.clone(), cx)
                        })),
                )
            }
        })
}

fn render_pending_attachment(
    pending: &crate::app_state::PendingAttachment,
    cx: &mut Context<AppState>,
) -> Div {
    let id = pending.id.clone();
    let title = match pending.kind {
        AttachmentKind::Voice => "Voice note".to_string(),
        AttachmentKind::File => pending.file_name.clone(),
    };
    let details = if pending.metadata.is_some() {
        format!("Pending · {}", format_attachment_size(pending.size))
    } else {
        "Preparing encrypted file…".into()
    };
    div()
        .max_w(px(230.0))
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .p_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::PANEL_HOVER))
        .child(
            div()
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(spacing::RADIUS_SM))
                .bg(rgb(colors::PANEL_HOVER))
                .child(crate::components::icon::render(
                    match pending.kind {
                        AttachmentKind::Voice => IconName::Microphone,
                        AttachmentKind::File => IconName::File,
                    },
                    16.0,
                    colors::ACCENT,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child(details),
                ),
        )
        .when(pending.metadata.is_some(), |this| {
            this.child(
                div()
                    .id(format!("remove-pending-attachment-{id}"))
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(spacing::RADIUS_SM))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(colors::PANEL_ACTIVE)))
                    .aria_label("Remove attachment")
                    .child(crate::components::icon::render(
                        IconName::Close,
                        12.0,
                        colors::MUTED,
                    ))
                    .on_click(
                        cx.listener(move |state, _, _, cx| {
                            state.remove_pending_attachment(&id, cx)
                        }),
                    ),
            )
        })
}

fn format_attachment_details(metadata: &AttachmentMetadata) -> String {
    let file_type = if metadata.kind == AttachmentKind::Voice {
        "WAV audio".to_string()
    } else if metadata.content_type == "application/octet-stream" {
        metadata
            .file_name
            .rsplit('.')
            .next()
            .filter(|extension| *extension != metadata.file_name)
            .map(|extension| extension.to_ascii_uppercase())
            .unwrap_or_else(|| "File".into())
    } else {
        metadata
            .content_type
            .rsplit('/')
            .next()
            .unwrap_or("File")
            .to_ascii_uppercase()
    };
    format!("{file_type} · {}", format_attachment_size(metadata.size))
}

fn format_attachment_size(size: usize) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    }
}

fn format_attachment_speed(bytes_per_second: u64) -> String {
    if bytes_per_second < 1024 {
        format!("{bytes_per_second} B/s")
    } else if bytes_per_second < 1024 * 1024 {
        format!("{:.1} KiB/s", bytes_per_second as f64 / 1024.0)
    } else {
        format!("{:.1} MiB/s", bytes_per_second as f64 / (1024.0 * 1024.0))
    }
}
