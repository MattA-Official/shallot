# Roadmap

Four phases, as agreed in the planning thread. Each phase ends at a milestone with exit criteria that can be checked. Work packages have IDs so issues, branches and pull requests can refer to them; each links to the section that specifies it.

Phases overlap: tracks that do not depend on each other run in parallel (for example the WGSL compiler and the platform layer in Phase 1, or the engine core and the decoders in Phase 2).

## Phase 1: platform and GPU foundations

Goal: draw a triangle from WGSL source in a window and headless, with no third-party crates.

| ID | Work package | Spec | Depends on |
|---|---|---|---|
| F1 | Workspace: `crates/platform`, `crates/gpu`, `crates/wgsl`; dependency-gate CI job | [architecture.md](architecture.md#workspace), [testing.md](testing.md#dependency-gate) | — |
| F2 | X11 window, event loop, frame callback, headless mode | [platform.md](platform.md) | F1 |
| F3 | Vulkan loader, instance, device, surface, swapchain | [gpu.md](gpu.md#vulkan-backend) | F1 |
| F4 | `shallot-gpu` API types for the triangle path; buffer, shader module, render pipeline, render pass, present; headless texture readback | [gpu.md](gpu.md#milestones) | F3 |
| F5 | WGSL lexer, parser, checker and SPIR-V writer for the triangle subset | [wgsl.md](wgsl.md#delivery-order) | F1 |
| F6 | Triangle example, verified headless (readback) and in a window | — | F2, F4, F5 |
| W1 | `scripts/dump-wgsl.ts`: dump the engine's resolved WGSL corpus with layouts | [wgsl.md](wgsl.md#inputs-we-must-accept) | — |

**Milestone 1:** `cargo run -p shallot-gpu --example triangle` shows a triangle on Linux; the headless test reads back the expected pixels on lavapipe; dependency gate green.

## Phase 2: engine core and bindings boundary

Goal: the engine runs CPU-only compositions and headless GPU compositions; the boundary exists and is versioned.

| ID | Work package | Spec | Depends on |
|---|---|---|---|
| E1 | ECS: entities, components (`component!`, runtime schemas), storage, change tracking | [engine.md](engine.md#components) | — |
| E2 | Queries, resources, scheduler, `Time`, fixed step | [engine.md](engine.md#systems-and-scheduling) | E1 |
| E3 | Snapshot and restore | [engine.md](engine.md#snapshot-and-restore) | E2 |
| E4 | App, plugins, executor, loading lifecycle, `create_app`, `run_app` | [engine.md](engine.md#app-and-plugins) | E2, F2 |
| E5 | `WorldGpu`, frame encoder, `GpuTable`, precompile, readback, shader composer and `gpu_struct!` | [engine.md](engine.md#gpu-context), [wgsl.md](wgsl.md#shader-modules) | E4, F4 |
| E6 | Maths, colour, encode, handles, registry | [engine.md](engine.md#maths) | — |
| C1 | `core`: transform, input (over platform events), mesh, physics and audio components | [module-map.md](module-map.md#core--shallot-core) | E2, E6 |
| G1 | GPU layer: everything `minimal` needs (bind groups, depth, MSAA, compute, indirect, error scopes, async pipelines, readback) | [gpu.md](gpu.md#milestones) | F4 |
| G2 | WGSL: full checker and SPIR-V for the whole corpus, `spirv-val` clean | [wgsl.md](wgsl.md#delivery-order) | F5, W1 |
| D1 | `shallot-formats`: inflate, PNG, JSON, TTF | [assets-ui.md](assets-ui.md#decoders-shallot-formats) | — |
| P1 | Physics: build kernel natively, native thread wait, `native.rs`; coordination audit | [physics-audio.md](physics-audio.md#changes-to-cratesphysics) | — |
| B1 | `shallot-abi`: `boundary!` macro, versioning, errors, worlds, components and columns, entities and queries, systems, reflection document | [bindings.md](bindings.md) | E3 |
| T1 | Port `engine/ecs` and `engine/app` tests (34 files) | [testing.md](testing.md#test-suite-mapping) | E1–E4 |

**Milestone 2:** the ported engine tests pass; a CPU-only composition steps deterministically; a headless GPU composition uploads a `GpuTable` and reads it back; physics kernel tests pass natively with hashes equal to the WASM build at 1, 2 and 8 threads; `shallot-cli api-dump` writes the reflection document; the TypeScript binding thread has reviewed the boundary.

## Phase 3: rendering and the examples

Goal: all 27 examples translated and matching the TypeScript build on Linux, Windows and macOS.

| ID | Work package | Spec | Depends on |
|---|---|---|---|
| R1 | Core rendering: views, cameras, frame uniforms, GlobalTransform history, targets, phases, tonemapping, capture | [rendering.md](rendering.md#core-rendering-port) | E5, G1, G2 |
| R2 | Standard rendering steps 1–6 (materials to forward pass) | [rendering.md](rendering.md#standard-rendering) | R1 |
| R3 | Standard rendering steps 7–10 (shadows, SSAO, fog hook, binding limits) | [rendering.md](rendering.md#standard-rendering) | R2 |
| U1 | `ui` extra and the `shallot_dark` loading screen | [assets-ui.md](assets-ui.md#ui) | R1, D1 |
| X1 | Extras in example order: orbit, profile, text, lines, outline, sky, bloom, effect stack, fog, sprite | [rendering.md](rendering.md#extras) | R2 |
| P2 | Standard physics coordination port, `CharacterPlugin` | [physics-audio.md](physics-audio.md#porting-the-typescript-coordination) | P1, C1 |
| A1 | Audio output on all OSes, Ogg Vorbis and WAV decode, `StandardAudioPlugin` | [physics-audio.md](physics-audio.md#audio), [platform.md](platform.md) | C1 |
| D2 | JPEG decoder | [assets-ui.md](assets-ui.md#decoders-shallot-formats) | — |
| F7 | Win32 platform backend | [platform.md](platform.md#windows) | F2 |
| F8 | Metal backend and MSL writer; Cocoa platform backend | [gpu.md](gpu.md#metal-backend), [wgsl.md](wgsl.md#msl), [platform.md](platform.md#macos) | G1, G2 |
| F9 | Wayland platform backend | [platform.md](platform.md#linux) | F2 |
| Q1 | Screenshot and hash parity harness | [testing.md](testing.md#screenshot-parity) | R1 |
| EX1–EX4 | Example waves 1–4 | [examples.md](examples.md) | as listed per wave |

**Milestone 3a:** Wave 1 examples run on Linux and pass parity.
**Milestone 3b:** Waves 2 and 3 pass parity on Linux and Windows.
**Milestone 3c (the port's "done"):** all 27 examples pass parity on Linux, Windows and macOS, and every row of the status table in [examples.md](examples.md#status) is ticked.

## Phase 4: WASM and upstream tracking

| ID | Work package | Spec | Depends on |
|---|---|---|---|
| U2 | Upstream merge routine, porting ledger and its script | [upstream.md](upstream.md) | — (start as soon as Phase 1 lands) |
| WA1 | Engine crates build for `wasm32-unknown-unknown` in CI | [wasm.md](wasm.md#delivery) | E4 (start early) |
| WA2 | Web GPU backend and JS shim; `minimal` in Chromium | [wasm.md](wasm.md) | WA1, R2 |
| WA3 | Web input, audio, fetch; Waves 1 and 2 in the browser | [wasm.md](wasm.md#delivery) | WA2 |
| WA4 | Threaded physics on the web; all examples in the browser | [wasm.md](wasm.md#delivery) | WA3, P2 |
| TS1 | TypeScript binding over the boundary | separate plan (`docs/bindings/typescript/`) | B1, WA2 |

**Milestone 4:** all examples run from the Rust WASM build in Chromium with parity against the TypeScript build in the same browser; upstream merges are routine and the ledger has no `pending` entries older than one sync.

U2 and WA1 are listed in Phase 4 to match the agreed outline but should start early: the ledger is cheapest to keep from the first merge, and a WASM build in CI from Phase 2 keeps native-only assumptions out of the engine API.
