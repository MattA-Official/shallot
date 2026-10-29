# 06 · Roadmap, risks and open questions

## How this fits the porting plan

The porting plan (see [`docs/porting/`](../../porting/)) has four steps:

1. Platform and GPU foundations
2. Engine core and the bindings boundary
3. All 27 examples translated to Rust
4. WASM backend and upstream tracking

The binding cannot start before step 2 has a boundary to bind to, and the browser half cannot
start before step 4. It does not need to wait for step 3 to finish: it trails it, example by
example.

```
porting   1 ──────► 2 ──────────► 3 ───────────────────────► 4 ─────────►
binding             T0 ─► T1 ──────► T2 ──────────────────────► T3 ─► T4
```

## Phases

### T0 · Boundary review (during porting step 2)

- Requirements R1 to R15 in [04-boundary-requirements.md](04-boundary-requirements.md) are
  accepted, changed or rejected by the porting plan, with links. (Done 2026-10-29: all answered in
  `docs/porting/bindings.md`.)
- The reflection dump format (R9) is agreed and the engine emits a first version.
- **Done when:** every requirement has a status and a link.

### T1 · Headless binding on Bun (after the porting step 2 core lands)

- FFI transport (L2), tier A facade, tier B generated from the dump.
- Error mapping, host systems, queries, resources, `step`/`tick`.
- Port the ECS and scheduler tests from `src/engine/ecs/*.test.ts` and `src/engine/app/*.test.ts`
  to run against the binding.
- **Done when:** those suites pass on Bun on Linux, macOS and Windows, and a benchmark shows
  per-entity field access at typed-array speed (no boundary calls).

### T2 · Windowed native (alongside porting step 3)

- Pumped `runApp` loop, engine window, input from the platform layer.
- Native WebGPU object model (R10) good enough for TypeGPU.
- Each example's `bun:test` and `*.node.ts` suites run headless against the binding as soon as
  the Rust translation of that example exists.
- `crates/native` (wry/winit/CEF) and the webview build scripts in `src/native` are retired.
- **Done when:** minimal, city, isometric, world-ui, materials and voxel run natively from
  TypeScript and match the screenshot harness.

### T3 · Browser (after porting step 4)

- Wasm transport, the L1 import glue (WebGPU, input, audio, fetch), Vite plugin, startup script,
  loading screens.
- **Done when:** minimal runs in Chrome, Firefox and Safari with WebGPU, and its Playwright specs
  pass.

### T4 · Parity and release

- All 27 TypeScript examples meet the targets in [05-examples.md](05-examples.md), on both
  runtimes where the table says so.
- The original TypeScript engine source in `src/` is removed (see Q3), and the package publishes
  with the binding.
- **Done when:** the three-way screenshot harness is green and the existing test suites pass.

## Where the code lives

Proposed layout for the binding, beside the porting plan's crates:

```
crates/…                    the engine (porting plan, see docs/porting/module-map.md)
crates/abi/                 shallot-abi: boundary! exports + api_description (docs/porting/bindings.md)
bindings/typescript/
  src/transport/            wasm.ts, ffi.ts            (L2)
  src/host/browser/         WebGPU, input, audio glue  (L1)
  src/generated/            emitted from the dump      (L3, not committed, or committed and checked)
  src/                      facade                     (L4)
  scripts/generate.ts       dump → TypeScript
examples/<name>/            TypeScript examples, as today
examples-rs/<name>/         Rust translations (porting plan; name to be agreed)
```

## Risks

| Risk | Effect | Mitigation |
| --- | --- | --- |
| Boundary cost is higher than expected for TypeScript-heavy systems | Examples with many small systems run slowly | Shared-memory columns (R2), query buffers (R4), the per-frame call budget measured from T1 |
| `memory.grow` detaches every view in wasm | Stale typed arrays read zeros or throw | Global memory generation checked on every view access; tests that force growth mid-frame |
| `bun:ffi` callbacks (`JSCallback`) are slower or less stable than plain calls | System dispatch overhead | One callback per system per tick; fall back to a single "run host systems" callback with an index if needed |
| macOS needs the event loop on the main thread, and so does JavaScript | Deadlock or a frozen window | Pumped `frame()` (R7) on the JavaScript main thread; never a blocking `run()` |
| TypeGPU uses WebGPU features the native object model lacks | Shader examples fail natively | Run TypeGPU's own tests against the native object model in T2 |
| The facade drifts from the engine as upstream changes | Silent API differences | Tier B and C are generated (R9); a CI check fails if the generated output is stale |
| No DOM on native | HUD examples are less complete natively | Browser-only HUDs first, engine UI later (Q2) |

## Decided questions

Matt accepted every recommendation below on 2026-10-29.

| ID | Question | Decision |
| --- | --- | --- |
| Q1 | Which native JavaScript runtimes are supported at first? | Bun only. It is already required by the repository, and `bun:ffi` needs no addon. Add Deno or Node later on request. |
| Q2 | Is "HUD in the browser only" acceptable for the T4 milestone on native? | Yes, with engine-UI HUDs as a follow-up once the Rust examples have them. |
| Q3 | When is the original TypeScript engine in `src/` deleted? | Keep it as the reference oracle for the screenshot harness and tests until T4, then delete it. |
| Q4 | What is the npm package called? `@dylanebert/shallot` is upstream's name. | A scope Matt owns, keeping the same subpath exports so only the package name changes in imports. The exact name is still to be picked before T4. |
| Q5 | Who owns the wasm import glue (L1)? | Shared: porting step 4 owns the list of imports (R15); this binding owns the JavaScript that implements them. |
| Q6 | Should Rust apps on the web also use this JavaScript glue? | Yes. A Rust-only web app loads L1 with no TypeScript facade on top. |
