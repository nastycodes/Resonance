use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn surface() -> Div {
    div()
        .rounded(px(spacing::RADIUS_LG))
        .border_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::SURFACE))
}

pub fn elevated() -> Div {
    surface().bg(rgb(colors::ELEVATED))
}

pub fn unrounded() -> Div {
    div()
        .border_b_1()
        .border_color(rgb(colors::BORDER))
        .bg(rgb(colors::SURFACE))
}
