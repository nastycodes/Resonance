use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::colors;

pub fn horizontal() -> Div {
    div().h(px(1.0)).w_full().bg(rgb(colors::BORDER))
}

pub fn vertical() -> Div {
    div().w(px(1.0)).h_full().bg(rgb(colors::BORDER))
}
