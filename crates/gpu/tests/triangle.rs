//! Renders the WGSL triangle off screen and reads it back. Needs a Vulkan driver; lavapipe
//! (mesa-vulkan-drivers) is enough. Skips when no adapter is available.

use shallot_gpu::*;

const SIZE: u32 = 64;

fn device() -> Option<Device> {
    let instance = Instance::new(&InstanceDescriptor::default()).ok()?;
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default()))?;
    let device = block_on(adapter.request_device(&DeviceDescriptor::default())).ok()?;
    // Any validation error fails the test.
    device.on_uncaptured_error(|e| panic!("{e}"));
    Some(device)
}

fn render(device: &Device, format: TextureFormat) -> Vec<u8> {
    let module = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("triangle"),
        code: include_str!("../examples/triangle.wgsl"),
    });
    assert!(module.get_compilation_info().is_empty());
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("triangle"),
        layout: PipelineLayoutChoice::Auto,
        vertex: VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(format.into())],
        }),
    });
    let target = device.create_texture(&TextureDescriptor {
        label: Some("target"),
        size: Extent3d::new(SIZE, SIZE, 1),
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: 256 * SIZE as u64,
        usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
        mapped_at_creation: false,
    });

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
        pass.draw(3, 1, 0, 0);
        pass.end();
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &target,
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
        Extent3d::new(SIZE, SIZE, 1),
    );
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap();
    let data = readback.get_mapped_range(0, None).to_vec();
    readback.unmap();
    data
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = (y * 256 + x * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn triangle_renders_and_reads_back() {
    let Some(device) = device() else {
        eprintln!("no Vulkan adapter; skipping");
        return;
    };
    let data = render(&device, TextureFormat::Rgba8Unorm);

    // Corners are the clear colour.
    assert_eq!(pixel(&data, 0, 0), [0, 0, 0, 255]);
    assert_eq!(pixel(&data, SIZE - 1, SIZE - 1), [0, 0, 0, 255]);

    // The centre blends all three vertex colours.
    let [r, g, b, a] = pixel(&data, SIZE / 2, SIZE / 2);
    assert_eq!(a, 255);
    assert!(r > 40 && g > 40 && b > 40, "centre {r} {g} {b}");

    // Clip-space +y is up: the red apex is near the top of the image, not the bottom.
    let top = pixel(&data, SIZE / 2, SIZE / 4 + 2);
    let bottom_left = pixel(&data, SIZE / 4 + 2, SIZE * 3 / 4 - 2);
    let bottom_right = pixel(&data, SIZE * 3 / 4 - 2, SIZE * 3 / 4 - 2);
    assert!(top[0] > top[1] && top[0] > top[2], "top {top:?}");
    assert!(
        bottom_left[1] > bottom_left[0] && bottom_left[1] > bottom_left[2],
        "bottom left {bottom_left:?}"
    );
    assert!(
        bottom_right[2] > bottom_right[0] && bottom_right[2] > bottom_right[1],
        "bottom right {bottom_right:?}"
    );
}

#[test]
fn bgra_target_swaps_channels() {
    let Some(device) = device() else { return };
    let data = render(&device, TextureFormat::Bgra8Unorm);
    let top = pixel(&data, SIZE / 2, SIZE / 4 + 2);
    // Red lands in the third byte of a BGRA texel.
    assert!(top[2] > top[0] && top[2] > top[1], "top {top:?}");
}

#[test]
fn write_buffer_round_trips() {
    let Some(device) = device() else { return };
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("data"),
        size: 16,
        usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
        mapped_at_creation: false,
    });
    device
        .queue()
        .write_buffer(&buffer, 4, &[1, 2, 3, 4, 5, 6, 7, 8]);
    block_on(buffer.map_async(MapMode::READ, 0, None)).unwrap();
    assert_eq!(
        &buffer.get_mapped_range(0, None)[..],
        &[0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0]
    );
    buffer.unmap();
}

#[test]
fn mapped_at_creation_uploads_through_staging() {
    let Some(device) = device() else { return };
    let source = device.create_buffer(&BufferDescriptor {
        label: Some("vertices"),
        size: 8,
        usage: BufferUsage::VERTEX | BufferUsage::COPY_SRC,
        mapped_at_creation: true,
    });
    source
        .get_mapped_range_mut(0, None)
        .copy_from_slice(&[9, 8, 7, 6, 5, 4, 3, 2]);
    source.unmap();
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: 8,
        usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(&source, 0, &readback, 0, 8);
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap();
    assert_eq!(
        &readback.get_mapped_range(0, None)[..],
        &[9, 8, 7, 6, 5, 4, 3, 2]
    );
}

#[test]
fn errors_go_to_error_scopes() {
    let Some(device) = device() else { return };
    device.push_error_scope(ErrorFilter::Validation);
    let module = device.create_shader_module(&ShaderModuleDescriptor {
        label: None,
        code: "fn f() -> i32 { return 1u; }",
    });
    let error = block_on(device.pop_error_scope());
    assert!(matches!(error, Some(GpuError::Validation(_))), "{error:?}");
    let info = module.get_compilation_info();
    assert_eq!((info[0].line_num, info[0].line_pos), (1, 24));

    device.push_error_scope(ErrorFilter::Validation);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 4,
        usage: BufferUsage::MAP_READ | BufferUsage::VERTEX,
        mapped_at_creation: false,
    });
    assert!(block_on(device.pop_error_scope()).is_some());
    device.push_error_scope(ErrorFilter::Validation);
    assert!(block_on(buffer.map_async(MapMode::READ, 0, None)).is_err());
    assert!(block_on(device.pop_error_scope()).is_some());
}
