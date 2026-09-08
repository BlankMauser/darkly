# Embedded Texture Presenter

**Status:** implemented; independent findings incorporated
**Source examined:** `2bd78bbc48241962fd6fb63d5de5048c7aacb52a`

## Independent Review

**Verdict: revise.** The proposed generic texture boundary is the right scope,
and it can reuse the production present, veil, and overlay path without a
readback or platform interop layer. The plan needs the following constraints
made explicit before implementation.

- **Put immutable presentation configuration in one construction owner.**
  `GpuContext::new_headless` currently fixes alpha to `Opaque` and
  `surface_format()` falls back to `Bgra8UnormSrgb`
  (`crates/darkly/src/gpu/context.rs:257-334`). `Compositor::new` compiles its
  present pipeline from that format and passes it to both `VeilChain` and
  `ToolOverlay` (`crates/darkly/src/gpu/compositor.rs:883-1170`). Add one
  explicit headless-presentation context/config constructor carrying the
  format and existing `PresentationAlphaPolicy`, then have compositor
  construction derive from it. Do not add a second, bridge-owned format or
  alpha field. The compositor may retain its immutable expected format solely
  for target validation because its pipelines already embody that dependency.

- **Validate before every stateful render step, then force only the final
  presentation.** wgpu 29 exposes a texture's size, dimension, mip count,
  sample count, format, and usage (`wgpu-29.0.4/src/api/texture.rs:117-178`),
  so the public entry point can return typed errors before changing dirty state
  or allocating veil/overlay resources. After validation it must sync the veil
  viewport from the target extent, run dirty-gated `render_offscreen`, and
  encode the final pass even when `has_pending_work` is false. That preserves
  rotation semantics while retaining offscreen dirty gating
  (`crates/darkly/src/gpu/compositor.rs:4742-4840`). Clear present flags only
  after the command buffer is submitted. A cross-device texture remains a
  wgpu host-contract validation failure, not a portable typed comparison.

- **Treat output extent as the caller's current viewport.** The normal engine
  resize path returns early for a headless context and is the current owner of
  `VeilChain::resize` (`crates/darkly/src/engine/rendering.rs:772-782`). The
  external presenter therefore has to resize the veil chain itself after a
  valid target is supplied. The caller must set the existing view-transform
  screen dimensions to that same extent before rendering; the cached matrix
  uses those dimensions (`crates/darkly/src/engine/rendering.rs:78-142`).

- **Keep the test executable through the public API.** A valid external
  target needs `RENDER_ATTACHMENT | COPY_SRC`: snapshot overlays copy the
  completed target before drawing (`crates/darkly/src/gpu/overlay.rs:705-755`).
  Test output textures and test-only readback can prove rotation, an unchanged
  second target, resize, a format mismatch, and missing usage. A real wgpu
  texture cannot normally be constructed with a zero extent, so do not promise
  that as a public-API GPU test; cover it only through a descriptor-level
  validation helper if one is deliberately introduced, otherwise retain the
  defensive guard and document it. Native export/acquire synchronization
  remains the caller's responsibility; this generic API submits on Darkly's
  queue and adds no platform synchronization objects.

## Problem and boundary

Review resolution: `GpuContext` owns the configured headless format and alpha;
the existing engine constructor passes that alpha into its pipelines. Public
target validation runs before the common frame prelude. The external compositor
resizes its veil viewport and forces only the final pass. Tests omit impossible
zero-sized wgpu texture construction, retaining a defensive extent guard.
The host owns cross-API GPU fences and must not reuse an output until readers
finish; submitting a Darkly frame does not itself transfer GPU ownership.
Final code review also required a filterable presentation format: the constructor
returns a typed error for formats outside RGBA8/BGRA8 unorm (linear or sRGB),
before constructing pipelines. A real two-layer texture exercises shape rejection.

`Compositor::render` owns surface acquisition, final command encoding, queue
submission, and `SurfaceTexture::present`. Consequently a headless
`DarklyEngine` performs polling but cannot emit its normal workspace image to a
native host texture. Flutter integration needs that image—view transform,
workspace background, veils, and tool overlays—written directly to a
caller-owned `wgpu::Texture`, with no production GPU-to-CPU transfer.

This is a generic Darkly engine API only. It does not add Flutter, DXGI,
external-memory import, or a platform texture wrapper.

## Contract

Add an explicit external-presentation construction option containing the output
`wgpu::TextureFormat` and `PresentationAlphaPolicy`, and a public
`DarklyEngine::render_to_texture(time_secs, &wgpu::Texture) -> Result<bool,
ExternalTexturePresentError>`-style API. The exact names may follow local Rust
style, but the format and alpha policy are immutable engine presentation
configuration: the present, veil-blit, and overlay pipelines are compiled for
that format and cannot safely infer it per frame.

The caller owns texture creation, rotation/acquisition, and release. It must
create the texture on the engine's device and keep it valid through the call;
Darkly borrows it, creates only a transient view, submits GPU commands, and
never caches, presents, reads back, or destroys it. The method validates before
encoding: a non-zero 2D, single-sample, single-layer output at mip 0; the
configured format; and `RENDER_ATTACHMENT | COPY_SRC` usage. `COPY_SRC` is
required because snapshot-sampling overlays copy the finished target into an
internal GPU texture; it is not CPU readback. Return typed errors without
mutating dirty state for invalid targets. Device identity is a wgpu host
contract (wgpu validation reports a cross-device resource), since it is not a
portable texture property to compare.

Each successful external call forces a final presentation even when the
document is clean. A host may rotate to a different texture with the same
extent; that texture still needs the current image. Normal dirty gates continue
to skip offscreen recomposition. On a valid extent change, update the shared
presentation viewport/veil resources and mark a present; the host must update
the existing view-transform screen dimensions as it does for a surface resize.

## Implementation

1. In `crates/darkly/src/engine/mod.rs` and `gpu/context.rs`, add the small
   presentation configuration path needed to build a headless engine for a
   declared texture format. Thread it into the existing compositor constructor;
   retain surface constructors and their alpha behavior. Store/expose the
   compositor's expected output format for validation rather than duplicating a
   format field in the host bridge.

2. In `crates/darkly/src/gpu/compositor.rs`, split the current `render` at the
   surface boundary. Keep surface acquisition/reconfiguration and
   `output.present()` in the surface wrapper. Move the shared work into one
   private target encoder that receives the destination texture and view plus a
   `force_present` decision: veil-scale synchronization, dirty-gated
   `render_offscreen`, overlay preparation in plane coordinates,
   `present_and_veils`, snapshot-overlay GPU copy/pass, submission, and dirty
   flag completion. The external wrapper validates, resizes the veil viewport
   from the texture extent, and calls that helper with `force_present = true`.
   Do not copy the renderer and do not repurpose the test-only
   `test_present_*` readback helpers.

3. In `crates/darkly/src/engine/rendering.rs`, factor the common frame prelude
   used by `render` and `render_to_texture`: pending-operation polling,
   recording tick, preview pump, thumbnail/readback scheduling, histogram
   pump, animation update, phase accounting, and `frame_needs_more`. The
   external path must run this prelude even for a headless context; only the
   old surface path retains its no-surface early return. Return the same
   reschedule signal after a successful external render so async work and
   animated veils/overlays continue to advance.

4. Add focused API docs describing the texture/device/usage/alpha/resize
   contract. Keep the Flutter adapter responsible only for supplying the
   acquired wgpu texture and current view dimensions.

## Validation

Add a native GPU integration test (for example
`crates/darkly/tests/embedded_texture_presenter.rs`) that constructs a headless
engine with an explicit RGBA8 target format, paints known content, applies a
non-identity view transform and non-zero canvas origin, enables a veil, and
uses both solid and snapshot-sampling overlay primitives. Render through the
public API into texture A, read it only through `gpu::test_utils` after the
call, then render unchanged state into texture B and verify B receives the
same presentation. Also cover a changed output extent (veil resources resize)
and typed rejection of wrong format, missing usage, and array shape. Zero extent
has a defensive guard but cannot be constructed as a valid wgpu texture. This
exercises the production GPU path; blocking readback remains test-only.

Run the targeted native test with `--features darkly/testing`, then the normal
native formatter/clippy suite and the existing wasm32 compile/clippy gate. The
WASM build is compile coverage for the shared core; no external-texture host is
added there.

## Estimate, risks, and open questions

Implementation verification: the public-API GPU test passes on Windows, including
non-zero canvas origin, transformed output, both overlay passes, clean target
rotation, veil resize, invalid target shape/usage/format, unsupported construction
format, and alpha propagation. DoughDraw's native check and WASM release check/
clippy pass. This does not verify a Flutter native sharing adapter or the full
upstream frontend/CI matrix. The Windows adapter remains separate work.

The broader native workspace run stopped at a library test: 627 passed, one
ignored, and `format::tests::embedded_font_renders_identically_after_reload`
failed its exact-pixel comparison. An isolated unchanged `2bd78bb` checkout
passed twice and failed once, confirming an intermittent baseline failure.
Do not report a green full suite; investigate font reload reproducibility
separately before relying on exact embedded-font pixel parity.

Estimated additions: **120–165 production LOC**, **105–150 test LOC**, and
**75–95 docs LOC** (this plan/API docs), **300–410 total**.

The main risk is a format mismatch: three existing final-stage pipeline owners
(`present_pipeline`, `VeilChain`, and `ToolOverlay`) are format-specialized, so
the constructor configuration and target validation must remain one contract.
Target rotation must never clear dirty state before an actual encode; the forced
final pass addresses that. A future API that accepts imported platform handles
belongs above this boundary and must establish the same-device contract before
calling this API.
