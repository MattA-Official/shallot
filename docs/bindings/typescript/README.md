# TypeScript binding: plan

Status: **plan agreed, not implemented**. Written 2026-10-29.

This folder plans how TypeScript users drive the Rust port of shallot. The Rust engine is the
product; TypeScript becomes one client of its bindings boundary, in the way GDScript, C# and
godot-cpp are clients of Godot's GDExtension interface. Nothing here changes the porting plan in
[`docs/porting/`](../../porting/); where the two disagree, the porting plan wins on anything
inside the engine, and this plan wins on anything inside the TypeScript package.

## Why a TypeScript binding

- Shallot today *is* a TypeScript engine. Its users, its 27 examples and roughly 77k lines of
  engine source are TypeScript. A binding keeps that audience while the engine moves to Rust.
- It is the first non-Rust client of the boundary, so it proves the boundary is genuinely
  language-neutral before anyone else depends on it.
- The existing TypeScript test suites (`bun:test` files that build a `World` headlessly) become a
  free parity suite: run them against the binding and every pass is evidence the port behaves
  like the original.

## Ground rules inherited from the project

1. **Rust first.** The boundary is defined in Rust. It uses `extern "C"` and `#[repr(C)]` only
   because Rust has no stable ABI; there is no C code and there are no C headers.
2. **No dependencies.** The engine crates use `std` and OS bindings only. The binding adds no
   crates and no runtime npm dependencies. It runs inside a JavaScript runtime the *user* already
   has (a browser or Bun); we do not embed or ship a JavaScript engine.
3. **Near-faithful API.** A TypeScript user should recognise the API. Code changes in examples
   should be mechanical, and listed.
4. **Track upstream.** The binding's API surface is generated from the engine's reflection data,
   so upstream features flow through without hand-writing TypeScript for each one.

## Reading order

| Doc | What it answers |
| --- | --- |
| [01-architecture.md](01-architecture.md) | The layers, from the Rust engine to the user's `import`, and the memory model that makes it fast. |
| [02-runtimes.md](02-runtimes.md) | Where TypeScript runs: browser over WASM, native desktop over Bun FFI, and what is out of scope. |
| [03-api-parity.md](03-api-parity.md) | Every part of today's public API and what happens to it in the binding. |
| [04-boundary-requirements.md](04-boundary-requirements.md) | What the binding needs from the Rust boundary. This is the contract with the porting plan. |
| [05-examples.md](05-examples.md) | How each of the 27 examples keeps working, and how parity is tested. |
| [06-roadmap.md](06-roadmap.md) | Phases, milestones, dependencies on the porting plan, risks and open questions. |

## Decisions so far

| # | Decision | Status |
| --- | --- | --- |
| D1 | The binding is a client of the same boundary every other language uses. No private back door into the engine. | Agreed (follows Matt's "bindings like Godot") |
| D2 | Browser: the engine compiles to `wasm32-unknown-unknown`; the binding's JavaScript is also the engine's WebGPU/DOM/audio import glue. No `wasm-bindgen`. | Proposed |
| D3 | Native: the user's Bun process loads the engine dynamic library through `bun:ffi`. No embedded JavaScript engine. | Agreed (Q1: Bun only at first) |
| D4 | Component data lives in engine memory. TypeScript reads and writes it through zero-copy typed-array views, so per-entity access never crosses the boundary. | Proposed |
| D5 | The TypeScript API surface is generated from a reflection dump the engine emits, then wrapped by a thin hand-written facade. | Proposed |
| D6 | The binding replaces the current native shell (`crates/native`, wry/winit/CEF). | Proposed |

Matt accepted the recommended answers to Q1 to Q6 on 2026-10-29 (which settles D3); see
[06-roadmap.md](06-roadmap.md#decided-questions).
