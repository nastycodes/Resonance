use gpui::prelude::*;
use gpui::{Context, IntoElement, MouseButton, Window, div, px, rgb};

use crate::{
    app_state::{AppState, InputField, PendingConfirmation},
    components::{badge, button, card, input_text},
    services::device_enrollment::pairing_code,
    theme::{colors, spacing},
};

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let focus = state.input_focus.clone();
    let account_id = state.current_user.as_ref().map(|user| user.id.clone());
    let current_device_id = state.current_device_id.clone();
    let active_count = state
        .registered_devices
        .iter()
        .filter(|device| device.revoked_at.is_none())
        .count();

    let mut device_cards = div().flex().flex_col().gap_3();
    for device in &state.registered_devices {
        let is_current = current_device_id.as_deref() == Some(device.device_id.as_str());
        let device_id = device.device_id.clone();
        let can_sync =
            device.encryption_public_key.is_some() && device.revoked_at.is_none() && !is_current;
        let status_text = if let Some(revoked_at) = &device.revoked_at {
            format!("Revoked {revoked_at}")
        } else if device.signing_public_key.is_none() || device.encryption_public_key.is_none() {
            "Legacy device".to_string()
        } else {
            "Active".to_string()
        };
        let initial = device
            .device_name
            .chars()
            .next()
            .unwrap_or('D')
            .to_uppercase()
            .to_string();

        let mut badges = div().flex().items_center().gap_2();
        if is_current {
            badges = badges.child(badge::render("This device", badge::Tone::Accent));
        }
        badges = badges.child(badge::render(
            status_text,
            if device.revoked_at.is_some() {
                badge::Tone::Danger
            } else if device.signing_public_key.is_none() || device.encryption_public_key.is_none()
            {
                badge::Tone::Neutral
            } else {
                badge::Tone::Success
            },
        ));

        let mut actions = div().flex().items_center().gap_2();
        if device.revoked_at.is_none() {
            actions = actions.child(
                button::secondary("Rename")
                    .id(format!("rename-device-{device_id}"))
                    .tab_index(0)
                    .on_click(cx.listener(move |state, _, window, cx| {
                        if state.open_device_rename(&device_id, cx) {
                            let focus = state.input_focus.clone();
                            window.focus(&focus, cx);
                        }
                    })),
            );
        }
        if can_sync {
            let target = device.device_id.clone();
            actions = actions.child(
                button::secondary("Send history")
                    .id(format!("sync-history-{target}"))
                    .tab_index(0)
                    .on_click(
                        cx.listener(move |state, _, _, cx| state.send_device_history(&target, cx)),
                    ),
            );
        }
        if !is_current && device.revoked_at.is_none() {
            let target = device.device_id.clone();
            actions = actions.child(
                button::destructive("Revoke")
                    .id(format!("revoke-device-{target}"))
                    .tab_index(0)
                    .on_click(cx.listener(move |state, _, _, cx| {
                        state.request_confirmation(
                            PendingConfirmation::RevokeDevice(target.clone()),
                            cx,
                        )
                    })),
            );
        }

        device_cards = device_cards.child(
            card::surface()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .size(px(40.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_lg()
                                        .bg(rgb(if is_current {
                                            colors::PANEL_ACTIVE
                                        } else {
                                            colors::CANVAS
                                        }))
                                        .text_base()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(if is_current {
                                            colors::ACCENT
                                        } else {
                                            colors::MUTED
                                        }))
                                        .child(initial),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(rgb(colors::TEXT))
                                                .child(device.device_name.clone()),
                                        )
                                        .child(badges),
                                ),
                        )
                        .child(actions),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_3()
                        .border_t_1()
                        .border_color(rgb(colors::BORDER))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::SUBTLE))
                                .child(format!("Device ID  {}", device.device_id)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!(
                                    "Registered {}  ·  Last active {}",
                                    device.created_at, device.last_seen_at
                                )),
                        ),
                ),
        );
    }

    let mut enrollments = div().flex().flex_col().gap_2();
    for enrollment in &state.pending_device_enrollments {
        let request_id = enrollment.request_id.clone();
        let code = account_id
            .as_deref()
            .map(|account_id| {
                pairing_code(
                    account_id,
                    &enrollment.request_id,
                    &enrollment.device_id,
                    &enrollment.signing_public_key,
                    &enrollment.encryption_public_key,
                    &enrollment.nonce,
                )
            })
            .unwrap_or_default();
        enrollments = enrollments.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .rounded_md()
                .bg(rgb(colors::CANVAS))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(colors::TEXT))
                                .child(enrollment.device_name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::MUTED))
                                .child(format!(
                                    "Pairing code: {code}  ·  Expires {}",
                                    enrollment.expires_at
                                )),
                        ),
                )
                .child(
                    button::primary("Authorize")
                        .id(format!("approve-device-{request_id}"))
                        .tab_index(0)
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.approve_device_enrollment(&request_id, cx)
                        })),
                ),
        );
    }

    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_4()
        .track_focus(&state.input_focus)
        .on_key_down(cx.listener(|state, event, _, cx| {
            if state.active_input != InputField::DeviceName {
                state.handle_input_key(event, cx);
            }
        }))
        .child(
            card::surface()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .size(px(44.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_lg()
                                .bg(rgb(colors::PANEL_ACTIVE))
                                .text_base()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors::ACCENT))
                                .child(state.registered_devices.len().to_string()),
                        )
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
                                        .child(if state.registered_devices.len() == 1 {
                                            "Registered installation"
                                        } else {
                                            "Registered installations"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors::MUTED))
                                        .child(format!(
                                            "{active_count} active  ·  {} awaiting authorization",
                                            state.pending_device_enrollments.len()
                                        )),
                                ),
                        ),
                )
                .child(
                    button::secondary("Refresh")
                        .id("refresh-devices")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| {
                            state.refresh_device_management(cx)
                        })),
                ),
        )
        .when_some(state.device_enrollment_status.as_ref(), |this, status| {
            this.child(
                card::elevated()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors::TEXT))
                            .child(format!(
                                "This installation is {}.",
                                status.state.replace('_', " ")
                            )),
                    )
                    .when_some(state.device_pairing_code.as_deref(), |this, code| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::ACCENT))
                                .child(format!("Pairing code: {code}")),
                        )
                    })
                    .when(status.state == "expired", |this| {
                        this.child(
                            button::secondary("Create a new authorization request")
                                .id("restart-device-enrollment")
                                .tab_index(0)
                                .on_click(cx.listener(|state, _, _, cx| {
                                    state.restart_device_enrollment(cx)
                                })),
                        )
                    }),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::TEXT))
                        .child("Device list"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child(format!("{}", state.registered_devices.len())),
                ),
        )
        .when(state.registered_devices.is_empty(), |this| {
            this.child(
                card::surface()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors::TEXT))
                            .child("No devices registered yet"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::MUTED))
                            .child("Authorized installations will appear here."),
                    ),
            )
        })
        .child(device_cards)
        .when(!state.pending_device_enrollments.is_empty(), |this| {
            this.child(
                card::surface()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
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
                                    .child("Authorization requests"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors::MUTED))
                                    .child("Confirm the pairing code on the new installation, then enter it here."),
                            ),
                    )
                    .child(
                        div()
                            .track_focus(&focus)
                            .id("device-pairing-code-input")
                            .min_h(px(spacing::CONTROL_HEIGHT))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(
                                if state.active_input == InputField::DevicePairingCode
                                    && focus.is_focused(window)
                                {
                                    colors::ACCENT
                                } else {
                                    colors::BORDER
                                },
                            ))
                            .bg(rgb(colors::CANVAS))
                            .focus(|style| style.border_color(rgb(colors::ACCENT)))
                            .text_sm()
                            .text_color(rgb(colors::TEXT))
                            .child(input_text::render(
                                &state.device_pairing_verification_input,
                                "Enter pairing code",
                                state.selection_range(),
                                state.active_input == InputField::DevicePairingCode
                                    && focus.is_focused(window),
                                false,
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|state, _, window, cx| {
                                    state.activate_input(InputField::DevicePairingCode);
                                    let focus = state.input_focus.clone();
                                    window.focus(&focus, cx);
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(enrollments),
            )
        })
}
