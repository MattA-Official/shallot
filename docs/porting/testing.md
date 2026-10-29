# Testing and parity

The TypeScript tree is the oracle. Every claim the Rust port makes about matching it is checked by running both and comparing, not by inspection.

## Test-suite mapping

Upstream's tiers (`CONTRIBUTING.md`, "Verification") map as follows.

| TypeScript tier | Files | Rust counterpart |
|---|---|---|
| Cheap (`*.test.ts`) | 255 | `#[test]` unit tests beside the module, no GPU |
| GPU (`*.gpu.test.ts`) | 46 | `#[test]` in `tests/gpu_*.rs`, run serially on one device (lavapipe in CI) |
| Node (`*.node.ts`) | 331 | Integration tests (`tests/*.rs`) for full compositions, multi-world isolation, allocation gates, physics kernel compositions |
| Browser (`*.e2e.ts`) | 63 | Headless-window app tests where the claim is about the engine; recorded as not applicable where the claim is about the browser (cross-origin isolation, Vite hot update, page composition) |
| Manual oracle (`*.oracle.ts`) | 18 | `#[ignore]` tests or binaries run by hand, same purpose |

Rules carried over:

- Test names state claims, and a ported test keeps its claim text, so the two suites can be diffed by name.
- A missing premise fails, never skips.
- Quarantine is `#[ignore = "reason"]` plus a tracking line in the port's status, equivalent to `test.todo`.
- Gameplay assertions drive the composed world through `World::tick` and `World::step`.
- Frame evidence uses the cheapest observable: CPU state, then GPU readback, then pixels, then a person.

A generated checklist (`docs/porting/status/tests.md`, produced by a script that lists TypeScript test names and searches the Rust tests for the same claim) tracks how much of each module's suite is ported.

## Screenshot parity

1. A capture script in the TypeScript tree (extending `scripts/capture-page.ts` and `scripts/packed-capture`) runs each example headlessly in Chromium on a fixed adapter, steps it a fixed number of frames with scripted input, and writes PNGs plus the step script to `crates/shallot/tests/parity/<example>/<shot>.png`.
2. The Rust example runs the same script headlessly (`WindowConfig::headless`) and captures with `capture_frame`.
3. A comparator (in `shallot::testing`, using `shallot-formats::png`) computes per-pixel ΔE in linear space and reports the share of pixels above threshold.
4. Thresholds: 99.5% of pixels within ΔE 2 for scene rendering; UI regions are masked out and compared separately at a looser threshold, since UI is not pixel-identical by design ([assets-ui.md](assets-ui.md#ui)).
5. Golden images are regenerated only from the TypeScript build, never from the Rust output, and never to make a check pass.

Both sides must render on comparable adapters. CI uses lavapipe for Rust and Chromium's SwiftShader or lavapipe-backed WebGPU for TypeScript; hardware runs are on demand.

## Determinism

- Physics world hashes after scripted sequences must equal the TypeScript build's for every Wave 3 and Wave 4 example and for the kernel oracles. The kernel is the same source on both sides, so any difference points at the coordination layer.
- Single-thread and multi-thread kernel schedules must produce identical hashes natively, as on WASM.
- Snapshot → restore → replay must reproduce the hash, as `save-and-load` and the recovery tests require.

## Allocation and memory

- Steady engine work allocates nothing beyond what its allocation oracle admits. A counting `#[global_allocator]` in test builds replaces the JavaScript allocation sampler; the `*-allocation.entry.ts` gates port to it.
- Memory claims create and dispose their subject and return to baseline, with a deliberately leaking control.

## GPU tests

- Lavapipe on Linux CI for every GPU test, as `scripts/lavapipe.sh` does upstream.
- Vulkan validation layers enabled in debug GPU tests; any validation message fails the test.
- `spirv-val` on every generated SPIR-V module; `xcrun metal` on every MSL module on macOS CI.

## Dependency gate

CI runs, for each `shallot-*` package except the existing `shallot-native` shell:

```
cargo tree -p <package> -e normal,build --prefix none | grep -v '^shallot'
```

and fails if anything is printed. Dev-dependencies are excluded because they are never linked into what we ship; adding one still needs a line in [decisions.md](decisions.md).

## CI layout

| Job | Runs on | Contents |
|---|---|---|
| `fmt-clippy` | Linux | `cargo fmt --check`, `cargo clippy --workspace -D warnings`, dependency gate |
| `unit` | Linux, Windows, macOS | All non-GPU tests |
| `gpu-lavapipe` | Linux | GPU and integration tests on lavapipe with validation layers |
| `metal` | macOS | GPU tests on the Metal backend |
| `parity` | Linux | Screenshot and hash parity for every example marked translated |
| `wasm` | Linux | `cargo build --target wasm32-unknown-unknown` for the engine crates; headless Chromium smoke test once the web backend exists |
| `upstream-ts` | Linux | The TypeScript tree's own pre-land check, unchanged, so merges from upstream stay green |
