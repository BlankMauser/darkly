//! Translate DoughDraw's deterministic brush program into a Darkly brush graph.
//!
//! This adapter accepts only capabilities Darkly can reproduce without changing
//! DoughDraw replay pixels. Unsupported programs stay on DoughDraw's CPU path.

use serde::Deserialize;
use thiserror::Error;

use crate::brush::{self, input_value::InputValue, wire::BrushWireType};
use crate::nodegraph::Graph;

pub const DOUGHDRAW_BRUSH_PROGRAM_FORMAT_V1: &str = "doughdraw.brush-execution-program.v1";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct DoughDrawBrushProgramV1 {
    pub format: String,
    pub schema_version: u32,
    pub source_brush_id: String,
    pub tip: u8,
    pub dynamics: u8,
    pub size_scale_q16: u32,
    pub spacing_scale_q16: u32,
    pub opacity_u16: u32,
    pub grain_scale_q16: u32,
    pub wet_mix_u16: u32,
    pub pressure_opacity_u16: u32,
    pub bitmap_tip_hash_hex: Option<String>,
}

/// One resolved DoughDraw round-brush dab.
///
/// Coordinates and radius use DoughDraw's Q8 fixed-point units so replay
/// sends the same values to every renderer. This is deliberately narrower
/// than a generic graph input: `compile_brush_program_v1` remains the gate
/// for the only brush shape this adapter can reproduce exactly today.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct DoughDrawCanonicalRoundDabV1 {
    pub center_x_q8: i32,
    pub center_y_q8: i32,
    pub radius_q8: u16,
    pub opacity_u16: u16,
    pub color_rgba8: [u8; 4],
}

impl DoughDrawCanonicalRoundDabV1 {
    pub fn radius_px(self) -> f32 {
        self.radius_q8 as f32 / 256.0
    }

    pub fn center_px(self) -> [f32; 2] {
        [
            self.center_x_q8 as f32 / 256.0,
            self.center_y_q8 as f32 / 256.0,
        ]
    }

    pub fn color(self) -> [f32; 4] {
        let opacity = self.opacity_u16 as f32 / u16::MAX as f32;
        [
            self.color_rgba8[0] as f32 / u8::MAX as f32,
            self.color_rgba8[1] as f32 / u8::MAX as f32,
            self.color_rgba8[2] as f32 / u8::MAX as f32,
            (self.color_rgba8[3] as f32 / u8::MAX as f32) * opacity,
        ]
    }

    pub fn validate(self) -> Result<(), DoughDrawBrushProgramError> {
        if self.radius_q8 == 0 {
            return Err(DoughDrawBrushProgramError::Invalid("canonical dab radius"));
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DoughDrawBrushProgramError {
    #[error("invalid DoughDraw brush program: {0}")]
    Invalid(&'static str),
    #[error("unsupported DoughDraw brush program: {0}")]
    Unsupported(&'static str),
}

pub fn compile_brush_program_v1(
    program: &DoughDrawBrushProgramV1,
) -> Result<Graph<BrushWireType>, DoughDrawBrushProgramError> {
    validate_program(program)?;

    // DoughDraw's canonical round brush spaces dabs at diameter / 8. Darkly
    // expresses spacing as a diameter ratio, so 0.125 is the exact unit scale.
    let spacing = 0.125_f32 * (program.spacing_scale_q16 as f32 / 65_536.0);
    let mut graph = brush::default_graph_with_paint_terminal(brush::nodes::paint_precise::TYPE_ID);
    let settings = graph
        .nodes()
        .values()
        .find(|node| node.type_id == "brush_settings")
        .map(|node| node.id.clone())
        .ok_or(DoughDrawBrushProgramError::Invalid(
            "Darkly default graph has no brush settings",
        ))?;
    graph
        .set_port_default(&settings, "spacing", spacing)
        .map_err(|_| DoughDrawBrushProgramError::Invalid("Darkly spacing input rejected"))?;
    let circle = graph
        .nodes()
        .values()
        .find(|node| node.type_id == "circle")
        .map(|node| node.id.clone())
        .ok_or(DoughDrawBrushProgramError::Invalid(
            "Darkly default graph has no circle",
        ))?;
    graph
        .set_port_value(&circle, "coverage", InputValue::Int(1))
        .map_err(|_| DoughDrawBrushProgramError::Invalid("Darkly coverage input rejected"))?;
    brush::compile_graph(&graph)
        .map_err(|_| DoughDrawBrushProgramError::Invalid("Darkly graph did not compile"))?;
    Ok(graph)
}

fn validate_program(program: &DoughDrawBrushProgramV1) -> Result<(), DoughDrawBrushProgramError> {
    if program.format != DOUGHDRAW_BRUSH_PROGRAM_FORMAT_V1 || program.schema_version != 1 {
        return Err(DoughDrawBrushProgramError::Invalid("format or version"));
    }
    if program.source_brush_id.is_empty() {
        return Err(DoughDrawBrushProgramError::Invalid("source brush id"));
    }
    if program.bitmap_tip_hash_hex.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(DoughDrawBrushProgramError::Invalid("bitmap tip hash"));
    }
    if program.tip != 0 {
        return Err(DoughDrawBrushProgramError::Unsupported("tip"));
    }
    if program.dynamics != 0 {
        return Err(DoughDrawBrushProgramError::Unsupported("dynamics"));
    }
    if program.size_scale_q16 != 65_536 {
        return Err(DoughDrawBrushProgramError::Unsupported("size scale"));
    }
    if program.opacity_u16 != 65_535 {
        return Err(DoughDrawBrushProgramError::Unsupported("opacity"));
    }
    if program.grain_scale_q16 != 0
        || program.wet_mix_u16 != 0
        || program.pressure_opacity_u16 != 0
        || program.bitmap_tip_hash_hex.is_some()
    {
        return Err(DoughDrawBrushProgramError::Unsupported(
            "grain, wet mix, pressure opacity, or bitmap tip",
        ));
    }
    if program.spacing_scale_q16 == 0 || program.spacing_scale_q16 > 262_144 {
        return Err(DoughDrawBrushProgramError::Invalid("spacing scale"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_round() -> DoughDrawBrushProgramV1 {
        DoughDrawBrushProgramV1 {
            format: DOUGHDRAW_BRUSH_PROGRAM_FORMAT_V1.into(),
            schema_version: 1,
            source_brush_id: "round-v1".into(),
            tip: 0,
            dynamics: 0,
            size_scale_q16: 65_536,
            spacing_scale_q16: 65_536,
            opacity_u16: 65_535,
            grain_scale_q16: 0,
            wet_mix_u16: 0,
            pressure_opacity_u16: 0,
            bitmap_tip_hash_hex: None,
        }
    }

    #[test]
    fn compiles_the_supported_round_program() {
        assert_eq!(
            brush::compile_graph(&brush::default_graph())
                .unwrap()
                .scratch_format(),
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let graph = compile_brush_program_v1(&unit_round()).unwrap();
        let circle = graph
            .nodes()
            .values()
            .find(|node| node.type_id == "circle")
            .unwrap();
        assert!(circle
            .ports
            .iter()
            .any(|port| { port.name == "coverage" && port.value == InputValue::Int(1) }));
        let runner = brush::compile_graph(&graph).unwrap();
        assert_eq!(runner.scratch_format(), wgpu::TextureFormat::Rgba16Float);
        assert!(runner.compiled_brush().unwrap().brush_extent_extra_px > 0.5);
    }

    #[test]
    fn leaves_wet_mix_on_doughdraws_exact_fallback() {
        let mut program = unit_round();
        program.wet_mix_u16 = 1;
        assert!(matches!(
            compile_brush_program_v1(&program),
            Err(DoughDrawBrushProgramError::Unsupported(_))
        ));
    }

    #[test]
    fn canonical_round_dab_preserves_fixed_point_inputs() {
        let dab = DoughDrawCanonicalRoundDabV1 {
            center_x_q8: -384,
            center_y_q8: 640,
            radius_q8: 1_024,
            opacity_u16: 32_768,
            color_rgba8: [23, 44, 63, 255],
        };

        assert_eq!(dab.center_px(), [-1.5, 2.5]);
        assert_eq!(dab.radius_px(), 4.0);
        assert_eq!(dab.color()[3], 32_768.0 / 65_535.0);
        assert!(dab.validate().is_ok());
        assert!(matches!(
            DoughDrawCanonicalRoundDabV1 {
                radius_q8: 0,
                ..dab
            }
            .validate(),
            Err(DoughDrawBrushProgramError::Invalid("canonical dab radius"))
        ));
    }
}
