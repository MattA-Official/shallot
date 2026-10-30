# WASM target

Lower priority than native, but designed in from the start so nothing native-only leaks into the engine's API.

## Shape

- Target `wasm32-unknown-unknown`. No `wasm-bindgen`, no `web-sys`, no Emscripten.
- The engine is one `.wasm` module. A hand-written JavaScript shim (`crates/platform/web/shallot.js`, plain ES module, no build step) instantiates it, supplies its imports and forwards browser events into it.
- The browser is the OS: `shallot-platform` and `shallot-gpu` have `web` backends that call imports instead of system libraries. Everything above them is target-independent.
- The physics kernel keeps its existing `simd128` and shared-memory builds; when the page is cross-origin isolated the engine uses the threaded build, otherwise the single-thread one, exactly as the TypeScript engine does today.

## Imports

The import list is a versioned contract shared with the TypeScript binding's browser glue (requirement R15 in [bindings.md](bindings.md#wasm-imports-r15)). Every import takes and returns only integers, floats and `(ptr, len)` pairs into WASM memory; objects on the JS side live in a handle table.

| Module | Imports |
|---|---|
| `shallot_gpu` | One function per WebGPU call the engine makes, plus a batched `gpu_submit_commands(ptr, len)` that replays an encoded command stream for passes, so a frame crosses the boundary a few times rather than thousands |
| `shallot_window` | canvas size and device-pixel ratio, cursor, pointer lock request and release, fullscreen |
| `shallot_events` | none; the shim calls the engine's exported `on_key`, `on_pointer`, `on_wheel`, `on_touch`, `on_focus`, `on_resize`, `on_frame(timestamp)` |
| `shallot_audio` | create the `AudioContext` and worklet, post control buffers, report state |
| `shallot_fetch` | start a fetch for an asset path, completion through an exported callback with a ticket |
| `shallot_time` | `performance.now` |
| `shallot_log` | console output |

The list is generated from the same declaration as the bindings boundary's reflection document, so the shim and the engine cannot drift.

## Frame loop

`requestAnimationFrame` calls `on_frame(timestamp)`; the engine runs one frame with the timestamp delta as today. All the frame-schedule rules in `CONTRIBUTING.md` apply unchanged, including skipping a callback while six frames are pending.

## Shaders

The web backend passes composed WGSL to the browser. `shallot-wgsl` still type-checks in debug builds and supplies reflection, but SPIR-V and MSL writers are compiled out.

## Delivery

1. Engine crates compile for `wasm32-unknown-unknown` in CI from Wave 1 (keeps the API honest).
2. Web GPU backend and shim; `minimal` runs in Chromium.
3. Input, audio and fetch; all Wave 1 and Wave 2 examples run in the browser.
4. Threaded physics with cross-origin isolation; all examples.

Screenshot parity on the web compares the Rust WASM build with the TypeScript build in the same browser, which is the cleanest comparison available because both use the same GPU implementation.
