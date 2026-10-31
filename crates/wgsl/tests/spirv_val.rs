//! Compiles shaders to SPIR-V and MSL, and checks the output with `spirv-val` and Apple's
//! Metal compiler when they are installed. Both are developer tools, run as processes, never
//! linked. Setting `SHALLOT_REQUIRE_METAL` makes a missing Metal compiler a failure, for CI.

use shallot_wgsl::{compile, Module, MslOptions, MslResource, MslSlot, SpirvOptions};
use std::process::Command;

fn validate_msl(name: &str, module: &Module) {
    let output = module
        .to_msl(&MslOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let dir = std::env::temp_dir().join("shallot-wgsl-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.metal"));
    std::fs::write(&path, &output.source).unwrap();
    let found = Command::new("xcrun")
        .args(["-sdk", "macosx", "-f", "metal"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        if std::env::var_os("SHALLOT_REQUIRE_METAL").is_some() {
            panic!("the Metal compiler is required but xcrun cannot find it");
        }
        eprintln!("Metal compiler not installed; skipping MSL validation of {name}");
        return;
    }
    let air = dir.join(format!("{name}.air"));
    let result = Command::new("xcrun")
        .args([
            "-sdk",
            "macosx",
            "metal",
            "-std=macos-metal2.3",
            "-Werror",
            "-c",
        ])
        .arg(&path)
        .arg("-o")
        .arg(&air)
        .output()
        .unwrap();
    if !result.status.success() {
        panic!(
            "{name} failed MSL compilation:\n{}\n{}",
            String::from_utf8_lossy(&result.stderr),
            numbered(&output.source)
        );
    }
}

fn numbered(source: &str) -> String {
    source
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{:4} {l}\n", i + 1))
        .collect()
}

fn validate(name: &str, source: &str) {
    let module = compile(source).unwrap_or_else(|e| panic!("{name}:\n{}", e.render(source)));
    validate_msl(name, &module);
    let words = module
        .to_spirv(&SpirvOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(words[0], 0x0723_0203);
    if Command::new("spirv-val").arg("--version").output().is_err() {
        eprintln!("spirv-val not installed; skipping validation of {name}");
        return;
    }
    let dir = std::env::temp_dir().join("shallot-wgsl-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.spv"));
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(&path, bytes).unwrap();
    let output = Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.1")
        .arg(&path)
        .output()
        .unwrap();
    if !output.status.success() {
        let dis = Command::new("spirv-dis")
            .arg(&path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        panic!(
            "{name} failed validation:\n{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            dis.unwrap_or_default()
        );
    }
}

#[test]
fn triangle() {
    validate("triangle", include_str!("../../gpu/examples/triangle.wgsl"));
}

#[test]
fn uniforms_and_structs() {
    validate(
        "uniforms",
        r#"
        struct Light { direction: vec3f, color: vec3f, intensity: f32 }
        struct Frame { view_proj: mat4x4f, lights: array<Light, 4>, count: u32 }
        @group(0) @binding(0) var<uniform> frame: Frame;

        struct VertexIn { @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) id: u32 }
        struct VertexOut {
            @builtin(position) clip: vec4f,
            @location(0) normal: vec3f,
            @location(1) @interpolate(flat) id: u32,
        }

        @vertex fn vs(v: VertexIn) -> VertexOut {
            var out: VertexOut;
            out.clip = frame.view_proj * vec4f(v.position, 1.0);
            out.normal = v.normal;
            out.id = v.id;
            return out;
        }

        fn shade(n: vec3f) -> vec3f {
            var total = vec3f(0.0);
            for (var i = 0u; i < min(frame.count, 4u); i++) {
                let light = frame.lights[i];
                total += light.color * max(dot(n, -light.direction), 0.0) * light.intensity;
            }
            return total;
        }

        @fragment fn fs(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
            let n = normalize(select(-in.normal, in.normal, front));
            if in.id == 7u && front { discard; }
            return vec4f(shade(n), 1.0);
        }
        "#,
    );
}

#[test]
fn compute_and_storage() {
    validate(
        "compute",
        r#"
        struct Particles { count: u32, data: array<vec4f> }
        @group(0) @binding(0) var<storage, read_write> particles: Particles;
        @group(0) @binding(1) var<storage> weights: array<f32>;
        var<workgroup> scratch: array<f32, 64>;
        var<private> seed: u32 = 12345u;

        fn rand() -> f32 {
            seed = seed * 1664525u + 1013904223u;
            return f32(seed >> 8u) / 16777216.0;
        }

        @compute @workgroup_size(64)
        fn main(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) local: u32) {
            let n = arrayLength(&particles.data);
            if id.x >= n { return; }
            scratch[local] = weights[id.x % arrayLength(&weights)];
            workgroupBarrier();
            var p = particles.data[id.x];
            var i = 0;
            loop {
                if i >= 4 { break; }
                p.y -= scratch[(local + u32(i)) % 64u] * 0.01 + rand() * 0.001;
                continuing { i += 1; }
            }
            switch i32(p.w) {
                case 0, 1: { p.w = 2.0; }
                default: { p.w = f32(bitcast<u32>(p.w) & 0xffu); }
            }
            let q = i / (i - 4);
            particles.data[id.x] = p + vec4f(f32(q));
            particles.count = max(particles.count, id.x);
        }
        "#,
    );
}

#[test]
fn expressions() {
    validate(
        "expressions",
        r#"
        const SCALE = 2.5;
        const OFFSETS = array(vec2(0.0, 1.0), vec2(1.0, 0.0));
        alias Color = vec4<f32>;

        fn mixes(a: vec3f, b: vec3f, t: f32) -> vec3f {
            let m = mat3x3f(a, b, cross(a, b));
            let v = m * vec3f(1.0, 2.0, 3.0) + transpose(m) * a;
            return mix(v, b, t) + smoothstep(vec3f(0.0), vec3f(1.0), a) + step(a, b);
        }

        @fragment fn fs(@builtin(position) p: vec4f, @location(0) @interpolate(linear) uv: vec2f,
                        @location(1) @interpolate(flat) k: i32) -> @location(0) Color {
            var total = vec3f(0.0);
            var j = 0;
            while j < 2 {
                total += vec3f(OFFSETS[j] * SCALE, 0.0);
                j++;
            }
            let bits = countOneBits(u32(k)) + reverseBits(1u) + firstLeadingBit(u32(k));
            let packed = unpack4x8unorm(pack4x8unorm(vec4f(uv, 0.5, 1.0)));
            let ok = all(uv > vec2f(0.0)) || any(vec2(k < 0, false));
            var c = Color(mixes(total, vec3f(uv, 1.0), fract(p.x)), 1.0);
            c.a = select(0.5, 1.0, ok) * f32(bits) * packed.z;
            c = c * clamp(length(uv), 0.0, 1.0) + vec4f(f32(k % 3), f32(k << 2u), f32(-k), f32(~k));
            return c;
        }
        "#,
    );
}

/// The string operands of every `OpName`.
fn debug_names(words: &[u32]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 5;
    while i < words.len() {
        let (count, opcode) = ((words[i] >> 16) as usize, words[i] & 0xFFFF);
        if opcode == 5 {
            let bytes: Vec<u8> = words[i + 2..i + count]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .take_while(|&b| b != 0)
                .collect();
            names.push(String::from_utf8(bytes).unwrap());
        }
        i += count.max(1);
    }
    names
}

/// MoltenVK translates SPIR-V to MSL with SPIRV-Cross, which names functions after `OpName`. An
/// entry point's body named like the entry point itself becomes a second MSL function of the
/// same name, and Metal rejects the pipeline.
#[test]
fn entry_point_bodies_are_not_named_like_their_entry_points() {
    let module = compile(include_str!("../../gpu/examples/triangle.wgsl")).unwrap();
    let names = debug_names(&module.to_spirv(&SpirvOptions::default()).unwrap());
    for entry in ["vs_main", "fs_main"] {
        assert!(!names.iter().any(|n| n == entry), "{names:?}");
        assert!(names.contains(&format!("{entry}_body")), "{names:?}");
    }
}

/// Vulkan forbids explicit layouts outside buffers (VUID-StandaloneSpirv-None-10684), so a
/// workgroup array carries no `ArrayStride`, while a storage buffer's array does.
#[test]
fn only_buffer_types_have_explicit_layouts() {
    let strides = |source: &str| {
        let words = compile(source)
            .unwrap()
            .to_spirv(&SpirvOptions::default())
            .unwrap();
        let mut count = 0;
        let mut i = 5;
        while i < words.len() {
            let (len, opcode) = ((words[i] >> 16) as usize, words[i] & 0xFFFF);
            // OpDecorate %id ArrayStride
            if opcode == 71 && words[i + 2] == 6 {
                count += 1;
            }
            i += len.max(1);
        }
        count
    };
    let workgroup = r#"
        var<workgroup> scratch: array<f32, 64>;
        @compute @workgroup_size(64) fn main(@builtin(local_invocation_index) i: u32) {
            scratch[i] = f32(i);
        }
    "#;
    assert_eq!(strides(workgroup), 0);
    let storage = r#"
        @group(0) @binding(0) var<storage, read_write> data: array<f32, 64>;
        @compute @workgroup_size(64) fn main(@builtin(local_invocation_index) i: u32) {
            data[i] = f32(i);
        }
    "#;
    assert_eq!(strides(storage), 1);
    validate("workgroup_array", workgroup);
}

#[test]
fn textures_and_samplers() {
    validate(
        "textures",
        r#"
        @group(0) @binding(0) var color: texture_2d<f32>;
        @group(0) @binding(1) var layers: texture_2d_array<f32>;
        @group(0) @binding(2) var sky: texture_cube<f32>;
        @group(0) @binding(3) var volume: texture_3d<f32>;
        @group(0) @binding(4) var shadow: texture_depth_2d_array;
        @group(0) @binding(5) var ids: texture_2d<u32>;
        @group(0) @binding(6) var msaa: texture_multisampled_2d<f32>;
        @group(0) @binding(7) var line: texture_1d<f32>;
        @group(1) @binding(0) var linear: sampler;
        @group(1) @binding(1) var compare: sampler_comparison;

        fn fetch_id(t: texture_2d<u32>, p: vec2i) -> u32 {
            return textureLoad(t, p, 0).x;
        }

        @fragment
        fn fs(@builtin(position) pos: vec4f, @location(0) uv: vec2f) -> @location(0) vec4f {
            var c = textureSample(color, linear, uv);
            c += textureSample(color, linear, uv, vec2i(1, -1));
            c += textureSampleBias(layers, linear, uv, 2, 0.5);
            c += textureSampleLevel(sky, linear, vec3f(uv, 1.0), 0.0);
            c += textureSampleGrad(volume, linear, vec3f(uv, 0.5), vec3f(0.1), vec3f(0.1));
            c += textureGather(1, color, linear, uv);
            c += textureGatherCompare(shadow, compare, uv, 1u, 0.5);
            c += textureSampleLevel(line, linear, uv.x, 0.0);
            let s = textureSampleCompare(shadow, compare, uv, 0, 0.5)
                + textureSampleCompareLevel(shadow, compare, uv, 1, 0.25, vec2i(2, 2))
                + textureSampleLevel(shadow, linear, uv, 0, 0);
            let p = vec2i(pos.xy);
            c += textureLoad(color, p, 1) + textureLoad(msaa, p, 3);
            let size = textureDimensions(color) + textureDimensions(color, 1u)
                + textureDimensions(layers) + textureDimensions(sky);
            let counts = textureNumLayers(layers) + textureNumLevels(color)
                + textureNumSamples(msaa) + textureDimensions(volume).z + textureDimensions(line);
            return c * s + vec4f(f32(size.x + counts + fetch_id(ids, p)));
        }
        "#,
    );
}

#[test]
fn storage_textures() {
    validate(
        "storage_textures",
        r#"
        @group(0) @binding(0) var src: texture_storage_2d<rgba8unorm, read>;
        @group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
        @group(0) @binding(2) var counts: texture_storage_2d_array<r32uint, read_write>;
        @group(0) @binding(3) var wide: texture_storage_3d<rg32float, write>;
        @group(0) @binding(4) var line: texture_storage_1d<rgba32sint, write>;

        @compute @workgroup_size(8, 8)
        fn main(@builtin(global_invocation_id) id: vec3u) {
            let p = vec2u(id.xy);
            let texel = textureLoad(src, p);
            textureStore(dst, p, texel * 2.0);
            let n = textureLoad(counts, vec2i(p), 1).x;
            textureStore(counts, p, 1u, vec4u(n + 1u));
            textureStore(wide, id, vec4f(1.0));
            textureStore(line, i32(id.x), vec4i(-1));
            let size = textureDimensions(dst) + textureDimensions(counts) + vec2u(textureNumLayers(counts));
            textureStore(dst, size - 1u, vec4f(0.0));
        }
        "#,
    );
}

#[test]
fn msl_bindings_and_layouts() {
    let source = r#"
        struct Instance {
            offset: vec3f,
            scale: f32,
            tint: vec3f,
        }
        struct Instances { count: u32, items: array<Instance> }
        struct Camera { view_proj: mat4x4f, eye: vec3f }
        struct Out {
            @builtin(position) clip: vec4f,
            @location(0) @interpolate(flat) id: u32,
            @location(1) tint: vec3f,
        }
        struct FragOut {
            @location(0) color: vec4f,
            @builtin(frag_depth) depth: f32,
        }

        @group(1) @binding(3) var<storage, read> instances: Instances;
        @group(0) @binding(0) var<uniform> camera: Camera;
        @group(0) @binding(1) var tex: texture_2d<f32>;
        @group(0) @binding(2) var samp: sampler;
        @group(2) @binding(0) var<storage, read_write> hits: array<u32>;
        var<private> calls: u32 = 1u;

        fn instance(i: u32) -> Instance {
            calls += 1u;
            let n = arrayLength(&instances.items);
            return instances.items[i % max(n, 1u)];
        }

        fn transform(p: vec3f, i: u32) -> vec4f {
            let inst = instance(i);
            return camera.view_proj * vec4f(p * inst.scale + inst.offset, 1.0);
        }

        @vertex
        fn vs(@location(0) position: vec3f, @builtin(instance_index) i: u32) -> Out {
            let inst = instance(i);
            return Out(transform(position, i), i, inst.tint);
        }

        @fragment
        fn fs(in: Out, @builtin(sample_mask) mask: u32) -> FragOut {
            var total = vec3f(0.0);
            var k = 0u;
            loop {
                k += 1u;
                if k == 2u { continue; }
                total += in.tint * f32(k);
                continuing {
                    break if k >= 4u;
                }
            }
            hits[in.id] = hits[in.id] + countOneBits(mask);
            let c = textureSample(tex, samp, in.clip.xy) * vec4f(total, 1.0);
            return FragOut(c, in.clip.z);
        }
    "#;
    validate("msl_bindings_and_layouts", source);

    let module = compile(source).unwrap();
    let msl = module.to_msl(&MslOptions::default()).unwrap();
    assert_eq!(msl.buffer_sizes, vec![(1, 3), (2, 0)]);

    let vs = msl.entry_point("vs").unwrap();
    assert_eq!(vs.function, "ep_vs");
    // Resources are numbered in (group, binding) order, per kind.
    assert_eq!(
        vs.resources,
        vec![
            MslResource {
                group: 0,
                binding: 0,
                slot: MslSlot::Buffer(0)
            },
            MslResource {
                group: 1,
                binding: 3,
                slot: MslSlot::Buffer(1)
            },
        ]
    );
    assert_eq!(vs.sizes_buffer, Some(2));
    assert_eq!(vs.vertex_buffer_base, 3);

    let fs = msl.entry_point("fs").unwrap();
    assert_eq!(
        fs.resources,
        vec![
            MslResource {
                group: 0,
                binding: 1,
                slot: MslSlot::Texture(0)
            },
            MslResource {
                group: 0,
                binding: 2,
                slot: MslSlot::Sampler(0)
            },
            MslResource {
                group: 2,
                binding: 0,
                slot: MslSlot::Buffer(0)
            },
        ]
    );
    assert_eq!(fs.sizes_buffer, Some(1));

    // WGSL's layout: `tint` sits at offset 16 of a 32-byte element, after 4 bytes of the
    // struct's own padding, and `items` starts at 16.
    assert!(msl.source.contains("packed_float3 offset_;\n    float scale_;\n    packed_float3 tint_;\n    char _pad_end[4];"), "{}", msl.source);
    assert!(
        msl.source
            .contains("uint count_;\n    char _pad1[12];\n    Instance_ items_[1];"),
        "{}",
        msl.source
    );
    assert!(
        msl.source.contains("((_buffer_sizes[0] - 16u) / 32u)"),
        "{}",
        msl.source
    );
}

#[test]
fn msl_point_size() {
    let module = compile(include_str!("../../gpu/examples/triangle.wgsl")).unwrap();
    let plain = module.to_msl(&MslOptions::default()).unwrap();
    let points = module.to_msl(&MslOptions { point_size: true }).unwrap();
    assert!(!plain.source.contains("[[point_size]]"));
    assert!(points.source.contains("float _point_size [[point_size]];"));
}
