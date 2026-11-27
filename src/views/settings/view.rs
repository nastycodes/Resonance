use gpui::prelude::*;
use gpui::{Context, IntoElement, KeyDownEvent, MouseButton, Window, div, hsla, px, rgb};

use crate::{
    app_state::{AppState, InputField},
    components::{button, icon::IconName, input_text, navigation_item},
    navigation::route::SettingsPage,
    theme::{colors, spacing, typography},
};

pub fn render(state: &AppState, window: &Window, cx: &mut Context<AppState>) -> impl IntoElement {
    let current = state.navigation.settings_page;
    let mut nav = div().flex().flex_col().gap_1().w(px(190.0)).flex_shrink_0();
    for page in SettingsPage::ALL {
        let active = page == current;
        nav = nav.child(
            navigation_item::render_item(
                format!("settings-{}", page.label()),
                match page {
                    SettingsPage::Account => IconName::Identity,
                    SettingsPage::Devices => IconName::Devices,
                    SettingsPage::General => IconName::Settings,
                    SettingsPage::Appearance => IconName::Eye,
                    SettingsPage::About => IconName::Info,
                },
                page.label(),
                active,
            )
            .on_click(cx.listener(move |state, _, _, cx| {
                state.select_settings_page(page, cx);
            })),
        );
    }

    let page = match current {
        SettingsPage::Account => super::account::render(state, window, cx).into_any_element(),
        SettingsPage::Devices => super::devices::render(state, window, cx).into_any_element(),
        SettingsPage::General => super::general::render(state, cx).into_any_element(),
        SettingsPage::Appearance => super::appearance::render().into_any_element(),
        SettingsPage::About => super::about::render().into_any_element(),
    };
    let (page_title, page_description) = match current {
        SettingsPage::Account => ("Account", "Manage your profile and account preferences."),
        SettingsPage::Devices => (
            "Devices",
            "Manage the installations connected to your account.",
        ),
        SettingsPage::General => ("General", "Configure account-level preferences."),
        SettingsPage::Appearance => ("Appearance", "Review the application display theme."),
        SettingsPage::About => ("About", "Product information and version details."),
    };
    let page = div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(colors::TEXT))
                        .child(page_title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::MUTED))
                        .child(page_description),
                ),
        )
        .child(
            div()
                .id("settings-page-scroll")
                .min_h_0()
                .flex_1()
                .overflow_y_scroll()
                .child(page),
        );
    div()
        .h_full()
        .w_full()
        .min_h_0()
        .relative()
        .flex()
        .flex_col()
        .gap_4()
        .p(px(spacing::PANEL_GUTTER))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_4()
                .child(
                    div().flex().h_full().flex_col().child(
                        div()
                            .text_size(px(typography::PAGE_TITLE))
                            .line_height(px(typography::PAGE_TITLE_LINE_HEIGHT))
                            .font_weight(gpui::FontWeight::BOLD)
                            .font_family(typography::HEADLINE_FONT)
                            .text_color(rgb(colors::TEXT))
                            .child("Settings"),
                    ), /*
                       .child(
                           div()
                               .text_sm()
                               .text_color(rgb(colors::MUTED))
                               .child("Make Resonance feel like yours."),
                       ) */
                )
                .child(
                    button::clear_icon(IconName::Close)
                        .id("close-settings")
                        .aria_label("Close settings")
                        .tab_index(0)
                        .on_click(cx.listener(|state, _, _, cx| state.close_settings(cx))),
                ),
        )
        .child(
            div()
                .min_h_0()
                .flex_1()
                .flex()
                .gap_8()
                .child(nav)
                .child(page),
        )
        .when(state.device_rename_target.is_some(), |this| {
            this.child(device_rename_dialog(state, window, cx))
        })
}

fn device_rename_dialog(
    state: &AppState,
    window: &Window,
    cx: &mut Context<AppState>,
) -> impl IntoElement {
    let target = state
        .device_rename_target
        .as_deref()
        .expect("rename dialog requires a selected device");
    let device_name = state
        .registered_devices
        .iter()
        .find(|device| device.device_id == target)
        .map(|device| device.device_name.as_str())
        .unwrap_or("this installation");
    let focus = state.input_focus.clone();
    let focused = state.active_input == InputField::DeviceName && focus.is_focused(window);

    div()
        .id("rename-device-dialog-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0.0, 0.0, 0.0, 0.68))
        .on_any_mouse_down(cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .on_key_down(cx.listener(|state, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key.eq_ignore_ascii_case("escape") {
                state.cancel_device_rename(cx);
            } else {
                state.handle_input_key(event, cx);
            }
            cx.stop_propagation();
        }))
        .on_click(cx.listener(|state, _, _, cx| {
            state.cancel_device_rename(cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .id("rename-device-dialog")
                .w(px(440.0))
                .max_w_full()
                .flex()
                .flex_col()
                .gap_4()
                .p_5()
                .rounded(px(spacing::RADIUS_XL))
                .border_1()
                .border_color(rgb(colors::BORDER))
                .bg(rgb(colors::PANEL))
                .shadow_lg()
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
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
                                .child("Rename installation"),
                        )
                        .child(
                            button::icon_button(IconName::Close)
                                .id("close-device-rename")
                                .aria_label("Cancel renaming")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.cancel_device_rename(cx)),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::MUTED))
                        .child(format!("Choose a new name for {device_name}.")),
                )
                .child(
                    div()
                        .track_focus(&focus)
                        .id("device-name-input")
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
                        .child(input_text::render(
                            &state.device_name_input,
                            "Installation name",
                            state.selection_range(),
                            focused,
                            false,
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|state, _, window, cx| {
                                state.activate_input(InputField::DeviceName);
                                let focus = state.input_focus.clone();
                                window.focus(&focus, cx);
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            button::secondary("Cancel")
                                .id("cancel-device-rename")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.cancel_device_rename(cx)),
                                ),
                        )
                        .child(
                            button::primary("Save changes")
                                .id("confirm-device-rename")
                                .tab_index(0)
                                .on_click(
                                    cx.listener(|state, _, _, cx| state.confirm_device_rename(cx)),
                                ),
                        ),
                ),
        )
}
