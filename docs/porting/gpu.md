# GPU layer (`shallot-gpu`)

The engine was written against WebGPU and its rules (binding limits, validation, the default-limits floor, error scopes, asynchronous pipeline creation) are part of shallot's contract. So the GPU layer is a WebGPU implementation with Rust naming, not a thinner abstraction. That keeps the port near-faithful at every call site, keeps the WASM backend a direct pass-through, and lets us check our behaviour against the browser's.

## API surface

Types mirror the WebGPU IDL one for one, with `GPU` prefixes dropped and methods in snake case:

`Instance`, `Adapter`, `AdapterInfo`, `Device`, `Queue`, `Buffer`, `Texture`, `TextureView`, `Sampler`, `BindGroupLayout`, `PipelineLayout`, `BindGroup`, `ShaderModule`, `RenderPipeline`, `ComputePipeline`, `CommandEncoder`, `CommandBuffer`, `RenderPassEncoder`, `ComputePassEncoder`, `RenderBundleEncoder`, `RenderBundle`, `QuerySet`, `Surface` (the `GPUCanvasContext` analogue).

- Descriptors are plain structs with `Default` where WebGPU has defaults, borrowing slices and labels (`&'a str`) so building one allocates nothing.
- Handles are reference-counted (`Arc` inside), cheap to clone, and destroyed on last drop or by explicit `destroy()`, matching WebGPU's `destroy()` semantics.
- Enums use WebGPU's names (`TextureFormat::Rgba16Float`, `BufferUsage::STORAGE | COPY_DST`).
- Asynchronous calls (`request_adapter`, `request_device`, `create_render_pipeline_async`, `create_compute_pipeline_async`, `Buffer::map_async`, `Queue::on_submitted_work_done`, `Device::pop_error_scope`) return `std` futures, polled by the engine's executor, and resolve from `Device::poll()`, which the frame loop calls.
- Error model: `push_error_scope`/`pop_error_scope` with `Validation`, `OutOfMemory` and `Internal` filters, an uncaptured-error callback, and a `lost()` future with reason `Destroyed` or `Unknown`. The engine relies on all of these (64 error-scope pairs, 91 device-loss sites).

## What the engine needs

Measured from the TypeScript tree at the snapshot in [README.md](README.md):

| Area | Required |
|---|---|
| Features | `indirect-first-instance`, `timestamp-query`, `depth32float-stencil8`, `bgra8unorm-storage` (each optional, requested only when a plugin declares it) |
| Limits | WebGPU default limits only, including 8 storage buffers per stage. Devices are requested with no `requiredLimits` |
| Formats | `rgba16float`, `rgba8unorm`, `rgba8unorm-srgb`, `bgra8unorm`, `r32uint`, `r32float`, `r32sint`, `r16float`, `rg16float`, `rg32float`, `rgba32float`, `r8unorm`, `rg8unorm`, `rgba16uint`, `r16uint`, `rgb10a2unorm`, `rg11b10ufloat`, `depth32float`, `depth24plus`, `depth24plus-stencil8` |
| Textures | 1D storage, 2D, 2D array, 3D, cube, cube array, multisampled 2D, depth 2D/array/multisampled, storage 2D/2D array/3D |
| Commands | render and compute passes, `draw_indexed_indirect`, `draw_indirect`, `dispatch_workgroups_indirect`, render bundles, `copy_buffer_to_texture`, `copy_texture_to_buffer`, `write_texture`, `write_buffer`, query sets with `resolve_query_set`, timestamp writes on passes, multisample resolve |
| Pipelines | async creation (39 sites), shader compilation info (2 sites) |
| Readback | `map_async` (18 sites) |
| Presentation | surface `configure` with format and alpha mode, `get_current_texture`, present |

`copy_external_image_to_texture` is used once (image upload) and becomes a `write_texture` of decoded pixels.

## Structure

```
shallot-gpu
├── api/          public types, descriptors, enums (backend-independent)
├── validate/     WebGPU validation rules, limits, feature gating, error scopes
├── track/        resource usage and state tracking per command buffer
├── backend/
│   ├── vulkan/   Vulkan 1.1 + extensions, loaded with dlopen
│   ├── metal/    Metal through the Objective-C runtime
│   └── web/      wasm32: forwards every call to navigator.gpu through the JS shim
└── shader/       glue to shallot-wgsl: reflection, layout derivation, per-backend codegen
```

- **Validation lives in the front end**, shared by Vulkan and Metal. It implements the WebGPU specification's validation steps for each call and reports errors through error scopes exactly as a browser would. The engine's tests that expect validation errors (for example the storage-binding checks in `engine/runtime/gpu.ts`) therefore hold natively. The web backend skips our validation because the browser does it.
- **State tracking** records each resource's usage per pass and inserts barriers (Vulkan) or relies on Metal's automatic hazard tracking for untracked-mode-off resources. WebGPU's usage scope rules (a resource may not be both writable storage and sampled in one pass) are validated, not just synchronised.
- **Bind groups** map to Vulkan descriptor sets (set = group index) and to Metal argument buffers tier 1 or direct slots; the WGSL backend emits matching bindings.

## Vulkan backend

- Target Vulkan 1.1 with `VK_KHR_dynamic_rendering`, `VK_KHR_timeline_semaphore`, `VK_KHR_synchronization2` when present, falling back to render passes and binary semaphores, so lavapipe (used by upstream CI as `scripts/lavapipe.sh`) and older Linux drivers work.
- Entry points loaded through `vkGetInstanceProcAddr`/`vkGetDeviceProcAddr` into function tables generated by hand for the subset we call. The table is listed in `backend/vulkan/fns.rs` and grows with use.
- Memory: our own sub-allocator over `vkAllocateMemory` (linear and buddy pools per memory type). Mapped staging ring for `write_buffer`/`write_texture`.
- Queues: one graphics+compute queue. Submissions are fenced with a timeline semaphore so `on_submitted_work_done` and `map_async` resolve from a single counter.
- Surfaces: Win32, Xlib, Wayland; `FIFO` present mode by default (vsync, matching rAF), `MAILBOX` opt-in.
- Timestamp queries map to `vkCmdWriteTimestamp`; `depth32float-stencil8` to `D32_SFLOAT_S8_UINT` where supported.
- Debug builds enable `VK_LAYER_KHRONOS_validation` when installed; a validation message fails tests.

## Metal backend

- Objective-C runtime calls only, as in the platform crate. `MTLCreateSystemDefaultDevice` is the one C entry point.
- Shaders come from `shallot-wgsl` as MSL source compiled with `newLibraryWithSource:`; pipeline caches persist compiled libraries keyed by shader hash.
- Buffers in shared or private storage depending on usage; staging for private resources.
- Indirect draws map directly; `indirect-first-instance` is native.
- Timestamp queries through `MTLCounterSampleBuffer` where the GPU supports stage-boundary sampling, otherwise the feature is not offered.

## Web backend

On `wasm32` every API call forwards to the browser through the hand-written shim described in [wasm.md](wasm.md). Objects are integer handles into a JS-side table. Descriptor structs are serialised into a compact binary command format to keep the number of boundary crossings low.

## Milestones

1. **Triangle**: instance, adapter, device, surface, buffer, shader module, render pipeline, one render pass, present. Headless variant renders to a texture and reads it back. (Foundations work.)
2. **Engine bring-up**: everything `minimal` needs (bind groups, uniform and storage buffers, depth, MSAA, indirect draws, compute, error scopes, async pipelines, readback).
3. **Full surface**: render bundles, query sets, all formats and texture kinds in the table above, every feature in the feature table.
4. **Metal** parity with Vulkan on the same tests.
5. **Web** pass-through.

## Tests

- Per-call validation tests written from the WebGPU specification; where possible the same test runs against a browser through the TypeScript engine's GPU tier to confirm the browser agrees.
- Golden readback tests for every format's read and write path.
- Every GPU test runs on lavapipe in CI, and on real hardware on demand, like upstream's GPU tier.
