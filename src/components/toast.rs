use gpui::prelude::*;
use gpui::{ClipboardItem, Context, IntoElement, KeyDownEvent, div, px, rgb};

use crate::{
    app_state::AppState,
    components::{
        card,
        icon::{self, IconName},
    },
    theme::colors,
};

pub fn render(message: &str, cx: &mut Context<AppState>) -> impl IntoElement {
    let message = message.to_string();
    card::elevated()
        .id("copy-toast")
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap_3()
        .p_3()
        .shadow_lg()
        .cursor_pointer()
        .aria_label("Copy notification to clipboard")
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .text_color(rgb(colors::TEXT))
                .whitespace_normal()
                .flex_wrap()
                .child(message.clone()),
        )
        .child(
            div()
                .id("dismiss-toast")
                .size(px(28.0))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .aria_label("Dismiss notification")
                .tab_index(0)
                .child(icon::render(IconName::Close, 14.0, colors::MUTED))
                .on_click(cx.listener(|state, _, _, cx| {
                    cx.stop_propagation();
                    state.dismiss_toast(cx);
                }))
                .on_key_down(cx.listener(|state, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key.eq_ignore_ascii_case("enter")
                        || event.keystroke.key == "space"
                    {
                        state.dismiss_toast(cx);
                    }
                })),
        )
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(message.clone()));
        }))
}
