use super::*;
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferKeys};
use gpui::{PaintSurface, Quad, ScaledPixels, rgb};

fn bounds() -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(64.), ScaledPixels(64.)),
    )
}

fn fill(buffer: &CVPixelBuffer, luma: u8) {
    assert_eq!(buffer.lock_base_address(0), 0);
    assert_eq!(buffer.get_plane_count(), 2);
    for plane in 0..2 {
        let length = buffer.get_bytes_per_row_of_plane(plane) * buffer.get_height_of_plane(plane);
        // The locked CVPixelBuffer owns this complete plane, including padding.
        unsafe {
            let address = buffer.get_base_address_of_plane(plane).cast::<u8>();
            assert!(!address.is_null());
            std::ptr::write_bytes(address, if plane == 0 { luma } else { 128 }, length);
        }
    }
    assert_eq!(buffer.unlock_base_address(0), 0);
}

#[test]
fn video_surfaces_render_and_update_after_an_ordinary_frame() {
    let mut renderer =
        MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())));
    let size = size(DevicePixels(64), DevicePixels(64));
    let mut ordinary = Scene::default();
    ordinary.insert_primitive(Quad {
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        background: rgb(0xff0000).into(),
        ..Default::default()
    });
    ordinary.finish();
    let image = renderer.render_scene_to_image(&ordinary, size).unwrap();
    assert_eq!(image.get_pixel(32, 32).0, [255, 0, 0, 255]);

    let properties = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
    let attributes = CFDictionary::from_CFType_pairs(&[
        (
            CFString::from(CVPixelBufferKeys::MetalCompatibility),
            CFBoolean::true_value().as_CFType(),
        ),
        (
            CFString::from(CVPixelBufferKeys::IOSurfaceProperties),
            properties.as_CFType(),
        ),
    ]);
    let buffer = CVPixelBuffer::new(
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        64,
        64,
        Some(&attributes),
    )
    .unwrap();
    let mut video = Scene::default();
    video.insert_primitive(PaintSurface {
        order: 0,
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        image_buffer: buffer.clone(),
    });
    video.finish();
    for luma in [0, 255] {
        fill(&buffer, luma);
        let image = renderer.render_scene_to_image(&video, size).unwrap();
        let pixel = image.get_pixel(32, 32).0;
        assert!(
            pixel[..3].iter().all(|channel| channel.abs_diff(luma) <= 2),
            "{pixel:?}"
        );
        assert_eq!(pixel[3], 255);
    }
}
