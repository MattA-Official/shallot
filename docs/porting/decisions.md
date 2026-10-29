# Decision log

Newest first. Each entry says what was decided, why, and who decided.

## 2026-10-29: Plan baseline

Decided by Matt in the planning thread:

- **Standard library and OS bindings only.** No third-party crates in anything shipped.
- **Native first, WASM later.**
- **Near-faithful port**, improving where it clearly helps.
- **Track upstream** as a fork for now; may diverge later.
- **Done = all 27 examples running**, translated to Rust.
- **Bindings boundary is Rust-first, Godot-like.** No C layer and no C headers; `extern "C"` is used only as the calling convention because Rust has no stable ABI. Other-language users write their own runtimes.
- **TypeScript binding planned separately**, as its own step.
- **Git hygiene:** commits use Matt's identity with no Claude attribution, and branch names do not reference Claude.

Decided while writing this plan (open to Matt's review):

- **Crate layout** follows the foundations work (`crates/platform`, `crates/gpu`, `crates/wgsl` in the root workspace) plus one crate per TypeScript layer. Reason: matches the layering rules already enforced in TypeScript, keeps compile units few. ([architecture.md](architecture.md#workspace))
- **The GPU layer is a WebGPU implementation**, not a thinner abstraction. Reason: the engine's contracts (limits floor, validation errors, error scopes) are WebGPU's, and the WASM backend becomes a pass-through. ([gpu.md](gpu.md))
- **Shaders leave TypeGPU for plain WGSL files** with a small import composer and a `gpu_struct!` macro. Reason: no TypeScript at run time; WGSL stays valid for the browser; layouts are declared once. ([wgsl.md](wgsl.md#shader-modules))
- **The physics and audio kernels are reused**, with minimal upstream-friendly changes to build the physics kernel natively. Reason: 43,000 lines of Rust already exist and are the determinism authority. ([physics-audio.md](physics-audio.md))
- **An immediate-mode `ui` extra replaces DOM and Svelte HUDs.** UI parity is functional, not pixel-exact. ([assets-ui.md](assets-ui.md#ui))
- **Plugin `initialize`/`warm` stay async** on a std-only executor. Reason: keeps the loading-screen behaviour and the TypeScript API shape. ([engine.md](engine.md#executor))
- **Release builds unwind** so a panicking system can be caught and paused, as the TypeScript engine logs and pauses a throwing system. ([architecture.md](architecture.md#error-handling))
- **Dev-dependencies are exempt from the dependency gate** (the physics crate's `serde_json` gold-vector reader stays while upstream uses it). ([testing.md](testing.md#dependency-gate))
- **Gamepads are out of scope** until upstream adds them; no example or engine module uses them today. ([platform.md](platform.md#scope))
