# Examples

The port is done when all 27 examples run on the Rust engine, translated to Rust, matching the TypeScript build. Each translation lives at `examples/<name>/rs/` as package `shallot-example-<name>`, beside the TypeScript it mirrors, and keeps the same module split (`src/welcome/index.ts` → `rs/src/welcome.rs`).

## Order

Examples are grouped into waves. Each wave needs only engine work from earlier waves plus the items listed, so a wave is a coherent milestone.

### Wave 1: engine bring-up

| Example | Lines (TS) | Needs | Notes |
|---|---|---|---|
| `minimal` | 86 | ECS, app, standard rendering (lit cube, directional shadow), `orbit` | First example on screen; the end-to-end smoke test for everything |
| `hud` | 164 | `ui` (swatches, slider, readout) | First Svelte panel translated |
| `loading-screen` | 185 | `standard/loading` splash, custom `Loading` with dwell, title card | Exercises async `initialize` with progress |
| `isometric` | 354 | orthographic cameras | |
| `city` | 249 | many instances, indirect draws, culling | First load test for standard rendering |
| `embedded-scene` | 1,284 | camera viewport inside a host-drawn frame | No host page natively: the "host" becomes a window whose frame and reveal animation are drawn with `ui`, with the scene in a camera viewport. Its browser-only claims (iframe-like embedding) are recorded as not applicable |

### Wave 2: rendering features

| Example | Lines | Needs |
|---|---|---|
| `tonemapping` | 366 | Tonemapping operators and LUTs, a custom pattern material (WGSL) |
| `color-grading` | 226 | Colour grading |
| `post-processing` | 218 | `effect_stack`, vignette |
| `bloom` | 250 | `bloom` |
| `ambient-occlusion` | 297 | SSAO, depth prepass |
| `low-res` | 439 | Pixel ratio below 1, nearest upscale, PNG, custom posterize shader |
| `day-night-cycle` | 293 | `sky`, `fog`, clock UI |
| `materials` | 540 | Custom material types (checker, wave, quantized), JPEG, `text` |
| `voxel` | 1,545 | Custom terrain material, compute meshing, `profile` overlay |
| `world-ui` | 265 | `sprite`, `text` with TTF, `lines`, PNG |

### Wave 3: physics and first-person

| Example | Lines | Needs |
|---|---|---|
| `first-person` | 2,233 | Standard physics, `CharacterPlugin`, pointer lock, touch, `text`; its player controller is the reference for movement feel |
| `view-model` | 735 | Stacked camera / view-model layer |
| `minimap` | 740 | Second camera to a texture |
| `lit-interior` | 766 | Point lights and point shadows, `sky` |
| `interact` | 1,052 | `outline`, interaction prompts |
| `save-and-load` | 654 | Snapshot and restore of the whole game, `outline`, `text` |
| `particles` | 1,187 | Custom particle compute shader |
| `volumetric-fog` | 1,037 | Volumetric `fog`, motes shader |
| `snow-trails` | 1,207 | Snow deformation compute, `outline`, grab |

### Wave 4: audio and the physics showcase

| Example | Lines | Needs |
|---|---|---|
| `sound` | 864 | Audio output, Ogg Vorbis, spatial voices |
| `physics` | 17,411 | Every physics feature, `lines`, `outline`, `text`, 40 sample plugins, 99 test files, JSON scenes |

`physics` is last because it is the largest translation and its 99 test files (including Box3D sample parity) are the final proof of the physics port. Its samples can be translated incrementally from Wave 3 onwards as physics features land.

## Translation rules

- Keep plugin, system, component and resource names identical, so the scheduler's ordering, profiler labels and snapshot keys match.
- Keep numeric constants and arithmetic order identical in anything that affects simulation, so determinism hashes match.
- Gameplay stays in the example over published engine names only, as `CONTRIBUTING.md` requires.
- UI panels are translated to `ui` with the same controls and the same effect on the world ([assets-ui.md](assets-ui.md#ui)).
- TypeGPU shaders become `.wgsl` files in the example ([wgsl.md](wgsl.md#porting-the-engines-shaders)).
- Each example's tests are translated with it ([testing.md](testing.md#test-suite-mapping)): `*.test.ts` and `*.node.ts` become Rust tests in the example package; `*.e2e.ts` browser claims become headless app tests or are recorded as not applicable with a reason.

## Definition of done per example

1. `cargo run -p shallot-example-<name>` opens the example on Windows, macOS and Linux.
2. Its translated tests pass.
3. Its screenshot fixtures match the TypeScript build within the thresholds in [testing.md](testing.md#screenshot-parity).
4. For physics-driven examples, the physics world hash after the scripted input sequence equals the TypeScript build's.
5. A row in the status table below is ticked.

## Status

| Example | Translated | Tests | Screenshots | Hash |
|---|---|---|---|---|
| minimal | ☐ | ☐ | ☐ | n/a |
| hud | ☐ | ☐ | ☐ | n/a |
| loading-screen | ☐ | ☐ | ☐ | n/a |
| isometric | ☐ | ☐ | ☐ | n/a |
| city | ☐ | ☐ | ☐ | n/a |
| embedded-scene | ☐ | ☐ | ☐ | n/a |
| tonemapping | ☐ | ☐ | ☐ | n/a |
| color-grading | ☐ | ☐ | ☐ | n/a |
| post-processing | ☐ | ☐ | ☐ | n/a |
| bloom | ☐ | ☐ | ☐ | n/a |
| ambient-occlusion | ☐ | ☐ | ☐ | n/a |
| low-res | ☐ | ☐ | ☐ | n/a |
| day-night-cycle | ☐ | ☐ | ☐ | n/a |
| materials | ☐ | ☐ | ☐ | n/a |
| voxel | ☐ | ☐ | ☐ | n/a |
| world-ui | ☐ | ☐ | ☐ | n/a |
| first-person | ☐ | ☐ | ☐ | ☐ |
| view-model | ☐ | ☐ | ☐ | ☐ |
| minimap | ☐ | ☐ | ☐ | ☐ |
| lit-interior | ☐ | ☐ | ☐ | ☐ |
| interact | ☐ | ☐ | ☐ | ☐ |
| save-and-load | ☐ | ☐ | ☐ | ☐ |
| particles | ☐ | ☐ | ☐ | ☐ |
| volumetric-fog | ☐ | ☐ | ☐ | ☐ |
| snow-trails | ☐ | ☐ | ☐ | ☐ |
| sound | ☐ | ☐ | ☐ | ☐ |
| physics | ☐ | ☐ | ☐ | ☐ |
