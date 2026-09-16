//! Decode and fit once, then rasterize requested quarter-degree poses off-thread.
use std::sync::{Arc, OnceLock};

pub(super) fn load(
    name: &str,
    bytes: &[u8],
    renderer: &gpui::SvgRenderer,
) -> Option<image::RgbaImage> {
    let started = std::time::Instant::now();
    let preview = super::content::decode(name, "", bytes, renderer, 384).ok()?;
    let plain = !preview.kind.is_image();
    let source = super::content::thumbnail_source(&preview, renderer)?;
    let decoded = started.elapsed();
    // Crop the source to the paper ratio first. Filtering pixels that cover
    // would throw away is expensive, especially for wide screenshots.
    let source = if plain {
        source
    } else {
        let (width, height) = (source.width(), source.height());
        let (crop_width, crop_height) = if width * 304 > height * 216 {
            ((height * 216 / 304).max(1), height)
        } else {
            (width, (width * 304 / 216).max(1))
        };
        source.crop_imm(
            (width - crop_width) / 2,
            (height - crop_height) / 2,
            crop_width,
            crop_height,
        )
    };
    // Composite transparency onto the actual white paper before resizing.
    // Filtering straight RGBA against transparent black creates dark fringes
    // around otherwise light content (including screenshots with alpha).
    let mut source = source.into_rgba8();
    for pixel in source.pixels_mut() {
        let alpha = pixel[3] as u16;
        if alpha < 255 {
            for c in 0..3 {
                pixel[c] = ((pixel[c] as u16 * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
    let source = image::DynamicImage::ImageRgba8(source);
    let margin = if plain { 20 } else { 0 };
    let content = if plain {
        source.resize(
            216 - margin * 2,
            304 - margin * 2,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        source.resize_exact(216, 304, image::imageops::FilterType::Lanczos3)
    }
    .to_rgba8();
    let mut page = image::RgbaImage::from_pixel(216, 304, image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(
        &mut page,
        &content,
        (216 - content.width()) as i64 / 2,
        if plain {
            margin as i64
        } else {
            (304 - content.height()) as i64 / 2
        },
    );
    #[cfg(feature = "headless-bench")]
    eprintln!(
        "preview source {:.2} ms, fit {:.2} ms",
        decoded.as_secs_f64() * 1000.,
        (started.elapsed() - decoded).as_secs_f64() * 1000.
    );
    #[cfg(not(feature = "headless-bench"))]
    let _ = decoded;
    Some(page)
}

fn paper_pixels(page: &image::RgbaImage, index: usize) -> image::RgbaImage {
    let angle = index as f32 * 0.25 - 12.;
    let mut output = image::RgbaImage::from_pixel(144, 144, image::Rgba([217, 220, 222, 0]));
    let (sin, cos) = angle.to_radians().sin_cos();
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5 - 72.) * (384. / 144.);
        let dy = (y as f32 + 0.5 - 72.) * (384. / 144.);
        let sx = dx * cos + dy * sin + 108.;
        let sy = -dx * sin + dy * cos + 152.;
        // Antialias only the paper silhouette; do not bake a grey outline
        // into the thumbnail. Extend edge RGB through transparent texels
        // so atlas filtering does not introduce a coloured fringe.
        let qx = (sx - 108.).abs() - (108. - 20.25);
        let qy = (sy - 152.).abs() - (152. - 20.25);
        let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - 20.25;
        let outer = (0.5 - distance / (384. / 144.)).clamp(0., 1.);
        let sx = sx.clamp(0., 214.999);
        let sy = sy.clamp(0., 302.999);
        let ix = sx.floor() as u32;
        let iy = sy.floor() as u32;
        let fx = sx - ix as f32;
        let fy = sy - iy as f32;
        for c in 0..3 {
            let content = (1. - fy)
                * ((1. - fx) * page[(ix, iy)][c] as f32 + fx * page[(ix + 1, iy)][c] as f32)
                + fy * ((1. - fx) * page[(ix, iy + 1)][c] as f32
                    + fx * page[(ix + 1, iy + 1)][c] as f32);
            pixel[c] = content.round() as u8;
        }
        pixel[3] = (outer * 255.).round() as u8;
    }
    output
}

pub(super) fn render(page: &image::RgbaImage, index: usize) -> Arc<gpui::RenderImage> {
    let mut buffer = paper_pixels(page, index);
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]))
}

// Loading and unsupported files use the same rotated paper and clipping as
// decoded previews. Only the content differs; the silhouette never changes.
pub(super) fn placeholder(index: usize) -> Arc<gpui::RenderImage> {
    static PAGE: OnceLock<image::RgbaImage> = OnceLock::new();
    render(PAGE.get_or_init(placeholder_page), index)
}

fn placeholder_page() -> image::RgbaImage {
    let icon = include_str!("../../../assets/icons/file.svg")
        .replace("currentColor", "#9b9fa4")
        .replace("width=\"24\"", "x=\"60\" y=\"64\" width=\"96\"")
        .replace("height=\"24\"", "height=\"96\"");
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="216" height="304"><rect width="216" height="304" fill="white"/>{icon}</svg>"#
    );
    let renderer = gpui::SvgRenderer::new(Arc::new(crate::assets::EmbeddedAssets));
    let rendered = renderer.render_single_frame(svg.as_bytes(), 0.5).unwrap();
    let mut page =
        image::RgbaImage::from_raw(216, 304, rendered.as_bytes(0).unwrap().to_vec()).unwrap();
    for pixel in page.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    page
}
