use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// Decodes a PNG into an image for the user interface.
pub fn decode_png(bytes: &[u8]) -> Option<Image> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );
    Some(Image::from_rgba8(buffer))
}

/// Wraps a page frame texture so the user interface can draw it without a copy.
pub fn frame_image(texture: slint::wgpu_30::wgpu::Texture) -> Option<Image> {
    match Image::try_from(texture) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!(%error, "page frame could not be shown");
            None
        }
    }
}
