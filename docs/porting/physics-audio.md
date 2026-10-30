# Physics and audio

Both simulations already have their computational core in Rust: `crates/physics` (41,300 lines, a port of Box3D `47d7f7cc`) and `crates/audio` (1,800 lines, voice mixing and HRTF spatialisation). Today they compile to WASM and TypeScript drives them. The port reuses both crates and replaces the TypeScript that drives them.

## Physics

### What exists

- The kernel's free functions are the internal Box3D-shaped API; parity oracles call them as Box3D's C samples call Box3D, and world hashes equal the native Box3D reference with no tolerance.
- 39 modules in `lib.rs` are gated to `target_arch = "wasm32"`; native `cargo test` builds only the subset its tests reach, through the scalar `FloatW` fallback that is bit-identical to the `simd128` path.
- The kernel exposes about 480 `extern "C"` / `#[no_mangle]` entry points. TypeScript reads results through typed-array views over WASM memory (`standard/physics/kernel/*columns.ts`) and drives a worker pool over shared memory (`kernel/pool.ts`).
- The kernel's task scheduler (`scheduler.rs`) already has a non-WASM path using `std::thread::yield_now`; its tests run with `std::thread::scope` at two and eight threads.

### Changes to `crates/physics`

Kept small and upstream-friendly, because upstream keeps editing this crate ([upstream.md](upstream.md)):

1. **Build every module natively.** Replace `#[cfg(target_arch = "wasm32")]` gates on simulation modules with a crate feature (`kernel`, default on) so the whole kernel builds for the host. Keep the wasm gates only on the export shims and memory-layout code that genuinely need WASM.
2. **Native threads.** Implement the worker wait/wake that WASM does with `memory.atomic.wait32`/`notify` using `std::thread::park`/`unpark` (or a futex-like spin-then-park) behind the existing `cfg` seam in `scheduler.rs`. Workers are `std::thread`s owned by the physics world, sized like the TypeScript pool (`maxWorkers`).
3. **A Rust-facing API.** Add a `pub` module (`native.rs`) exposing safe functions over the same internals the `extern "C"` shims call: create world, create body/shape/joint, step, read body columns as slices, queries with callbacks. The shims stay for the WASM build, so both paths run the same phase code.
4. **SIMD.** Native starts on the scalar `FloatW` path, which is bit-identical. An SSE2/NEON `FloatW` through `core::arch` (in `std`) can follow, with a test that compares hashes against the scalar path.
5. **Test-only JSON.** `serde_json` is a dev-dependency used to read gold vectors. Dev-dependencies are not linked into anything we ship, and `cargo tree -e normal` stays clean, so it can stay while upstream uses it. If we diverge, swap it for `shallot-formats::json`.

### Porting the TypeScript coordination

`standard/physics` is 24,200 lines including tests. Its responsibilities after the port:

| Responsibility | TypeScript | Rust |
|---|---|---|
| Public API | `runtime.ts` (`apply_force`, `set_transform`, `set_linear_velocity`, `set_awake`, `step_physics`, `hash_physics` …) | `shallot_standard::physics::api` |
| ECS ↔ kernel bridge | `body-sync.ts`, `shape-sync.ts`, `authoring.ts`, `field-candidates.ts`, `body-dependencies.ts`, `creation-order.ts` | `physics::sync`; compares marked authoring fields against kernel state, as now |
| Publishing poses | writes `Transform` and `GlobalTransform` from kernel body columns | same, reading kernel slices directly |
| Geometry builds | `geometry-builds.ts`, `shapes/build-*.ts`, `marshal*.ts`, `shape-mesh.ts` | `physics::geometry`; no marshalling, kernel takes slices |
| Events and contacts | `events.ts`, `contact.ts`, `kernel/eventbuffers.ts` | `physics::events` over kernel event buffers |
| Joints | `joints.ts`, `solver/*Joint.ts` | `physics::joints` |
| Character | `character.ts` and the mover queries | `physics::character`, `CharacterPlugin` |
| Snapshot | `api/snapshot.ts` | Kernel image bytes plus ECS bindings, as `Plugin::recovery` |
| Typed-array views, pool, WASM loading | `kernel/*columns.ts`, `views.ts`, `pool.ts`, `kernel.ts` | Dropped; direct calls |

#### Coordination audit

Before porting, W-P1 in [roadmap.md](roadmap.md) classifies every file under `standard/physics/{collision, solver, shapes, world, common, api, kernel}` as one of: *mirror* (restates kernel layout for TypeScript: dropped), *wrapper* (one kernel call: replaced by the call), or *logic* (owns behaviour: ported). The result is recorded as a table in this section so later upstream merges know where each file went.

### Determinism

Because the TypeScript build and the Rust build run the same kernel source, the existing world-hash oracles must produce identical hashes in both. That is the strongest parity check we have and it runs in CI for every physics change. Single-thread and multi-thread schedules must also match, as they do today.

## Audio

### What exists

`crates/audio` is the AudioWorklet kernel: 64 pooled voices, 128-frame blocks, gain smoothing, HRTF spatialisation, a master limiter. The worklet (`standard/audio/worklet.ts`) calls its 15 exported functions; control crosses from the main thread as a transferred typed buffer.

### Native design

- `shallot-platform` opens the OS output device ([platform.md](platform.md)) at the device's rate and calls back for blocks. The callback runs the kernel directly, in 128-frame steps with a small carry buffer when the OS block size differs.
- Control blocks cross from the main thread through a single-producer single-consumer ring of fixed-size control buffers (`std::sync::atomic` indices), mirroring the transferred-buffer protocol. The callback never allocates or locks.
- Clip decoding replaces `decodeAudioData`: `shallot-formats` decodes Ogg Vorbis and WAV to f32 PCM, resampled to the device rate with the kernel's existing interpolation.
- `AudioDevice` state (`none`, `suspended`, `running`, `closed`) keeps its meaning. Browsers start suspended until a gesture; native starts running, and `suspended` is used when the OS device is lost or the app is occluded with audio paused.
- `StandardAudioPlugin`'s promise is unchanged: clip playback, basic controls, spatialisation, and `AudioOutput`'s bus ahead of the master limiter.

### Changes to `crates/audio`

None required; it is already `std`-only and target-independent. Its `no_mangle` exports stay for the WASM build, and the native path calls the same functions through Rust.
