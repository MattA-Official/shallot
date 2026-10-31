//! Bind groups, samplers and compute, checked by reading results back. Needs a Vulkan driver;
//! lavapipe (mesa-vulkan-drivers) is enough. Skips when no adapter is available.

use shallot_gpu::*;

fn device() -> Option<Device> {
    let instance = Instance::new(&InstanceDescriptor::default()).ok()?;
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default()))?;
    let device = block_on(adapter.request_device(&DeviceDescriptor::default())).ok()?;
    // Any validation error the test does not expect fails it.
    device.on_uncaptured_error(|e| panic!("{e}"));
    Some(device)
}

fn shader(device: &Device, code: &str) -> ShaderModule {
    let module = device.create_shader_module(&ShaderModuleDescriptor { label: None, code });
    assert!(
        module.get_compilation_info().is_empty(),
        "{:?}",
        module.get_compilation_info()
    );
    module
}

fn buffer(device: &Device, size: u64, usage: BufferUsage) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: None,
        size,
        usage,
        mapped_at_creation: false,
    })
}

fn read(device: &Device, source: &Buffer, size: u64) -> Vec<u8> {
    let readback = buffer(device, size, BufferUsage::COPY_DST | BufferUsage::MAP_READ);
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, size);
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap();
    let data = readback.get_mapped_range(0, None).to_vec();
    readback.unmap();
    data
}

fn words(data: &[u8]) -> Vec<u32> {
    data.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn compute(device: &Device, code: &str) -> ComputePipeline {
    let module = shader(device, code);
    device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: None,
        layout: PipelineLayoutChoice::Auto,
        compute: ProgrammableStage {
            module: &module,
            entry_point: None,
        },
    })
}

fn whole(buffer: &Buffer) -> BindingResource<'_> {
    BindingResource::Buffer(BufferBinding {
        buffer,
        offset: 0,
        size: None,
    })
}

#[test]
fn compute_reads_uniforms_and_writes_storage() {
    let Some(device) = device() else {
        eprintln!("no Vulkan adapter; skipping");
        return;
    };
    let pipeline = compute(
        &device,
        "struct Params { scale: u32, bias: u32 }
         @group(0) @binding(0) var<uniform> params: Params;
         @group(0) @binding(1) var<storage, read> input: array<u32>;
         @group(1) @binding(0) var<storage, read_write> output: array<u32>;
         @compute @workgroup_size(8)
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {
             if (id.x < arrayLength(&output)) {
                 output[id.x] = input[id.x] * params.scale + params.bias;
             }
         }",
    );
    let params = buffer(&device, 8, BufferUsage::UNIFORM | BufferUsage::COPY_DST);
    let input = buffer(&device, 64, BufferUsage::STORAGE | BufferUsage::COPY_DST);
    let output = buffer(&device, 64, BufferUsage::STORAGE | BufferUsage::COPY_SRC);
    device.queue().write_buffer(&params, 0, &bytes(&[3, 1]));
    device
        .queue()
        .write_buffer(&input, 0, &bytes(&(0..16).collect::<Vec<_>>()));
    let group0 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("inputs"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: whole(&params),
            },
            BindGroupEntry {
                binding: 1,
                resource: whole(&input),
            },
        ],
    });
    let group1 = device.create_bind_group(&BindGroupDescriptor {
        label: Some("outputs"),
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[BindGroupEntry {
            binding: 0,
            resource: whole(&output),
        }],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group0), &[]);
        pass.set_bind_group(1, Some(&group1), &[]);
        pass.dispatch_workgroups(2, 1, 1);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    let expected: Vec<u32> = (0..16).map(|i| i * 3 + 1).collect();
    assert_eq!(words(&read(&device, &output, 64)), expected);
}

#[test]
fn dynamic_offsets_select_uniforms() {
    let Some(device) = device() else { return };
    let module = shader(
        &device,
        "@group(0) @binding(0) var<uniform> value: vec4<u32>;
         @group(0) @binding(1) var<storage, read_write> output: array<u32>;
         @compute @workgroup_size(1)
         fn main() { output[value.y] = value.x; }",
    );
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: 16,
                },
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage,
                    has_dynamic_offset: false,
                    min_binding_size: 0,
                },
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
    });
    let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: None,
        layout: PipelineLayoutChoice::Layout(&pipeline_layout),
        compute: ProgrammableStage {
            module: &module,
            entry_point: Some("main"),
        },
    });
    let uniforms = buffer(&device, 512, BufferUsage::UNIFORM | BufferUsage::COPY_DST);
    device
        .queue()
        .write_buffer(&uniforms, 0, &bytes(&[11, 0, 0, 0]));
    device
        .queue()
        .write_buffer(&uniforms, 256, &bytes(&[22, 1, 0, 0]));
    let output = buffer(&device, 8, BufferUsage::STORAGE | BufferUsage::COPY_SRC);
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: Some(16),
                }),
            },
            BindGroupEntry {
                binding: 1,
                resource: whole(&output),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group), &[0]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_bind_group(0, Some(&group), &[256]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    assert_eq!(words(&read(&device, &output, 8)), [11, 22]);
}

#[test]
fn indirect_dispatch_reads_its_counts() {
    let Some(device) = device() else { return };
    let pipeline = compute(
        &device,
        "@group(0) @binding(0) var<storage, read_write> output: array<u32>;
         @compute @workgroup_size(1)
         fn main(@builtin(workgroup_id) id: vec3<u32>) { output[id.x + id.y * 4u] = 1u; }",
    );
    let args = buffer(&device, 16, BufferUsage::INDIRECT | BufferUsage::COPY_DST);
    device.queue().write_buffer(&args, 4, &bytes(&[3, 2, 1]));
    let output = buffer(&device, 32, BufferUsage::STORAGE | BufferUsage::COPY_SRC);
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[BindGroupEntry {
            binding: 0,
            resource: whole(&output),
        }],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.dispatch_workgroups_indirect(&args, 4);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    assert_eq!(words(&read(&device, &output, 32)), [1, 1, 1, 0, 1, 1, 1, 0]);
}

fn texture(device: &Device, size: u32, format: TextureFormat, usage: TextureUsage) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d::new(size, size, 1),
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Reads an RGBA8 texture back, 4 bytes a texel.
fn read_texture(device: &Device, texture: &Texture) -> Vec<[u8; 4]> {
    let size = texture.width();
    let readback = buffer(
        device,
        256 * size as u64,
        BufferUsage::COPY_DST | BufferUsage::MAP_READ,
    );
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        Extent3d::new(size, size, 1),
    );
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap();
    let data = readback.get_mapped_range(0, None).to_vec();
    readback.unmap();
    (0..size * size)
        .map(|i| {
            let at = ((i / size) * 256 + (i % size) * 4) as usize;
            data[at..at + 4].try_into().unwrap()
        })
        .collect()
}

#[test]
fn compute_writes_a_storage_texture() {
    let Some(device) = device() else { return };
    let pipeline = compute(
        &device,
        "@group(0) @binding(0) var target: texture_storage_2d<rgba8unorm, write>;
         @compute @workgroup_size(4, 4)
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {
             let size = textureDimensions(target);
             let uv = vec2<f32>(id.xy) / vec2<f32>(size - 1u);
             textureStore(target, id.xy, vec4<f32>(uv, 0.0, 1.0));
         }",
    );
    let target = texture(
        &device,
        4,
        TextureFormat::Rgba8Unorm,
        TextureUsage::STORAGE_BINDING | TextureUsage::COPY_SRC,
    );
    let view = target.create_view(&TextureViewDescriptor::default());
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::TextureView(&view),
        }],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    let texels = read_texture(&device, &target);
    assert_eq!(texels[0], [0, 0, 0, 255]);
    assert_eq!(texels[3], [255, 0, 0, 255]);
    assert_eq!(texels[15], [255, 255, 0, 255]);
}

const BLIT: &str = "
    struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
    @vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
        let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
        return Out(vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0), uv);
    }
    @group(0) @binding(0) var image: texture_2d<f32>;
    @group(0) @binding(1) var image_sampler: sampler;
    @group(0) @binding(2) var<uniform> tint: vec4<f32>;
    @fragment fn fs(in: Out) -> @location(0) vec4<f32> {
        return textureSample(image, image_sampler, in.uv) * tint;
    }";

#[test]
fn render_pass_samples_a_texture() {
    let Some(device) = device() else { return };
    let module = shader(&device, BLIT);
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("blit"),
        layout: PipelineLayoutChoice::Auto,
        vertex: VertexState {
            module: &module,
            entry_point: None,
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &module,
            entry_point: None,
            targets: &[Some(TextureFormat::Rgba8Unorm.into())],
        }),
    });
    // A 2x2 checker: red, green / blue, white.
    let source = texture(
        &device,
        2,
        TextureFormat::Rgba8Unorm,
        TextureUsage::TEXTURE_BINDING | TextureUsage::COPY_DST,
    );
    device.queue().write_texture(
        TexelCopyTextureInfo {
            texture: &source,
            mip_level: 0,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        &[
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ],
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: None,
        },
        Extent3d::new(2, 2, 1),
    );
    let sampler = device.create_sampler(&SamplerDescriptor::default());
    let tint = buffer(&device, 16, BufferUsage::UNIFORM | BufferUsage::COPY_DST);
    let half: Vec<u8> = [1.0f32, 0.5, 1.0, 1.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    device.queue().write_buffer(&tint, 0, &half);
    let source_view = source.create_view(&TextureViewDescriptor::default());
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&source_view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(&sampler),
            },
            BindGroupEntry {
                binding: 2,
                resource: whole(&tint),
            },
        ],
    });
    let target = texture(
        &device,
        8,
        TextureFormat::Rgba8Unorm,
        TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
    );
    let target_view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                load_op: LoadOp::Clear(Color::BLACK),
                store_op: StoreOp::Store,
            })],
            depth_stencil_attachment: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.draw(3, 1, 0, 0);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    let texels = read_texture(&device, &target);
    // Nearest filtering: each quadrant is one source texel, with green halved by the tint.
    assert_eq!(texels[0], [255, 0, 0, 255]);
    assert_eq!(texels[7], [0, 128, 0, 255]);
    assert_eq!(texels[56], [0, 0, 255, 255]);
    assert_eq!(texels[63], [255, 128, 255, 255]);
}

#[test]
fn draw_indirect_reads_its_arguments() {
    let Some(device) = device() else { return };
    let module = shader(
        &device,
        "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
             let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
             return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
         }
         @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 1.0, 0.0, 1.0); }",
    );
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: None,
        layout: PipelineLayoutChoice::Auto,
        vertex: VertexState {
            module: &module,
            entry_point: None,
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &module,
            entry_point: None,
            targets: &[Some(TextureFormat::Rgba8Unorm.into())],
        }),
    });
    let args = buffer(&device, 16, BufferUsage::INDIRECT | BufferUsage::COPY_DST);
    device.queue().write_buffer(&args, 0, &bytes(&[3, 1, 0, 0]));
    let target = texture(
        &device,
        4,
        TextureFormat::Rgba8Unorm,
        TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
    );
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                load_op: LoadOp::Clear(Color::BLACK),
                store_op: StoreOp::Store,
            })],
            depth_stencil_attachment: None,
        });
        pass.set_pipeline(&pipeline);
        pass.draw_indirect(&args, 0);
        pass.end();
    }
    device.queue().submit([encoder.finish()]);
    assert!(read_texture(&device, &target)
        .iter()
        .all(|t| *t == [0, 255, 0, 255]));
}

#[test]
fn bind_group_misuse_is_a_validation_error() {
    let Some(device) = device() else { return };
    let pipeline = compute(
        &device,
        "@group(0) @binding(0) var<storage, read_write> output: array<u32>;
         @compute @workgroup_size(1) fn main() { output[0] = 1u; }",
    );
    let layout = pipeline.get_bind_group_layout(0);
    assert_eq!(layout.entries().len(), 1);
    let expect_error = |what: &str, f: &dyn Fn()| {
        device.push_error_scope(ErrorFilter::Validation);
        f();
        let error = block_on(device.pop_error_scope());
        assert!(
            matches!(error, Some(GpuError::Validation(_))),
            "{what}: {error:?}"
        );
    };

    // A uniform-only buffer cannot be a storage binding.
    let uniform = buffer(&device, 16, BufferUsage::UNIFORM);
    expect_error("wrong usage", &|| {
        device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: whole(&uniform),
            }],
        });
    });
    expect_error("missing entry", &|| {
        device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[],
        });
    });
    expect_error("duplicate layout binding", &|| {
        let entry = BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
        };
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: &[entry, entry],
        });
    });
    expect_error("dispatch without bind group", &|| {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.finish();
    });
    expect_error("layout mismatch", &|| {
        let module = shader(
            &device,
            "@group(0) @binding(0) var<uniform> value: vec4<f32>;
             @compute @workgroup_size(1) fn main() { _ = value; }",
        );
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
        });
        device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: None,
            layout: PipelineLayoutChoice::Layout(&pipeline_layout),
            compute: ProgrammableStage {
                module: &module,
                entry_point: None,
            },
        });
    });
}

#[test]
fn render_pass_reads_one_mip_while_writing_another() {
    let Some(device) = device() else { return };
    let module = shader(&device, BLIT);
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: None,
        layout: PipelineLayoutChoice::Auto,
        vertex: VertexState {
            module: &module,
            entry_point: None,
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &module,
            entry_point: None,
            targets: &[Some(TextureFormat::Rgba8Unorm.into())],
        }),
    });
    let chain = device.create_texture(&TextureDescriptor {
        label: Some("chain"),
        size: Extent3d::new(4, 4, 1),
        mip_level_count: 2,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage::TEXTURE_BINDING
            | TextureUsage::RENDER_ATTACHMENT
            | TextureUsage::COPY_DST
            | TextureUsage::COPY_SRC,
        view_formats: &[],
    });
    device.queue().write_texture(
        TexelCopyTextureInfo {
            texture: &chain,
            mip_level: 0,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        &[200; 64],
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: None,
        },
        Extent3d::new(4, 4, 1),
    );
    let mip = |level| {
        chain.create_view(&TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        })
    };
    let (mip0, mip1) = (mip(0), mip(1));
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..Default::default()
    });
    let tint = buffer(&device, 16, BufferUsage::UNIFORM | BufferUsage::COPY_DST);
    let ones: Vec<u8> = [1.0f32; 4].iter().flat_map(|f| f.to_le_bytes()).collect();
    device.queue().write_buffer(&tint, 0, &ones);
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&mip0),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(&sampler),
            },
            BindGroupEntry {
                binding: 2,
                resource: whole(&tint),
            },
        ],
    });
    let readback = buffer(&device, 512, BufferUsage::COPY_DST | BufferUsage::MAP_READ);
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &mip1,
                resolve_target: None,
                load_op: LoadOp::Clear(Color::BLACK),
                store_op: StoreOp::Store,
            })],
            depth_stencil_attachment: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.draw(3, 1, 0, 0);
        pass.end();
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &chain,
            mip_level: 1,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        Extent3d::new(2, 2, 1),
    );
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap();
    let data = readback.get_mapped_range(0, None).to_vec();
    assert_eq!(&data[0..8], &[200; 8]);
    assert_eq!(&data[256..264], &[200; 8]);
}
