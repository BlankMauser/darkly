//! High-precision variant of the compiled paint terminal.

use crate::brush::node::BrushNodeRegistration;

pub const TYPE_ID: &str = "paint_precise";

pub fn register() -> BrushNodeRegistration {
    super::paint::register_with_format(TYPE_ID, "Precise Paint", wgpu::TextureFormat::Rgba16Float)
}
