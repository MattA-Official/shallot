//! Command encoding, the queue, and translation of recorded commands to Vulkan.
//!
//! Encoders record backend-independent commands holding references to the resources they use.
//! `Queue::submit` validates them, translates them into one Vulkan command buffer with layout
//! transitions and barriers inserted from tracked state, and keeps the resources alive until
//! the submission's fence signals.

use super::binding::{BindGroupShared, Bound, PipelineLayoutShared};
use super::resource::{
    BufferShared, ComputePipelineShared, RenderPipelineShared, TextureShared, TextureViewShared,
};
use super::{check, conv, vk, Device, DeviceShared, DeviceState, Submission};
use crate::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// A `write_buffer` or `write_texture` copy waiting for the next submission.
pub(crate) enum PendingWrite {
    Buffer {
        staging: Arc<BufferShared>,
        dst: Arc<BufferShared>,
        dst_offset: u64,
        size: u64,
    },
    Texture {
        staging: Arc<BufferShared>,
        texture: Arc<TextureShared>,
        region: vk::BufferImageCopy,
    },
}

pub(crate) struct ColorRecord {
    view: Arc<TextureViewShared>,
    resolve: Option<Arc<TextureViewShared>>,
    load: LoadOp<Color>,
    store: StoreOp,
}

pub(crate) struct DepthRecord {
    view: Arc<TextureViewShared>,
    depth: Option<(LoadOp<f32>, StoreOp)>,
    stencil: Option<(LoadOp<u32>, StoreOp)>,
}

pub(crate) struct PassRecord {
    colors: Vec<Option<ColorRecord>>,
    depth: Option<DepthRecord>,
    extent: (u32, u32),
}

pub(crate) enum Command {
    BeginRenderPass(PassRecord),
    EndRenderPass,
    SetPipeline(Arc<RenderPipelineShared>),
    BeginComputePass,
    EndComputePass,
    SetComputePipeline(Arc<ComputePipelineShared>),
    SetBindGroup {
        index: u32,
        group: Arc<BindGroupShared>,
        offsets: Vec<u32>,
    },
    SetVertexBuffer {
        slot: u32,
        buffer: Arc<BufferShared>,
        offset: u64,
    },
    SetIndexBuffer {
        buffer: Arc<BufferShared>,
        format: IndexFormat,
        offset: u64,
    },
    SetViewport(vk::Viewport),
    SetScissor(vk::Rect2D),
    SetBlendConstant([f32; 4]),
    SetStencilReference(u32),
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    },
    DrawIndirect {
        buffer: Arc<BufferShared>,
        offset: u64,
    },
    DrawIndexedIndirect {
        buffer: Arc<BufferShared>,
        offset: u64,
    },
    Dispatch([u32; 3]),
    DispatchIndirect {
        buffer: Arc<BufferShared>,
        offset: u64,
    },
    CopyBufferToBuffer {
        src: Arc<BufferShared>,
        src_offset: u64,
        dst: Arc<BufferShared>,
        dst_offset: u64,
        size: u64,
    },
    CopyBufferToTexture {
        buffer: Arc<BufferShared>,
        texture: Arc<TextureShared>,
        region: vk::BufferImageCopy,
    },
    CopyTextureToBuffer {
        texture: Arc<TextureShared>,
        buffer: Arc<BufferShared>,
        region: vk::BufferImageCopy,
    },
}

// Commands only hold `Arc`s of `Send + Sync` resources and plain Vulkan structs.
unsafe impl Send for Command {}
unsafe impl Sync for Command {}
unsafe impl Send for PendingWrite {}
unsafe impl Sync for PendingWrite {}

impl Command {
    /// Checks that every resource the command uses is still usable at submit time.
    fn validate(&self) -> Result<(), String> {
        match self {
            Command::BeginRenderPass(pass) => {
                for color in pass.colors.iter().flatten() {
                    color.view.texture.usable()?;
                    if let Some(resolve) = &color.resolve {
                        resolve.texture.usable()?;
                    }
                }
                if let Some(depth) = &pass.depth {
                    depth.view.texture.usable()?;
                }
                Ok(())
            }
            Command::SetVertexBuffer { buffer, .. }
            | Command::SetIndexBuffer { buffer, .. }
            | Command::DrawIndirect { buffer, .. }
            | Command::DrawIndexedIndirect { buffer, .. }
            | Command::DispatchIndirect { buffer, .. } => buffer.usable(),
            Command::SetBindGroup { group, .. } => group.usable(),
            Command::CopyBufferToBuffer { src, dst, .. } => {
                src.usable()?;
                dst.usable()
            }
            Command::CopyBufferToTexture {
                buffer, texture, ..
            }
            | Command::CopyTextureToBuffer {
                texture, buffer, ..
            } => {
                buffer.usable()?;
                texture.usable()
            }
            _ => Ok(()),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Command encoders

/// `GPUCommandEncoder`.
pub struct CommandEncoder {
    device: Arc<DeviceShared>,
    commands: Vec<Command>,
    error: Option<String>,
    label: String,
}

/// `GPUCommandBuffer`. Submitting it consumes it, as WebGPU allows one submission.
pub struct CommandBuffer {
    commands: Vec<Command>,
    error: Option<String>,
    label: String,
}

impl Device {
    /// `createCommandEncoder`.
    pub fn create_command_encoder(&self, desc: &CommandEncoderDescriptor<'_>) -> CommandEncoder {
        CommandEncoder {
            device: self.shared.clone(),
            commands: Vec::new(),
            error: None,
            label: desc.label.unwrap_or("").to_string(),
        }
    }
}

/// Bytes per texel block for one aspect of a format, for copies.
fn copy_block_size(format: TextureFormat, aspect: TextureAspect) -> Option<u32> {
    use TextureFormat::*;
    match (format, aspect) {
        (Depth32Float | Depth32FloatStencil8, TextureAspect::DepthOnly)
        | (Depth32Float, TextureAspect::All) => Some(4),
        (Depth16Unorm, TextureAspect::DepthOnly | TextureAspect::All) => Some(2),
        (Stencil8 | Depth24PlusStencil8 | Depth32FloatStencil8, TextureAspect::StencilOnly)
        | (Stencil8, TextureAspect::All) => Some(1),
        (f, _) if f.is_depth_stencil() => None,
        (f, TextureAspect::All) => f.block_size(),
        _ => None,
    }
}

/// Validates a buffer-texture copy and builds its Vulkan region. `strict_rows` applies the
/// 256-byte `bytesPerRow` rule of encoder copies.
fn texel_copy_region(
    buffer_size: u64,
    buffer_name: &str,
    layout: &TexelCopyBufferLayout,
    texture: &TexelCopyTextureInfo<'_>,
    size: Extent3d,
    strict_rows: bool,
) -> Result<vk::BufferImageCopy, String> {
    let tex = &texture.texture.shared;
    tex.usable()?;
    let block = copy_block_size(tex.format, texture.aspect).ok_or_else(|| {
        format!(
            "{} cannot be copied with aspect {:?}",
            tex.format.name(),
            texture.aspect
        )
    })?;
    if tex.sample_count > 1 {
        return Err("multisampled textures cannot be copied".into());
    }
    if texture.mip_level >= tex.mip_level_count {
        return Err("mipLevel is out of bounds".into());
    }
    let mip_width = (tex.size.width >> texture.mip_level).max(1);
    let mip_height = (tex.size.height >> texture.mip_level).max(1);
    let mip_depth = match tex.dimension {
        TextureDimension::D3 => (tex.size.depth_or_array_layers >> texture.mip_level).max(1),
        _ => tex.size.depth_or_array_layers,
    };
    let o = texture.origin;
    if o.x + size.width > mip_width
        || o.y + size.height > mip_height
        || o.z + size.depth_or_array_layers > mip_depth
    {
        return Err("the copy is outside the texture".into());
    }
    let row_bytes = size.width as u64 * block as u64;
    let bytes_per_row = match layout.bytes_per_row {
        Some(b) => {
            if strict_rows && b % 256 != 0 {
                return Err("bytesPerRow must be a multiple of 256".into());
            }
            if (b as u64) < row_bytes {
                return Err("bytesPerRow is smaller than one row of the copy".into());
            }
            b as u64
        }
        None if size.height > 1 || size.depth_or_array_layers > 1 => {
            return Err("bytesPerRow is required for copies of more than one row".into());
        }
        None => row_bytes,
    };
    let rows_per_image = match layout.rows_per_image {
        Some(r) if r < size.height => {
            return Err("rowsPerImage is smaller than the copy height".into())
        }
        Some(r) => r as u64,
        None if size.depth_or_array_layers > 1 => {
            return Err("rowsPerImage is required for copies of more than one image".into());
        }
        None => size.height as u64,
    };
    if !layout.offset.is_multiple_of(block as u64) {
        return Err("the buffer offset must be a multiple of the texel block size".into());
    }
    let needed = if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 {
        0
    } else {
        bytes_per_row * rows_per_image * (size.depth_or_array_layers as u64 - 1)
            + bytes_per_row * (size.height as u64 - 1)
            + row_bytes
    };
    if layout.offset + needed > buffer_size {
        return Err(format!(
            "the copy needs {} bytes of {buffer_name}, which has {buffer_size}",
            layout.offset + needed
        ));
    }
    let is_3d = tex.dimension == TextureDimension::D3;
    Ok(vk::BufferImageCopy {
        buffer_offset: layout.offset,
        buffer_row_length: (bytes_per_row / block as u64) as u32,
        buffer_image_height: rows_per_image as u32,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: conv::aspect_mask(tex.format, texture.aspect),
            mip_level: texture.mip_level,
            base_array_layer: if is_3d { 0 } else { o.z },
            layer_count: if is_3d { 1 } else { size.depth_or_array_layers },
        },
        image_offset: vk::Offset3D {
            x: o.x as i32,
            y: o.y as i32,
            z: if is_3d { o.z as i32 } else { 0 },
        },
        image_extent: vk::Extent3D {
            width: size.width,
            height: size.height,
            depth: if is_3d { size.depth_or_array_layers } else { 1 },
        },
    })
}

impl CommandEncoder {
    fn fail(&mut self, message: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(message.into());
        }
    }

    /// `beginRenderPass`. The pass borrows the encoder until it ends.
    pub fn begin_render_pass(&mut self, desc: &RenderPassDescriptor<'_>) -> RenderPassEncoder<'_> {
        let mut extent = None;
        let mut sample_count = None;
        let mut error = None;
        let mut check_view = |view: &TextureViewShared, what: &str| -> Result<(), String> {
            if !view.valid {
                return Err(format!("{what} {} is invalid", view.describe()));
            }
            view.texture.usable()?;
            if !view.texture.usage.contains(TextureUsage::RENDER_ATTACHMENT) {
                return Err(format!(
                    "{what} {} lacks RENDER_ATTACHMENT usage",
                    view.texture.describe()
                ));
            }
            if view.mip_level_count != 1 || view.array_layer_count != 1 {
                return Err(format!(
                    "{what} views must have one mip level and one layer"
                ));
            }
            let e = view.extent();
            if *extent.get_or_insert(e) != e {
                return Err("render pass attachments must all be the same size".into());
            }
            Ok(())
        };
        let mut colors = Vec::new();
        let mut color_formats = Vec::new();
        for attachment in desc.color_attachments {
            let Some(a) = attachment else {
                colors.push(None);
                color_formats.push(None);
                continue;
            };
            let view = &a.view.shared;
            let mut result = check_view(view, "colour attachment");
            if result.is_ok() && view.format.is_depth_stencil() {
                result = Err("colour attachments need colour formats".into());
            }
            if result.is_ok()
                && *sample_count.get_or_insert(view.texture.sample_count)
                    != view.texture.sample_count
            {
                result = Err("render pass attachments must have the same sample count".into());
            }
            if let (Ok(()), Some(resolve)) = (&result, a.resolve_target) {
                let r = &resolve.shared;
                result = if view.texture.sample_count == 1 {
                    Err("resolve targets need a multisampled attachment".into())
                } else if r.texture.sample_count != 1
                    || r.format != view.format
                    || r.extent() != view.extent()
                {
                    Err("a resolve target must be single-sampled with the attachment's format and size".into())
                } else if !r.valid || !r.texture.usage.contains(TextureUsage::RENDER_ATTACHMENT) {
                    Err("resolve targets need RENDER_ATTACHMENT usage".into())
                } else {
                    r.texture.usable()
                };
            }
            if let Err(e) = result {
                error.get_or_insert(e);
            }
            color_formats.push(Some(view.format));
            colors.push(Some(ColorRecord {
                view: view.clone(),
                resolve: a.resolve_target.map(|r| r.shared.clone()),
                load: a.load_op,
                store: a.store_op,
            }));
        }
        let mut depth = None;
        let mut depth_format = None;
        if let Some(d) = &desc.depth_stencil_attachment {
            let view = &d.view.shared;
            let mut result = check_view(view, "depth-stencil attachment");
            if result.is_ok() && !view.format.is_depth_stencil() {
                result = Err("the depth-stencil attachment needs a depth or stencil format".into());
            }
            if result.is_ok()
                && *sample_count.get_or_insert(view.texture.sample_count)
                    != view.texture.sample_count
            {
                result = Err("render pass attachments must have the same sample count".into());
            }
            if result.is_ok()
                && view.format.has_depth()
                && d.depth_ops.is_none()
                && !d.depth_read_only
            {
                result = Err(
                    "depth load and store ops are required unless the depth aspect is read-only"
                        .into(),
                );
            }
            if let Err(e) = result {
                error.get_or_insert(e);
            }
            depth_format = Some(view.format);
            depth = Some(DepthRecord {
                view: view.clone(),
                depth: d.depth_ops,
                stencil: d.stencil_ops,
            });
        }
        let extent = extent.unwrap_or((0, 0));
        if colors.iter().all(Option::is_none) && depth.is_none() {
            error.get_or_insert("a render pass needs at least one attachment".into());
        }
        if colors.len() as u32 > self.device.limits.max_color_attachments {
            error.get_or_insert("too many colour attachments".into());
        }
        if let Some(e) = error {
            self.fail(e);
        }
        self.commands.push(Command::BeginRenderPass(PassRecord {
            colors,
            depth,
            extent,
        }));
        RenderPassEncoder {
            encoder: self,
            pipeline: None,
            binds: BindState::default(),
            vertex_buffers: 0,
            index: None,
            color_formats,
            depth_format,
            sample_count: sample_count.unwrap_or(1),
            extent,
            ended: false,
        }
    }

    /// `beginComputePass`. The pass borrows the encoder until it ends.
    pub fn begin_compute_pass(
        &mut self,
        _desc: &ComputePassDescriptor<'_>,
    ) -> ComputePassEncoder<'_> {
        self.commands.push(Command::BeginComputePass);
        ComputePassEncoder {
            encoder: self,
            pipeline: None,
            binds: BindState::default(),
            ended: false,
        }
    }

    /// `copyBufferToBuffer`.
    pub fn copy_buffer_to_buffer(
        &mut self,
        src: &Buffer,
        src_offset: u64,
        dst: &Buffer,
        dst_offset: u64,
        size: u64,
    ) {
        let (s, d) = (&src.shared, &dst.shared);
        let error = if !s.usage.contains(BufferUsage::COPY_SRC) {
            Some(format!("{} lacks COPY_SRC usage", s.describe()))
        } else if !d.usage.contains(BufferUsage::COPY_DST) {
            Some(format!("{} lacks COPY_DST usage", d.describe()))
        } else if !size.is_multiple_of(4)
            || !src_offset.is_multiple_of(4)
            || !dst_offset.is_multiple_of(4)
        {
            Some("copy offsets and size must be multiples of 4".into())
        } else if src_offset + size > s.size || dst_offset + size > d.size {
            Some("the copy is out of bounds".into())
        } else if Arc::ptr_eq(s, d) {
            Some("a buffer cannot be copied to itself".into())
        } else {
            None
        };
        if let Some(e) = error {
            return self.fail(e);
        }
        if size > 0 {
            self.commands.push(Command::CopyBufferToBuffer {
                src: s.clone(),
                src_offset,
                dst: d.clone(),
                dst_offset,
                size,
            });
        }
    }

    /// `copyBufferToTexture`.
    pub fn copy_buffer_to_texture(
        &mut self,
        src: TexelCopyBufferInfo<'_>,
        dst: TexelCopyTextureInfo<'_>,
        size: Extent3d,
    ) {
        let buffer = &src.buffer.shared;
        if !buffer.usage.contains(BufferUsage::COPY_SRC) {
            return self.fail(format!("{} lacks COPY_SRC usage", buffer.describe()));
        }
        if !dst.texture.shared.usage.contains(TextureUsage::COPY_DST) {
            return self.fail(format!(
                "{} lacks COPY_DST usage",
                dst.texture.shared.describe()
            ));
        }
        match texel_copy_region(
            buffer.size,
            &buffer.describe(),
            &src.layout,
            &dst,
            size,
            true,
        ) {
            Ok(region) => self.commands.push(Command::CopyBufferToTexture {
                buffer: buffer.clone(),
                texture: dst.texture.shared.clone(),
                region,
            }),
            Err(e) => self.fail(e),
        }
    }

    /// `copyTextureToBuffer`.
    pub fn copy_texture_to_buffer(
        &mut self,
        src: TexelCopyTextureInfo<'_>,
        dst: TexelCopyBufferInfo<'_>,
        size: Extent3d,
    ) {
        let buffer = &dst.buffer.shared;
        if !buffer.usage.contains(BufferUsage::COPY_DST) {
            return self.fail(format!("{} lacks COPY_DST usage", buffer.describe()));
        }
        if !src.texture.shared.usage.contains(TextureUsage::COPY_SRC) {
            return self.fail(format!(
                "{} lacks COPY_SRC usage",
                src.texture.shared.describe()
            ));
        }
        match texel_copy_region(
            buffer.size,
            &buffer.describe(),
            &dst.layout,
            &src,
            size,
            true,
        ) {
            Ok(region) => self.commands.push(Command::CopyTextureToBuffer {
                texture: src.texture.shared.clone(),
                buffer: buffer.clone(),
                region,
            }),
            Err(e) => self.fail(e),
        }
    }

    /// `finish`. Encoding errors are reported here, as WebGPU does.
    pub fn finish(self) -> CommandBuffer {
        if let Some(e) = &self.error {
            let label = if self.label.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.label)
            };
            self.device
                .validation(format!("command encoder{label}: {e}"));
        }
        CommandBuffer {
            commands: self.commands,
            error: self.error,
            label: self.label,
        }
    }
}

/// `GPURenderPassEncoder`. Ends when [`RenderPassEncoder::end`] is called or it is dropped.
pub struct RenderPassEncoder<'a> {
    encoder: &'a mut CommandEncoder,
    pipeline: Option<Arc<RenderPipelineShared>>,
    binds: BindState,
    vertex_buffers: u32,
    index: Option<(IndexFormat, u64)>,
    color_formats: Vec<Option<TextureFormat>>,
    depth_format: Option<TextureFormat>,
    sample_count: u32,
    extent: (u32, u32),
    ended: bool,
}

impl RenderPassEncoder<'_> {
    fn fail(&mut self, message: impl Into<String>) {
        self.encoder.fail(message);
    }

    /// `setPipeline`.
    pub fn set_pipeline(&mut self, pipeline: &RenderPipeline) {
        let p = &pipeline.shared;
        if !p.valid {
            return self.fail(format!("{} is invalid", p.describe()));
        }
        let mut pass_colors = self.color_formats.clone();
        let mut pipeline_colors = p.color_formats.clone();
        while pass_colors.last() == Some(&None) {
            pass_colors.pop();
        }
        while pipeline_colors.last() == Some(&None) {
            pipeline_colors.pop();
        }
        if pass_colors != pipeline_colors || p.depth_stencil_format != self.depth_format {
            return self.fail(format!(
                "{} does not match the render pass's attachment formats",
                p.describe()
            ));
        }
        if p.sample_count != self.sample_count {
            return self.fail(format!(
                "{} does not match the render pass's sample count",
                p.describe()
            ));
        }
        self.pipeline = Some(p.clone());
        self.encoder.commands.push(Command::SetPipeline(p.clone()));
    }

    /// `setVertexBuffer`.
    pub fn set_vertex_buffer(
        &mut self,
        slot: u32,
        buffer: &Buffer,
        offset: u64,
        size: Option<u64>,
    ) {
        let b = &buffer.shared;
        let size = size.unwrap_or(b.size.saturating_sub(offset));
        if slot >= self.encoder.device.limits.max_vertex_buffers {
            return self.fail("vertex buffer slot is out of range");
        }
        if !b.usage.contains(BufferUsage::VERTEX) {
            return self.fail(format!("{} lacks VERTEX usage", b.describe()));
        }
        if !offset.is_multiple_of(4) || offset + size > b.size {
            return self.fail("vertex buffer offset is misaligned or out of bounds");
        }
        self.vertex_buffers |= 1 << slot;
        self.encoder.commands.push(Command::SetVertexBuffer {
            slot,
            buffer: b.clone(),
            offset,
        });
    }

    /// `setIndexBuffer`.
    pub fn set_index_buffer(
        &mut self,
        buffer: &Buffer,
        format: IndexFormat,
        offset: u64,
        size: Option<u64>,
    ) {
        let b = &buffer.shared;
        let size = size.unwrap_or(b.size.saturating_sub(offset));
        let index_size = match format {
            IndexFormat::Uint16 => 2,
            IndexFormat::Uint32 => 4,
        };
        if !b.usage.contains(BufferUsage::INDEX) {
            return self.fail(format!("{} lacks INDEX usage", b.describe()));
        }
        if !offset.is_multiple_of(index_size) || offset + size > b.size {
            return self.fail("index buffer offset is misaligned or out of bounds");
        }
        self.index = Some((format, size / index_size));
        self.encoder.commands.push(Command::SetIndexBuffer {
            buffer: b.clone(),
            format,
            offset,
        });
    }

    /// `setViewport`, in framebuffer pixels from the top-left corner.
    pub fn set_viewport(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) {
        let (w, h) = (self.extent.0 as f32, self.extent.1 as f32);
        if x < 0.0 || y < 0.0 || width < 0.0 || height < 0.0 || x + width > w || y + height > h {
            return self.fail("the viewport is outside the render targets");
        }
        if !(0.0..=1.0).contains(&min_depth)
            || !(0.0..=1.0).contains(&max_depth)
            || min_depth > max_depth
        {
            return self.fail("viewport depths must satisfy 0 <= minDepth <= maxDepth <= 1");
        }
        // Flipped so that clip-space +y is up, as in WebGPU.
        self.encoder
            .commands
            .push(Command::SetViewport(vk::Viewport {
                x,
                y: y + height,
                width,
                height: -height,
                min_depth,
                max_depth,
            }));
    }

    /// `setScissorRect`.
    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if x + width > self.extent.0 || y + height > self.extent.1 {
            return self.fail("the scissor rectangle is outside the render targets");
        }
        self.encoder.commands.push(Command::SetScissor(vk::Rect2D {
            offset: vk::Offset2D {
                x: x as i32,
                y: y as i32,
            },
            extent: vk::Extent2D { width, height },
        }));
    }

    /// `setBlendConstant`.
    pub fn set_blend_constant(&mut self, color: Color) {
        let c = [
            color.r as f32,
            color.g as f32,
            color.b as f32,
            color.a as f32,
        ];
        self.encoder.commands.push(Command::SetBlendConstant(c));
    }

    /// `setStencilReference`.
    pub fn set_stencil_reference(&mut self, reference: u32) {
        self.encoder
            .commands
            .push(Command::SetStencilReference(reference));
    }

    /// `setBindGroup`. `None` unsets the group.
    pub fn set_bind_group(&mut self, index: u32, group: Option<&BindGroup>, offsets: &[u32]) {
        match self.binds.set(&self.encoder.device, index, group, offsets) {
            Ok(Some(command)) => self.encoder.commands.push(command),
            Ok(None) => {}
            Err(e) => self.fail(e),
        }
    }

    fn check_draw(&mut self) -> bool {
        let Some(pipeline) = &self.pipeline else {
            self.fail("draw called without a pipeline set");
            return false;
        };
        if let Some(layout) = &pipeline.layout {
            if let Err(e) = self.binds.check(layout) {
                let message = format!("{}: {e}", pipeline.describe());
                self.fail(message);
                return false;
            }
        }
        let needed = (1u32 << pipeline.vertex_buffers) - 1;
        if self.vertex_buffers & needed != needed {
            self.fail(format!(
                "{} needs {} vertex buffers set",
                pipeline.describe(),
                pipeline.vertex_buffers
            ));
            return false;
        }
        true
    }

    /// `draw`.
    pub fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        if self.check_draw() {
            self.encoder.commands.push(Command::Draw {
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            });
        }
    }

    /// `drawIndexed`.
    pub fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    ) {
        if !self.check_draw() {
            return;
        }
        match self.index {
            None => self.fail("drawIndexed called without an index buffer set"),
            Some((_, available)) if first_index as u64 + index_count as u64 > available => {
                self.fail("drawIndexed reads past the end of the index buffer")
            }
            Some(_) => self.encoder.commands.push(Command::DrawIndexed {
                index_count,
                instance_count,
                first_index,
                base_vertex,
                first_instance,
            }),
        }
    }

    /// `drawIndirect`: the arguments are four `u32`s at `offset` in `buffer`.
    pub fn draw_indirect(&mut self, buffer: &Buffer, offset: u64) {
        if !self.check_draw() {
            return;
        }
        match indirect_buffer(buffer, offset, 16) {
            Ok(buffer) => self
                .encoder
                .commands
                .push(Command::DrawIndirect { buffer, offset }),
            Err(e) => self.fail(e),
        }
    }

    /// `drawIndexedIndirect`: the arguments are five 32-bit values at `offset` in `buffer`.
    pub fn draw_indexed_indirect(&mut self, buffer: &Buffer, offset: u64) {
        if !self.check_draw() {
            return;
        }
        if self.index.is_none() {
            return self.fail("drawIndexedIndirect called without an index buffer set");
        }
        match indirect_buffer(buffer, offset, 20) {
            Ok(buffer) => self
                .encoder
                .commands
                .push(Command::DrawIndexedIndirect { buffer, offset }),
            Err(e) => self.fail(e),
        }
    }

    /// `end`.
    pub fn end(mut self) {
        self.finish_pass();
    }

    fn finish_pass(&mut self) {
        if !self.ended {
            self.ended = true;
            self.encoder.commands.push(Command::EndRenderPass);
        }
    }
}

impl Drop for RenderPassEncoder<'_> {
    fn drop(&mut self) {
        self.finish_pass();
    }
}

/// Checks an indirect argument buffer and range.
fn indirect_buffer(buffer: &Buffer, offset: u64, size: u64) -> Result<Arc<BufferShared>, String> {
    let b = &buffer.shared;
    if !b.usage.contains(BufferUsage::INDIRECT) {
        return Err(format!("{} lacks INDIRECT usage", b.describe()));
    }
    if !offset.is_multiple_of(4) || offset + size > b.size {
        return Err("the indirect offset is misaligned or out of bounds".into());
    }
    Ok(b.clone())
}

/// The bind groups set in a pass, for draw- and dispatch-time validation.
#[derive(Default)]
struct BindState {
    groups: Vec<Option<Arc<BindGroupShared>>>,
}

impl BindState {
    fn set(
        &mut self,
        device: &DeviceShared,
        index: u32,
        group: Option<&BindGroup>,
        offsets: &[u32],
    ) -> Result<Option<Command>, String> {
        let limits = &device.limits;
        if index >= limits.max_bind_groups {
            return Err(format!("bind group index {index} is out of range"));
        }
        let slot = index as usize;
        if self.groups.len() <= slot {
            self.groups.resize(slot + 1, None);
        }
        let Some(group) = group else {
            self.groups[slot] = None;
            return Ok(None);
        };
        let g = &group.shared;
        if !g.valid {
            return Err(format!("{} is invalid", g.describe()));
        }
        let dynamic: Vec<_> = g.dynamic_buffers().collect();
        if dynamic.len() != offsets.len() {
            return Err(format!(
                "{} needs {} dynamic offsets, {} were given",
                g.describe(),
                dynamic.len(),
                offsets.len()
            ));
        }
        for ((buffer, offset, size, ty), &dynamic) in dynamic.iter().zip(offsets) {
            let alignment = match ty {
                BufferBindingType::Uniform => limits.min_uniform_buffer_offset_alignment,
                _ => limits.min_storage_buffer_offset_alignment,
            };
            if dynamic % alignment != 0 {
                return Err(format!(
                    "dynamic offset {dynamic} is not a multiple of {alignment}"
                ));
            }
            if offset + size + dynamic as u64 > buffer.size {
                return Err(format!(
                    "dynamic offset {dynamic} puts the binding past the end of {}",
                    buffer.describe()
                ));
            }
        }
        self.groups[slot] = Some(g.clone());
        Ok(Some(Command::SetBindGroup {
            index,
            group: g.clone(),
            offsets: offsets.to_vec(),
        }))
    }

    /// Checks that every group the layout has is set, with an equivalent layout.
    fn check(&self, layout: &PipelineLayoutShared) -> Result<(), String> {
        for (index, needed) in layout.groups.iter().enumerate() {
            match self.groups.get(index).and_then(|g| g.as_ref()) {
                None => return Err(format!("no bind group is set at index {index}")),
                Some(group) if !group.layout.equivalent(needed) => {
                    return Err(format!(
                        "{} at index {index} does not match the pipeline's layout",
                        group.describe()
                    ))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

/// `GPUComputePassEncoder`. Ends when [`ComputePassEncoder::end`] is called or it is dropped.
pub struct ComputePassEncoder<'a> {
    encoder: &'a mut CommandEncoder,
    pipeline: Option<Arc<ComputePipelineShared>>,
    binds: BindState,
    ended: bool,
}

impl ComputePassEncoder<'_> {
    fn fail(&mut self, message: impl Into<String>) {
        self.encoder.fail(message);
    }

    /// `setPipeline`.
    pub fn set_pipeline(&mut self, pipeline: &ComputePipeline) {
        let p = &pipeline.shared;
        if !p.valid {
            return self.fail(format!("{} is invalid", p.describe()));
        }
        self.pipeline = Some(p.clone());
        self.encoder
            .commands
            .push(Command::SetComputePipeline(p.clone()));
    }

    /// `setBindGroup`. `None` unsets the group.
    pub fn set_bind_group(&mut self, index: u32, group: Option<&BindGroup>, offsets: &[u32]) {
        match self.binds.set(&self.encoder.device, index, group, offsets) {
            Ok(Some(command)) => self.encoder.commands.push(command),
            Ok(None) => {}
            Err(e) => self.fail(e),
        }
    }

    fn check_dispatch(&mut self) -> bool {
        let Some(pipeline) = &self.pipeline else {
            self.fail("dispatch called without a pipeline set");
            return false;
        };
        if let Some(layout) = &pipeline.layout {
            if let Err(e) = self.binds.check(layout) {
                let message = format!("{}: {e}", pipeline.describe());
                self.fail(message);
                return false;
            }
        }
        true
    }

    /// `dispatchWorkgroups`.
    pub fn dispatch_workgroups(&mut self, x: u32, y: u32, z: u32) {
        if !self.check_dispatch() {
            return;
        }
        let max = self
            .encoder
            .device
            .limits
            .max_compute_workgroups_per_dimension;
        if x > max || y > max || z > max {
            return self.fail(format!(
                "dispatches are limited to {max} workgroups per dimension"
            ));
        }
        self.encoder.commands.push(Command::Dispatch([x, y, z]));
    }

    /// `dispatchWorkgroupsIndirect`: the counts are three `u32`s at `offset` in `buffer`.
    pub fn dispatch_workgroups_indirect(&mut self, buffer: &Buffer, offset: u64) {
        if !self.check_dispatch() {
            return;
        }
        match indirect_buffer(buffer, offset, 12) {
            Ok(buffer) => self
                .encoder
                .commands
                .push(Command::DispatchIndirect { buffer, offset }),
            Err(e) => self.fail(e),
        }
    }

    /// `end`.
    pub fn end(mut self) {
        self.finish_pass();
    }

    fn finish_pass(&mut self) {
        if !self.ended {
            self.ended = true;
            self.encoder.commands.push(Command::EndComputePass);
        }
    }
}

impl Drop for ComputePassEncoder<'_> {
    fn drop(&mut self) {
        self.finish_pass();
    }
}

// ------------------------------------------------------------------------------------------
// The queue

/// `GPUQueue`.
#[derive(Clone)]
pub struct Queue {
    pub(crate) device: Arc<DeviceShared>,
}

impl Queue {
    /// `submit`. Returns the submission's index, which [`Queue::on_submitted_work_done`] and
    /// buffer mapping wait on.
    pub fn submit(&self, buffers: impl IntoIterator<Item = CommandBuffer>) -> u64 {
        let device = &self.device;
        let buffers: Vec<CommandBuffer> = buffers.into_iter().collect();
        for buffer in &buffers {
            let error = buffer
                .error
                .clone()
                .or_else(|| buffer.commands.iter().find_map(|c| c.validate().err()));
            if let Some(e) = error {
                let label = if buffer.label.is_empty() {
                    String::new()
                } else {
                    format!(" \"{}\"", buffer.label)
                };
                device.validation(format!("submit: command buffer{label} is invalid: {e}"));
                return 0;
            }
        }
        let lists: Vec<Vec<Command>> = buffers.into_iter().map(|b| b.commands).collect();
        let mut state = device.lock();
        match device.submit_locked(&mut state, lists, None) {
            Ok(index) => index,
            Err(e) => {
                drop(state);
                device.report(e);
                0
            }
        }
    }

    /// `writeBuffer`. The data is copied now and reaches the buffer before later submissions.
    pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) {
        let device = &self.device;
        let b = &buffer.shared;
        let error = if let Err(e) = b.usable() {
            Some(e)
        } else if !b.usage.contains(BufferUsage::COPY_DST) {
            Some(format!("{} lacks COPY_DST usage", b.describe()))
        } else if !offset.is_multiple_of(4) || !data.len().is_multiple_of(4) {
            Some("writeBuffer offset and size must be multiples of 4".into())
        } else if offset + data.len() as u64 > b.size {
            Some("writeBuffer is out of bounds".into())
        } else {
            None
        };
        if let Some(e) = error {
            return device.validation(format!("writeBuffer: {e}"));
        }
        if data.is_empty() {
            return;
        }
        match BufferShared::staging(device, data) {
            Ok(staging) => device.lock().pending_writes.push(PendingWrite::Buffer {
                staging,
                dst: b.clone(),
                dst_offset: offset,
                size: data.len() as u64,
            }),
            Err(e) => device.report(e),
        }
    }

    /// `writeTexture`.
    pub fn write_texture(
        &self,
        dst: TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: TexelCopyBufferLayout,
        size: Extent3d,
    ) {
        let device = &self.device;
        let texture = &dst.texture.shared;
        if !texture.usage.contains(TextureUsage::COPY_DST) {
            return device.validation(format!(
                "writeTexture: {} lacks COPY_DST usage",
                texture.describe()
            ));
        }
        // Validate against the caller's layout, then repack rows tightly for the staging copy.
        let region =
            match texel_copy_region(data.len() as u64, "the data", &layout, &dst, size, false) {
                Ok(r) => r,
                Err(e) => return device.validation(format!("writeTexture: {e}")),
            };
        let block = copy_block_size(texture.format, dst.aspect).unwrap() as usize;
        let row = size.width as usize * block;
        let bytes_per_row = layout.bytes_per_row.map_or(row, |b| b as usize);
        let rows_per_image = layout
            .rows_per_image
            .map_or(size.height as usize, |r| r as usize);
        let mut packed =
            Vec::with_capacity(row * size.height as usize * size.depth_or_array_layers as usize);
        for image in 0..size.depth_or_array_layers as usize {
            for y in 0..size.height as usize {
                let start = layout.offset as usize
                    + image * rows_per_image * bytes_per_row
                    + y * bytes_per_row;
                packed.extend_from_slice(&data[start..start + row]);
            }
        }
        if packed.is_empty() {
            return;
        }
        let region = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            ..region
        };
        match BufferShared::staging(device, &packed) {
            Ok(staging) => device.lock().pending_writes.push(PendingWrite::Texture {
                staging,
                texture: texture.clone(),
                region,
            }),
            Err(e) => device.report(e),
        }
    }

    /// `onSubmittedWorkDone`.
    pub fn on_submitted_work_done(&self) -> impl Future<Output = ()> {
        self.device.flush_pending_writes();
        let index = self.device.lock().next_index - 1;
        WorkDone {
            device: self.device.clone(),
            index,
        }
    }
}

struct WorkDone {
    device: Arc<DeviceShared>,
    index: u64,
}

impl Future for WorkDone {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.device.is_complete(self.index) {
            Poll::Ready(())
        } else {
            self.device.register_waker(self.index, cx.waker().clone());
            Poll::Pending
        }
    }
}

impl DeviceShared {
    pub fn register_waker(&self, index: u64, waker: Waker) {
        self.lock().wakers.push((index, waker));
    }

    /// Submits queued writes on their own if no other submission is coming to carry them.
    pub fn flush_pending_writes(&self) {
        let mut state = self.lock();
        if state.pending_writes.is_empty() {
            return;
        }
        if let Err(e) = self.submit_locked(&mut state, Vec::new(), None) {
            drop(state);
            self.report(e);
        }
    }

    /// Records and submits command lists (after any queued writes). With `present`, also
    /// moves that swapchain texture to the present layout and signals the semaphore.
    pub(crate) fn submit_locked(
        &self,
        state: &mut DeviceState,
        lists: Vec<Vec<Command>>,
        present: Option<(&Arc<TextureShared>, vk::Semaphore)>,
    ) -> Result<u64, GpuError> {
        if state.lost {
            return Err(GpuError::Internal("the device was lost".into()));
        }
        self.retire(state);
        let fns = &self.fns;
        unsafe {
            let alloc = vk::CommandBufferAllocateInfo {
                s_type: vk::STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                p_next: std::ptr::null(),
                command_pool: state.command_pool,
                level: vk::COMMAND_BUFFER_LEVEL_PRIMARY,
                command_buffer_count: 1,
            };
            let mut cmd = std::ptr::null_mut();
            check(
                (fns.allocate_command_buffers)(self.raw, &alloc, &mut cmd),
                "vkAllocateCommandBuffers",
            )?;
            let begin = vk::CommandBufferBeginInfo {
                s_type: vk::STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                p_next: std::ptr::null(),
                flags: vk::COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
                p_inheritance_info: std::ptr::null(),
            };
            (fns.begin_command_buffer)(cmd, &begin);

            let mut recorder = Recorder::new(self, cmd);
            let writes = std::mem::take(&mut state.pending_writes);
            recorder.pending_writes(&writes);
            for list in &lists {
                for (i, command) in list.iter().enumerate() {
                    recorder.command(command, &list[i + 1..]);
                }
            }
            if let Some((texture, _)) = present {
                recorder.use_texture(texture);
                recorder.barrier(&[(texture.clone(), vk::IMAGE_LAYOUT_PRESENT_SRC_KHR)]);
            }
            let (waits, buffers) = recorder.finish();
            check((fns.end_command_buffer)(cmd), "vkEndCommandBuffer")?;

            let fence_info = vk::FenceCreateInfo {
                s_type: vk::STRUCTURE_TYPE_FENCE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
            };
            let mut fence = 0;
            check(
                (fns.create_fence)(self.raw, &fence_info, std::ptr::null(), &mut fence),
                "vkCreateFence",
            )?;
            let wait_stages = vec![vk::PIPELINE_STAGE_ALL_COMMANDS_BIT; waits.len()];
            let signal: Vec<vk::Semaphore> = present.map(|(_, s)| s).into_iter().collect();
            let submit = vk::SubmitInfo {
                s_type: vk::STRUCTURE_TYPE_SUBMIT_INFO,
                p_next: std::ptr::null(),
                wait_semaphore_count: waits.len() as u32,
                p_wait_semaphores: waits.as_ptr(),
                p_wait_dst_stage_mask: wait_stages.as_ptr(),
                command_buffer_count: 1,
                p_command_buffers: &cmd,
                signal_semaphore_count: signal.len() as u32,
                p_signal_semaphores: signal.as_ptr(),
            };
            let r = (fns.queue_submit)(self.queue, 1, &submit, fence);
            if r != vk::SUCCESS {
                (fns.destroy_fence)(self.raw, fence, std::ptr::null());
                (fns.free_command_buffers)(self.raw, state.command_pool, 1, &cmd);
                if r == vk::ERROR_DEVICE_LOST {
                    state.lost = true;
                }
                return Err(check(r, "vkQueueSubmit").unwrap_err());
            }
            let index = state.next_index;
            state.next_index += 1;
            for buffer in &buffers {
                buffer.state.lock().unwrap().last_use = index;
            }
            let mut resources: Vec<Arc<dyn std::any::Any + Send + Sync>> =
                vec![Arc::new(lists), Arc::new(writes)];
            if let Some((texture, _)) = present {
                resources.push(texture.clone());
            }
            state.in_flight.push_back(Submission {
                index,
                fence,
                command_buffer: cmd,
                resources,
            });
            Ok(index)
        }
    }
}

// ------------------------------------------------------------------------------------------
// Translation to Vulkan

/// Records commands into one Vulkan command buffer, tracking texture layouts as it goes.
struct Recorder<'d> {
    device: &'d DeviceShared,
    cmd: vk::CommandBuffer,
    layouts: Vec<(Arc<TextureShared>, i32)>,
    waits: Vec<vk::Semaphore>,
    buffers: Vec<Arc<BufferShared>>,
    in_pass: bool,
    /// The pass's bind groups and the current pipeline's layout, bound lazily at each draw
    /// or dispatch because Vulkan binds sets against a pipeline layout.
    groups: Vec<Option<(Arc<BindGroupShared>, Vec<u32>)>>,
    layout: Option<Arc<PipelineLayoutShared>>,
    binds_dirty: bool,
}

impl<'d> Recorder<'d> {
    fn new(device: &'d DeviceShared, cmd: vk::CommandBuffer) -> Self {
        Recorder {
            device,
            cmd,
            layouts: Vec::new(),
            waits: Vec::new(),
            buffers: Vec::new(),
            in_pass: false,
            groups: Vec::new(),
            layout: None,
            binds_dirty: false,
        }
    }

    fn layout_slot(&mut self, texture: &Arc<TextureShared>) -> usize {
        if let Some(i) = self
            .layouts
            .iter()
            .position(|(t, _)| Arc::ptr_eq(t, texture))
        {
            return i;
        }
        let current = *texture.layout.lock().unwrap();
        self.layouts.push((texture.clone(), current));
        self.layouts.len() - 1
    }

    /// Notes a texture's use: swapchain images make the submission wait for their acquire.
    fn use_texture(&mut self, texture: &Arc<TextureShared>) {
        if let Some(semaphore) = texture.acquire.lock().unwrap().take() {
            self.waits.push(semaphore);
        }
    }

    fn use_buffer(&mut self, buffer: &Arc<BufferShared>) {
        if !self.buffers.iter().any(|b| Arc::ptr_eq(b, buffer)) {
            self.buffers.push(buffer.clone());
        }
    }

    /// A full memory barrier plus layout transitions. Conservative: every command waits for
    /// all earlier work, which is correct and leaves finer tracking for later.
    fn barrier(&mut self, transitions: &[(Arc<TextureShared>, i32)]) {
        let mut images = Vec::new();
        for (texture, new_layout) in transitions {
            let slot = self.layout_slot(texture);
            let old = self.layouts[slot].1;
            if old == *new_layout
                && images
                    .iter()
                    .all(|b: &vk::ImageMemoryBarrier2| b.image != texture.raw)
            {
                continue;
            }
            self.layouts[slot].1 = *new_layout;
            images.push(vk::ImageMemoryBarrier2 {
                s_type: vk::STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2,
                p_next: std::ptr::null(),
                src_stage_mask: vk::PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
                src_access_mask: vk::ACCESS_2_MEMORY_WRITE_BIT,
                dst_stage_mask: vk::PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
                dst_access_mask: vk::ACCESS_2_MEMORY_READ_BIT | vk::ACCESS_2_MEMORY_WRITE_BIT,
                old_layout: old,
                new_layout: *new_layout,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: texture.raw,
                subresource_range: texture.full_range(),
            });
        }
        let memory = vk::MemoryBarrier2 {
            s_type: vk::STRUCTURE_TYPE_MEMORY_BARRIER_2,
            p_next: std::ptr::null(),
            src_stage_mask: vk::PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
            src_access_mask: vk::ACCESS_2_MEMORY_WRITE_BIT,
            dst_stage_mask: vk::PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
            dst_access_mask: vk::ACCESS_2_MEMORY_READ_BIT | vk::ACCESS_2_MEMORY_WRITE_BIT,
        };
        let dependency = vk::DependencyInfo {
            s_type: vk::STRUCTURE_TYPE_DEPENDENCY_INFO,
            p_next: std::ptr::null(),
            dependency_flags: 0,
            memory_barrier_count: 1,
            p_memory_barriers: &memory,
            buffer_memory_barrier_count: 0,
            p_buffer_memory_barriers: std::ptr::null(),
            image_memory_barrier_count: images.len() as u32,
            p_image_memory_barriers: images.as_ptr(),
        };
        unsafe { (self.device.rendering.cmd_pipeline_barrier2)(self.cmd, &dependency) };
    }

    fn pending_writes(&mut self, writes: &[PendingWrite]) {
        if writes.is_empty() {
            return;
        }
        let textures: Vec<(Arc<TextureShared>, i32)> = writes
            .iter()
            .filter_map(|w| match w {
                PendingWrite::Texture { texture, .. } => {
                    Some((texture.clone(), vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL))
                }
                _ => None,
            })
            .collect();
        self.barrier(&textures);
        let fns = &self.device.fns;
        for write in writes {
            unsafe {
                match write {
                    PendingWrite::Buffer {
                        staging,
                        dst,
                        dst_offset,
                        size,
                    } => {
                        self.use_buffer(dst);
                        let region = vk::BufferCopy {
                            src_offset: 0,
                            dst_offset: *dst_offset,
                            size: *size,
                        };
                        (fns.cmd_copy_buffer)(self.cmd, staging.raw, dst.raw, 1, &region);
                    }
                    PendingWrite::Texture {
                        staging,
                        texture,
                        region,
                    } => {
                        (fns.cmd_copy_buffer_to_image)(
                            self.cmd,
                            staging.raw,
                            texture.raw,
                            vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                            1,
                            region,
                        );
                    }
                }
            }
        }
    }

    /// Textures bound through the pass's bind groups, which live in the `GENERAL` layout.
    fn bound_textures(
        groups: impl Iterator<Item = Arc<BindGroupShared>>,
    ) -> Vec<Arc<TextureShared>> {
        let mut textures: Vec<Arc<TextureShared>> = Vec::new();
        for group in groups {
            for resource in &group.resources {
                if let Bound::Texture(view) = resource {
                    if !textures.iter().any(|t| Arc::ptr_eq(t, &view.texture)) {
                        textures.push(view.texture.clone());
                    }
                }
            }
        }
        textures
    }

    /// Binds the pass's groups against the current pipeline's layout, if they changed.
    fn flush_binds(&mut self, bind_point: i32) {
        if !self.binds_dirty {
            return;
        }
        self.binds_dirty = false;
        let Some(layout) = &self.layout else { return };
        for (index, slot) in self.groups.iter().enumerate().take(layout.groups.len()) {
            if let Some((group, offsets)) = slot {
                unsafe {
                    (self.device.fns.cmd_bind_descriptor_sets)(
                        self.cmd,
                        bind_point,
                        layout.raw,
                        index as u32,
                        1,
                        &group.set,
                        offsets.len() as u32,
                        offsets.as_ptr(),
                    )
                };
            }
        }
    }

    fn command(&mut self, command: &Command, rest: &[Command]) {
        let fns = &self.device.fns;
        let cmd = self.cmd;
        unsafe {
            match command {
                Command::BeginRenderPass(pass) => {
                    let groups = rest
                        .iter()
                        .take_while(|c| !matches!(c, Command::EndRenderPass))
                        .filter_map(|c| match c {
                            Command::SetBindGroup { group, .. } => Some(group.clone()),
                            _ => None,
                        });
                    let general = Self::bound_textures(groups);
                    self.groups.clear();
                    self.layout = None;
                    self.begin_pass(pass, &general);
                }
                Command::EndRenderPass => {
                    if self.in_pass {
                        (self.device.rendering.cmd_end_rendering)(cmd);
                        self.in_pass = false;
                    }
                }
                Command::BeginComputePass => {
                    self.groups.clear();
                    self.layout = None;
                }
                Command::EndComputePass => {}
                Command::SetPipeline(p) => {
                    (fns.cmd_bind_pipeline)(cmd, vk::PIPELINE_BIND_POINT_GRAPHICS, p.raw);
                    self.layout = p.layout.clone();
                    self.binds_dirty = true;
                }
                Command::SetComputePipeline(p) => {
                    (fns.cmd_bind_pipeline)(cmd, vk::PIPELINE_BIND_POINT_COMPUTE, p.raw);
                    self.layout = p.layout.clone();
                    self.binds_dirty = true;
                }
                Command::SetBindGroup {
                    index,
                    group,
                    offsets,
                } => {
                    for resource in &group.resources {
                        match resource {
                            Bound::Buffer { buffer, .. } => self.use_buffer(buffer),
                            Bound::Texture(view) => self.use_texture(&view.texture),
                            Bound::Sampler(_) => {}
                        }
                    }
                    let slot = *index as usize;
                    if self.groups.len() <= slot {
                        self.groups.resize(slot + 1, None);
                    }
                    self.groups[slot] = Some((group.clone(), offsets.clone()));
                    self.binds_dirty = true;
                }
                Command::SetVertexBuffer {
                    slot,
                    buffer,
                    offset,
                } => {
                    self.use_buffer(buffer);
                    (fns.cmd_bind_vertex_buffers)(cmd, *slot, 1, &buffer.raw, offset);
                }
                Command::SetIndexBuffer {
                    buffer,
                    format,
                    offset,
                } => {
                    self.use_buffer(buffer);
                    let kind = match format {
                        IndexFormat::Uint16 => vk::INDEX_TYPE_UINT16,
                        IndexFormat::Uint32 => vk::INDEX_TYPE_UINT32,
                    };
                    (fns.cmd_bind_index_buffer)(cmd, buffer.raw, *offset, kind);
                }
                Command::SetViewport(v) => (fns.cmd_set_viewport)(cmd, 0, 1, v),
                Command::SetScissor(r) => (fns.cmd_set_scissor)(cmd, 0, 1, r),
                Command::SetBlendConstant(c) => (fns.cmd_set_blend_constants)(cmd, c),
                Command::SetStencilReference(r) => (fns.cmd_set_stencil_reference)(cmd, 0x3, *r),
                Command::Draw {
                    vertex_count,
                    instance_count,
                    first_vertex,
                    first_instance,
                } => {
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw)(
                        cmd,
                        *vertex_count,
                        *instance_count,
                        *first_vertex,
                        *first_instance,
                    )
                }
                Command::DrawIndexed {
                    index_count,
                    instance_count,
                    first_index,
                    base_vertex,
                    first_instance,
                } => {
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw_indexed)(
                        cmd,
                        *index_count,
                        *instance_count,
                        *first_index,
                        *base_vertex,
                        *first_instance,
                    )
                }
                Command::DrawIndirect { buffer, offset } => {
                    self.use_buffer(buffer);
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw_indirect)(cmd, buffer.raw, *offset, 1, 16);
                }
                Command::DrawIndexedIndirect { buffer, offset } => {
                    self.use_buffer(buffer);
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw_indexed_indirect)(cmd, buffer.raw, *offset, 1, 20);
                }
                Command::Dispatch([x, y, z]) => {
                    self.before_dispatch();
                    (fns.cmd_dispatch)(cmd, *x, *y, *z);
                }
                Command::DispatchIndirect { buffer, offset } => {
                    self.use_buffer(buffer);
                    self.before_dispatch();
                    (fns.cmd_dispatch_indirect)(cmd, buffer.raw, *offset);
                }
                Command::CopyBufferToBuffer {
                    src,
                    src_offset,
                    dst,
                    dst_offset,
                    size,
                } => {
                    self.use_buffer(src);
                    self.use_buffer(dst);
                    self.barrier(&[]);
                    let region = vk::BufferCopy {
                        src_offset: *src_offset,
                        dst_offset: *dst_offset,
                        size: *size,
                    };
                    (fns.cmd_copy_buffer)(cmd, src.raw, dst.raw, 1, &region);
                }
                Command::CopyBufferToTexture {
                    buffer,
                    texture,
                    region,
                } => {
                    self.use_buffer(buffer);
                    self.use_texture(texture);
                    self.barrier(&[(texture.clone(), vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL)]);
                    (fns.cmd_copy_buffer_to_image)(
                        cmd,
                        buffer.raw,
                        texture.raw,
                        vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        1,
                        region,
                    );
                }
                Command::CopyTextureToBuffer {
                    texture,
                    buffer,
                    region,
                } => {
                    self.use_buffer(buffer);
                    self.use_texture(texture);
                    self.barrier(&[(texture.clone(), vk::IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL)]);
                    (fns.cmd_copy_image_to_buffer)(
                        cmd,
                        texture.raw,
                        vk::IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                        buffer.raw,
                        1,
                        region,
                    );
                }
            }
        }
    }

    /// Waits for earlier work and moves the bound textures to `GENERAL` before a dispatch.
    fn before_dispatch(&mut self) {
        let count = self.layout.as_ref().map_or(0, |l| l.groups.len());
        let groups: Vec<_> = self
            .groups
            .iter()
            .take(count)
            .flatten()
            .map(|(g, _)| g.clone())
            .collect();
        let transitions: Vec<_> = Self::bound_textures(groups.into_iter())
            .into_iter()
            .map(|t| (t, vk::IMAGE_LAYOUT_GENERAL))
            .collect();
        self.barrier(&transitions);
        self.flush_binds(vk::PIPELINE_BIND_POINT_COMPUTE);
    }

    /// `general` holds the textures the pass also binds: they use the `GENERAL` layout
    /// throughout, attachments among them included.
    fn begin_pass(&mut self, pass: &PassRecord, general: &[Arc<TextureShared>]) {
        let layout_of = |texture: &Arc<TextureShared>, optimal: i32| {
            if general.iter().any(|t| Arc::ptr_eq(t, texture)) {
                vk::IMAGE_LAYOUT_GENERAL
            } else {
                optimal
            }
        };
        let color_layout = |view: &TextureViewShared| {
            layout_of(&view.texture, vk::IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL)
        };
        let mut transitions: Vec<(Arc<TextureShared>, i32)> = general
            .iter()
            .map(|t| (t.clone(), vk::IMAGE_LAYOUT_GENERAL))
            .collect();
        for texture in general {
            self.use_texture(texture);
        }
        for color in pass.colors.iter().flatten() {
            self.use_texture(&color.view.texture);
            transitions.push((color.view.texture.clone(), color_layout(&color.view)));
            if let Some(resolve) = &color.resolve {
                self.use_texture(&resolve.texture);
                transitions.push((resolve.texture.clone(), color_layout(resolve)));
            }
        }
        let depth_layout = pass.depth.as_ref().map(|d| {
            layout_of(
                &d.view.texture,
                vk::IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            )
        });
        if let (Some(depth), Some(layout)) = (&pass.depth, depth_layout) {
            self.use_texture(&depth.view.texture);
            transitions.push((depth.view.texture.clone(), layout));
        }
        self.barrier(&transitions);

        let colors: Vec<vk::RenderingAttachmentInfo> = pass
            .colors
            .iter()
            .map(|color| match color {
                Some(c) => vk::RenderingAttachmentInfo {
                    s_type: vk::STRUCTURE_TYPE_RENDERING_ATTACHMENT_INFO,
                    p_next: std::ptr::null(),
                    image_view: c.view.raw,
                    image_layout: color_layout(&c.view),
                    resolve_mode: if c.resolve.is_some() {
                        vk::RESOLVE_MODE_AVERAGE_BIT
                    } else {
                        vk::RESOLVE_MODE_NONE
                    },
                    resolve_image_view: c.resolve.as_ref().map_or(0, |r| r.raw),
                    resolve_image_layout: c
                        .resolve
                        .as_ref()
                        .map_or(vk::IMAGE_LAYOUT_UNDEFINED, |r| color_layout(r)),
                    load_op: match c.load {
                        LoadOp::Clear(_) => vk::ATTACHMENT_LOAD_OP_CLEAR,
                        LoadOp::Load => vk::ATTACHMENT_LOAD_OP_LOAD,
                    },
                    store_op: match c.store {
                        StoreOp::Store => vk::ATTACHMENT_STORE_OP_STORE,
                        StoreOp::Discard => vk::ATTACHMENT_STORE_OP_DONT_CARE,
                    },
                    clear_value: match c.load {
                        LoadOp::Clear(color) => conv::clear_color(color, c.view.format),
                        LoadOp::Load => vk::ClearValue {
                            color_float: [0.0; 4],
                        },
                    },
                },
                None => vk::RenderingAttachmentInfo {
                    s_type: vk::STRUCTURE_TYPE_RENDERING_ATTACHMENT_INFO,
                    p_next: std::ptr::null(),
                    image_view: 0,
                    image_layout: vk::IMAGE_LAYOUT_UNDEFINED,
                    resolve_mode: vk::RESOLVE_MODE_NONE,
                    resolve_image_view: 0,
                    resolve_image_layout: vk::IMAGE_LAYOUT_UNDEFINED,
                    load_op: vk::ATTACHMENT_LOAD_OP_DONT_CARE,
                    store_op: vk::ATTACHMENT_STORE_OP_DONT_CARE,
                    clear_value: vk::ClearValue {
                        color_float: [0.0; 4],
                    },
                },
            })
            .collect();
        let depth_stencil = pass.depth.as_ref().map(|d| {
            let ops = |load: Option<(i32, vk::ClearValue)>, store: Option<StoreOp>| (load, store);
            let depth = ops(
                d.depth.map(|(l, _)| match l {
                    LoadOp::Clear(v) => (
                        vk::ATTACHMENT_LOAD_OP_CLEAR,
                        vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: v,
                                stencil: 0,
                            },
                        },
                    ),
                    LoadOp::Load => (
                        vk::ATTACHMENT_LOAD_OP_LOAD,
                        vk::ClearValue {
                            color_float: [0.0; 4],
                        },
                    ),
                }),
                d.depth.map(|(_, s)| s),
            );
            let stencil = ops(
                d.stencil.map(|(l, _)| match l {
                    LoadOp::Clear(v) => (
                        vk::ATTACHMENT_LOAD_OP_CLEAR,
                        vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: 0.0,
                                stencil: v,
                            },
                        },
                    ),
                    LoadOp::Load => (
                        vk::ATTACHMENT_LOAD_OP_LOAD,
                        vk::ClearValue {
                            color_float: [0.0; 4],
                        },
                    ),
                }),
                d.stencil.map(|(_, s)| s),
            );
            let info = |(load, store): (Option<(i32, vk::ClearValue)>, Option<StoreOp>)| {
                let (load_op, clear_value) = load.unwrap_or((
                    vk::ATTACHMENT_LOAD_OP_LOAD,
                    vk::ClearValue {
                        color_float: [0.0; 4],
                    },
                ));
                vk::RenderingAttachmentInfo {
                    s_type: vk::STRUCTURE_TYPE_RENDERING_ATTACHMENT_INFO,
                    p_next: std::ptr::null(),
                    image_view: d.view.raw,
                    image_layout: depth_layout.unwrap(),
                    resolve_mode: vk::RESOLVE_MODE_NONE,
                    resolve_image_view: 0,
                    resolve_image_layout: vk::IMAGE_LAYOUT_UNDEFINED,
                    load_op,
                    store_op: match store {
                        Some(StoreOp::Discard) => vk::ATTACHMENT_STORE_OP_DONT_CARE,
                        _ => vk::ATTACHMENT_STORE_OP_STORE,
                    },
                    clear_value,
                }
            };
            (d.view.format, info(depth), info(stencil))
        });
        let (width, height) = pass.extent;
        let info = vk::RenderingInfo {
            s_type: vk::STRUCTURE_TYPE_RENDERING_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D { width, height },
            },
            layer_count: 1,
            view_mask: 0,
            color_attachment_count: colors.len() as u32,
            p_color_attachments: colors.as_ptr(),
            p_depth_attachment: match &depth_stencil {
                Some((format, depth, _)) if format.has_depth() => depth,
                _ => std::ptr::null(),
            },
            p_stencil_attachment: match &depth_stencil {
                Some((format, _, stencil)) if format.has_stencil() => stencil,
                _ => std::ptr::null(),
            },
        };
        let fns = &self.device.fns;
        unsafe {
            (self.device.rendering.cmd_begin_rendering)(self.cmd, &info);
            // WebGPU's defaults: the whole target, clip-space +y up.
            let viewport = vk::Viewport {
                x: 0.0,
                y: height as f32,
                width: width as f32,
                height: -(height as f32),
                min_depth: 0.0,
                max_depth: 1.0,
            };
            (fns.cmd_set_viewport)(self.cmd, 0, 1, &viewport);
            (fns.cmd_set_scissor)(self.cmd, 0, 1, &info.render_area);
            (fns.cmd_set_blend_constants)(self.cmd, &[0.0; 4]);
            (fns.cmd_set_stencil_reference)(self.cmd, 0x3, 0);
        }
        self.in_pass = true;
    }

    /// Writes tracked layouts back to the textures and hands back the semaphores to wait on
    /// and the buffers used.
    fn finish(self) -> (Vec<vk::Semaphore>, Vec<Arc<BufferShared>>) {
        for (texture, layout) in &self.layouts {
            *texture.layout.lock().unwrap() = *layout;
        }
        (self.waits, self.buffers)
    }
}
