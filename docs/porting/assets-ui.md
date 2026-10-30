# Assets and UI

The browser decodes images and audio, lays out text in HUDs, and hosts Svelte panels. Natively the engine does all of that itself.

## Decoders (`shallot-formats`)

Pure Rust over byte slices, no OS calls, so every decoder builds for WASM too (where we still prefer the browser's decoders for speed, behind the same API).

| Format | Needed by | Scope |
|---|---|---|
| DEFLATE / zlib | PNG | Inflate only, with stored, fixed and dynamic Huffman blocks |
| PNG | `low-res` (`crate.png`), `world-ui` (`attention.png`, `white.png`), app icons | All colour types and bit depths, Adam7 interlace, `tRNS`, `gAMA`/`sRGB` ignored (assets are sRGB). Output RGBA8 |
| JPEG | `materials` (`wood.jpg`, baseline 512×512) | Baseline and progressive Huffman, 4:4:4/4:2:2/4:2:0, EXIF orientation. Output RGBA8 matching the browser's IDCT within ±1 per channel |
| Ogg + Vorbis | `sound` (`Project_Utopia.ogg`, 44.1 kHz stereo) | Ogg page/packet layer and a Vorbis I decoder (floor 1, residue 0–2). Floor 0 rejected with a clear error (unused since 2004) |
| WAV | convenience for user projects | PCM 8/16/24/32 and float |
| TrueType | `extras/text` (Inter) | Already hand-written in `extras/text/font.ts`; ports to `formats::ttf`: `cmap`, `glyf`/`loca`, `hmtx`, `kern`/GPOS pair kerning as the TS supports |
| JSON | `physics` example scene files (`building.json`, `human.json` …) | Strict RFC 8259 parser to a small `Value` tree, plus `get`/index helpers |
| SVG | loading-screen brand mark | Not decoded: the brand mark in `standard/loading/brand-data.ts` is already path data; it is tessellated for the 2D renderer |

WOFF2 fonts in the examples are used only by HTML and CSS for the HUD panels. The Rust UI uses TTF only, so we ship IBM Plex Sans as TTF beside each example's WOFF2 (OFL licence) and need no Brotli decoder.

Each decoder is tested against fixtures decoded by the browser: the TypeScript tree's capture tooling writes reference RGBA and PCM for each example asset, and the Rust decoder must match (exactly for PNG and WAV, within tolerance for JPEG and Vorbis, whose reference decoders round differently).

## Asset loading

- `register_image(world, "public/crate.png", options)`, `register_font`, `load_clip` keep their names. The argument is a path relative to the project's asset root instead of a URL.
- In development the asset root is the example's directory; `shallot-cli package` copies `public/` beside the executable. `include_asset!` embeds an asset into the binary when a project wants a single file.
- Loading is asynchronous through the engine executor; decoding runs on a worker thread and the result is uploaded on the main thread.

## UI

### What the TypeScript examples do

Thirteen examples use DOM for UI:

- Svelte panels with buttons, swatches, sliders and readouts: `ambient-occlusion`, `bloom`, `color-grading`, `day-night-cycle` (clock), `hud`, `low-res`, `post-processing`, `tonemapping`, `physics`.
- Plain DOM from TypeScript: crosshairs and prompts in `first-person`, `interact`, `lit-interior`, `minimap`, `particles`, `save-and-load`, `sound`, `view-model`, `volumetric-fog`; the title card in `loading-screen`; the host page in `embedded-scene`.
- Engine-owned DOM: the loading screen (`standard/loading`), the diagnostics overlay (`extras/profile`, `DiagnosticsOverlayPlugin`), and `mountOverlay`.

These are part of what each example demonstrates, so the Rust translations need an equivalent, but they are application chrome, not engine rendering. The parity target for UI is functional (same controls, same effect on the world) and visually close, not pixel-identical.

### Design: `shallot_extras::ui`

An immediate-mode UI drawn by the engine after `PresentationSystem`, in the overlay slot.

- **Rendering**: one batched 2D pipeline for rounded rectangles, lines, images and SDF text (reusing `extras/text`'s glyph atlas and SDF generator). Output is in physical pixels, scaled by the device-pixel ratio so CSS pixel sizes in the TypeScript CSS carry over.
- **Layout**: a small flex-like row/column model with padding, gap and alignment, which covers every panel in the examples. Panels anchor to window corners like the CSS they replace.
- **Widgets**: `label`, `button`, `toggle`, `swatch`, `slider` (range input), `select` (segmented buttons), `readout`, `panel`, `crosshair`, `prompt`, `progress`. That is the full set the examples use.
- **Input**: the UI consumes pointer and keyboard events before `core/input` sees them, as DOM controls swallow events today (`onmousedown` `preventDefault` in the Svelte panels). Pointer lock disables UI hit-testing.
- **State**: immediate mode keeps state in the caller, so a Svelte component's `$state` becomes fields in a resource, and `$effect` becomes code that runs after the widget returns a change. That maps one-to-one onto the existing panels.
- **Theme**: the dark Shallot palette and IBM Plex Sans from the examples' `controls.css`, as constants.

```rust
fn hud(ui: &mut Ui, world: &mut World) {
    let turn = world.resource::<Turn>();
    ui.panel(Anchor::BottomLeft, |ui| {
        ui.readout("Angle", format_args!("{}°", turn.degrees()));
        if let Some(speed) = ui.slider("Speed", turn.speed, 0.0..=4.0) { turn.speed = speed; }
    });
}
```

`AppConfig::ui` takes such a function. The loading screen (`shallot_dark`) and the diagnostics overlay are written with the same primitives.

### Accessibility

DOM gave the examples ARIA labels for free. The native UI keeps the labels as data so a later platform accessibility bridge (UI Automation, NSAccessibility, AT-SPI) can expose them; building that bridge is not part of reaching example parity and is listed in [risks.md](risks.md).
