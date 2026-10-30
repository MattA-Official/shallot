# Module map

Every TypeScript module and where it lands in Rust. Paths on the left are under `src/`; paths on the right are crate-relative module paths. "Port" means a line-for-line behavioural port; "Replace" means the TypeScript leans on a browser or toolchain facility that the Rust side implements differently; "Drop" means it has no Rust counterpart.

Each row is one work item in [roadmap.md](roadmap.md). Tests beside a TypeScript module are ported with it ([testing.md](testing.md#test-suite-mapping)).

## engine → `shallot-engine`

| TypeScript | Rust | Kind | Notes |
|---|---|---|---|
| `engine/ecs/entity.ts` | `ecs::entity` | Port | `Entity(u32)` index plus generation; `EntityRef` packs both, as now |
| `engine/ecs/component.ts`, `columns.ts`, `registry.ts` | `ecs::component`, `ecs::registry` | Port | Field types `f32 i32 u32 u8 u16 f16 vec2 vec3 vec4 entity`; runtime schemas plus a derive-free `component!` macro ([engine.md](engine.md#components)) |
| `engine/ecs/storage.ts` | `ecs::storage` | Port | eid-indexed SoA columns, change ticks, `WorldField` accessors |
| `engine/ecs/query.ts` | `ecs::query` | Port | `and`/`or`/`not` terms, registered queries kept incrementally |
| `engine/ecs/scheduler.ts` | `ecs::scheduler` | Port | Groups `setup fixed simulation draw`, `first/last/terminal/before/after`, fixed 60 Hz accumulator, `Time` |
| `engine/ecs/snapshot.ts` | `ecs::snapshot` | Port | `Recovery`, `WorldSnapshot`, `recover_resource` |
| `engine/ecs/table.ts` | `gpu_table` | Port | `GpuTable` rows described by `shallot_gpu` layouts instead of TypeGPU structs |
| `engine/ecs/world.ts` | `ecs::world` | Port | Ownership replaces `own`/`onDispose` where possible; `on_dispose` kept for external effects |
| `engine/ecs/tick-allocation.entry.ts` | `tests/allocation` | Port | Allocation gate via a counting global allocator (test only) |
| `engine/app/index.ts`, `compose.ts` | `app` | Port | `Plugin` trait, `AppConfig`, `create_app`, `run_app`, `warm_plugins`, default plugin registry |
| `engine/app/frame-delta.ts` | `app::frame` | Port | Delta from presentation timestamps, not `now()` |
| `engine/app` overlay (`mountOverlay`, `OverlayCanvases`) | `shallot-extras::ui` | Replace | No DOM; see [assets-ui.md](assets-ui.md#ui) |
| `engine/runtime/gpu.ts` | `runtime::gpu` | Port | `WorldGpu`, error kinds, binding-limit checks, `precompile`, `precompile_all`, shader hashing. TypeGPU root and `checkTgsl` dropped |
| `engine/runtime/adapter.ts`, `probe.ts`, `dawn-host.ts`, `webgpu.ts` | `runtime::adapter` | Replace | Adapter classification from `shallot-gpu` adapter info; no Dawn/Bun globals |
| `engine/runtime/readback.ts` | `runtime::readback` | Port | Accepted-record readback with tick assignment |
| `engine/runtime/log.ts`, `startup.ts`, `platform.ts`, `viewport.ts` | `runtime::{log, platform, viewport}` | Port | `platform` reads `shallot-platform` instead of `navigator` |
| `engine/runtime/allocation-site.ts` | `runtime::alloc_site` | Port | Diagnostics only |
| `engine/diagnostics/*` | `diagnostics` | Port | User-timing spans become an in-process span recorder |
| `engine/utils/math.ts` | `math` | Port | `Vec3`, `Quat`, `Mat4` as plain `[f32; N]` newtypes plus the same free functions (`perspective`, `look_at_rotation` …). f64 where TS keeps f64 |
| `engine/utils/color.ts` | `color` | Port | |
| `engine/utils/encode.ts` | `encode` + `shaders/engine/encode.wgsl` | Port | CPU packers in Rust; GPU halves become a WGSL module |
| `engine/utils/handles.ts`, `registry.ts` | `util::{handles, registry}` | Port | |
| `engine/utils/tgsl.ts` | — | Drop | TypeGPU helpers. `chunk` becomes the WGSL module system ([wgsl.md](wgsl.md#shader-modules)) |

## core → `shallot-core`

| TypeScript | Rust | Kind | Notes |
|---|---|---|---|
| `core/transform/*` | `transform` | Port | `Transform`, `GlobalTransform`, `TransformPlugin`, placement boundaries |
| `core/input/index.ts` | `input` | Port + Replace | `Devices`, `Keys`, `Pointer`, `Touch`, `PointerLock`, the test drivers (`press_key` …) port as is. `createBrowserInputPlugin` and `InputHost` are reimplemented over `shallot-platform` events. Key codes keep the DOM `KeyboardEvent.code` names |
| `core/mesh/*` | `mesh` | Port | Mesh data, GPU storage, primitives, `MeshInstance` |
| `core/physics/*` | `physics` | Port | `Body`, `Shape`, joints, geometry registries, `PhysicsPlugin` |
| `core/audio/index.ts` | `audio` | Port | `Sound`, `Listener`, clip registry, `play`, `AudioOutput` |
| `core/rendering/camera*.ts`, `view.ts`, `frame*.ts`, `frustum.ts`, `xform.ts`, `viewport-to-world.ts`, `anchor.ts`, `billboard.ts`, `render.ts` | `rendering::{camera, view, frame, frustum}` | Port | |
| `core/rendering/global-transform.ts` | `rendering::global_transform` | Port | GPU history and interpolation table |
| `core/rendering/phases.ts`, `targets.ts`, `substrate.ts`, `upscaling.ts` | `rendering::{phases, targets, substrate, upscaling}` | Port | `CorePipelinePlugin`, depth prepass lane, render phases |
| `core/rendering/lighting.ts`, `shadow-filtering.ts`, `depth-bias.ts` | `rendering::lighting` | Port | |
| `core/rendering/tonemap*.ts`, `color-grading.ts`, `agx-lut.ts`, `tony-lut.ts` | `rendering::tonemapping` | Port | LUT bytes embedded with `include_bytes!` |
| `core/rendering/image.ts`, `image-sampling.ts` | `rendering::image` | Replace | `createImageBitmap` becomes `shallot-formats` PNG/JPEG decode |
| `core/rendering/capture.ts` | `rendering::capture` | Port | `capture_frame`, `capture_texture`, `probe_texture` over `shallot-gpu` readback |
| `core/rendering/bevy/*.wesl`, `blender/*` | reference only | — | Kept for provenance; their WGSL already lives in the TS modules |

## standard → `shallot-standard`

| TypeScript | Rust | Kind | Notes |
|---|---|---|---|
| `standard/index.ts` | `shallot::default_plugins()` | Port | Default plugin set |
| `standard/rendering/*` (36 files) | `rendering` | Port | Clustered forward pipeline: surfaces, materials, `material_type`, light packing, culling, indirect draws, backgrounds, shadows, SSAO, preprocess, regather, atlas, bundles ([rendering.md](rendering.md#standard-rendering)) |
| `standard/physics/{runtime, index, authoring, body-*, shape-*, joints, events, contact, character, geometry-builds, marshal*, mesh, rotation, field-candidates, creation-order, eid-column, world-definition}.ts` | `physics` | Port | The ECS ↔ kernel bridge and public API (`apply_force`, `set_transform` …) |
| `standard/physics/kernel/*columns.ts`, `*records.ts`, `views.ts`, `kernel.ts`, `pool.ts` | — | Replace | Typed-array views over WASM memory and the worker pool. Natively the Rust bridge calls `shallot-physics` directly ([physics-audio.md](physics-audio.md)) |
| `standard/physics/{collision, solver, shapes, world, common, api, tree}/*` | `physics::{query, shapes, api}` and kernel | Port / Drop | Most are TS mirrors of kernel layout or thin wrappers; each is either replaced by a direct kernel call or ported where it holds logic. The audit list is in [physics-audio.md](physics-audio.md#coordination-audit) |
| `standard/physics/oracle/*` | `crates/physics` tests | Port | Box3D gold vectors stay the authority |
| `standard/audio/runtime.ts`, `worklet.ts` | `audio` | Replace | AudioWorklet becomes an OS audio callback running `shallot-audio` |
| `standard/loading/*` | `loading` | Replace | `shallot_dark` splash drawn with the engine's own 2D/text path, not DOM |

## extras → `shallot-extras`

| TypeScript | Rust (feature) | Kind | Notes |
|---|---|---|---|
| `extras/bloom` | `bloom` | Port | Bevy-derived shaders keep their licence header |
| `extras/effect-stack` | `effect_stack` | Port | Includes vignette |
| `extras/fog` | `fog` | Port | Volumetric fog, temporal |
| `extras/lines` | `lines` | Port | |
| `extras/orbit` | `orbit` | Port | |
| `extras/outline` | `outline` | Port | |
| `extras/profile` | `profile` | Port + Replace | GPU timestamp queries port; the DOM overlay (`DiagnosticsOverlayPlugin`) is drawn with `ui` |
| `extras/sky` | `sky` | Port | |
| `extras/sprite` | `sprite` | Port | |
| `extras/text` | `text` | Port | TTF parsing and SDF generation are already hand-written TS; they move to `shallot-formats::ttf` and `text::sdf` |
| — | `ui` | New | Immediate-mode HUD widgets replacing Svelte/DOM panels ([assets-ui.md](assets-ui.md#ui)) |

## Tooling

| TypeScript | Rust | Kind | Notes |
|---|---|---|---|
| `cli/index.ts`, `create.ts`, `build.ts`, `preview.ts` | `shallot-cli` | Replace | `shallot new`, `run`, `build`, `package` wrap `cargo`; no Vite |
| `cli/publish*.ts` | `shallot-cli publish` | Port later | itch.io and Cloudflare upload over raw HTTPS is not possible with `std` alone (no TLS); the CLI shells out to the platform's `curl` or butler. Recorded in [risks.md](risks.md) |
| `native/*` | — | Drop | The wry/CEF shell is what the port replaces |
| `project/*` (vite, bun, testing) | `shallot::testing`, `shallot-cli` | Replace | Shared timeouts and test helpers |
| `scripts/*` | `xtask`-style binaries under `crates/cli` or shell scripts | Replace as needed | Only the checks that still apply: dependency gate, imports, parity |

## Deliberate differences

Each difference is chosen because Rust or the absence of a browser makes the TypeScript shape worse, not for taste. Add to this list rather than diverging silently.

1. **No TypeGPU.** Shaders are WGSL source files composed by a small module system; GPU struct layouts are declared once in Rust and emitted into WGSL, so CPU and GPU agree by construction ([wgsl.md](wgsl.md#shader-modules)).
2. **Typed handles.** Components, resources and systems are Rust values with types (`world.storage::<Transform>()`), not looked up by string. Names stay for diagnostics, snapshots and the bindings reflection.
3. **Ownership instead of manual disposal.** GPU objects drop when their owner drops. `world.own` survives only for objects whose lifetime the world must extend.
4. **Explicit asynchrony.** `initialize` and `warm` return futures driven by the engine's executor; everything else is synchronous.
5. **No DOM.** HUDs, the loading screen and the diagnostics overlay render through the engine. `AppConfig::ui` takes a closure that draws with `shallot::ui` each frame.
6. **Frame clock.** `run_app` is clocked by the platform's present/vsync timestamps, the native analogue of `requestAnimationFrame` timestamps. The six-frames-pending rule becomes a swapchain frame-latency limit.
7. **Key and button names** stay as the DOM `code` strings (`"KeyW"`, `"Space"`) so tests and saved bindings carry over, exposed through a `KeyCode` enum.
