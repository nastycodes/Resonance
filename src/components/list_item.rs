use gpui::prelude::*;
use gpui::{Div, div, px, rgb};

use crate::theme::{colors, spacing};

pub fn render() -> Div {
    div()
        .w_full()
        .min_w_0()
        .min_h(px(spacing::ACTION_TARGET))
        .flex()
        .gap_3()
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
        .focus_visible(|style| style.border_color(rgb(colors::FOCUS)).shadow_sm())
}
