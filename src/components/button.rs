use gpui::prelude::*;
use gpui::{BoxShadow, Div, SharedString, div, hsla, point, px, rgb};

use crate::{
    app_state::AppState,
    components::icon::{self, IconName},
    theme::{colors, spacing, typography},
};

pub fn primary(label: impl Into<SharedString>) -> Div {
    button(
        label,
        colors::ACCENT_FOREGROUND,
        colors::ACCENT,
        colors::ACCENT,
        colors::ACCENT,
    )
}

pub fn secondary(label: impl Into<SharedString>) -> Div {
    button(
        label,
        colors::TEXT,
        colors::PANEL_ACTIVE,
        colors::PANEL_HOVER,
        colors::BORDER,
    )
}

pub fn quiet(label: impl Into<SharedString>) -> Div {
    button(
        label,
        colors::MUTED,
        colors::SIDEBAR,
        colors::PANEL_HOVER,
        colors::SIDEBAR,
    )
}

pub fn destructive(label: impl Into<SharedString>) -> Div {
    button(
        label,
        colors::DANGER,
        colors::PANEL,
        colors::PANEL_HOVER,
        colors::PANEL,
    )
}

pub fn icon_button(icon_name: IconName) -> Div {
    icon_button_with_color(
        icon_name,
        colors::TEXT,
        colors::PANEL_ACTIVE,
        colors::BORDER,
    )
}

pub fn primary_icon(icon_name: IconName) -> Div {
    div()
        .size(px(spacing::ACTION_TARGET))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(colors::ACCENT))
        .cursor_pointer()
        .bg(rgb(colors::ACCENT))
        .hover(|style| style.bg(rgb(colors::ACCENT)))
        .focus_visible(|style| style.border_color(rgb(colors::TEXT)).shadow_sm())
        .child(icon::render(icon_name, 18.0, colors::ACCENT_FOREGROUND))
}

pub fn secondary_icon(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::ACCENT, colors::PANEL, colors::PANEL)
}

pub fn success_icon(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::GREEN, colors::PANEL, colors::PANEL)
}

pub fn warning_icon(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::WARNING, colors::PANEL, colors::PANEL)
}

pub fn info_icon(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::INFO, colors::PANEL, colors::PANEL)
}

pub fn clear_icon(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::MUTED, colors::SIDEBAR, colors::SIDEBAR)
}

pub fn composer_icon(icon_name: IconName) -> Div {
    div()
        .size(px(spacing::COMPOSER_ACTION_TARGET))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(spacing::RADIUS_MD))
        .cursor_pointer()
        .bg(rgb(colors::PANEL_ACTIVE))
        .hover(|style| style.bg(rgb(colors::PANEL)))
        .focus_visible(|style| style.border_color(rgb(colors::TEXT)).shadow_sm())
        .child(icon::render(icon_name, 16.0, colors::ACCENT))
}

pub fn composer_clear_icon(icon_name: IconName) -> Div {
    div()
        .size(px(spacing::COMPOSER_ACTION_TARGET))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(spacing::RADIUS_MD))
        .cursor_pointer()
        .bg(rgb(colors::PANEL_ACTIVE))
        .hover(|style| style.bg(rgb(colors::PANEL)))
        .focus_visible(|style| style.border_color(rgb(colors::TEXT)).shadow_sm())
        .child(icon::render(icon_name, 16.0, colors::MUTED))
}

pub fn destructive_icon_button(icon_name: IconName) -> Div {
    icon_button_with_color(icon_name, colors::DANGER, colors::PANEL, colors::PANEL)
}

fn icon_button_with_color(
    icon_name: IconName,
    icon_color: u32,
    background: u32,
    border: u32,
) -> Div {
    div()
        .size(px(spacing::ACTION_TARGET))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(border))
        .cursor_pointer()
        .bg(rgb(background))
        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
        .focus_visible(|style| style.border_color(rgb(colors::ACCENT)).shadow_sm())
        .child(icon::render(icon_name, 16.0, icon_color))
}

pub fn menu_surface(menu_id: &str, cx: &mut Context<AppState>) -> Div {
    let menu_id = menu_id.to_string();
    div()
        .absolute()
        .top(px(spacing::ACTION_TARGET))
        .right_0()
        .w(px(208.0))
        .flex()
        .flex_col()
        .gap_1()
        .p_1()
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::PANEL))
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, spacing::POPOVER_SHADOW_OPACITY),
            offset: point(px(0.0), px(spacing::POPOVER_SHADOW_OFFSET_Y)),
            blur_radius: px(spacing::POPOVER_SHADOW_BLUR),
            spread_radius: px(0.0),
            inset: false,
        }])
        .rounded(px(spacing::RADIUS_LG))
        .on_mouse_down_out(cx.listener(move |state, _, _, cx| state.dismiss_menu(&menu_id, cx)))
}

pub fn menu_item(label: impl Into<SharedString>, destructive: bool) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(px(spacing::ACTION_TARGET))
        .w_full()
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(colors::PANEL))
        .cursor_pointer()
        .focus_visible(|style| style.border_color(rgb(colors::ACCENT)))
        .bg(rgb(colors::PANEL))
        .text_sm()
        .text_color(rgb(if destructive {
            colors::DANGER
        } else {
            colors::TEXT
        }))
        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
        .child(label.into())
}

fn button(
    label: impl Into<SharedString>,
    foreground: u32,
    background: u32,
    hover: u32,
    border: u32,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(spacing::ACTION_TARGET))
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(border))
        .cursor_pointer()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_size(px(typography::BODY))
        .hover(move |style| style.bg(rgb(hover)))
        .focus_visible(|style| style.border_color(rgb(colors::ACCENT)).shadow_sm())
        .child(label.into())
}
