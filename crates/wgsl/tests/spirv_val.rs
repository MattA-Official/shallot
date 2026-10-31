//! Compiles shaders to SPIR-V and checks the output with `spirv-val` when it is installed.
//! `spirv-val` is a developer tool from SPIRV-Tools, run as a process, never linked.

use shallot_wgsl::{compile, SpirvOptions};
use std::process::Command;

fn validate(name: &str, source: &str) {
    let module = compile(source).unwrap_or_else(|e| panic!("{name}:\n{}", e.render(source)));
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
