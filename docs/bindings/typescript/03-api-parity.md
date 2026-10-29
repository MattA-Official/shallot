# 03 · API parity

The goal is that today's TypeScript API keeps working with mechanical changes at most. This doc
walks the public surface (`package.json` `exports`, roughly 20 entry points) and says what
happens to each part.

## Tiers

| Tier | What | How it is provided | Expected user-visible change |
| --- | --- | --- | --- |
| **A. ECS data** | `component()`, field types, `world.storage()`, columns, `query`, `and`/`or`/`not`, entities, `World` create/add/remove/destroy | Facade over engine memory (L4 + L2) | None |
| **B. Built-in components, resources and enums** | `Transform`, `Camera`, `Body`, `Shape`, `ShapeKind`, `GlobalAmbientLight`, `Devices`, ... | Generated from the reflection dump (L3) | None |
| **C. Handle resources and engine functions** | `Materials.add`, `Meshes`, `Images`, `registerImage`, `applyForce`, `pressKey`, `worldToViewport`, `attachTexture`, ... | Generated stubs that make one boundary call each (L3) | None, except where a function took a browser object (see tier E) |
| **D. Pure TypeScript helpers** | Maths (`lookAtRotation`, `clamp`, `lerp`, `xformPoint`), mesh builders (`cube`, `sphere`), colour helpers | Kept as TypeScript, checked against the Rust results by tests | None |
| **E. Host integration** | `runApp`, loading screens (`shallotDark`), `mountOverlay`, browser input wiring, the Vite plugin, `startup.js`, `testing` helpers | Hand-written per runtime (L4 + L1) | Small; native has no DOM |
| **F. Engine internals** | `GpuTable`, `writeLane`, `uploadGpuTable`, readback pools, `frameEncoder`, `own`, anything marked `@internal` | Not exposed | Code using them is rewritten; only engine code does today |

## Tier A in detail

### Components

```ts
export const Player = component("Player", { yaw: f32, speed: f32, camera: entity }, {
    defaults: () => ({ yaw: 0, speed: 4.317, camera: 0 }),
});
```

`component()` builds the same descriptor object as today. Registration happens when a plugin that
lists it is added to a world: the facade sends the name, field list and defaults to the engine
(requirement R1). Built-in components come from tier B and are the same kind of object, so user
and engine components are indistinguishable from TypeScript.

Re-registering an identical schema under the same key is a no-op, which keeps Vite hot reload
(`Resource.key`, "re-evaluated declarations") working.

### Storage

`world.storage(C).field.get(eid)`, `.set(eid, v)`, `.column`, and for vector fields
`.x/.y/.z/.w` and `.set(eid, x, y, z, w)` all behave as today, over engine memory (see the memory
model in [01-architecture.md](01-architecture.md#memory-model)). Change detection uses the
engine's dirty bitsets, written by `set` directly.

### Queries

`world.query([A, not(B)])` returns an iterable as today. Under the hood, the first iteration in a
frame asks the engine to fill a buffer with matching entity ids, and later iterations of the same
query in the same structural epoch reuse it. `world.only([...])` is a query plus a length check.

### Entities

Entities stay plain `number`s in TypeScript. Today `entity` fields are stored as `Float64Array`
and generation checks go through `world.ref(eid)` / `world.resolve(ref)`. If the Rust port packs
index and generation into a `u64`, the facade still hands TypeScript the index as a `number` and
keeps `EntityRef` for generation-checked references, so no user code changes. Whatever the porting
plan decides, the TypeScript surface does not move.

### Systems and plugins

```ts
const SpinCube: System = { name: "Welcome/SpinCube", update(world) { ... } };
export const Welcome: Plugin = {
    name: "Welcome", components: [WelcomeCube], dependencies: [OrbitPlugin],
    initialize: makeScene, systems: [SpinCube],
};
```

`Plugin` and `System` stay plain objects. When a world adds a plugin:

- Engine plugins (tier B objects such as `OrbitPlugin`, `InputPlugin`) are enabled by name in the
  engine.
- User plugins register their components, then each system is registered as a host callback with
  its group, `first`/`last`/`terminal` and `before`/`after` edges (requirement R3). `before` and
  `after` may point at engine systems by their stable names.
- `initialize`, `setup` and `dispose` run in TypeScript at the same points they run today.
- `recovery` participants (snapshot and restore) are host callbacks too (requirement R11).

`resolvePlugins` stays a pure TypeScript function over plugin objects.

### Resources

Resources split in two:

- **Plain-data resources** (`GlobalAmbientLight`, `Time`, `Devices` state): exposed as objects
  whose properties are accessors over engine memory, so `ambient.color = 0xd8e4ff` still works.
- **Handle resources** (`Materials`, `Meshes`, `Images`, `Views`): exposed as objects whose
  methods are tier C calls. `materials.add(StandardMaterial({...}))` returns the same kind of
  handle number it does today.
- **User resources** (`world.resource(Route)` with a `create` function): stay purely in
  TypeScript, stored by the facade per world. The engine never needs to see them.

### Time and stepping

`world.time`, `world.step(dt)`, `world.tick()`, `pause`, `resume` and `setTimeScale` map one to one
onto boundary calls. `Time.FIXED_DT` and `Time.DEFAULT_DT` are generated constants.

## Tier E in detail

| API | Browser | Native (Bun) |
| --- | --- | --- |
| `runApp({ plugins, loading })` | Instantiates wasm, attaches to `#canvas`, starts `requestAnimationFrame` pumping | Opens an engine window, starts the pumped frame loop |
| Loading screens (`shallotDark({ bootstrap })`) | DOM overlay, as today | Engine-drawn splash with the same colours, or none |
| `mountOverlay(canvas, world)` | As today | Returns `undefined`; callers already guard |
| Browser input (`BrowserInputPlugin`) | L1 forwards DOM events | Not needed; the engine's platform layer supplies input |
| Input injection (`pressKey`, `touchPoint`, ...) | Boundary calls | Boundary calls (used by tests) |
| `@…/vite` | Serves wasm, glue and assets | n/a |
| `@…/testing`, `@…/testing/gpu` (`openGpuApp`) | Playwright configs, as today | Headless engine plus native WebGPU object model |

## Package shape

One npm package, the same `exports` map as today, so import paths do not change except for the
package name if the fork publishes under a new scope (open question Q4). The engine binaries ship
in the package: `shallot.wasm` for the browser, and per-platform dynamic libraries for Bun, picked
at `dlopen` time by `process.platform` and `process.arch`.

The package has zero runtime npm dependencies (today it has five, mostly build-time tooling such as `unplugin-typegpu` and `ci-info`; those move to dev or optional peer dependencies). `typegpu` stays an optional peer for users who
author shaders with it; the engine itself no longer needs it.
