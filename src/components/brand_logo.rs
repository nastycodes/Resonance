use std::sync::Arc;

use gpui::{Image, ImageFormat, IntoElement, Styled, img, px};

pub fn render(size: f32) -> impl IntoElement {
    img(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../assets/images/resonance-logo.png").to_vec(),
    )))
    .size(px(size))
}
