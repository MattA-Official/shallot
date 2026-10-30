# 05 · Keeping the TypeScript examples working

The porting plan translates all 27 examples to Rust. This plan keeps the *TypeScript* versions
running on the Rust engine too, as the binding's acceptance test. The two sets should look the
same on screen; the screenshot harness from the porting plan compares both against the original
TypeScript engine.

## What each example depends on

Counts are from each example's own `src/` (tests, oracles and node-only files excluded), taken on
2026-10-29 at `6293e19`. "DOM" means the example's source touches `document`, `window`,
`HTMLElement`, `localStorage` or `mountOverlay`. "GPU" means it calls the GPU directly.

| Example | Lines | DOM | Svelte HUD | GPU | TypeGPU | Browser | Native (Bun) |
| --- | ---: | :-: | :-: | :-: | :-: | --- | --- |
| minimal | 86 | | | | | Unchanged | Unchanged |
| city | 170 | | | | | Unchanged | Unchanged |
| isometric | 186 | | | | | Unchanged | Unchanged |
| world-ui | 265 | | | | | Unchanged | Unchanged |
| materials | 347 | | | | ✓ | Unchanged | Native WebGPU object model |
| voxel | 1,281 | | | ✓ | ✓ | Unchanged | Native WebGPU object model |
| loading-screen | 150 | ✓ | | | | Unchanged | Engine-drawn splash |
| embedded-scene | 164 | ✓ | | | | Unchanged | Browser only: the point is embedding in a page |
| save-and-load | 472 | ✓ | | | | Unchanged | Swap `localStorage` for a small file-backed store |
| first-person | 1,042 | ✓ | | ✓ | | Unchanged | Runs; touch overlay skipped (already guarded) |
| interact | 1,052 | ✓ | | | | Unchanged | Runs; overlay skipped |
| lit-interior | 766 | ✓ | | | | Unchanged | Runs; overlay skipped |
| minimap | 740 | ✓ | | | | Unchanged | Runs; overlay skipped |
| particles | 978 | ✓ | | ✓ | ✓ | Unchanged | Runs; overlay skipped |
| snow-trails | 750 | ✓ | | ✓ | ✓ | Unchanged | Read the viewport size from the engine, not `window` |
| sound | 864 | ✓ | | | | Unchanged | Runs; overlay skipped |
| view-model | 735 | ✓ | | | | Unchanged | Runs; overlay skipped |
| volumetric-fog | 959 | ✓ | | ✓ | ✓ | Unchanged | Runs; overlay skipped |
| ambient-occlusion | 190 | ✓ | ✓ | | | Unchanged | Runs with defaults; HUD browser only |
| bloom | 184 | ✓ | ✓ | | | Unchanged | Runs with defaults; HUD browser only |
| color-grading | 200 | ✓ | ✓ | | | Unchanged | Runs with defaults; HUD browser only |
| day-night-cycle | 293 | ✓ | ✓ | | | Unchanged | Runs with defaults; HUD browser only |
| hud | 164 | ✓ | ✓ | | | Unchanged | HUD rebuilt with engine UI (the example is about the HUD) |
| low-res | 268 | ✓ | ✓ | ✓ | ✓ | Unchanged | Runs with defaults; HUD browser only |
| post-processing | 218 | ✓ | ✓ | | | Unchanged | Runs with defaults; HUD browser only |
| tonemapping | 278 | ✓ | ✓ | | ✓ | Unchanged | Runs with defaults; HUD browser only |
| physics | 17,411 | ✓ | ✓ | | | Unchanged | Runs; scene picker browser only until engine UI covers it |

"Unchanged" in the browser column is the target: the only edit is the package name, if it
changes (Q4). The mountOverlay overlays are on-screen touch controls for phones, so skipping them
on desktop loses nothing.

"HUD browser only" is the cheap first answer. Once the Rust translations have engine-UI HUDs
(the porting plan's step 3), the TypeScript examples can adopt the same engine UI and drop the
split; that is a follow-up, not a milestone blocker.

## Order of bring-up

Follow the porting plan's example order so the engine features exist when the binding needs them,
but within that, the binding goes in this order:

1. **Headless first.** Run each example's existing `bun:test` and `*.node.ts` suites against the
   binding on native. These need no window and no DOM, and they already assert behaviour in
   detail. `first-person/src/touch.node.ts`, for example, drives `World`, `InputPlugin` and the
   player's systems with injected input and checks component values.
2. **minimal** windowed on native, then in the browser.
3. **No-DOM examples**: city, isometric, world-ui, materials, voxel.
4. **Overlay examples**, then **HUD examples**.
5. **physics** last: it is 17k lines and exercises almost everything.

## How parity is tested

- **Behaviour:** the existing TypeScript test files, unmodified apart from imports, run under
  `bun test` against the binding. A failing test is either a port bug or a documented, deliberate
  API change listed in [03-api-parity.md](03-api-parity.md).
- **Pixels:** the porting plan's screenshot-diff harness gains a third column. For each example it
  captures the original TypeScript engine, the Rust translation, and the TypeScript example on the
  binding, at fixed seeds and fixed time steps.
- **Boundary cost:** each example records boundary calls per frame. The budget in
  [01-architecture.md](01-architecture.md#call-model) is under 100 for a typical example.
- **Browser:** the existing Playwright `*.e2e.ts` specs run against the wasm build.
