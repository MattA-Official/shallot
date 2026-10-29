# Bindings boundary (`shallot-abi`)

Matt's requirement: a clean, Godot-like way for other languages to use the engine, designed in Rust with no C layer. People who want another language write their own runtime against it. This document is the boundary's design; the TypeScript binding that will be its first client is planned separately (see the end of this page).

## Principles

1. **Rust is the source of truth.** The boundary is declared once in Rust, in `crates/abi`. We publish no C headers and maintain no C code. The machine-readable API description (below) is generated from the Rust declaration, and binding authors generate their language's glue from that description, as Godot's binding generators read `extension_api.json`.
2. **`extern "C"` only as a calling convention.** Rust has no stable ABI, so exported functions use `extern "C"` and `#[repr(C)]` types. That is the only "C" in the design: no C types beyond fixed-width integers, floats and pointers, no C strings, no C library.
3. **Data-oriented and bulk.** The engine is a columnar ECS. The boundary exposes columns and bulk operations, never per-entity callbacks or per-field calls in hot paths.
4. **Same boundary on every target.** One declaration compiles to native dynamic-library exports and to `wasm32` exports with the same names and signatures.
5. **Additive evolution.** A major version changes only on removal or signature change; everything else is added and discoverable.

## Two ways in

| Mode | Who loads whom | Use |
|---|---|---|
| **Host mode** | A foreign runtime loads the engine (`libshallot.so`, `shallot.dll`, `libshallot.dylib`, or `shallot.wasm`) and calls its exports | The TypeScript binding (Bun FFI natively, WASM in browsers), Python, C# … driving the whole app |
| **Extension mode** | A Rust app loads an extension library, GDExtension-style, and calls its `shallot_extension_entry(api: &Api, ext: &mut Extension)` | Plugins written in another language and compiled to a native library, registered into a Rust game |

Both modes use the same `Api` function table, so an extension and a host see identical functions. In host mode the table is reached through the `shallot_api(version) -> *const Api` export; individual functions are also exported by name for runtimes that bind symbols directly (WASM, Bun FFI).

## Surface

Grouped by area. Every item lists the TypeScript binding requirement it answers (numbers from `docs/bindings/typescript/04-boundary-requirements.md`).

### Versioning (R13)

- `boundary_version() -> (major, minor)`; a host checks it first.
- `has_feature(name) -> bool` for optional parts (`gpu`, `audio`, `physics`, `ui`, `window`).

### Errors (R6)

- Every fallible function returns `Status` (`i32`: `Ok = 0`, then `InvalidArgument`, `NotFound`, `WrongState`, `Gpu`, `Panic`, …).
- `last_error(out_ptr, out_cap) -> len` copies the thread's last error message as UTF-8.
- No unwinding crosses the boundary: every export runs inside `catch_unwind` and turns a panic into `Status::Panic` with the message.
- Strings are always `(ptr, len)` UTF-8, never NUL-terminated.

### Worlds and apps (R7)

- `app_create(config) -> AppHandle`, `app_dispose`, `app_world(app) -> WorldHandle`.
- Host-driven stepping: `world_step(world, dt)`, `world_tick(world)`, as headless tests use.
- Engine-owned loop: `app_run(app)` opens the window and runs until exit, calling host systems as below.
- `app_frame(app)` runs exactly one frame for hosts that own their loop (the browser's `requestAnimationFrame`).

### Components (R1, R2)

- `component_register(world, schema) -> ComponentId`, where `schema` is `(name, fields[], defaults, requires[])` with field types `f32 i32 u32 u8 u16 f16 vec2 vec3 vec4 entity`, the same set as Rust's `component!`.
- `component_find(world, name) -> ComponentId` for built-ins.
- `column(world, component, field) -> ColumnView { ptr, len, lanes, generation }`. Columns are engine-owned eid-indexed SoA arrays. `generation` changes whenever the column reallocates; hosts keep zero-copy views (typed arrays over WASM memory or Bun FFI pointers) and refresh them when the generation moves.
- `column_mark(world, component, field, eid_ptr, count)` marks writes made through a view, so change tracking and GPU uploads see them. `column_set_*` bulk setters mark implicitly.

### Entities and queries (R4)

- Bulk: `entities_create(world, count, out_ptr)`, `entities_destroy(world, ptr, count)`, `component_add(world, component, eid_ptr, count, values_ptr)`, `component_remove(…)`.
- `query_register(world, terms) -> QueryHandle`, `query_count(world, q)`, `query_fill(world, q, out_ptr, cap) -> written`, writing eids into a caller buffer in ascending order.
- `entity_ref(world, eid) -> u64`, `entity_resolve(world, ref) -> eid or NONE`.

### Systems (R3, R12)

- `system_register(world, desc, callback, user_data)`, where `desc` carries name, group, `first/last/terminal` and `before/after` lists by system name. The engine calls the host callback once per system run, on the main thread, never per entity and never from a worker thread.
- `system_remove(world, id)`.

### Resources (R5)

- POD resources: `resource_register(world, schema)` and `resource_view(world, id) -> (ptr, len)`, for plain data like `GlobalAmbientLight`.
- Handle resources: opaque handles with methods exported as functions, for example `materials_add(world, desc) -> MaterialHandle`, `materials_update(world, handle, desc)`. The generated API description names them as methods so bindings can present `world.resource(Materials).add(…)`.

### Plugins

- Built-in plugins by name: `plugin_find(name) -> PluginHandle`; an app config lists handles.
- Host plugins: `plugin_define(desc)` with components, systems, dependencies, GPU requirements and lifecycle callbacks (`initialize`, `warm`, `dispose`). Asynchronous lifecycle steps report completion through tickets (below).

### Snapshot and restore (R11)

- `world_snapshot(world) -> SnapshotHandle`, `world_restore(world, snapshot)`, `snapshot_bytes`.
- Host recovery participants: `recovery_register(world, plugin, snapshot_cb, restore_cb, user_data)`; callbacks exchange an opaque byte blob for state the host owns.

### Asynchronous work (R12)

- Asset loads, GPU readback and other async operations return a `Ticket`. `ticket_poll(ticket) -> Pending | Ready | Failed` and `ticket_take(ticket, out)`; tickets also complete during `app_frame`/`world_step`. No callback is ever invoked from an engine worker thread.

### Input (R14)

The test drivers in `core/input` become exports: `input_press_key(world, code)`, `input_release_key`, `input_pointer_move`, `input_press_button`, `input_release_button`, `input_wheel`, `input_touch_point`, `input_cancel_touch`, `input_pointer_lock_changed`, `input_focus_canvas`, `input_blur_canvas`, `viewport_resize`. Hosts that own their window (the browser) forward events through these; tests drive input through them.

### GPU access (R10)

- `world_gpu_device(world) -> DeviceHandle`; buffers, textures, bind groups and pipelines as handles with create and write functions mirroring `shallot-gpu`.
- Shader modules accept WGSL text, so hosts that generate WGSL (TypeGPU in TypeScript) can author custom materials and passes. `material_type_register(world, wgsl, layout)` exposes the standard renderer's extension point.

### Reflection (R9)

- `api_description(out_ptr, cap)` returns a JSON document (generated at build time from the `boundary!` declaration plus registered built-ins) listing every function with its signature, every built-in component and resource schema, every plugin and its dependencies, and every enum. `shallot-cli api-dump` writes it to a file. Binding generators consume it the way Godot's consume `extension_api.json`.

### WASM imports (R15)

On `wasm32` the engine imports a documented, versioned list of host functions for WebGPU, input events, audio output, fetch and time ([wasm.md](wasm.md#imports)). The TypeScript binding's browser glue implements that list, and a Rust-only web build loads the same glue.

## How it is declared

```rust
shallot_abi::boundary! {
    /// Registers a component from a runtime schema.
    fn component_register(world: WorldHandle, schema: &SchemaDesc) -> Result<ComponentId>;
    /// Writes up to `cap` matching entity ids into `out`.
    fn query_fill(world: WorldHandle, query: QueryHandle, out: OutSlice<u32>) -> Result<usize>;
}
```

`boundary!` is a `macro_rules!` macro that generates, for each function: the `extern "C"` export with `#[no_mangle]` and the `shallot_` prefix, panic catching and `Status` conversion, argument checks (null pointers, lengths, handle generations), the `Api` table entry, and the function's entry in the reflection document. Handles are `u64` with an index and generation so a stale handle is an error, not undefined behaviour.

## What is not in the boundary

- Rust generics, traits and closures. Rust users use the `shallot` crate directly; the boundary is for everything else.
- Per-entity accessors in hot paths (they exist for convenience but bindings are steered to columns).
- Threads: the boundary is main-thread only, apart from `last_error`, which is thread-local.

## TypeScript binding

Placeholder. A separate planning thread owns it and writes `docs/bindings/typescript/`. Its requirements on this boundary (R1–R15) are answered above; disagreements are settled in that plan and reflected back here.
