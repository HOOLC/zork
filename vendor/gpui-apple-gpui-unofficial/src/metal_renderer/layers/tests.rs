use super::*;
use gpui::{PaintEffect, Quad, ScaledPixels, px, rgb, rgba};

fn bounds() -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(64.), ScaledPixels(64.)),
    )
}

fn quad(color: u32, left: f32) -> Quad {
    Quad {
        bounds: Bounds::new(
            point(ScaledPixels(left), ScaledPixels(0.)),
            size(ScaledPixels(64. - left), ScaledPixels(64.)),
        ),
        content_mask: ContentMask { bounds: bounds() },
        background: rgba(color).into(),
        ..Default::default()
    }
}

fn content(id: u64) -> Arc<RetainedContent> {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(0xff000080, 0.));
    scene.insert_primitive(quad(0x0000ff80, 32.));
    scene.finish();
    Arc::new(RetainedContent {
        id,
        bounds: bounds(),
        scene,
    })
}

fn effect(opacity: f32) -> gpui::PaintEffect {
    let mut effect = PaintEffect::new(bounds().origin, 1., vec![bounds()]);
    effect.opacity = opacity;
    effect
}

fn scene(content: Arc<RetainedContent>, effect: PaintEffect) -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(Quad {
        bounds: bounds(),
        content_mask: ContentMask { bounds: bounds() },
        background: rgb(0xffffff).into(),
        ..Default::default()
    });
    scene.retained_layers.push(RetainedLayer {
        order: 1,
        content,
        effects: vec![effect],
    });
    scene.finish();
    scene
}

fn renderer() -> MetalRenderer {
    MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())))
}

fn pixel(image: &image::RgbaImage, x: u32, expected: [u8; 4]) {
    let actual = image.get_pixel(x, 16).0;
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
        "{x}: {actual:?} != {expected:?}"
    );
}

#[test]
fn transparent_content_and_nested_opacity_preserve_premultiplied_color() {
    let mut renderer = renderer();
    let size = size(DevicePixels(64), DevicePixels(64));
    let content = content(1);
    let first = renderer
        .render_scene_to_image(&scene(content.clone(), effect(1.)), size)
        .unwrap();
    pixel(&first, 16, [255, 127, 127, 255]);
    pixel(&first, 48, [127, 63, 191, 255]);
    let second = renderer
        .render_scene_to_image(&scene(content.clone(), effect(0.5)), size)
        .unwrap();
    pixel(&second, 16, [255, 191, 191, 255]);
    pixel(&second, 48, [191, 159, 223, 255]);
    assert_eq!(
        renderer.render_stats.content_redraws, 0,
        "opacity must reuse color pixels"
    );

    let mut nested = Scene::default();
    nested.retained_layers.push(RetainedLayer {
        order: 0,
        content,
        effects: vec![effect(0.5)],
    });
    nested.finish();
    let parent = Arc::new(RetainedContent {
        id: 2,
        bounds: bounds(),
        scene: nested,
    });
    let nested = renderer
        .render_scene_to_image(&scene(parent.clone(), effect(1.)), size)
        .unwrap();
    assert_eq!(nested, second, "nested transparent targets changed alpha");
    renderer
        .render_scene_to_image(&scene(parent, effect(1.)), size)
        .unwrap();
    assert_eq!(renderer.render_stats.content_redraws, 0);
}

fn mask(left: f32, right: f32) -> Arc<Path<ScaledPixels>> {
    let mut path = Path::new(point(px(left), px(0.)));
    if right > left {
        path.line_to(point(px(right), px(0.)));
        path.line_to(point(px(right), px(64.)));
        path.line_to(point(px(left), px(64.)));
        path.line_to(point(px(left), px(0.)));
    }
    let mut path = path.scale(1.);
    path.content_mask.bounds = bounds();
    path.color = rgb(0xffffff).into();
    Arc::new(path)
}

#[test]
fn changed_and_empty_masks_reuse_color_without_stale_pixels() {
    let mut renderer = renderer();
    let size = size(DevicePixels(64), DevicePixels(64));
    let content = content(3);
    for (index, (left, right, a, b)) in [
        (0., 32., [255, 127, 127, 255], [255; 4]),
        (32., 64., [255; 4], [127, 63, 191, 255]),
        (0., 0., [255; 4], [255; 4]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut effect = effect(1.);
        effect.gpu_mask = Some(mask(left, right));
        let image = renderer
            .render_scene_to_image(&scene(content.clone(), effect), size)
            .unwrap();
        pixel(&image, 16, a);
        pixel(&image, 48, b);
        assert_eq!(renderer.render_stats.content_redraws, u32::from(index == 0));
        assert_eq!(renderer.render_stats.mask_redraws, 1);
    }
    renderer
        .render_scene_to_image(
            &scene(content, effect(1.)),
            gpui::size(DevicePixels(96), DevicePixels(96)),
        )
        .unwrap();
    assert_eq!(
        renderer.render_stats.content_redraws, 1,
        "resizing invalidates texture coordinates"
    );
}

#[test]
fn destination_translation_reuses_pixels_and_matches_the_unmoved_picture_on_return() {
    let mut renderer = renderer();
    let size = size(DevicePixels(64), DevicePixels(64));
    let content = content(4);
    let original = renderer.render_scene_to_image(&scene(content.clone(), effect(1.)), size).unwrap();
    let mut moved = effect(1.);
    moved.scale = 0.5;
    moved.translation = point(ScaledPixels(16.), ScaledPixels(0.));
    let translated = renderer.render_scene_to_image(&scene(content.clone(), moved), size).unwrap();
    pixel(&translated, 8, [255; 4]);
    pixel(&translated, 24, [255, 127, 127, 255]);
    pixel(&translated, 40, [127, 63, 191, 255]);
    pixel(&translated, 56, [255; 4]);
    assert_eq!(renderer.render_stats.content_redraws, 0);
    let returned = renderer.render_scene_to_image(&scene(content, effect(1.)), size).unwrap();
    assert_eq!(original, returned);
    assert_eq!(renderer.render_stats.content_redraws, 0);
}

#[test]
fn source_and_replacement_versions_coexist_without_redrawing_each_other() {
    let mut renderer = renderer();
    let dimensions = size(DevicePixels(64), DevicePixels(64));
    let source = content(61);
    let mut green = Scene::default();
    green.insert_primitive(quad(0x00ff00ff, 0.));
    green.finish();
    let replacement = Arc::new(RetainedContent { id: source.id, bounds: bounds(), scene: green });
    renderer.render_scene_to_image(&scene(source.clone(), effect(1.)), dimensions).unwrap();
    let mut combined = scene(replacement, effect(1.));
    combined.retained_layers[0].effects[0].clips = Arc::new(vec![Bounds::new(
        point(ScaledPixels(32.), ScaledPixels(0.)), size(ScaledPixels(32.), ScaledPixels(64.)))]);
    let mut left = effect(1.);
    left.clips = Arc::new(vec![Bounds::new(bounds().origin, size(ScaledPixels(32.), ScaledPixels(64.)))]);
    combined.retained_layers.push(RetainedLayer { order: 2, content: source, effects: vec![left] });
    combined.finish();
    let first = renderer.render_scene_to_image(&combined, dimensions).unwrap();
    pixel(&first, 16, [255, 127, 127, 255]);
    pixel(&first, 48, [0, 255, 0, 255]);
    assert_eq!(renderer.render_stats.content_redraws, 1, "the source's old pixels were needlessly redrawn");
    let second = renderer.render_scene_to_image(&combined, dimensions).unwrap();
    assert_eq!(first, second);
    assert_eq!(renderer.render_stats.content_redraws, 0, "two versions fought over one texture slot");
}

#[test]
fn changed_versions_recycle_unneeded_allocations_and_repeated_instances_count_toward_budget() {
    let mut renderer = renderer();
    let dimensions = size(DevicePixels(64), DevicePixels(64));
    let first = content(62);
    renderer.render_scene_to_image(&scene(first.clone(), effect(1.)), dimensions).unwrap();
    let allocation = renderer.retained.entries.values().next().unwrap().texture.as_ptr();
    for _ in 0..3 {
        let replacement = content(62);
        renderer.render_scene_to_image(&scene(replacement, effect(1.)), dimensions).unwrap();
        assert_eq!(renderer.retained.entries.len(), 1);
        assert_eq!(renderer.retained.entries.values().next().unwrap().texture.as_ptr(), allocation);
        assert_eq!(renderer.retained.pending_bytes, 0);
        assert_eq!(renderer.retained.pending_layers, 0);
    }
    let mut repeated = scene(first.clone(), effect(1.));
    for order in 2..=9 { repeated.retained_layers.push(RetainedLayer { order, content: first.clone(), effects: vec![effect(1.)] }); }
    assert!(!admitted(&repeated, dimensions));
    let mut two = scene(first.clone(), effect(1.));
    two.retained_layers.push(RetainedLayer { order: 2, content: first, effects: vec![effect(1.)] });
    assert!(!admitted(&two, size(DevicePixels(4097), DevicePixels(4096))));
}
