//! Translation of recorded commands into Vulkan command buffers.
//!
//! Every command waits for all earlier work through a full memory barrier, and textures move
//! between layouts as their uses require. This is correct and conservative; finer tracking can
//! come later without changing the front end.

use super::{check, conv, vk, Device, Submission};
use crate::frontend::{
    copy_block_size, BindGroupShared, Bound, Command, PassRecord, PendingWrite,
    PipelineLayoutShared, TextureShared, TextureViewShared,
};
use crate::hal;
use crate::*;
use std::sync::Arc;

impl Device {
    /// Records the queued writes and then the command lists into one command buffer and
    /// submits it. With `present`, also moves that surface image to the present layout and
    /// signals its semaphore.
    pub fn submit(
        &self,
        writes: &[PendingWrite],
        lists: &[Vec<Command>],
        present: Option<hal::Present<'_>>,
    ) -> Result<Submission, GpuError> {
        let device = &self.inner;
        let fns = &device.fns;
        let pool = device.command_pool.lock().unwrap();
        unsafe {
            let alloc = vk::CommandBufferAllocateInfo {
                s_type: vk::STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                p_next: std::ptr::null(),
                command_pool: *pool,
                level: vk::COMMAND_BUFFER_LEVEL_PRIMARY,
                command_buffer_count: 1,
            };
            let mut cmd = std::ptr::null_mut();
            check(
                (fns.allocate_command_buffers)(device.raw, &alloc, &mut cmd),
                "vkAllocateCommandBuffers",
            )?;
            let begin = vk::CommandBufferBeginInfo {
                s_type: vk::STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                p_next: std::ptr::null(),
                flags: vk::COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
                p_inheritance_info: std::ptr::null(),
            };
            (fns.begin_command_buffer)(cmd, &begin);

            let mut recorder = Recorder::new(device, cmd);
            recorder.pending_writes(writes);
            for list in lists {
                for (i, command) in list.iter().enumerate() {
                    recorder.command(command, &list[i + 1..]);
                }
            }
            let mut signal = Vec::new();
            if let Some(present) = &present {
                recorder.use_texture(present.texture);
                recorder.barrier(&[(present.texture.clone(), vk::IMAGE_LAYOUT_PRESENT_SRC_KHR)]);
                signal.push(present.surface.present_semaphore(present.image));
            }
            let waits = recorder.finish();
            let free = |cmd| (fns.free_command_buffers)(device.raw, *pool, 1, &cmd);
            if let Err(e) = check((fns.end_command_buffer)(cmd), "vkEndCommandBuffer") {
                free(cmd);
                return Err(e);
            }

            let fence_info = vk::FenceCreateInfo {
                s_type: vk::STRUCTURE_TYPE_FENCE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
            };
            let mut fence = 0;
            if let Err(e) = check(
                (fns.create_fence)(device.raw, &fence_info, std::ptr::null(), &mut fence),
                "vkCreateFence",
            ) {
                free(cmd);
                return Err(e);
            }
            let wait_stages = vec![vk::PIPELINE_STAGE_ALL_COMMANDS_BIT; waits.len()];
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
            let r = (fns.queue_submit)(device.queue, 1, &submit, fence);
            if r != vk::SUCCESS {
                (fns.destroy_fence)(device.raw, fence, std::ptr::null());
                free(cmd);
                return Err(if r == vk::ERROR_DEVICE_LOST {
                    GpuError::Internal("the device was lost".into())
                } else {
                    check(r, "vkQueueSubmit").unwrap_err()
                });
            }
            Ok(Submission {
                device: device.clone(),
                fence,
                command_buffer: cmd,
            })
        }
    }
}

/// The Vulkan region of a validated buffer-texture copy.
fn region(texture: &TextureShared, copy: &hal::TexelCopy) -> vk::BufferImageCopy {
    let block = copy_block_size(texture.format, copy.aspect).expect("validated copy aspect");
    let is_3d = texture.dimension == TextureDimension::D3;
    let o = copy.origin;
    vk::BufferImageCopy {
        buffer_offset: copy.buffer_offset,
        buffer_row_length: copy.bytes_per_row / block,
        buffer_image_height: copy.rows_per_image,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: conv::aspect_mask(texture.format, copy.aspect),
            mip_level: copy.mip_level,
            base_array_layer: if is_3d { 0 } else { o.z },
            layer_count: if is_3d {
                1
            } else {
                copy.size.depth_or_array_layers
            },
        },
        image_offset: vk::Offset3D {
            x: o.x as i32,
            y: o.y as i32,
            z: if is_3d { o.z as i32 } else { 0 },
        },
        image_extent: vk::Extent3D {
            width: copy.size.width,
            height: copy.size.height,
            depth: if is_3d {
                copy.size.depth_or_array_layers
            } else {
                1
            },
        },
    }
}

/// Records commands into one Vulkan command buffer, tracking texture layouts as it goes.
struct Recorder<'d> {
    device: &'d super::DeviceInner,
    cmd: vk::CommandBuffer,
    layouts: Vec<(Arc<TextureShared>, i32)>,
    waits: Vec<vk::Semaphore>,
    in_pass: bool,
    /// The pass's bind groups and the current pipeline's layout, bound lazily at each draw
    /// or dispatch because Vulkan binds sets against a pipeline layout.
    groups: Vec<Option<(Arc<BindGroupShared>, Vec<u32>)>>,
    layout: Option<Arc<PipelineLayoutShared>>,
    binds_dirty: bool,
}

impl<'d> Recorder<'d> {
    fn new(device: &'d super::DeviceInner, cmd: vk::CommandBuffer) -> Self {
        Recorder {
            device,
            cmd,
            layouts: Vec::new(),
            waits: Vec::new(),
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
        let current = *texture.hal().layout.lock().unwrap();
        self.layouts.push((texture.clone(), current));
        self.layouts.len() - 1
    }

    /// Notes a texture's use: surface images make the submission wait for their acquire.
    fn use_texture(&mut self, texture: &Arc<TextureShared>) {
        if let Some(semaphore) = texture.hal().acquire.lock().unwrap().take() {
            self.waits.push(semaphore);
        }
    }

    /// A full memory barrier plus layout transitions.
    fn barrier(&mut self, transitions: &[(Arc<TextureShared>, i32)]) {
        let mut images = Vec::new();
        for (texture, new_layout) in transitions {
            let slot = self.layout_slot(texture);
            let old = self.layouts[slot].1;
            let raw = texture.hal();
            if old == *new_layout
                && images
                    .iter()
                    .all(|b: &vk::ImageMemoryBarrier2| b.image != raw.raw)
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
                image: raw.raw,
                subresource_range: raw.full_range(),
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
                        let region = vk::BufferCopy {
                            src_offset: 0,
                            dst_offset: *dst_offset,
                            size: *size,
                        };
                        (fns.cmd_copy_buffer)(
                            self.cmd,
                            staging.hal().raw,
                            dst.hal().raw,
                            1,
                            &region,
                        );
                    }
                    PendingWrite::Texture {
                        staging,
                        texture,
                        copy,
                    } => {
                        (fns.cmd_copy_buffer_to_image)(
                            self.cmd,
                            staging.hal().raw,
                            texture.hal().raw,
                            vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                            1,
                            &region(texture, copy),
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
                        layout.hal().raw,
                        index as u32,
                        1,
                        &group.hal().set,
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
                    (fns.cmd_bind_pipeline)(cmd, vk::PIPELINE_BIND_POINT_GRAPHICS, p.hal().raw);
                    self.layout = p.layout.clone();
                    self.binds_dirty = true;
                }
                Command::SetComputePipeline(p) => {
                    (fns.cmd_bind_pipeline)(cmd, vk::PIPELINE_BIND_POINT_COMPUTE, p.hal().raw);
                    self.layout = p.layout.clone();
                    self.binds_dirty = true;
                }
                Command::SetBindGroup {
                    index,
                    group,
                    offsets,
                } => {
                    for resource in &group.resources {
                        if let Bound::Texture(view) = resource {
                            self.use_texture(&view.texture);
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
                    (fns.cmd_bind_vertex_buffers)(cmd, *slot, 1, &buffer.hal().raw, offset);
                }
                Command::SetIndexBuffer {
                    buffer,
                    format,
                    offset,
                } => {
                    let kind = match format {
                        IndexFormat::Uint16 => vk::INDEX_TYPE_UINT16,
                        IndexFormat::Uint32 => vk::INDEX_TYPE_UINT32,
                    };
                    (fns.cmd_bind_index_buffer)(cmd, buffer.hal().raw, *offset, kind);
                }
                Command::SetViewport {
                    x,
                    y,
                    width,
                    height,
                    min_depth,
                    max_depth,
                } => {
                    // Flipped so that clip-space +y is up, as in WebGPU.
                    let viewport = vk::Viewport {
                        x: *x,
                        y: y + height,
                        width: *width,
                        height: -height,
                        min_depth: *min_depth,
                        max_depth: *max_depth,
                    };
                    (fns.cmd_set_viewport)(cmd, 0, 1, &viewport)
                }
                Command::SetScissor {
                    x,
                    y,
                    width,
                    height,
                } => {
                    let rect = vk::Rect2D {
                        offset: vk::Offset2D {
                            x: *x as i32,
                            y: *y as i32,
                        },
                        extent: vk::Extent2D {
                            width: *width,
                            height: *height,
                        },
                    };
                    (fns.cmd_set_scissor)(cmd, 0, 1, &rect)
                }
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
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw_indirect)(cmd, buffer.hal().raw, *offset, 1, 16);
                }
                Command::DrawIndexedIndirect { buffer, offset } => {
                    self.flush_binds(vk::PIPELINE_BIND_POINT_GRAPHICS);
                    (fns.cmd_draw_indexed_indirect)(cmd, buffer.hal().raw, *offset, 1, 20);
                }
                Command::Dispatch([x, y, z]) => {
                    self.before_dispatch();
                    (fns.cmd_dispatch)(cmd, *x, *y, *z);
                }
                Command::DispatchIndirect { buffer, offset } => {
                    self.before_dispatch();
                    (fns.cmd_dispatch_indirect)(cmd, buffer.hal().raw, *offset);
                }
                Command::CopyBufferToBuffer {
                    src,
                    src_offset,
                    dst,
                    dst_offset,
                    size,
                } => {
                    self.barrier(&[]);
                    let region = vk::BufferCopy {
                        src_offset: *src_offset,
                        dst_offset: *dst_offset,
                        size: *size,
                    };
                    (fns.cmd_copy_buffer)(cmd, src.hal().raw, dst.hal().raw, 1, &region);
                }
                Command::CopyBufferToTexture {
                    buffer,
                    texture,
                    copy,
                } => {
                    self.use_texture(texture);
                    self.barrier(&[(texture.clone(), vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL)]);
                    (fns.cmd_copy_buffer_to_image)(
                        cmd,
                        buffer.hal().raw,
                        texture.hal().raw,
                        vk::IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        1,
                        &region(texture, copy),
                    );
                }
                Command::CopyTextureToBuffer {
                    texture,
                    buffer,
                    copy,
                } => {
                    self.use_texture(texture);
                    self.barrier(&[(texture.clone(), vk::IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL)]);
                    (fns.cmd_copy_image_to_buffer)(
                        cmd,
                        texture.hal().raw,
                        vk::IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                        buffer.hal().raw,
                        1,
                        &region(texture, copy),
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
                    image_view: c.view.hal().raw,
                    image_layout: color_layout(&c.view),
                    resolve_mode: if c.resolve.is_some() {
                        vk::RESOLVE_MODE_AVERAGE_BIT
                    } else {
                        vk::RESOLVE_MODE_NONE
                    },
                    resolve_image_view: c.resolve.as_ref().map_or(0, |r| r.hal().raw),
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
                    image_view: d.view.hal().raw,
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

    /// Writes tracked layouts back to the textures and hands back the semaphores to wait on.
    fn finish(self) -> Vec<vk::Semaphore> {
        for (texture, layout) in &self.layouts {
            *texture.hal().layout.lock().unwrap() = *layout;
        }
        self.waits
    }
}
