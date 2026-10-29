//! A window showing a triangle drawn by a WGSL shader.
//!
//! ```text
//! cargo run -p shallot-gpu --example triangle                  # window, until closed
//! cargo run -p shallot-gpu --example triangle -- --frames 120  # window, then exit
//! cargo run -p shallot-gpu --example triangle -- --headless triangle.png
//! ```

use shallot_gpu::*;
use shallot_platform::{time::Clock, Event, Key, Window, WindowConfig};

const SHADER: &str = include_str!("triangle.wgsl");

struct Args {
    headless: Option<String>,
    frames: Option<u64>,
}

fn args() -> Args {
    let mut args = Args {
        headless: None,
        frames: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--headless" => {
                args.headless = Some(it.next().unwrap_or_else(|| "triangle.png".into()))
            }
            "--frames" => args.frames = it.next().and_then(|n| n.parse().ok()),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    args
}

fn pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let module = device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("triangle"),
        code: SHADER,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
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
    })
}

fn draw(device: &Device, pipeline: &RenderPipeline, view: &TextureView) -> CommandEncoder {
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("frame"),
    });
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("triangle"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view,
            resolve_target: None,
            load_op: LoadOp::Clear(Color::new(0.05, 0.05, 0.08, 1.0)),
            store_op: StoreOp::Store,
        })],
        depth_stencil_attachment: None,
    });
    pass.set_pipeline(pipeline);
    pass.draw(3, 1, 0, 0);
    pass.end();
    encoder
}

fn main() {
    let args = args();
    let instance = Instance::new(&InstanceDescriptor::default()).unwrap_or_else(|e| fail(e));
    match &args.headless {
        Some(path) => headless(&instance, path),
        None => windowed(&instance, args.frames),
    }
}

fn fail(error: impl std::fmt::Display) -> ! {
    eprintln!("triangle: {error}");
    std::process::exit(1);
}

fn headless(instance: &Instance, path: &str) {
    let (width, height) = (640, 480);
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .unwrap_or_else(|| fail("no suitable GPU adapter"));
    let device =
        block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap_or_else(|e| fail(e));
    let format = TextureFormat::Rgba8Unorm;
    let pipeline = pipeline(&device, format);
    let target = device.create_texture(&TextureDescriptor {
        label: Some("target"),
        size: Extent3d::new(width, height, 1),
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
        view_formats: &[],
    });
    let bytes_per_row = width * 4;
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * height) as u64,
        usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = draw(
        &device,
        &pipeline,
        &target.create_view(&TextureViewDescriptor::default()),
    );
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
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        Extent3d::new(width, height, 1),
    );
    device.queue().submit([encoder.finish()]);
    block_on(readback.map_async(MapMode::READ, 0, None)).unwrap_or_else(|e| fail(e));
    let pixels = readback.get_mapped_range(0, None).to_vec();
    std::fs::write(path, png::encode_rgba(width, height, &pixels)).unwrap_or_else(|e| fail(e));
    println!(
        "wrote {path} ({width}x{height}) using {}",
        adapter.info().description
    );
}

fn windowed(instance: &Instance, frames: Option<u64>) {
    let mut window =
        Window::new(&WindowConfig::new("shallot triangle", 800, 600)).unwrap_or_else(|e| fail(e));
    let surface =
        unsafe { instance.create_surface(window.raw_handle()) }.unwrap_or_else(|e| fail(e));
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .unwrap_or_else(|| fail("no GPU adapter can present to this window"));
    let device =
        block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap_or_else(|e| fail(e));
    let capabilities = surface.capabilities(&adapter);
    let preferred = instance.preferred_surface_format();
    let format = if capabilities.formats.contains(&preferred) {
        preferred
    } else {
        capabilities.formats[0]
    };
    let configure = |width, height| {
        surface.configure(
            &device,
            &SurfaceConfiguration {
                format,
                usage: TextureUsage::RENDER_ATTACHMENT,
                width,
                height,
                present_mode: PresentMode::Fifo,
                alpha_mode: CompositeAlphaMode::Opaque,
            },
        )
    };
    let (width, height) = window.size();
    configure(width, height);
    let pipeline = pipeline(&device, format);
    println!(
        "rendering with {} ({})",
        adapter.info().description,
        format.name()
    );

    let mut clock = Clock::new();
    let mut events = Vec::new();
    let mut drawn = 0u64;
    'run: loop {
        window.poll_events(&mut events);
        for event in events.drain(..) {
            match event {
                Event::CloseRequested
                | Event::KeyDown {
                    key: Key::Escape, ..
                } => break 'run,
                Event::Resized { width, height } => configure(width, height),
                _ => {}
            }
        }
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(SurfaceError::Outdated | SurfaceError::Timeout) => {
                let (width, height) = window.size();
                configure(width, height);
                continue;
            }
            Err(e) => fail(e),
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        device
            .queue()
            .submit([draw(&device, &pipeline, &view).finish()]);
        frame.present();
        drawn += 1;
        let time = clock.tick();
        if drawn.is_multiple_of(60) {
            window.set_title(&format!(
                "shallot triangle ({:.0} fps)",
                1.0 / time.delta.max(1e-6)
            ));
        }
        if frames.is_some_and(|n| drawn >= n) {
            break;
        }
    }
    device.poll(PollType::Wait);
    println!(
        "presented {drawn} frames in {:.2}s",
        clock.elapsed().as_secs_f64()
    );
}

/// A minimal PNG encoder (stored, uncompressed deflate) for writing screenshots.
mod png {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn adler32(bytes: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in bytes {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let crc = crc32(&out[start..]);
        out.extend(crc.to_be_bytes());
    }

    pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
        for row in pixels.chunks(width as usize * 4).take(height as usize) {
            raw.push(0);
            raw.extend(row);
        }
        let mut zlib = vec![0x78, 0x01];
        let mut blocks = raw.chunks(65535).peekable();
        while let Some(block) = blocks.next() {
            zlib.push(blocks.peek().is_none() as u8);
            zlib.extend((block.len() as u16).to_le_bytes());
            zlib.extend((!(block.len() as u16)).to_le_bytes());
            zlib.extend(block);
        }
        zlib.extend(adler32(&raw).to_be_bytes());

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend(width.to_be_bytes());
        header.extend(height.to_be_bytes());
        header.extend([8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }
}
