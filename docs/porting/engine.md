# Engine (`shallot-engine`)

The engine layer owns domain-free storage and scheduling, the app and plugin lifecycle, and the per-world GPU context. Its contracts are the ones in `CONTRIBUTING.md` under "Device and world ownership", "GPU tables", "Readback" and "Frame schedule"; this document says how each is expressed in Rust.

## Entities

- `Entity` is a `u32` index ("eid"). Indices recycle; a per-index generation tells stale references apart.
- `EntityRef` packs index and generation into a `u64` (TypeScript packs them into a float64's integer range), survives eid recycling, and is what events and callbacks carry, as today.
- `world.create()`, `destroy`, `exists`, `ref`, `resolve`, `entities()` keep their meaning.

## Components

Components are runtime schemas, because three consumers need them as data: snapshots, GPU tables, and the bindings boundary (other languages register components at run time).

```rust
shallot::component! {
    /// Doc comments carry over.
    pub Transform {
        translation: vec3,
        rotation: vec4 = [0.0, 0.0, 0.0, 1.0],
        scale: vec3 = [1.0, 1.0, 1.0],
    }
}
```

- The macro is `macro_rules!` (no proc-macro crates are allowed, and a proc macro would need `syn`). It produces a zero-sized marker type `Transform`, a `const SCHEMA: Schema` listing fields, lanes and defaults, and a typed accessor struct `TransformStorage<'w>` with one typed column per field.
- Field types are exactly the TypeScript ones: `f32 i32 u32 u8 u16 f16 vec2 vec3 vec4 entity`. `vec3` occupies four lanes, as now, so GPU uploads are direct copies.
- `requires` and per-world `defaults` closures are supported as in `component(key, fields, options)`.
- A schema can also be built at run time (`Schema::builder("Name").field("speed", FieldType::F32)…`) and registered by a plugin; that path is what `shallot-abi` exposes.
- Component keys are stable strings and must be unique per world; the numeric id is assigned at registration.

## Storage

- Each field is an eid-indexed column (`Vec<T>` with `lanes` values per eid), grown to the highest eid holding the component, matching "a one-holder component on a high eid costs its row bytes per eid".
- Change tracking: one dirty bit per eid per field, grouped in 32-eid words, each word stamped with the world's change tick, as in `storage.ts`. Field marks are frame-scoped with the same clearing rules.
- Access is through `world.storage::<Transform>()`, which returns the typed accessor. Writes go through `set`, which marks; `get` reads. Columns are also reachable as raw slices for bulk work and for the bindings boundary (pointer, length, and a generation counter bumped on reallocation).
- Borrowing: `World` hands out storages through interior mutability checked at run time (one `RefCell`-style flag per column), because systems routinely read one component while writing another, and the TypeScript API allows any mix. A violation is a panic naming both borrows, caught by the scheduler like any system error.

## Queries

- `world.query(&[Term::with::<A>(), Term::not::<B>()])` returns an iterator of eids; `and`, `or`, `not` compose as in `query.ts`.
- Registered queries are kept incrementally on membership change, and iteration order is ascending eid, which determinism depends on.
- `world.only(terms)` returns the single match or panics with the count, as now.

## Resources

- `Resource<T>` is a typed key with a `create(&World) -> T` function, as the TypeScript `{ create }` objects are. `world.resource::<GlobalAmbientLight>()` returns a mutable guard.
- Plugin state lives in resources, never in statics, so several worlds coexist in one process.

## Systems and scheduling

```rust
pub struct System {
    pub name: &'static str,
    pub group: Group,                // Setup, Fixed, Simulation (default), Draw
    pub update: fn(&mut World),
    pub setup: Option<fn(&mut World)>,
    pub dispose: Option<fn(&mut World)>,
    pub first: bool, pub last: bool, pub terminal: bool,
    pub before: &'static [&'static System],
    pub after: &'static [&'static System],
}
```

- Systems are `'static` values compared by address, as TypeScript compares object identity. Closures are supported through `System::boxed(name, move |world| …)` for runtime-added systems such as the HUD readers.
- Ordering rules, cycle refusal, terminal and boundary slots port from `scheduler.ts` unchanged.
- `Time` fields and semantics are identical: fixed 60 Hz ticks from an accumulator, at most four per step, `fixed_alpha` for interpolation, pause and time-scale affecting `step` only.
- `world.step(dt)` and `world.tick()` are the test entry points, as in TypeScript.
- A panicking system is caught, logged with its name and paused (`log_and_pause_system_errors`).

## Snapshot and restore

- `Plugin::recovery` returns a `Recovery { snapshot: fn(&World) -> Box<dyn Any>, restore: fn(&mut World, &dyn Any) }` or declares `Stateless`. Snapshot images are independent and reusable, and restore sees recovered ECS identity, fields and `fixed_tick`.
- ECS state snapshots copy columns, membership, generations and the change clock.
- Byte-blob participants (`snapshot -> Vec<u8>`) exist for the bindings boundary, where a host owns state the engine cannot see.
- Determinism holds within one runtime and engine version. Rust and TypeScript physics hashes must match each other, because the kernel is the same code (see [physics-audio.md](physics-audio.md#determinism)).

## App and plugins

```rust
pub trait Plugin: 'static {
    fn name(&self) -> &'static str;
    fn deterministic(&self) -> bool { true }
    fn recovery(&self) -> Recovery { Recovery::None }
    fn components(&self) -> &[&'static Schema] { &[] }
    fn systems(&self) -> &[&'static System] { &[] }
    fn dependencies(&self) -> &[&'static dyn Plugin] { &[] }
    fn gpu(&self) -> Option<GpuRequirements> { None }
    fn initialize<'a>(&'a self, world: &'a mut World, progress: Progress) -> LocalBoxFuture<'a, Result<()>> { ready_ok() }
    fn warm<'a>(&'a self, world: &'a mut World, progress: Progress) -> LocalBoxFuture<'a, Result<()>> { ready_ok() }
    fn dispose(&self, world: &mut World) {}
}
```

- Plugins are usually unit structs in statics (`pub static ORBIT: OrbitPlugin = OrbitPlugin;`) so they can be listed as dependencies by reference, mirroring the TypeScript object literals.
- `LocalBoxFuture` is `Pin<Box<dyn Future<Output = T> + 'a>>`, defined in the engine with `std` types only.
- `AppConfig { plugins, loading, defaults, exclude, setup, ui, device, adapter, pixel_ratio }` keeps every field. `ui` takes a per-frame drawing closure instead of a DOM container ([assets-ui.md](assets-ui.md#ui)).
- `create_app(config)` builds a world without running a loop (tests and headless use). `run_app(config)` opens the window through `shallot-platform` and runs the frame loop. Both return `App { world, dispose }`.
- Build order, dependency validation, GPU requirement resolution from enabled plugins only, `Loading::{show, update, complete, notice, error}`, and the pipeline-precompile drain with its "pipeline created after the drain" warning all port from `engine/app/index.ts` and `engine/runtime/gpu.ts`.

### Executor

The engine's executor is a single-threaded queue of `LocalBoxFuture`s and a `Waker` that re-queues the task. The frame loop polls it once per frame and after `Device::poll`, so GPU futures (pipeline creation, `map_async`) and asset futures complete on the main thread. During the build the loading screen draws between polls. It is about 150 lines and has no other role.

## GPU context

- `WorldGpu` wraps the `shallot_gpu::Device`, owns the world's lazy frame encoder and its single submission per frame, tracks retired resources until the frame fence passes, and records pipeline ownership for diagnostics.
- `GpuTable<Row>` ports `table.ts`: one `gpu_struct!` row type per table, eid-indexed or active-row lists, bulk fill from bound columns or byte ranges, uploads ordered across buffer growth, runtime-sized shader arrays.
- `precompile(world, owner, pipeline_future)` and `precompile_all` keep compile-under-the-loading-screen.
- Binding-limit checks (`check_pipeline_storage_bindings`, `check_texture_limits`) port as is; with our own validator they also run natively.

## Readback

`readback.ts` ports unchanged in meaning: steady play reads nothing back; an asynchronous result enters simulation only as an accepted record with an assigned tick; fixed consumers of readback declare `deterministic: false`.

## Maths

`math` provides the TypeScript free functions over arrays (`perspective`, `look_at`, `compose_mat4`, `slerp` …) with identical argument order and results, so ported call sites read the same. f64 is used exactly where the TypeScript uses `Float64Array` (aim and placement maths); everything stored in components is f32. Tests compare against values captured from the TypeScript functions.

## Tests to port first

`engine/ecs/*.test.ts` (26 files) and `engine/app/*.test.ts` (8) are the first parity checklist: they define storage, query, scheduling, snapshot and lifecycle behaviour without a GPU, and they are what the bindings boundary must also satisfy.
