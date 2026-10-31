//! Command encoding and the queue.
//!
//! Encoders record backend-independent commands holding references to the resources they use.
//! `Queue::submit` validates them, hands them to the backend to record and submit, and keeps
//! the resources alive until the submission completes.

use super::binding::{BindGroupShared, Bound, PipelineLayoutShared};
use super::resource::{
    BufferShared, ComputePipelineShared, RenderPipelineShared, TextureShared, TextureViewShared,
};
use super::{Device, DeviceShared, DeviceState, Submission};
use crate::hal;
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
        copy: hal::TexelCopy,
    },
}

pub(crate) struct ColorRecord {
    pub view: Arc<TextureViewShared>,
    pub resolve: Option<Arc<TextureViewShared>>,
    pub load: LoadOp<Color>,
    pub store: StoreOp,
}

pub(crate) struct DepthRecord {
    pub view: Arc<TextureViewShared>,
    pub depth: Option<(LoadOp<f32>, StoreOp)>,
    pub stencil: Option<(LoadOp<u32>, StoreOp)>,
}

pub(crate) struct PassRecord {
    pub colors: Vec<Option<ColorRecord>>,
    pub depth: Option<DepthRecord>,
    pub extent: (u32, u32),
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
    /// In framebuffer coordinates, y down; backends flip as they need.
    SetViewport {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    },
    SetScissor {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
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
        copy: hal::TexelCopy,
    },
    CopyTextureToBuffer {
        texture: Arc<TextureShared>,
        buffer: Arc<BufferShared>,
        copy: hal::TexelCopy,
    },
}

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
pub(crate) fn copy_block_size(format: TextureFormat, aspect: TextureAspect) -> Option<u32> {
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

/// Validates a buffer-texture copy and describes it for the backend. `strict_rows` applies the
/// 256-byte `bytesPerRow` rule of encoder copies.
fn texel_copy_region(
    buffer_size: u64,
    buffer_name: &str,
    layout: &TexelCopyBufferLayout,
    texture: &TexelCopyTextureInfo<'_>,
    size: Extent3d,
    strict_rows: bool,
) -> Result<hal::TexelCopy, String> {
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
    Ok(hal::TexelCopy {
        buffer_offset: layout.offset,
        bytes_per_row: bytes_per_row as u32,
        rows_per_image: rows_per_image as u32,
        mip_level: texture.mip_level,
        origin: o,
        size,
        aspect: texture.aspect,
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
            Ok(copy) => self.commands.push(Command::CopyBufferToTexture {
                buffer: buffer.clone(),
                texture: dst.texture.shared.clone(),
                copy,
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
            Ok(copy) => self.commands.push(Command::CopyTextureToBuffer {
                texture: src.texture.shared.clone(),
                buffer: buffer.clone(),
                copy,
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
        self.encoder.commands.push(Command::SetViewport {
            x,
            y,
            width,
            height,
            min_depth,
            max_depth,
        });
    }

    /// `setScissorRect`.
    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if x + width > self.extent.0 || y + height > self.extent.1 {
            return self.fail("the scissor rectangle is outside the render targets");
        }
        self.encoder.commands.push(Command::SetScissor {
            x,
            y,
            width,
            height,
        });
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
        let copy =
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
        let copy = hal::TexelCopy {
            buffer_offset: 0,
            bytes_per_row: row as u32,
            rows_per_image: size.height,
            ..copy
        };
        match BufferShared::staging(device, &packed) {
            Ok(staging) => device.lock().pending_writes.push(PendingWrite::Texture {
                staging,
                texture: texture.clone(),
                copy,
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
    /// readies that surface image for presentation.
    pub(crate) fn submit_locked(
        &self,
        state: &mut DeviceState,
        lists: Vec<Vec<Command>>,
        present: Option<hal::Present<'_>>,
    ) -> Result<u64, GpuError> {
        if state.lost {
            return Err(GpuError::Internal("the device was lost".into()));
        }
        self.retire(state);
        let writes = std::mem::take(&mut state.pending_writes);
        let presented = present.as_ref().map(|p| p.texture.clone());
        let raw = match self.raw.submit(&writes, &lists, present) {
            Ok(raw) => raw,
            Err(e) => {
                if e.message() == "the device was lost" {
                    state.lost = true;
                }
                return Err(e);
            }
        };
        let index = state.next_index;
        state.next_index += 1;
        for_each_buffer(&writes, &lists, |buffer| {
            buffer.state.lock().unwrap().last_use = index;
        });
        let mut resources: Vec<Arc<dyn std::any::Any + Send + Sync>> =
            vec![Arc::new(lists), Arc::new(writes)];
        if let Some(texture) = presented {
            resources.push(texture);
        }
        state.in_flight.push_back(Submission {
            index,
            raw,
            resources,
        });
        Ok(index)
    }
}

/// Calls `f` for every buffer a submission's writes and commands use.
fn for_each_buffer(
    writes: &[PendingWrite],
    lists: &[Vec<Command>],
    mut f: impl FnMut(&Arc<BufferShared>),
) {
    for write in writes {
        match write {
            PendingWrite::Buffer { staging, dst, .. } => {
                f(staging);
                f(dst);
            }
            PendingWrite::Texture { staging, .. } => f(staging),
        }
    }
    for command in lists.iter().flatten() {
        match command {
            Command::SetBindGroup { group, .. } => {
                for resource in &group.resources {
                    if let Bound::Buffer { buffer, .. } = resource {
                        f(buffer);
                    }
                }
            }
            Command::SetVertexBuffer { buffer, .. }
            | Command::SetIndexBuffer { buffer, .. }
            | Command::DrawIndirect { buffer, .. }
            | Command::DrawIndexedIndirect { buffer, .. }
            | Command::DispatchIndirect { buffer, .. }
            | Command::CopyBufferToTexture { buffer, .. }
            | Command::CopyTextureToBuffer { buffer, .. } => f(buffer),
            Command::CopyBufferToBuffer { src, dst, .. } => {
                f(src);
                f(dst);
            }
            _ => {}
        }
    }
}
