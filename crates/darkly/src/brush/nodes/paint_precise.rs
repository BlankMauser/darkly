//! High-precision variant of the compiled paint terminal.

use crate::brush::node::BrushNodeRegistration;

pub const TYPE_ID: &str = "paint_precise";

pub fn register() -> BrushNodeRegistration {
    let mut registration = super::paint::register_with_format(
        TYPE_ID,
        "Precise Paint",
        wgpu::TextureFormat::Rgba16Float,
        super::paint::max_coverage_evaluator,
    );
    registration.node.description =
        "High-precision maximum-coverage paint for deterministic external renderers.";
    registration
}
