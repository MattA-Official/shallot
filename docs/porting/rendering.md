# Rendering

Rendering is the largest body of code to port after physics (core 5,800 lines, standard 15,600, extras 11,800) and holds nearly all of the shaders. The Rust port keeps its structure, ownership rules and frame ordering exactly, because screenshot parity with the TypeScript build is the acceptance test.

## Ownership (unchanged)

- `RenderingPlugin` (`shallot_core::rendering`) owns cameras, shared views, surface binding, projection, view and frame uniforms, capture, light components and the frame-ordering anchors. It knows no meshes or materials.
- `CorePipelinePlugin` owns each view's clear, depth and multisampled colour targets, resolve, the opt-in depth prepass lane, `RenderPhases` and tonemapping.
- `shallot_core::mesh` owns mesh data, GPU storage, primitives and `MeshInstance`.
- `StandardRenderingPlugin` (`shallot_standard::rendering`) is the one clustered forward pipeline: surfaces, materials, light packing, culling, indirect draws, backgrounds, shadows and SSAO. It records into core's phases.
- Effects live in extras. Custom materials and backgrounds extend standard through `material_type` with `MaterialPlugin` and `register_background`.
- Scene effects run before `OverlaySystem`, overlays between it and `PresentationSystem`, presentation after it.

## Core rendering port

| Part | Notes |
|---|---|
| Views and cameras | `view.ts` grows view state without recompiling pipelines up to `MAX_VIEWS`; same constant and layout |
| Frame uniforms | `frame.ts` (32-byte UBO), `frame-state.ts`; declared with `gpu_struct!` |
| GlobalTransform GPU history | `core/rendering/global-transform.ts`: placement capture after each fixed tick's boundary, history copies and interpolation through the world's frame encoder |
| Targets and phases | `targets.ts`, `phases.ts`, `substrate.ts`; the `DepthPrepassRequests` contract |
| Tonemapping and grading | `tonemap*.ts`, `color-grading.ts`; AgX and Tony McMapface LUTs embedded with `include_bytes!` from the bytes the TypeScript embeds as base64 |
| Upscaling and pixel ratio | `upscaling.ts`; nearest-neighbour upscale for pixel ratios below 1 (`low-res`) |
| Images | `image.ts` decodes through `shallot-formats` and uploads with `write_texture`, replacing `createImageBitmap` and `copyExternalImageToTexture`; mip generation ports its compute path |
| Capture | `capture_frame`, `capture_texture`, `probe_texture` read back through `shallot-gpu`; these power screenshot parity |
| Presentation | surface format chosen as the TypeScript does (`bgra8unorm` or `rgba8unorm` per platform preference), alpha mode `opaque` |

## Standard rendering

Port order follows the dependency chain inside the module, each step landing with its tests:

1. `contract.ts`, `registry.ts`, `material-data.ts`, `material.ts`, `standard-material.ts`: material records, the `Materials` resource and handles.
2. `engine.ts` (substrate): the shared group-0 layout, `lit`, `lit_pbr`, `cluster_of` WGSL modules.
3. `pack.ts`, `producer-pack.ts`, `producer-table.ts`, `mesh-attributes.ts`, `mesh-count.ts`, `mesh-render.ts`: instance packing and producer tables.
4. `preprocess.ts`, `regather.ts`, `indirect.ts`, `bundle.ts`, `bound.ts`: GPU culling, compaction and indirect draw arguments, render bundles.
5. `cluster.ts`, `lighting.ts`: clustered light assignment.
6. `forward.ts`, `pipelines.ts`, `material-type.ts`, `shade.ts`: the forward pass, pipeline families and the material-type extension point.
7. `shadows.ts`, `shadow-layout.ts`, `atlas.ts`: cascaded directional and point shadows.
8. `ssao.ts`, `ssao-shaders.ts`, `prepass-textures.ts`: SSAO and prepass textures.
9. `fog.ts` (standard's fog hook), backgrounds.
10. `binding-limits.ts`: storage-binding budget checks under the 8-per-stage floor.

The existing tests that pin exact pipeline counts ("a built-in composition change adds an exact native-pipeline count") port as is and catch accidental pipeline-family changes.

## Extras

Ported in the order the examples need them (see [examples.md](examples.md)): `orbit` → `profile` (with the UI overlay) → `text` → `lines` → `outline` → `sky` → `bloom` → `effect_stack` → `fog` → `sprite`.

## Shader porting procedure

For each module above:

1. Take its resolved WGSL from the corpus (`crates/wgsl/corpus`, see [wgsl.md](wgsl.md#inputs-we-must-accept)).
2. Split into `.wgsl` files per TypeScript function group, with imports and `gpu_struct!` layouts.
3. Port the module's GPU tests (`*.gpu.test.ts`, `*.node.ts`) to Rust readback tests with the TypeScript tolerances.
4. Run the screenshot fixtures that touch it (`*.fixture.ts` scenes such as `receding-plane`, `points`, `tonemapping-bevy`, `outline/reference`) through both builds and diff.

Shader text taken from Bevy (`bloom`, `effect-stack`, tonemapping) keeps its provenance comment and `MIT OR Apache-2.0` notice.

## Floor and limits

The engine stays within WebGPU default limits, including 8 storage buffers per stage, and requests devices with only the features the composition declares. Natively our validator enforces the same limits regardless of what the Vulkan or Metal device could do, so a pipeline that would fail in a browser fails natively too. A "floor device" test mode requests no optional features at all, as `project/floor.ts` does.
