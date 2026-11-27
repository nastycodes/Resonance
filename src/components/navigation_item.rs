use gpui::prelude::*;
use gpui::{Stateful, div, px, rgb};

use crate::{
    components::icon::{IconName, render},
    theme::{colors, spacing},
};

pub fn render_item(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    label: impl Into<gpui::SharedString>,
    active: bool,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .gap_3()
        .min_h(px(spacing::ACTION_TARGET))
        .px_3()
        .py_2()
        .rounded(px(spacing::RADIUS_MD))
        .border_1()
        .border_color(rgb(if active {
            colors::PANEL_ACTIVE
        } else {
            colors::SIDEBAR
        }))
        .cursor_pointer()
        .bg(rgb(if active {
            colors::PANEL_ACTIVE
        } else {
            colors::SIDEBAR
        }))
        .text_color(rgb(if active { colors::TEXT } else { colors::MUTED }))
        .hover(|style| style.bg(rgb(colors::PANEL_HOVER)))
        .focus_visible(|style| style.border_color(rgb(colors::FOCUS)))
        .child(render(
            icon,
            18.0,
            if active { colors::TEXT } else { colors::MUTED },
        ))
        .child(div().text_sm().child(label.into()))
}
