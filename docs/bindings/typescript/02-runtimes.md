# 02 · Runtimes

Where TypeScript code runs, and how it reaches the engine in each place.

## Summary

| Runtime | Engine artefact | Transport | DOM | Status |
| --- | --- | --- | --- | --- |
| Browser | `shallot.wasm` (`wasm32-unknown-unknown`) | wasm exports and memory | Yes | Primary for TS users; needs porting step 4 |
| Bun on desktop | `libshallot.so` / `.dylib` / `shallot.dll` | `bun:ffi` | No | Primary for native TS; available after porting step 2 |
| Bun headless (tests, tools) | same dynamic library | `bun:ffi` | No | Same as above; runs today's `bun:test` suites |
| Node | same dynamic library | Node-API module written in Rust (no crates) | No | Later, if asked for |
| Deno | same dynamic library | `Deno.dlopen` | No | Later, if asked for; the FFI transport ports almost directly |
| Embedded JS engine inside a Rust executable | n/a | n/a | n/a | **Out of scope**: it is either a dependency (V8, QuickJS) or a JavaScript engine of our own |

## Browser (WASM)

The engine is built for `wasm32-unknown-unknown` with no `wasm-bindgen`. The page loads
`shallot.wasm` with `WebAssembly.instantiateStreaming`, passing the L1 import object (WebGPU,
input, audio, fetch, time). The binding wraps the instance as the wasm transport.

Points of note:

- **WebGPU objects live on the JavaScript side.** The engine holds integer handles; L1 keeps a
  handle table of real `GPUDevice`, `GPUBuffer` and so on. That means user TypeScript can be given
  the *real* `GPUDevice` when it asks for `world.gpu.device`, and TypeGPU, which wants a real
  device, works unchanged. The engine and the user share one device.
- **Input.** L1 listens to DOM events on the canvas and forwards them into the engine's input
  layer, the same way today's `pressKey`, `touchPoint` and `pointerLockChanged` functions feed the
  TypeScript input plugin.
- **Audio.** L1 owns an `AudioContext` and an `AudioWorklet` that pulls mixed frames from engine
  memory. The `crates/audio` DSP is already Rust; the port moves its output side into the engine.
- **Memory growth** detaches all views; see the memory model in
  [01-architecture.md](01-architecture.md#memory-model).
- **Threads.** Single-threaded first. Shared-memory threads need cross-origin isolation headers;
  the Vite plugin already handles isolation (`src/project/isolation.js`), so this is a later
  optimisation, not a blocker.
- **Build.** The Vite plugin (`@…/vite`) keeps its role: it serves the wasm, the startup script,
  the loading screen and assets. Users' `vite.config.ts` does not change.

## Native desktop (Bun FFI)

The user runs their game with Bun. The binding `dlopen`s the engine library through `bun:ffi`.
The engine opens the window itself through its own platform layer (Win32, Cocoa, X11/Wayland) and
renders with its own GPU layer (Vulkan, Metal). Bun only hosts the TypeScript.

Why Bun:

- The repository already requires Bun (`"engines": { "bun": ">=1.4.3" }`), and every example's
  tests run under `bun:test`.
- `bun:ffi` is built in, so the binding adds no dependency and needs no compiled addon.
- `bun build --compile` produces a single executable, which, with the engine library beside it,
  replaces the current packaged native shell.

### The frame loop on native

The engine's event loop and the JavaScript event loop must share one thread (macOS insists the
window's event loop runs on the main thread, and JavaScript callbacks must run on the JavaScript
thread). A blocking `run()` inside the engine would starve promises, so asset loading and
`await world.gpu.sync()` would never resolve. Instead the facade pumps:

```ts
// inside runApp() on native, simplified
for (;;) {
    const status = engine.frame(); // poll OS events, run due fixed ticks and one variable frame, present
    if (status === QUIT) break;
    await nextMacrotask(); // let promises and I/O run
}
```

`frame()` returns promptly; presentation pacing (vsync) is the only wait inside it. Async engine
operations (asset loads, GPU readback) return a ticket; `frame()` reports completed tickets and
the facade resolves the matching promises.

### What is missing on native: the DOM

There is no DOM under Bun. Today, 21 of the 27 examples touch the DOM in their own source somewhere (a Svelte HUD, a
touch overlay, a `document.querySelector("canvas")`). On native these need one of:

1. **Engine UI.** Use the engine's own in-world UI and text (`extras/text`, `world-ui`) for HUDs.
   This is what the Rust translations of the examples will do anyway, so the TypeScript versions
   can follow the same design.
2. **Guarded DOM code.** Many examples already guard with `typeof document !== "undefined"`
   (the first-person touch controls do). Those run on native without their DOM extras.
3. **Browser only.** An example that is mostly HUD stays a browser example in TypeScript.

The per-example choice is in [05-examples.md](05-examples.md). Writing a DOM for native is out of
scope.

### Replacing the current native shell

`crates/native` today wraps the TypeScript bundle in a webview (wry, or CEF on some targets). That
is incompatible with the no-dependencies rule and becomes unnecessary: the window, input and GPU
all come from the Rust engine. It is retired once the Bun path renders the first example (see the
roadmap). The `src/native` build scripts are rewritten to produce `bun build --compile` output plus
the engine library.

## GPU access from user code

Six examples write GPU code directly (`world.gpu.device.createBuffer`, `beginComputePass`,
`world.gpu.root.createComputePipeline`), and seven author shaders or materials with TypeGPU.

- **Browser:** user code gets the real `GPUDevice` from L1's handle table. Nothing changes.
- **Native:** the binding ships a WebGPU-shaped JavaScript object model (`GPUDevice`, `GPUBuffer`,
  `GPUCommandEncoder`, ...) implemented over the engine's GPU layer through FFI handles. Because
  the Rust GPU layer is deliberately WebGPU-shaped, this is a mechanical mapping. It is the same
  object model TypeGPU needs, and it also replaces the `webgpu` (Dawn) npm package the repository
  uses for headless GPU tests today.
- **Shaders:** TypeGPU turns TypeScript shader functions into WGSL at build time. The engine
  compiles WGSL with its own compiler on native and passes it to the browser on the web, so
  TypeGPU-authored shaders work on both without a TypeScript-side compiler.
