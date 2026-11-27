use gpui::prelude::*;
use gpui::{
    Context, IntoElement, KeyDownEvent, ObjectFit, Window, div, hsla, img, px, relative, rgb,
};

use crate::{
    app_state::{AppState, PendingConfirmation},
    components::{button, confirmation_dialog, dialog, icon::IconName, toast},
    navigation::route::Route,
    theme::{colors, typography},
};

pub fn render(
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let mut shell = div()
        .size_full()
        .relative()
        .flex()
        .bg(rgb(colors::CANVAS))
        .text_color(rgb(colors::TEXT))
        .font_family(typography::BODY_FONT)
        .child(super::sidebar::render(state, cx))
        .child(super::main_content::render(state, window, cx))
        .when(state.navigation.settings_open, |shell| {
            shell.child(
                dialog::overlay()
                    .id("settings-modal-overlay")
                    .backdrop_blur(px(8.0))
                    .on_any_mouse_down(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(
                        dialog::surface()
                            .w(relative(0.8))
                            .h(relative(0.8))
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .p_0()
                            .child(crate::views::settings::view::render(state, window, cx)),
                    ),
            )
        })
        .when(
            state.image_preview.is_some() || state.image_preview_loading.is_some(),
            |shell| {
                let preview = state.image_preview.as_ref();
                let title = preview
                    .map(|preview| preview.file_name.clone())
                    .unwrap_or_else(|| "Loading image preview…".into());
                let image = preview.map(|preview| {
                    img(preview.image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(|| {
                            crate::components::empty_state::render(
                                "Image unavailable",
                                "This attachment could not be displayed.",
                            )
                            .into_any_element()
                        })
                        .into_any_element()
                });
                shell.child(
                    div()
                        .id("image-preview-overlay")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(hsla(0.0, 0.0, 0.0, 0.72))
                        .backdrop_blur(px(8.0))
                        .on_any_mouse_down(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                        .on_click(cx.listener(|state, _, _, cx| state.close_image_preview(cx)))
                        .child(
                            div()
                                .id("image-preview-modal")
                                .w(relative(0.9))
                                .h(relative(0.9))
                                .min_w_0()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .rounded(px(crate::theme::spacing::RADIUS_XL))
                                .border_1()
                                .border_color(rgb(crate::theme::colors::BORDER))
                                .bg(rgb(crate::theme::colors::CANVAS))
                                .shadow_lg()
                                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .px_4()
                                        .py_3()
                                        .border_b_1()
                                        .border_color(rgb(crate::theme::colors::BORDER))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_sm()
                                                .text_color(rgb(crate::theme::colors::TEXT))
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(title),
                                        )
                                        .child(
                                            button::icon_button(IconName::Close)
                                                .id("close-image-preview")
                                                .aria_label("Close image preview")
                                                .tab_index(0)
                                                .on_click(cx.listener(|state, _, _, cx| {
                                                    state.close_image_preview(cx)
                                                })),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .min_h_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .p_3()
                                        .when_some(image, |this, image| this.child(image))
                                        .when(preview.is_none(), |this| {
                                            this.child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(crate::theme::colors::MUTED))
                                                    .child("Loading image…"),
                                            )
                                        }),
                                ),
                        ),
                )
            },
        )
        .on_key_down(cx.listener(|state, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key.eq_ignore_ascii_case("escape") {
                if state.image_preview.is_some() || state.image_preview_loading.is_some() {
                    state.close_image_preview(cx);
                    cx.stop_propagation();
                } else if state.pending_confirmation.is_some() {
                    state.cancel_confirmation(cx);
                    cx.stop_propagation();
                } else if state.navigation.settings_open {
                    state.close_settings(cx);
                    cx.stop_propagation();
                } else if state.active_menu.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            }
        }));

    if let Some(confirmation) = state.pending_confirmation.as_ref() {
        let (title, description) = match confirmation {
            PendingConfirmation::RemoveFriend(_) => (
                "Remove friend",
                "This removes the contact from your friend list. Existing conversation history stays on this device.",
            ),
            PendingConfirmation::DeleteConversation(_) => (
                "Delete conversation",
                "This permanently removes the conversation and its local messages from this device.",
            ),
            PendingConfirmation::RevokeDevice(_) => (
                "Revoke device",
                "This device will lose access to future account synchronization.",
            ),
        };
        shell = shell.child(
            dialog::overlay()
                .id("confirmation-dialog-overlay")
                .on_click(cx.listener(|state, _, _, cx| state.cancel_confirmation(cx)))
                .child(
                    confirmation_dialog::surface(title, description)
                        .id("confirmation-dialog")
                        .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button::secondary("Cancel")
                                        .id("cancel-confirmation")
                                        .tab_index(0)
                                        .on_click(cx.listener(|state, _, _, cx| {
                                            state.cancel_confirmation(cx)
                                        })),
                                )
                                .child(
                                    button::destructive("Confirm")
                                        .id("confirm-pending-action")
                                        .tab_index(0)
                                        .on_click(cx.listener(|state, _, _, cx| {
                                            state.confirm_pending_action(cx)
                                        })),
                                ),
                        ),
                ),
        );
    }

    if let Some(message) = state.message.as_deref() {
        shell = shell.child(
            div()
                .absolute()
                .top(px(16.0))
                .right(px(16.0))
                .w(px(380.0))
                .flex_shrink_0()
                .child(toast::render(message, cx)),
        );
    }
    let call_is_in_selected_conversation = state.navigation.route == Route::Messages
        && (state.incoming_voice_call.as_ref().is_some_and(|call| {
            state.selected_conversation.as_deref() == Some(call.conversation_id.as_str())
        }) || state.active_voice_call.as_ref().is_some_and(|call| {
            state.selected_conversation.as_deref() == Some(call.conversation_id.as_str())
        }));
    if let Some(incoming) = state
        .incoming_voice_call
        .as_ref()
        .filter(|_| !call_is_in_selected_conversation)
    {
        let caller_name = incoming.caller_name.clone();
        shell = shell.child(
            div()
                .id("incoming-voice-call")
                .absolute()
                .top(px(64.0))
                .right(px(16.0))
                .w(px(360.0))
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .rounded(px(crate::theme::spacing::RADIUS_LG))
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::PANEL))
                .shadow_lg()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::TEXT))
                        .child(format!("{caller_name} is calling")),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button::primary_icon(IconName::Phone)
                                .id("accept-voice-call")
                                .aria_label("Accept call")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.accept_voice_call(cx)),
                                ),
                        )
                        .child(
                            button::destructive_icon_button(IconName::PhoneOff)
                                .id("decline-voice-call")
                                .aria_label("Decline call")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.decline_voice_call(cx)),
                                ),
                        ),
                ),
        );
    }
    if let Some(call) = state
        .active_voice_call
        .as_ref()
        .filter(|_| !call_is_in_selected_conversation)
    {
        let participant_count = call.participants.len();
        let muted = call.muted;
        let call_status = if call.connected {
            format!("Voice call · {participant_count} participants")
        } else if state
            .current_user
            .as_ref()
            .is_some_and(|user| user.id == call.initiator_id)
        {
            "Calling…".to_string()
        } else {
            "Joining call…".to_string()
        };
        let status_conversation_id = call.conversation_id.clone();
        let status_element = div()
            .id("active-voice-call-return")
            .text_sm()
            .text_color(rgb(colors::TEXT))
            .cursor_pointer()
            .aria_label("Return to active call")
            .tab_index(0)
            .child(call_status)
            .on_click(cx.listener(move |state, _, _, cx| {
                state.open_conversation_by_id(&status_conversation_id, cx)
            }));
        shell = shell.child(
            div()
                .id("active-voice-call")
                .absolute()
                .top(px(64.0))
                .right(px(16.0))
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(crate::theme::spacing::RADIUS_LG))
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::PANEL))
                .shadow_lg()
                .child(status_element)
                .child(
                    button::icon_button(if muted {
                        IconName::MicrophoneOff
                    } else {
                        IconName::Microphone
                    })
                    .id("toggle-voice-call-mute")
                    .aria_label(if muted {
                        "Unmute microphone"
                    } else {
                        "Mute microphone"
                    })
                    .tab_index(0)
                    .on_click(cx.listener(|state, _, _, cx| state.toggle_voice_call_mute(cx))),
                )
                .child(
                    button::destructive_icon_button(IconName::PhoneOff)
                        .id("end-voice-call")
                        .aria_label("End call")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| state.end_voice_call(true, cx))),
                ),
        );
    }
    shell
}
