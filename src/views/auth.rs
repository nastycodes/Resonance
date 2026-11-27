use gpui::prelude::*;
use gpui::{Context, IntoElement, MouseButton, Window, div, px, rgb};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    app_state::{AppState, AuthMode, InputField},
    components::{button, card, error_state, form_field, input_text},
    theme::{colors, spacing, typography},
};

pub fn render(
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let registering = state.auth_mode == AuthMode::Register;
    let mut form = card::elevated()
        .w(px(420.0))
        .flex()
        .flex_col()
        .gap_5()
        .p_8()
        .font_family(typography::BODY_FONT)
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(crate::components::brand_logo::render(48.0))
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::BOLD)
                        .font_family(typography::HEADLINE_FONT)
                        .text_color(rgb(colors::TEXT))
                        .child(
                            div()
                                .text_2xl()
                                .line_height(px(typography::PAGE_TITLE_LINE_HEIGHT))
                                .font_weight(gpui::FontWeight::BOLD)
                                .font_family(typography::HEADLINE_FONT)
                                .text_color(rgb(colors::TEXT))
                                .child(if registering {
                                    "Create an account"
                                } else {
                                    "Sign in"
                                })
                        ),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(colors::MUTED))
                .child(if registering {
                    "Create an account with an invite code from your group admin. Your private identity key is encrypted before it is stored by the API."
                } else {
                    "Sign in with your account password to unlock your encrypted identity from MySQL."
                }),
        );

    form = form.child(input_field(
        state,
        window,
        "Username",
        "Enter your username",
        &state.username_input,
        InputField::Username,
        cx,
    ));
    if registering {
        form = form.child(input_field(
            state,
            window,
            "Display name",
            "Enter your display name",
            &state.display_name_input,
            InputField::DisplayName,
            cx,
        ));
        form = form.child(input_field(
            state,
            window,
            "Invite code",
            "Enter your one-time invite code",
            &state.invite_code_input,
            InputField::InviteCode,
            cx,
        ));
    }
    form = form.child(input_field(
        state,
        window,
        "Password",
        "Enter your password",
        &state.password_input,
        InputField::Password,
        cx,
    ));
    if !registering {
        form = form.child(remember_me(state, cx));
    }

    if let Some(error) = state.startup_error.as_deref() {
        form = form.child(message_box(error, true));
    }
    let button_text = if registering {
        "Create account"
    } else {
        "Sign in"
    };
    form = form
        .child(
            button::primary(button_text)
                .id("auth-submit")
                .w_full()
                .mt_1()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .tab_index(0)
                .on_click(cx.listener(|state, _, _, cx| state.submit_auth(cx))),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(rgb(colors::MUTED))
                .child(if registering {
                    "Already have an account?"
                } else {
                    "New to Resonance?"
                })
                .child(
                    div()
                        .id("auth-switch")
                        .cursor_pointer()
                        .text_color(rgb(colors::ACCENT))
                        .focus_visible(|style| style.border_color(rgb(colors::ACCENT)))
                        .tab_index(0)
                        .child(if registering {
                            "Sign in"
                        } else {
                            "Create an account"
                        })
                        .on_click(cx.listener(move |state, _, _, cx| {
                            state.set_auth_mode(
                                if registering {
                                    AuthMode::Login
                                } else {
                                    AuthMode::Register
                                },
                                cx,
                            )
                        })),
                ),
        );
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(colors::CANVAS))
        .track_focus(&state.input_focus)
        .on_key_down(cx.listener(|state, event, _, cx| state.handle_input_key(event, cx)))
        .child(form)
}

fn input_field(
    state: &AppState,
    window: &Window,
    label: &'static str,
    placeholder: &'static str,
    value: &str,
    field: InputField,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let focus = state.input_focus.clone();
    let active = state.active_input == field && focus.is_focused(window);
    let displayed_value = if field == InputField::Password {
        "•".repeat(value.graphemes(true).count())
    } else {
        value.to_string()
    };
    form_field::wrapper().child(form_field::label(label)).child(
        form_field::input_frame(active)
            .track_focus(&focus)
            .id(format!("input-{label}"))
            .child(input_text::render(
                &displayed_value,
                placeholder,
                state.selection_range(),
                active,
                false,
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |state, _, window, cx| {
                    state.activate_input(field);
                    window.focus(&state.input_focus.clone(), cx);
                    cx.notify();
                }),
            ),
    )
}

fn remember_me(state: &AppState, cx: &mut Context<AppState>) -> impl IntoElement {
    let checked = state.remember_me;
    let focused = state.active_input == InputField::RememberMe;
    div()
        .id("auth-remember-me")
        .flex()
        .items_center()
        .gap_3()
        .min_h(px(spacing::ACTION_TARGET))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(colors::TEXT))
        .focus_visible(|style| style.border_color(rgb(colors::ACCENT)).shadow_sm())
        .child(crate::components::icon::checkbox(checked))
        .child(
            div()
                .flex()
                .flex_col()
                .child("Remember me on this device")
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::MUTED))
                        .child("Saved securely in the system keychain"),
                ),
        )
        .when(focused, |this| this.border_color(rgb(colors::ACCENT)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|state, _, window, cx| {
                state.activate_input(InputField::RememberMe);
                let focus = state.input_focus.clone();
                window.focus(&focus, cx);
                state.toggle_remember_me(cx);
            }),
        )
}

fn message_box(message: &str, error: bool) -> impl IntoElement {
    if error {
        return error_state::render("Unable to continue", message).into_any_element();
    }
    div()
        .p_3()
        .rounded_md()
        .bg(rgb(colors::PANEL_ACTIVE))
        .text_sm()
        .text_color(rgb(colors::GREEN))
        .child(message.to_string())
        .into_any_element()
}
