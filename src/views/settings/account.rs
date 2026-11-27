use gpui::prelude::*;
use gpui::{ClipboardItem, Context, IntoElement, MouseButton, Window, div, px, rgb};

use crate::{
    app_state::{AppState, InputField},
    components::{avatar, button, card, input_text, select, switch},
    theme::{colors, spacing},
};

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let user = state
        .current_user
        .as_ref()
        .expect("account settings require an identity");
    let focus = state.input_focus.clone();

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_4()
        .track_focus(&state.input_focus)
        .on_key_down(cx.listener(|state, event, _, cx| state.handle_input_key(event, cx)))
        .child(
            card::surface()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .p_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(avatar::render_user(user, 48.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(colors::TEXT))
                                        .child(user.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(colors::MUTED))
                                        .child(format!("@{}", user.username)),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button::secondary("Change photo")
                                .id("choose-profile-image")
                                .tab_index(0)
                                .on_click(cx.listener(|state, _, _, cx| {
                                    state.choose_profile_image(cx)
                                })),
                        )
                        .when(user.profile_image.is_some(), |this| {
                            this.child(
                                button::secondary("Remove photo")
                                    .id("remove-profile-image")
                                    .tab_index(0)
                                    .on_click(cx.listener(|state, _, _, cx| {
                                        state.remove_profile_image(cx)
                                    })),
                            )
                        }),
                ),
        )
        .child(
            card::surface()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::TEXT))
                        .child("Change password"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child("The new password also re-encrypts your identity for future sign-ins."),
                )
                .child(password_field(
                    state,
                    window,
                    cx,
                    InputField::CurrentPassword,
                    "Current password",
                    "current-account-password",
                    &state.current_password_input,
                ))
                .child(password_field(
                    state,
                    window,
                    cx,
                    InputField::NewPassword,
                    "New password",
                    "new-account-password",
                    &state.new_password_input,
                ))
                .child(password_field(
                    state,
                    window,
                    cx,
                    InputField::ConfirmNewPassword,
                    "Confirm new password",
                    "confirm-account-password",
                    &state.confirm_password_input,
                ))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .child(
                            button::primary("Update password")
                                .id("change-account-password")
                                .tab_index(0)
                                .on_click(cx.listener(|state, _, _, cx| {
                                    state.change_account_password(cx)
                                })),
                        ),
                ),
        )
        .child(
            card::surface()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .p_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(colors::TEXT))
                                .child("Public identity"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors::SUBTLE))
                                .child(user.id.clone()),
                        ),
                )
                .child(
                    button::secondary("Copy")
                        .id("copy-peer-identity")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| {
                            if let Some(user) = state.current_user.as_ref() {
                                cx.write_to_clipboard(ClipboardItem::new_string(user.id.clone()));
                                state.set_toast(Some("Public identity copied.".into()), cx);
                            }
                            cx.notify();
                        })),
                ),
        )
        .child(
            card::surface()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::TEXT))
                        .child("Profile"),
                )
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors::MUTED))
                                        .child("Display name"),
                                )
                                .child(
                                    div()
                                        .track_focus(&focus)
                                        .id("profile-display-name")
                                        .min_h(px(spacing::CONTROL_HEIGHT))
                                        .px_3()
                                        .py_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(
                                            if state.active_input == InputField::ProfileName
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
                                            &state.profile_name_input,
                                            "",
                                            state.selection_range(),
                                            state.active_input == InputField::ProfileName
                                                && focus.is_focused(window),
                                            false,
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|state, _, window, cx| {
                                                state.activate_input(InputField::ProfileName);
                                                let focus = state.input_focus.clone();
                                                window.focus(&focus, cx);
                                                cx.notify();
                                            }),
                                        ),
                                ),
                        )
                        .child(
                            button::primary("Save")
                                .id("save-profile")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.save_profile(cx)),
                                ),
                        ),
                ),
        )
        .child(
            card::surface()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::TEXT))
                        .child("Preferences"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_sm().text_color(rgb(colors::TEXT)).child("Presence"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors::SUBTLE))
                                        .child("Choose the status shown to your friends."),
                                ),
                        )
                        .child(
                            div().flex().gap_2().children(
                                ["Online", "Away", "Do not disturb"]
                                    .into_iter()
                                    .map(|presence| {
                                        let active = state.presence == presence;
                                        select::render(presence, active)
                                            .id(format!("presence-{presence}"))
                                            .on_click(cx.listener(move |state, _, _, cx| {
                                                state.set_presence(presence, cx)
                                            }))
                                    }),
                            ),
                        ),
                )
                .child(
                    div()
                        .h(px(1.0))
                        .bg(rgb(colors::BORDER)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(colors::TEXT))
                                        .child("Notification preference"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors::SUBTLE))
                                        .child("Saved to your account; desktop notifications are not implemented."),
                                ),
                        )
                        .child(
                            div()
                                .id("toggle-notifications")
                                .tab_index(0)
                                .cursor_pointer()
                                .focus_visible(|style| style.border_color(rgb(colors::FOCUS)))
                                .child(switch::render(state.notifications_enabled))
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.toggle_notifications(cx)),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .pt_2()
                .border_t_1()
                .border_color(rgb(colors::BORDER))
                .child(
                    button::destructive("Log out")
                        .id("logout")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| state.logout(cx))),
                ),
        )
}

fn password_field(
    state: &AppState,
    window: &Window,
    cx: &mut Context<AppState>,
    field: InputField,
    label: &'static str,
    id: &'static str,
    value: &str,
) -> impl IntoElement {
    let focus = state.input_focus.clone();
    let focused = state.active_input == field && focus.is_focused(window);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_xs().text_color(rgb(colors::MUTED)).child(label))
        .child(
            div()
                .track_focus(&focus)
                .id(id)
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
                .text_sm()
                .text_color(rgb(colors::TEXT))
                .child(input_text::render_password(
                    value,
                    "",
                    state.selection_range(),
                    focused,
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |state, _, window, cx| {
                        state.activate_input(field);
                        let focus = state.input_focus.clone();
                        window.focus(&focus, cx);
                        cx.notify();
                    }),
                ),
        )
}
