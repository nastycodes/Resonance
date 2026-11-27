use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn render(label: impl Into<gpui::SharedString>) -> Div {
    div()
        .px_2()
        .py_1()
        .rounded(px(spacing::RADIUS_SM))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::CANVAS))
        .text_xs()
        .text_color(rgb(colors::TEXT))
        .shadow_lg()
        .child(label.into())
}
