use std::{ops::Range, time::Duration};

use gpui::prelude::*;
use gpui::{Animation, AnimationExt, IntoElement, div, px, rgb};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::colors;

pub fn render(
    value: &str,
    placeholder: &str,
    selection: Range<usize>,
    focused: bool,
    wrap: bool,
) -> impl IntoElement {
    render_value(
        value.to_string(),
        placeholder.to_string(),
        selection,
        focused,
        wrap,
    )
}

fn render_value(
    value: String,
    placeholder: String,
    selection: Range<usize>,
    focused: bool,
    wrap: bool,
) -> impl IntoElement {
    let showing_placeholder = value.is_empty() && !focused;
    let displayed_value = if showing_placeholder {
        placeholder
    } else {
        value
    };
    let char_count = displayed_value.graphemes(true).count();
    let selection = if showing_placeholder || !focused {
        0..0
    } else {
        selection.start.min(char_count)..selection.end.min(char_count)
    };
    let start = char_offset_to_byte(&displayed_value, selection.start);
    let end = char_offset_to_byte(&displayed_value, selection.end);
    let mut rendered = div().flex().items_center();
    if wrap {
        rendered = rendered
            .w_full()
            .min_w_0()
            .items_start()
            .whitespace_normal()
            .flex_wrap();
    }

    if showing_placeholder {
        rendered = rendered
            .text_color(rgb(colors::MUTED))
            .child(displayed_value.to_string());
    } else {
        let prefix = &displayed_value[..start];
        let selected = &displayed_value[start..end];
        let suffix = &displayed_value[end..];

        if !prefix.is_empty() {
            rendered = rendered.child(prefix.to_string());
        }
        if focused && selection.is_empty() {
            rendered = rendered.child(blinking_caret());
        } else if !selected.is_empty() {
            rendered = rendered.child(
                div()
                    .bg(rgb(colors::ACCENT))
                    .text_color(rgb(colors::ACCENT_FOREGROUND))
                    .child(selected.to_string()),
            );
        }
        if !suffix.is_empty() {
            rendered = rendered.child(suffix.to_string());
        }
    }

    rendered
}

pub fn render_password(
    value: &str,
    placeholder: &str,
    selection: Range<usize>,
    focused: bool,
) -> impl IntoElement {
    let masked = value.graphemes(true).map(|_| "•").collect::<String>();
    render_value(masked, placeholder.to_string(), selection, focused, false)
}

fn char_offset_to_byte(text: &str, char_offset: usize) -> usize {
    text.grapheme_indices(true)
        .nth(char_offset)
        .map(|(byte_offset, _)| byte_offset)
        .unwrap_or(text.len())
}

fn blinking_caret() -> impl IntoElement {
    div()
        .mx(px(1.0))
        .w(px(1.0))
        .h(px(16.0))
        .bg(rgb(colors::ACCENT))
        .with_animation(
            "input-caret",
            Animation::new(Duration::from_millis(1000)).repeat(),
            |caret, delta| caret.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
        )
}
