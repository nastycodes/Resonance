use std::sync::Arc;

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::prelude::*;
use gpui::{Image, ImageFormat, IntoElement, ObjectFit, div, img, px, rgb};

pub fn render(initials: &str, color: u32, size: f32) -> impl IntoElement {
    div()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(rgb(color))
        .text_color(rgb(0xffffff))
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(initials.to_string())
}

pub fn render_user(user: &crate::models::user::User, size: f32) -> impl IntoElement {
    let image = user
        .profile_image
        .as_deref()
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|bytes| {
            let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                ImageFormat::Png
            } else if bytes.starts_with(b"\xff\xd8\xff") {
                ImageFormat::Jpeg
            } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
                ImageFormat::Webp
            } else {
                return None;
            };
            Some(Arc::new(Image::from_bytes(format, bytes)))
        });

    match image {
        Some(image) => div()
            .size(px(size))
            .rounded_full()
            .overflow_hidden()
            .child(img(image).size_full().object_fit(ObjectFit::Cover))
            .into_any_element(),
        None => render(&user.initials, user.color, size).into_any_element(),
    }
}
