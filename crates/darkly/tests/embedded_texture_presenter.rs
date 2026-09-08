//! Real GPU output through the production external-texture boundary.
use darkly::engine::{rendering::ExternalTexturePresentError, DarklyEngine};
use darkly::gpu::{
    context::{GpuContext, PresentationAlphaPolicy},
    overlay::{OverlayPrimitive, FLAG_INVERT_COLOR, KIND_FILLED_RECT},
    test_utils::{readback_texture, test_device},
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::COPY_SRC);

fn texture(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("embedded-output"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

#[test]
fn external_output_rotates_resizes_and_preserves_overlay_passes() {
    let (device, queue) = test_device();
    let shared = GpuContext::new_headless(device, queue).shared_device();
    let gpu =
        GpuContext::new_texture_target(shared.clone(), FORMAT, PresentationAlphaPolicy::Opaque)
            .unwrap();
    let mut engine = DarklyEngine::new(gpu, 32, 24);
    let paper = engine.add_raster(None);
    engine.fill_background_color(paper, [210, 50, 30, 255]);
    engine.resize_canvas(darkly::coord::CanvasRect::from_xywh(2, 1, 30, 23));
    engine.set_view_transform(3., -2., 1.25, 0., false, 64., 48.);
    let a = texture(&shared.device, 64, 48, FORMAT, USAGE);
    let b = texture(&shared.device, 64, 48, FORMAT, USAGE);
    let read = |t: &wgpu::Texture| {
        readback_texture(
            &shared.device,
            &shared.queue,
            t,
            FORMAT,
            t.width(),
            t.height(),
        )
    };
    engine.render_to_texture(0., &a).unwrap();
    let base = read(&a);
    assert!(base.chunks_exact(4).any(|p| p[0] > 150 && p[1] < 100));

    let solid = OverlayPrimitive::new(KIND_FILLED_RECT, 0, [4., 4.], [14., 14.]);
    let invert = OverlayPrimitive::new(KIND_FILLED_RECT, FLAG_INVERT_COLOR, [20., 20.], [30., 30.]);
    engine.set_overlay_primitives(vec![solid, invert]);
    engine.render_to_texture(0., &a).unwrap();
    let overlays = read(&a);
    assert_ne!(base, overlays, "production overlays must reach the output");
    engine.render_to_texture(0., &b).unwrap();
    assert_eq!(
        overlays,
        read(&b),
        "a clean rotating target must receive the image"
    );

    engine.set_overlay_primitives(vec![]);
    let array = shared.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("invalid-output-array"),
        size: wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: USAGE,
        view_formats: &[],
    });
    assert_eq!(
        engine.render_to_texture(0., &array),
        Err(ExternalTexturePresentError::Shape)
    );
    assert!(GpuContext::new_texture_target(
        shared.clone(),
        wgpu::TextureFormat::Rgba32Float,
        PresentationAlphaPolicy::Opaque
    )
    .is_err());
    let bad = texture(
        &shared.device,
        64,
        48,
        wgpu::TextureFormat::Bgra8Unorm,
        USAGE,
    );
    assert_eq!(
        engine.render_to_texture(0., &bad),
        Err(ExternalTexturePresentError::Format)
    );
    let bad = texture(
        &shared.device,
        64,
        48,
        FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    assert_eq!(
        engine.render_to_texture(0., &bad),
        Err(ExternalTexturePresentError::Usage)
    );
    engine.render_to_texture(0., &a).unwrap();
    assert_eq!(
        base,
        read(&a),
        "rejected outputs must preserve pending state"
    );

    engine.add_veil_layer("black_and_white", &[]);
    engine.set_view_transform(0., 0., 1., 0., false, 40., 30.);
    let resized = texture(&shared.device, 40, 30, FORMAT, USAGE);
    engine.render_to_texture(0., &resized).unwrap();
    let pixels = read(&resized);
    assert_eq!(pixels.len(), 40 * 30 * 4);
    assert!(pixels.chunks_exact(4).all(|p| p[3] == 255));
    assert!(
        pixels
            .chunks_exact(4)
            .all(|p| p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1),
        "resized output must include the veil"
    );

    let gpu = GpuContext::new_texture_target(
        shared.clone(),
        FORMAT,
        PresentationAlphaPolicy::PreMultiplied,
    )
    .unwrap();
    let mut transparent = DarklyEngine::new(gpu, 8, 8);
    transparent.set_view_transform(0., 0., 1., 0., false, 8., 8.);
    let output = texture(&shared.device, 8, 8, FORMAT, USAGE);
    transparent.render_to_texture(0., &output).unwrap();
    assert!(
        read(&output).iter().all(|v| *v == 0),
        "the context alpha policy must reach the engine's final pipeline"
    );
}
