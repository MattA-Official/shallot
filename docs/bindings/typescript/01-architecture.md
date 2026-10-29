# 01 · Architecture

## Layers

```
 user code          import { World, component, Transform } from "shallot"
                                   │
 ┌─────────────────────────────────┴─────────────────────────────────┐
 │ L4  facade (hand-written TS)                                       │
 │     World, component(), Plugin, System, runApp, testing helpers    │
 ├────────────────────────────────────────────────────────────────────┤
 │ L3  generated API (TS emitted from the engine's reflection dump)   │
 │     built-in components, resources, enums, handle-resource methods │
 ├────────────────────────────────────────────────────────────────────┤
 │ L2  transport (one interface, two implementations)                 │
 │     wasm: WebAssembly.Instance exports + memory                    │
 │     ffi:  bun:ffi dlopen symbols + toArrayBuffer(ptr, len)          │
 ├────────────────────────────────────────────────────────────────────┤
 │ L1  host glue (browser only)                                       │
 │     WebGPU, DOM input, WebAudio, fetch: imports the wasm engine     │
 │     needs because it cannot reach the browser any other way         │
 └─────────────────────────────────┬──────────────────────────────────┘
                                   │  the bindings boundary (Rust, extern "C")
 ┌─────────────────────────────────┴──────────────────────────────────┐
 │ L0  shallot engine (Rust, std + OS bindings only)                   │
 │     ECS, scheduler, renderer, physics, audio, platform, GPU layer   │
 └────────────────────────────────────────────────────────────────────┘
```

**L0 · engine.** Owned by the porting plan. The binding never reaches past the boundary.

**L1 · host glue.** On the web, the Rust engine needs JavaScript to call WebGPU, read DOM input,
play audio and fetch files. Because we use no `wasm-bindgen`, someone has to hand-write those
imports. That someone is this package: the browser half of the TypeScript binding and the
engine's wasm import shim are the same code. A Rust-only web app would still load this glue, with
no user TypeScript on top. This makes L1 a shared deliverable with porting step 4 (see
[06-roadmap.md](06-roadmap.md)).

**L2 · transport.** A tiny interface, roughly:

```ts
interface Transport {
    call: Record<string, (...args: number[]) => number>; // boundary functions, by name
    view(ptr: number, bytes: number): ArrayBuffer; // zero-copy window onto engine memory
    utf8In(text: string): [ptr: number, len: number]; // into a scratch arena
    utf8Out(ptr: number, len: number): string;
    callback(fn: (...args: number[]) => number, signature: string): number; // host fn pointer
}
```

The wasm transport implements `view` as a slice of `memory.buffer` and `callback` as an entry in
an exported function table. The FFI transport uses `bun:ffi`'s `toArrayBuffer` and `JSCallback`.
Everything above L2 is shared and does not know which runtime it is on.

**L3 · generated API.** The engine emits a reflection dump (see requirement R9 in
[04-boundary-requirements.md](04-boundary-requirements.md)). A build script turns it into
TypeScript: one `Component` object per built-in component with its schema and defaults, one
`Resource` per resource, enums such as `ShapeKind` and `BodyType`, and typed stubs for methods on
handle resources (`Materials.add`, `Meshes.add`). Upstream adds a component, the dump changes, the
TypeScript regenerates. This is how the binding keeps tracking upstream without hand work.

**L4 · facade.** The parts that are a TypeScript idiom rather than engine data: `component()`,
the `World` class, `Plugin` and `System` objects, `runApp`, `query` iteration, loading screens,
the Vite plugin and the testing helpers. These are hand-written but thin; they translate to L3/L2
calls.

## Memory model

This is the part that decides whether the binding is fast or unusable.

Today a component's storage is a set of typed-array columns, one per field
(`world.storage(Transform).translation.x.get(eid)`), with a dirty bitset and per-word change ticks
for change detection. The Rust engine keeps the same structure-of-arrays layout. The binding
**does not copy** it. Instead:

1. For each registered component field, the engine exposes `(ptr, byteLength, generation)`.
2. The facade builds a typed-array view over that memory once and caches it with the generation.
3. A boundary call that can grow storage (create, add, a system tick) may reallocate a column.
   The engine bumps the generation; the facade compares on access and rebuilds the view only then.
   In wasm, `memory.grow` also detaches every view, so the transport keeps a single global
   generation that changes on growth.
4. `.get(eid)` and `.set(eid, v)` are plain typed-array reads and writes. `set` also appends the
   entity to a per-field list in TypeScript, which is flushed to the engine's change tracking with
   one `column_mark` call when the current system returns (see
   [04-boundary-requirements.md](04-boundary-requirements.md#how-the-binding-uses-the-answers)).
   No boundary crossing per write.
5. `column` (the raw typed array) stays available, so code such as
   `input.move.column.slice(eid * 2, eid * 2 + 2)` keeps working unchanged.

`entity` fields are `Float64Array` in today's engine; the Rust port is free to change the
representation (see [03-api-parity.md](03-api-parity.md#entities)), and the facade adapts.

## Call model

| Kind of work | Crosses the boundary? |
| --- | --- |
| Reading or writing a component field | No (shared memory) |
| Iterating a query | Once per query per frame: the engine fills a caller buffer with entity ids, the facade iterates it |
| `create`, `add`, `remove`, `destroy` | Yes, one call each. Batch variants exist for hot paths |
| A TypeScript system | The engine calls the host callback once per system per tick |
| `world.step(dt)` | One call; the engine runs every system, calling back into TypeScript for TS systems |
| Methods on handle resources (`Materials.add`) | One call, arguments packed into a scratch buffer |
| GPU calls from user code | See [02-runtimes.md](02-runtimes.md#gpu-access-from-user-code) |

Target: a typical example frame makes fewer than 100 boundary calls, dominated by system
callbacks and query fills. The phase 1 milestone in the roadmap measures this.

## Errors

The engine never unwinds across the boundary. Each call returns a status; on failure the facade
reads the last-error string and throws a JavaScript `Error` with the same message the TypeScript
engine throws today where a test asserts on it. Exceptions thrown inside a TypeScript system are
caught by the facade's callback trampoline, stored, reported to the engine as a failure status
(so `logAndPauseSystemErrors` keeps its meaning), and rethrown from `world.step()`.

## Threading

JavaScript callbacks must run on the JavaScript thread. The engine therefore calls host systems
only from inside a boundary call made on that thread (`step`, `frame`). The engine may use its own
worker threads internally (physics, culling), but never calls back into TypeScript from them.
