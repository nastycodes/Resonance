use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, Context, ElementId, IntoElement, SharedString, Window, ease_out_quint,
};

use crate::{app_state::AppState, theme::spacing};

pub fn item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    destructive: bool,
    cx: &mut Context<AppState>,
    action: impl Fn(&mut AppState, &mut Window, &mut Context<AppState>) + 'static,
) -> impl IntoElement {
    crate::components::button::menu_item(label, destructive)
        .id(id)
        .tab_index(0)
        .on_click(cx.listener(move |state, _, window, cx| action(state, window, cx)))
        .with_animation(
            "menu-item-fade",
            Animation::new(Duration::from_millis(spacing::MOTION_DURATION_MS))
                .with_easing(ease_out_quint()),
            |item, delta| item.opacity(delta),
        )
}
