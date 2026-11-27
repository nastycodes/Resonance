use gpui::prelude::*;
use gpui::{Div, div, px, relative, rgb};

use crate::theme::{colors, spacing};

pub fn bubble(mine: bool) -> Div {
    let bubble = div()
        .relative()
        .flex()
        .items_start()
        .self_start()
        .max_w(relative(0.4))
        .gap_3()
        .p_3()
        .rounded(px(spacing::RADIUS_LG))
        .bg(rgb(if mine {
            colors::PANEL_ACTIVE
        } else {
            colors::PANEL_HOVER
        }));
    if mine {
        bubble.self_end().justify_end()
    } else {
        bubble
    }
}
