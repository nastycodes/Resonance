use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn frame(focused: bool) -> Div {
    div()
        .w_full()
        .h(px(spacing::CONTROL_HEIGHT))
        .min_w_0()
        .flex()
        .items_center()
        .px_3()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(if focused {
            colors::FOCUS
        } else {
            colors::BORDER
        }))
        .bg(rgb(colors::CANVAS))
        .focus(|style| style.border_color(rgb(colors::FOCUS)))
}
