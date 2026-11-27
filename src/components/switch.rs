use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn render(checked: bool) -> Div {
    div()
        .relative()
        .w(px(36.0))
        .h(px(20.0))
        .rounded(px(spacing::RADIUS_XL))
        .bg(rgb(if checked {
            colors::ACCENT
        } else {
            colors::BORDER
        }))
        .border_1()
        .border_color(rgb(if checked {
            colors::ACCENT
        } else {
            colors::BORDER
        }))
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .size(px(14.0))
                .rounded_full()
                .bg(rgb(if checked {
                    colors::ACCENT_FOREGROUND
                } else {
                    colors::MUTED
                }))
                .left(if checked { px(18.0) } else { px(2.0) }),
        )
}
