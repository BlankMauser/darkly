use serde_json::json;

use crate::engine::protocol::{bad_payload, decode, RequestRegistration, Response};
use crate::integrations::doughdraw::{
    compile_brush_program_v1, DoughDrawBrushProgramError, DoughDrawBrushProgramV1,
    DoughDrawCanonicalRoundDabV1,
};

pub fn registrations() -> Vec<RequestRegistration> {
    vec![
        RequestRegistration::new(
        "doughdraw_brush_program_install",
        |engine, payload, _bytes| {
            let program: DoughDrawBrushProgramV1 = decode(payload)?;
            match compile_brush_program_v1(&program) {
                Ok(graph) => {
                    let json = serde_json::to_string(&graph).map_err(bad_payload)?;
                    engine.set_brush_graph(&json).map_err(bad_payload)?;
                    Ok(Response::json(json!({
                        "kind": "installed",
                        "backendId": "darkly",
                    })))
                }
                Err(DoughDrawBrushProgramError::Unsupported(reason)) => {
                    Ok(Response::json(json!({
                        "kind": "unsupported",
                        "backendId": "darkly",
                        "reason": reason,
                    })))
                }
                Err(error) => Err(bad_payload(error)),
            }
        },
    )
    .send()
    .req::<DoughDrawBrushProgramV1>()
    .resp_literal(
        "{ kind: 'installed'; backendId: 'darkly' } | { kind: 'unsupported'; backendId: 'darkly'; reason: string }",
    ),
        RequestRegistration::new("doughdraw_canonical_round_dab", |engine, payload, _bytes| {
            let dab: DoughDrawCanonicalRoundDabV1 = decode(payload)?;
            dab.validate().map_err(bad_payload)?;
            match engine.doughdraw_canonical_round_dab(dab) {
                Ok(()) => Ok(Response::json(json!({
                    "kind": "applied",
                    "backendId": "darkly",
                }))),
                Err(reason) => Ok(Response::json(json!({
                    "kind": "rejected",
                    "backendId": "darkly",
                    "reason": reason,
                }))),
            }
        })
        .send()
        .req::<DoughDrawCanonicalRoundDabV1>()
        .resp_literal(
            "{ kind: 'applied'; backendId: 'darkly' } | { kind: 'rejected'; backendId: 'darkly'; reason: string }",
        ),
    ]
}
