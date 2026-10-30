# Architecture

## Layering

The Rust port keeps the TypeScript layering from `CONTRIBUTING.md`: dependencies point inward, and outer layers make more of the choices a game can replace. Two new layers sit underneath, because the browser no longer provides them.

```
            examples/<name>/rs        games and the example translations
                    │
   shallot  ────────┤                 facade: prelude, default plugins, run_app
                    │
   shallot-extras ──┤                 bloom, effect stack, fog, lines, orbit, outline, profile, sky, sprite, text, ui
   shallot-standard ┤                 standard rendering, physics, audio, loading screen
   shallot-core ────┤                 transform, input, mesh, rendering, physics and audio components
   shallot-engine ──┤                 ECS, world, scheduler, app and plugins, GPU context and tables, maths
                    │
   shallot-gpu ─────┼─ shallot-wgsl   WebGPU-shaped API over Vulkan, Metal or browser WebGPU; WGSL compiler
   shallot-platform ┤                 windows, events, input devices, timing, threads, files, audio output
   shallot-formats ─┘                 PNG, JPEG, Ogg Vorbis, TTF, JSON decoders (pure Rust, no OS)

   shallot-physics, shallot-audio     existing kernels, reused (crates/physics, crates/audio)
   shallot-abi                        the bindings boundary, over shallot (see bindings.md)
```

Rules:

- A crate depends only on crates below it in the diagram. `shallot-engine` names no placement, rendering or physics code, as in the TypeScript engine.
- `shallot-platform` and `shallot-gpu` are the only crates that declare `extern` OS functions. Everything above them is safe Rust apart from the kernels and `shallot-abi`.
- `shallot-gpu` does not know the ECS. `shallot-engine` owns the per-world GPU context (`WorldGpu`) on top of it, as `engine/runtime/gpu.ts` does today.
- `shallot-formats` is pure computation on byte slices so it builds for every target, including WASM, and is fuzzable.

## Workspace

All crates live in the root Cargo workspace beside the existing kernels, so one `cargo` invocation builds and tests everything. The TypeScript tree is untouched by the port apart from the kernels (see [physics-audio.md](physics-audio.md)).

| Path | Package | Status |
|---|---|---|
| `crates/platform` | `shallot-platform` | New. Started by the foundations work. |
| `crates/gpu` | `shallot-gpu` | New. Started by the foundations work. |
| `crates/wgsl` | `shallot-wgsl` | New. Started by the foundations work. |
| `crates/formats` | `shallot-formats` | New |
| `crates/engine` | `shallot-engine` | New |
| `crates/core` | `shallot-core` | New |
| `crates/standard` | `shallot-standard` | New |
| `crates/extras` | `shallot-extras` | New; each extra behind a cargo feature, all on by default |
| `crates/shallot` | `shallot` | New facade |
| `crates/abi` | `shallot-abi` | New |
| `crates/cli` | `shallot-cli` (binary `shallot`) | New; native `dev`, `build`, `package`, `create` |
| `crates/physics` | `shallot-physics` | Existing; made to build natively |
| `crates/audio` | `shallot-audio` | Existing; reused as is |
| `crates/native` | `shallot-native` | Existing wry/CEF shell for the TypeScript engine. Untouched; not a dependency of anything in the port |
| `examples/<name>/rs` | `shallot-example-<name>` | One package per example, beside the TypeScript source it translates |

Why one crate per TypeScript layer rather than one per module: the TypeScript layers already enforce the dependency rule, compile times stay manageable with four crates, and module boundaries inside a crate are enforced by `pub(crate)` and review, as `scripts/check-imports.node.ts` does for TypeScript today. A module that grows its own consumers (text, for instance) can be split out later without changing the public paths, because `shallot` re-exports everything.

Why examples live at `examples/<name>/rs`: each translation sits next to the TypeScript it mirrors, so an upstream change to an example shows up in the same directory diff, and the parity harness pairs them by path.

## Public paths

The TypeScript package exports subpaths (`@dylanebert/shallot/physics`, `/extras`, `/standard/rendering` …). The facade mirrors them as modules:

| TypeScript import | Rust path |
|---|---|
| `@dylanebert/shallot` | `shallot::prelude::*` and `shallot::*` |
| `@dylanebert/shallot/ecs` | `shallot::ecs` |
| `@dylanebert/shallot/app` | `shallot::app` |
| `@dylanebert/shallot/utils` | `shallot::math`, `shallot::color` |
| `@dylanebert/shallot/runtime`, `/webgpu` | `shallot::gpu` (re-exports `shallot_gpu` and `WorldGpu`) |
| `@dylanebert/shallot/input`, `/mesh`, `/rendering`, `/physics`, `/transform`, `/audio` | `shallot::input`, `::mesh`, `::rendering`, `::physics`, `::transform`, `::audio` |
| `@dylanebert/shallot/standard/rendering`, `/standard/physics`, `/standard/audio` | `shallot::standard::rendering`, `::physics`, `::audio` |
| `@dylanebert/shallot/extras`, `/bloom`, `/effect-stack`, `/vignette` | `shallot::extras::*` |
| `@dylanebert/shallot/testing`, `/testing/gpu` | `shallot::testing` (feature `testing`) |
| `@dylanebert/shallot/vite`, `/bun` | none; replaced by `shallot-cli` |

## What counts as an OS binding

Allowed, written by hand in `shallot-platform` or `shallot-gpu`:

- `extern "C"` / `extern "system"` declarations of functions exported by system libraries that ship with the OS or its graphics driver: `kernel32`, `user32`, `gdi32`, `ole32`, `xinput1_4`, `winmm` on Windows; `libobjc`, `CoreFoundation`, `AppKit` (through the Objective-C runtime), `Metal`, `QuartzCore`, `CoreAudio`/`AudioToolbox` on macOS; `libc`'s `dlopen`/`dlsym`, `libX11`, `libXi`, `libXcursor`, `libxkbcommon`, `libwayland-client`, `libasound` and `libpulse` on Linux; the Vulkan loader (`vulkan-1.dll`, `libvulkan.so.1`, MoltenVK is not used).
- Hand-written type, constant and struct declarations for those APIs, transcribed from the platform headers or specifications. Each binding file names the header and version it follows.
- On `wasm32`, imports from a hand-written JavaScript shim that calls browser APIs (`navigator.gpu`, canvas, DOM events, Web Audio). The browser is the OS there.

Not allowed: crates that generate or wrap these (`windows-sys`, `objc2`, `ash`, `x11-dl`, `libc`, `wasm-bindgen`), build-time code generators that need non-`std` crates, and linking any C or C++ source of our own. Build scripts (`build.rs`) may only emit link directives.

Libraries are loaded at run time with `dlopen`/`LoadLibraryW` where the library may be missing (Vulkan, Wayland, X11 extensions, PulseAudio), so one binary runs on systems that lack some of them and can report which is missing. Libraries that are always present (`kernel32`, `libobjc`, `libc`) link normally.

Developer tools that are not linked into anything we ship, such as Vulkan validation layers, `spirv-val`, the Metal shader validator, `cargo` itself and the TypeScript toolchain for producing reference images, are allowed as test oracles. See [testing.md](testing.md).

## Concurrency model

- The main thread owns the window, the event loop, the GPU queue and the ECS `World`, as the browser's main thread does today.
- The physics kernel runs its existing worker-pool schedule on `std::thread` workers (today: Web Workers on shared WASM memory). See [physics-audio.md](physics-audio.md#changes-to-cratesphysics).
- Audio runs on the OS audio callback thread, fed through a lock-free single-producer ring of control blocks, mirroring the transferred-buffer protocol of the AudioWorklet.
- Asset decoding and pipeline compilation may use a small `std::thread` pool. Results enter the world only at defined points, as asynchronous GPU results do today.
- Plugin `initialize` and `warm` stay asynchronous. Rust's `core::future::Future` is in `std`; the engine ships a minimal single-threaded executor driven by the frame loop, so the loading screen keeps painting while plugins await. No async runtime crate is used.

## Error handling

- Recoverable failures (missing adapter, unsupported feature, asset decode error, device loss) are `Result<T, shallot::Error>` with the same kinds as the TypeScript error classes: `UnsupportedError`, `GpuError { kind: Validation | OutOfMemory | Internal | DeviceLost }`, `GpuDiagnosticError`.
- A panicking system is caught at the scheduler boundary with `std::panic::catch_unwind`, logged and paused, matching `logAndPauseSystemErrors`. Release builds use `panic = "unwind"` for this reason, which differs from the kernels' current `panic = "abort"` profile; the kernels keep `abort` for their WASM artefacts.
- Nothing unwinds across the bindings boundary ([bindings.md](bindings.md#errors-r6)).
