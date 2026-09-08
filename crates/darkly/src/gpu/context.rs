use std::sync::Arc;

/// Shareable GPU device + queue. Multiple `GpuContext`s — and thus multiple
/// `DarklyEngine` instances rendering to different canvases — can hold an
/// `Arc<GpuDevice>` and use a single underlying WebGPU device. This avoids
/// duplicate adapter/device acquisition (mandatory on web, where browsers
/// typically expose only one device per origin) and lets shaders/pipelines
/// be compiled once per device rather than once per engine.
///
/// `wgpu::Device` and `wgpu::Queue` are `Send + Sync` on native but not on
/// wasm32. Darkly is single-threaded everywhere (the JS event loop on web,
/// the main thread on native), so the `Arc` is only ever used for shared
/// ownership across engines on the same thread — `Rc` would work too, but
/// we keep `Arc` so the `GpuDevice` type doesn't fork by platform. The
/// clippy `arc_with_non_send_sync` lint is suppressed at construction.
pub struct GpuDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub struct GpuContext {
    pub gpu: Arc<GpuDevice>,
    pub surface: Option<wgpu::Surface<'static>>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
    presentation_alpha: PresentationAlphaPolicy,
    headless_format: wgpu::TextureFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("texture presentation requires RGBA8 or BGRA8 unorm, optionally sRGB")]
pub struct TextureTargetFormatError;

/// How the present shader must encode alpha for its configured surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAlphaPolicy {
    /// Draw Darkly's normal opaque checkerboard.
    Opaque,
    /// Emit premultiplied RGBA for a premultiplied-alpha surface.
    PreMultiplied,
    /// Emit straight RGBA for a postmultiplied-alpha surface.
    PostMultiplied,
}

impl PresentationAlphaPolicy {
    pub const fn is_transparent(self) -> bool {
        !matches!(self, Self::Opaque)
    }

    pub const fn shader_flag(self) -> f32 {
        match self {
            Self::Opaque => 0.0,
            Self::PreMultiplied => 1.0,
            Self::PostMultiplied => 2.0,
        }
    }
}

/// A requested alpha-preserving presentation property is not supported by this
/// surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePresentationError {
    /// The embedding surface cannot be configured to preserve alpha.
    TransparentAlphaUnsupported,
}

impl std::fmt::Display for SurfacePresentationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TransparentAlphaUnsupported => {
                f.write_str("This surface does not support alpha-preserving presentation")
            }
        }
    }
}

impl std::error::Error for SurfacePresentationError {}

// Field access through `Deref` lets the engine keep using `self.gpu.device`
// and `self.gpu.queue` everywhere — `self.gpu` is a `GpuContext`, has no
// `device` field, autoderefs to `GpuDevice`, finds it.
impl std::ops::Deref for GpuContext {
    type Target = GpuDevice;
    fn deref(&self) -> &GpuDevice {
        &self.gpu
    }
}

impl GpuContext {
    /// Create a GPU context from a pre-built instance and surface.
    ///
    /// The caller is responsible for platform-specific instance and surface
    /// creation (e.g. from an HTML canvas or a native window handle).
    /// `limits` controls device capability requirements (e.g.
    /// `Limits::downlevel_webgl2_defaults()` for WASM,
    /// `Limits::default()` for native).
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        limits: wgpu::Limits,
        initial_width: u32,
        initial_height: u32,
    ) -> Self {
        Self::new_inner(
            instance,
            surface,
            limits,
            initial_width,
            initial_height,
            false,
        )
        .await
        .expect("opaque surface configuration must be supported")
    }

    /// Create a context whose surface is explicitly configured for alpha-
    /// preserving presentation. Browser WebGPU uses premultiplied alpha;
    /// native surfaces prefer it and can fall back to postmultiplied alpha.
    /// Unlike [`Self::new`], this rejects native surfaces that cannot preserve
    /// alpha instead of silently falling back to an opaque compositor mode.
    pub async fn new_transparent_present(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        limits: wgpu::Limits,
        initial_width: u32,
        initial_height: u32,
    ) -> Result<Self, SurfacePresentationError> {
        Self::new_inner(
            instance,
            surface,
            limits,
            initial_width,
            initial_height,
            true,
        )
        .await
    }

    async fn new_inner(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        limits: wgpu::Limits,
        initial_width: u32,
        initial_height: u32,
        transparent_present: bool,
    ) -> Result<Self, SurfacePresentationError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("darkly-device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits.using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .expect("Failed to create device");

        let (surface_config, presentation_alpha) = configure_surface(
            &surface,
            &adapter,
            &device,
            initial_width,
            initial_height,
            transparent_present,
        )?;

        Ok(GpuContext {
            #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice docs
            gpu: Arc::new(GpuDevice { device, queue }),
            surface: Some(surface),
            surface_config: Some(surface_config),
            presentation_alpha,
            headless_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        })
    }

    /// Build a context that re-uses an existing shared `GpuDevice`. Use this
    /// to attach a second (or Nth) canvas to the same device — e.g. for the
    /// multi-tab editor. Picks the surface format the same way as `new`, but
    /// does not allocate a new device or queue.
    pub async fn new_with_shared_device(
        gpu: Arc<GpuDevice>,
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        initial_width: u32,
        initial_height: u32,
    ) -> Self {
        Self::new_with_shared_device_inner(
            gpu,
            instance,
            surface,
            initial_width,
            initial_height,
            false,
        )
        .await
        .expect("opaque surface configuration must be supported")
    }

    /// Attach a surface to a shared device with alpha-preserving presentation.
    /// This must be chosen before the surface's first render.
    pub async fn new_with_shared_device_transparent_present(
        gpu: Arc<GpuDevice>,
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        initial_width: u32,
        initial_height: u32,
    ) -> Result<Self, SurfacePresentationError> {
        Self::new_with_shared_device_inner(
            gpu,
            instance,
            surface,
            initial_width,
            initial_height,
            true,
        )
        .await
    }

    async fn new_with_shared_device_inner(
        gpu: Arc<GpuDevice>,
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        initial_width: u32,
        initial_height: u32,
        transparent_present: bool,
    ) -> Result<Self, SurfacePresentationError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable GPU adapter");

        let (surface_config, presentation_alpha) = configure_surface(
            &surface,
            &adapter,
            &gpu.device,
            initial_width,
            initial_height,
            transparent_present,
        )?;

        Ok(GpuContext {
            gpu,
            surface: Some(surface),
            surface_config: Some(surface_config),
            presentation_alpha,
            headless_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        })
    }

    /// Create a headless GPU context — no surface or window needed.
    /// Used for testing and headless rendering.
    pub fn new_headless(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        GpuContext {
            #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice docs
            gpu: Arc::new(GpuDevice { device, queue }),
            surface: None,
            surface_config: None,
            presentation_alpha: PresentationAlphaPolicy::Opaque,
            headless_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }

    /// Like `new_headless`, but reuses an existing shared `GpuDevice`. The
    /// shared-device multi-engine integration test uses this to construct two
    /// engines on the same device without a surface.
    pub fn new_headless_shared(gpu: Arc<GpuDevice>) -> Self {
        GpuContext {
            gpu,
            surface: None,
            surface_config: None,
            presentation_alpha: PresentationAlphaPolicy::Opaque,
            headless_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }

    /// Configure presentation into caller-owned textures on this shared device.
    /// Format and alpha policy must match the host's texture compositor.
    /// Only filterable 8-bit RGBA/BGRA outputs are supported by this boundary.
    pub fn new_texture_target(
        gpu: Arc<GpuDevice>,
        format: wgpu::TextureFormat,
        alpha: PresentationAlphaPolicy,
    ) -> Result<Self, TextureTargetFormatError> {
        if !matches!(
            format,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            return Err(TextureTargetFormatError);
        }
        Ok(Self {
            gpu,
            surface: None,
            surface_config: None,
            presentation_alpha: alpha,
            headless_format: format,
        })
    }

    /// Cheap clone of the underlying shared device handle. Use this when
    /// constructing a sibling engine that should render to the same WebGPU
    /// device as this one.
    pub fn shared_device(&self) -> Arc<GpuDevice> {
        Arc::clone(&self.gpu)
    }

    /// The alpha convention selected while this presentation surface was
    /// configured. Ordinary headless contexts use [`PresentationAlphaPolicy::Opaque`];
    /// texture targets use the host's declared policy.
    pub fn presentation_alpha_policy(&self) -> PresentationAlphaPolicy {
        self.presentation_alpha
    }

    /// Create a command encoder, run `f`, and submit the resulting commands.
    ///
    /// Eliminates the 4-line boilerplate pattern that appears ~30 times in the
    /// engine: create encoder → do work → queue.submit.
    pub fn encode(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder)) {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        f(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
    }

    /// Like `encode`, but returns a value from the closure.
    pub fn encode_ret<T>(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder) -> T) -> T {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        let result = f(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        result
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            if let (Some(surface), Some(config)) = (&self.surface, &mut self.surface_config) {
                config.width = width;
                config.height = height;
                surface.configure(&self.gpu.device, config);
            }
        }
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        match &self.surface_config {
            Some(config) => config.format,
            // Ordinary headless contexts default to desktop sRGB; embedded
            // texture targets explicitly select their host's output format.
            None => self.headless_format,
        }
    }

    /// True when running headless (no presentation surface).
    pub fn is_headless(&self) -> bool {
        self.surface.is_none()
    }
}

fn configure_surface(
    surface: &wgpu::Surface<'static>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    width: u32,
    height: u32,
    transparent_present: bool,
) -> Result<(wgpu::SurfaceConfiguration, PresentationAlphaPolicy), SurfacePresentationError> {
    let surface_caps = surface.get_capabilities(adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .find(|f| f.is_srgb())
        .copied()
        .unwrap_or(surface_caps.formats[0]);

    let (alpha_mode, presentation_alpha) =
        select_presentation_alpha(&surface_caps.alpha_modes, transparent_present)?;

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        format: surface_format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode,
        view_formats: vec![],
        // One queued frame, not wgpu's default of two: a paint app wants the
        // freshest stroke on screen, so trade present-queue depth (throughput
        // headroom) for a frame less of input→display latency.
        desired_maximum_frame_latency: 1,
    };
    surface.configure(device, &config);
    Ok((config, presentation_alpha))
}

fn select_presentation_alpha(
    alpha_modes: &[wgpu::CompositeAlphaMode],
    transparent_present: bool,
) -> Result<(wgpu::CompositeAlphaMode, PresentationAlphaPolicy), SurfacePresentationError> {
    if !transparent_present {
        return Ok((alpha_modes[0], PresentationAlphaPolicy::Opaque));
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Browser WebGPU exposes `GPUCanvasConfiguration.alphaMode =
        // "premultiplied"`, but wgpu reports only `Opaque` capabilities for
        // canvas surfaces. Configure its supported browser alpha mode directly.
        return Ok(webgpu_transparent_presentation_alpha());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            return Ok((
                wgpu::CompositeAlphaMode::PreMultiplied,
                PresentationAlphaPolicy::PreMultiplied,
            ));
        }
        if alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
            return Ok((
                wgpu::CompositeAlphaMode::PostMultiplied,
                PresentationAlphaPolicy::PostMultiplied,
            ));
        }

        Err(SurfacePresentationError::TransparentAlphaUnsupported)
    }
}

#[cfg(target_arch = "wasm32")]
fn webgpu_transparent_presentation_alpha() -> (wgpu::CompositeAlphaMode, PresentationAlphaPolicy) {
    (
        wgpu::CompositeAlphaMode::PreMultiplied,
        PresentationAlphaPolicy::PreMultiplied,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_alpha_selection_keeps_browser_and_native_contracts() {
        #[cfg(target_arch = "wasm32")]
        assert_eq!(
            select_presentation_alpha(&[wgpu::CompositeAlphaMode::Opaque], true),
            Ok((
                wgpu::CompositeAlphaMode::PreMultiplied,
                PresentationAlphaPolicy::PreMultiplied,
            ))
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            assert_eq!(
                select_presentation_alpha(
                    &[
                        wgpu::CompositeAlphaMode::PostMultiplied,
                        wgpu::CompositeAlphaMode::PreMultiplied,
                    ],
                    true,
                ),
                Ok((
                    wgpu::CompositeAlphaMode::PreMultiplied,
                    PresentationAlphaPolicy::PreMultiplied,
                ))
            );
            assert_eq!(
                select_presentation_alpha(&[wgpu::CompositeAlphaMode::PostMultiplied], true),
                Ok((
                    wgpu::CompositeAlphaMode::PostMultiplied,
                    PresentationAlphaPolicy::PostMultiplied,
                ))
            );
            assert_eq!(
                select_presentation_alpha(
                    &[
                        wgpu::CompositeAlphaMode::Auto,
                        wgpu::CompositeAlphaMode::Opaque,
                        wgpu::CompositeAlphaMode::Inherit,
                    ],
                    true,
                ),
                Err(SurfacePresentationError::TransparentAlphaUnsupported)
            );
        }
    }
}
