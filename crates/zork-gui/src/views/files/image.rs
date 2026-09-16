//! Image decoding is bounded and runs away from layout/paint. SVG documents
//! retain their parsed tree so zooming can rasterize at the displayed scale.
use std::sync::Arc;

pub(in crate::views) use zork_ui::attachment_viewer::DecodedImage;

pub(in crate::views) fn format(name: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat as F;
    match std::path::Path::new(name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some(F::Png),
        "jpg" | "jpeg" => Some(F::Jpeg),
        "svg" => Some(F::Svg),
        "webp" => Some(F::Webp),
        "gif" => Some(F::Gif),
        "bmp" => Some(F::Bmp),
        "tif" | "tiff" => Some(F::Tiff),
        "ico" => Some(F::Ico),
        _ => None,
    }
}

pub(in crate::views) fn kind(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "md" | "markdown" => "Markdown".into(),
            _ => s.to_ascii_uppercase(),
        })
        .unwrap_or_default()
}

fn raster(bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}

pub(super) fn render(mut buffer: image::RgbaImage) -> Arc<gpui::RenderImage> {
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]))
}

pub(super) fn from_raster(image: image::DynamicImage) -> DecodedImage {
    DecodedImage {
        size: gpui::size(image.width() as f32, image.height() as f32),
        rendered: render(image.to_rgba8()),
        svg: None,
    }
}

pub(in crate::views) fn decode(
    bytes: &[u8],
    format: gpui::ImageFormat,
    renderer: &gpui::SvgRenderer,
) -> anyhow::Result<DecodedImage> {
    if format == gpui::ImageFormat::Svg {
        let svg = Arc::new(renderer.parse_svg(bytes)?);
        let rendered = renderer.render_parsed(&svg, 1.)?;
        // GPUI 1.17 uses a 2x smoothing raster for SVG ScaleFactor renders.
        let size = rendered.size(0);
        return Ok(DecodedImage {
            size: gpui::size(
                i32::from(size.width) as f32 / 2.0,
                i32::from(size.height) as f32 / 2.0,
            ),
            rendered,
            svg: Some(svg),
        });
    }
    let image = raster(bytes)?;
    Ok(DecodedImage {
        size: gpui::size(image.width() as f32, image.height() as f32),
        rendered: render(image.to_rgba8()),
        svg: None,
    })
}

// Duplicate edge texels outside the visible image. GPUI's shared atlas uses
// linear sampling without tile clamping, so upscaling otherwise reads neighbors.
pub(super) fn pad_thumbnail(source: image::RgbaImage) -> image::RgbaImage {
    let (width, height) = source.dimensions();
    image::RgbaImage::from_fn(width + 2, height + 2, |x, y| {
        source[(
            x.saturating_sub(1).min(width - 1),
            y.saturating_sub(1).min(height - 1),
        )]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_preserves_intrinsic_size_and_transparency() {
        let renderer = gpui::SvgRenderer::new(Arc::new(crate::assets::EmbeddedAssets));
        let image = decode(
            include_bytes!("../../../../zork-ui/assets/avatars/portraits/fox.svg"),
            gpui::ImageFormat::Svg,
            &renderer,
        )
        .unwrap();
        assert_eq!(image.size, gpui::size(256., 256.));
        assert_eq!(image.rendered.as_bytes(0).unwrap()[3], 0);
        assert!(image.svg.is_some());
    }
    #[test]
    fn very_tall_svg_thumbnail_stays_bounded() {
        let renderer = gpui::SvgRenderer::new(Arc::new(crate::assets::EmbeddedAssets));
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="2400"><rect width="12" height="2400" fill="#ff0000"/></svg>"##;
        let image = super::super::content::thumbnail("long.SVG", bytes, &renderer).unwrap();
        assert!(i32::from(image.size(0).width) <= 602);
        assert!(i32::from(image.size(0).height) <= 402);
        assert!(image.as_bytes(0).unwrap().len() <= 602 * 402 * 4);
    }
}
