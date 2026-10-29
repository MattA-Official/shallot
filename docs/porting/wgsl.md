# WGSL compiler (`shallot-wgsl`) and shader authoring

Without `naga` or a driver that accepts WGSL, the native backends need our own compiler from WGSL to SPIR-V (Vulkan) and to MSL (Metal). The browser backend passes WGSL through. This document fixes the compiler's scope and the way the engine's shaders move from TypeGPU to plain WGSL.

## Inputs we must accept

The engine's shaders today are TypeGPU: 346 `"use gpu"` TypeScript functions transpiled to WGSL and 145 `tgpu.fn` raw WGSL snippets, resolved at run time by `tgpu.resolve`. Whatever we write by hand to replace them, the corpus to support is the WGSL that TypeGPU emits. Constructs observed in that corpus:

| Category | Used |
|---|---|
| Stages | vertex, fragment, compute (`@workgroup_size`) |
| Address spaces | `function`, `private`, `workgroup`, `uniform`, `storage` (`read`, `read_write`) |
| Types | scalars `bool i32 u32 f32`, vectors, matrices, fixed and runtime-sized arrays, structs, `atomic<u32/i32>`, pointers as function parameters |
| Textures | `texture_2d`, `texture_2d_array`, `texture_3d`, `texture_cube`, `texture_cube_array`, `texture_multisampled_2d`, `texture_depth_2d`, `texture_depth_2d_array`, `texture_depth_multisampled_2d`, `texture_storage_1d/2d/2d_array/3d`, `sampler`, `sampler_comparison` |
| Texture builtins | `textureSample` (with offsets), `textureSampleLevel`, `textureSampleGrad`, `textureSampleCompare`, `textureSampleCompareLevel`, `textureGather`, `textureLoad`, `textureStore`, `textureDimensions`, `textureNumLayers` |
| Sync and atomics | `workgroupBarrier`, `storageBarrier`, `workgroupUniformLoad`, `atomicLoad/Store/Add/Sub/Min/Max` |
| Derivatives | `dpdx`, `dpdy`, `fwidth` |
| Bit and packing | `bitcast`, `countOneBits`, `insertBits`, `pack2x16float`, `unpack2x16float/snorm/unorm`, `unpack4x8unorm` and friends |
| Control flow | `if`, `for`, `while`, `loop`/`continuing`/`break if`, `switch`, `discard`, `return` |
| Other | `arrayLength`, `select`, `const` and `let` declarations, `alias`, module-scope `const`, attributes `@builtin @location @interpolate @group @binding @id @align @size` |

Not observed and deferred until something needs them: `f16` (`enable f16`), `override` constants, subgroup builtins (one `subgroups` mention is a feature probe), dual-source blending, clip distances.

The corpus is captured, not guessed: work package W1 in [roadmap.md](roadmap.md) adds `scripts/dump-wgsl.ts` to the TypeScript tree, which resolves every pipeline registered with `precompile` across all examples and writes the final WGSL to `crates/wgsl/corpus/<owner>/<pipeline>.wgsl` with the bind-group layouts beside it. The corpus is regenerated whenever upstream changes shaders, and the compiler's test suite parses, validates and compiles every file in it.

## Pipeline

```
source text ─▶ lexer ─▶ parser ─▶ AST ─▶ resolver ─▶ type checker ─▶ IR ─┬─▶ SPIR-V 1.3 writer
                                         (names,     (WGSL types,       ├─▶ MSL 2.3 writer
                                          ordering)   const-eval,       └─▶ WGSL writer (web, after
                                                      uniformity)            module composition)
```

- **Lexer and parser** follow the WGSL grammar in the W3C Recommendation, including template-list disambiguation. Errors carry byte spans and are formatted like the browser's `getCompilationInfo` messages so the engine's diagnostics read the same.
- **Resolver** handles out-of-order module-scope declarations and shadowing.
- **Type checker** implements WGSL's type rules, abstract numerics and constant evaluation (`const` expressions, including the builtins allowed in constant expressions), address-space and access-mode rules, entry-point interface rules, and resource-binding collection for reflection.
- **Uniformity analysis** is implemented to the spec so that shaders the browser rejects are rejected natively too. It is the last front-end stage to land; until then it is a known gap listed in tests.
- **IR** is an expression-tree-per-function form with explicit types, pointer origins and resolved builtins: close to the AST but desugared (compound assignment, increments, `for`/`while` lowered to `loop`, short-circuit operators made explicit).
- **Reflection** returns entry points, workgroup sizes, bindings with types and sizes, and vertex inputs. `shallot-gpu` uses it for `layout: "auto"` and for validation of bind-group compatibility.

## Backends

### SPIR-V

- Targets SPIR-V 1.3 with `Shader` capability, plus `StorageImageExtendedFormats`, `ImageQuery`, `SampledCubeArray`, `Sampled1D`/`Image1D`, `DerivativeControl` as used.
- Structured control flow emitted directly from the IR's structured form.
- WGSL behaviours that SPIR-V leaves undefined are emulated in generated code: integer division and remainder by zero and `INT_MIN / -1`, shift amounts masked to the bit width, out-of-bounds array and runtime-array indices clamped (WebGPU's robustness choice), `textureLoad` out-of-bounds returning zero, `arrayLength` derived from `OpArrayLength`.
- Layout decorations (`Offset`, `ArrayStride`, `MatrixStride`) computed from WGSL's layout rules; `uniform` buffers validated against the WGSL uniform layout constraints.
- Output checked by `spirv-val` in CI (a developer tool, not linked).

### MSL

- Targets MSL 2.3 (macOS 11+). Resources passed as entry-point arguments with explicit `[[buffer(n)]]`, `[[texture(n)]]`, `[[sampler(n)]]` indices derived from a per-pipeline binding map that `shallot-gpu` shares.
- Runtime-sized arrays get their lengths from an implicit sizes buffer, the approach every WebGPU-on-Metal implementation uses.
- Same undefined-behaviour emulation as SPIR-V.
- Output checked by compiling with `xcrun metal` on macOS CI.

### WGSL

- Emits the composed module as text for the browser. The browser compiles and validates it; our checker still runs in debug builds so native and web report the same errors.

## Shader modules

TypeGPU gave the engine three things besides transpilation: composition of functions across files (`chunk`, `$uses`), struct layouts shared between TypeScript and WGSL (`d.struct`), and specialisation by slots and config. The Rust replacement keeps those three and nothing more.

1. **Source files.** Shaders are `.wgsl` files beside the Rust module that owns them (`crates/standard/src/rendering/shaders/shade.wgsl`), embedded with `include_str!`. Function names match the TypeScript functions they replace, so upstream diffs map across.
2. **Imports.** A file may start with `// #import encode::unpack_hdr_color` lines. `shallot_engine::shader::Composer` resolves imports from a registry of named modules, emits each dependency once, and keeps a source map from composed lines back to files for error messages. This is the `chunk` mechanism, done before compilation instead of by TypeGPU at run time.
3. **Shared layouts.** A `gpu_struct!` macro in `shallot-engine` declares a struct once and generates the `#[repr(C)]` Rust type with WGSL-correct padding, a WGSL `struct` declaration registered as a module, and compile-time size and offset assertions. This replaces `d.struct` and the hand-maintained byte offsets in packers.
4. **Specialisation.** Config-dependent variants (TypeGPU slots and `tgpu.lazy`) become `const` declarations injected by the composer (`const SHADOW_CASCADES: u32 = 4u;`) and, where whole functions differ, alternative modules selected at composition. There is no text preprocessor with `#if`, so every variant is still valid WGSL a browser could check.

Bind-group layouts declared with `tgpu.bindGroupLayout` become Rust constants of `BindGroupLayoutDescriptor` entries plus generated WGSL declarations, so the binding indices exist in one place.

## Porting the engine's shaders

1. Dump the corpus (above).
2. For each owning module, split its pipelines' resolved WGSL back into the functions the TypeScript defines, and place them in `.wgsl` files with imports. Shared helpers (encode, colour, frame uniforms, view uniforms, lighting) become engine and core modules first.
3. Prove equivalence with readback tests: the TypeScript GPU tier has tests for most shaders (`*.gpu.test.ts`, `*.node.ts`); their Rust ports must produce the same buffers and textures within the tolerance the TypeScript test uses.
4. Screenshot parity at the example level catches anything left ([testing.md](testing.md#screenshot-parity)).

Examples that author their own TypeGPU shaders (`low-res`, `materials`, `particles`, `snow-trails`, `tonemapping`, `volumetric-fog`, `voxel`) are translated the same way, into `.wgsl` files in the example.

## Delivery order

1. Triangle subset: lexer, parser, checker for module-scope declarations, vertex and fragment stages, SPIR-V output. (Foundations work.)
2. Full type checker and constant evaluation; the whole corpus parses and checks.
3. SPIR-V for the whole corpus; `spirv-val` clean; the `minimal` example renders.
4. MSL backend.
5. Uniformity analysis.
6. Error-message parity pass against Chrome's Tint for the cases the engine's tests assert.

## Tests

- Spec-derived unit tests per grammar rule and type rule.
- The corpus: every file parses, checks, and compiles for each backend, with `spirv-val` and `metal` validation.
- Execution tests: small compute shaders that exercise each builtin, run through `shallot-gpu`, compared with expected values computed in Rust; the same shaders run in a browser through the TypeScript tree to confirm the expected values.
- Fuzzing the parser with a std-only generator of random token streams and of random valid programs from the grammar.
