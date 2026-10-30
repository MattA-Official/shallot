# shallot-rs porting plan

This folder is the reference plan for porting shallot from TypeScript on WebGPU to Rust that uses only the Rust standard library and operating-system bindings. It is written to be read piecemeal: each file owns one layer or one process, and the work packages in [roadmap.md](roadmap.md) link back to the section that specifies them.

When the code and these documents disagree, fix whichever is wrong in the same change. A decision that changes a document's design goes in the [decision log](decisions.md) with its date.

## Goals

1. **Standard library and OS bindings only.** No third-party crates in anything the engine or its examples link: no `wgpu`, `winit`, `naga`, `image`, `serde`, `libc`, `windows-sys`, `objc2` or `wasm-bindgen`. What counts as an "OS binding" is defined in [architecture.md](architecture.md#what-counts-as-an-os-binding).
2. **Native first, WASM second.** Windows, macOS and Linux desktop are the primary targets. `wasm32-unknown-unknown` calling the browser's WebGPU is wanted but later ([wasm.md](wasm.md)).
3. **A near-faithful port.** The public API, plugin model, frame schedule, determinism rules and rendering output follow the TypeScript engine. We change things where Rust gives a clear benefit (ownership instead of `world.own`, typed handles instead of strings, no TypeGPU), and every deliberate difference is listed in [module-map.md](module-map.md#deliberate-differences).
4. **Track upstream.** This repository is a fork of `dylanebert/shallot`. The TypeScript tree stays in place and keeps merging from upstream; the Rust port follows it feature by feature ([upstream.md](upstream.md)). We may diverge later.
5. **Done means every example runs.** All 27 examples in `examples/` are translated to Rust and match the TypeScript build in screenshots and in their behavioural tests ([examples.md](examples.md)).
6. **A Rust-first bindings boundary.** Like Godot's GDExtension, other languages can drive the engine, but the boundary is designed and written in Rust. There is no C layer and we ship no C headers; other-language users write their own runtimes against it ([bindings.md](bindings.md)).

## Standing rules for contributors

- Every commit uses Matt's identity and carries no Claude attribution (no `Co-Authored-By` or `Claude-Session` trailers).
- `cargo tree -e normal,build` for any `shallot-*` crate must list only `shallot-*` crates. CI enforces this ([testing.md](testing.md#dependency-gate)).
- Unsafe code is allowed only in `shallot-platform`, `shallot-gpu`, the audio and physics kernels and `shallot-abi`, and every `unsafe` block carries a `// SAFETY:` comment.
- Upstream licences travel with ported code: shallot is MIT, Box3D-derived physics is MIT (`LICENSE-BOX3D`), and shaders taken from Bevy keep their `MIT OR Apache-2.0` headers.

## Documents

| Document | Owns |
|---|---|
| [architecture.md](architecture.md) | Layering, workspace and crate map, dependency rules, what counts as an OS binding |
| [module-map.md](module-map.md) | Every TypeScript module mapped to its Rust crate and module, plus deliberate API differences |
| [platform.md](platform.md) | Windows, input, timing, threads, file access, audio output and dynamic loading on each OS |
| [gpu.md](gpu.md) | The WebGPU-shaped GPU layer and its Vulkan, Metal and browser backends |
| [wgsl.md](wgsl.md) | The WGSL compiler: scope, stages, SPIR-V and MSL output, and how shaders leave TypeGPU |
| [engine.md](engine.md) | ECS, world, scheduler, app and plugins, GPU tables, snapshot and restore |
| [rendering.md](rendering.md) | Core and standard rendering, the extras, and shader porting |
| [physics-audio.md](physics-audio.md) | Reusing the existing Rust kernels natively, and porting their TypeScript coordination |
| [assets-ui.md](assets-ui.md) | Image, audio, font and JSON decoders; the native replacement for DOM and Svelte UI |
| [bindings.md](bindings.md) | The Rust-first extension boundary; the TypeScript binding placeholder |
| [examples.md](examples.md) | Translation order for the 27 examples and what each one needs |
| [testing.md](testing.md) | Parity strategy: test-suite mapping, screenshot diffs, determinism hashes, CI gates |
| [wasm.md](wasm.md) | The browser target |
| [upstream.md](upstream.md) | How upstream changes flow into the port |
| [risks.md](risks.md) | Risk register with mitigations |
| [roadmap.md](roadmap.md) | Phases, work packages, milestones and exit criteria |
| [decisions.md](decisions.md) | Dated log of decisions and their reasons |

## Snapshot this plan was written against

- Upstream commit `6293e19` (package `0.10.2-next.1`), 29 October 2026.
- About 76,000 lines of non-test TypeScript in `src/`: engine 9,900, core 9,500, standard 41,800 (physics coordination 24,200, rendering 15,600), extras 11,800, CLI and native shell 3,300.
- 41,300 lines of Rust in `crates/physics` (the Box3D kernel) and 1,800 in `crates/audio`, both compiled to WASM today.
- 346 TypeGPU `"use gpu"` functions and 145 `tgpu.fn` WGSL snippets across 148 files, with 64 precompiled pipeline registrations.
- 255 cheap, 46 GPU, 331 Node, 63 browser and 18 oracle test files.
