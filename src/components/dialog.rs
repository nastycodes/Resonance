use gpui::prelude::*;
use gpui::{Div, div, hsla, px, relative, rgb};

use crate::theme::{colors, spacing};

pub fn overlay() -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0.0, 0.0, 0.0, 0.68))
}

pub fn surface() -> Div {
    div()
        .max_w_full()
        .max_h(relative(0.9))
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .rounded(px(spacing::RADIUS_XL))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::PANEL))
        .shadow_lg()
}

pub fn header(title: impl Into<gpui::SharedString>) -> Div {
    div().flex().items_center().justify_between().gap_3().child(
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(colors::TEXT))
            .child(title.into()),
    )
}
