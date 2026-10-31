//! Vulkan objects: buffers, images, views, samplers, descriptor sets, shader modules and
//! pipelines.

use super::{check, conv, vk, Device, DeviceInner};
use crate::hal;
use crate::*;
use std::ffi::{c_void, CString};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------------------------------
// Buffers

pub(crate) struct Buffer {
    device: Arc<DeviceInner>,
    pub raw: vk::Buffer,
    memory: vk::DeviceMemory,
    /// Persistent mapping of host-visible memory, or null.
    mapped: *mut u8,
}

unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

impl Buffer {
    /// The persistent mapping of a host-visible buffer, or null.
    pub fn mapped(&self) -> *mut u8 {
        self.mapped
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_buffer)(self.device.raw, self.raw, std::ptr::null());
            (self.device.fns.free_memory)(self.device.raw, self.memory, std::ptr::null());
        }
    }
}

impl Device {
    /// Creates a buffer. `host_visible` asks for persistently mapped memory, for mapping and
    /// staging.
    pub fn create_buffer(
        &self,
        size: u64,
        usage: BufferUsage,
        host_visible: bool,
    ) -> Result<Buffer, GpuError> {
        let device = &self.inner;
        let info = vk::BufferCreateInfo {
            s_type: vk::STRUCTURE_TYPE_BUFFER_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            size: size.max(4),
            usage: conv::buffer_usage(usage),
            sharing_mode: vk::SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
        };
        unsafe {
            let mut raw = 0;
            check(
                (device.fns.create_buffer)(device.raw, &info, std::ptr::null(), &mut raw),
                "vkCreateBuffer",
            )?;
            let mut requirements = vk::MemoryRequirements::default();
            (device.fns.get_buffer_memory_requirements)(device.raw, raw, &mut requirements);
            let (required, preferred) = if host_visible {
                let cached = if usage.contains(BufferUsage::MAP_READ) {
                    vk::MEMORY_PROPERTY_HOST_CACHED_BIT
                } else {
                    0
                };
                (
                    vk::MEMORY_PROPERTY_HOST_VISIBLE_BIT | vk::MEMORY_PROPERTY_HOST_COHERENT_BIT,
                    cached,
                )
            } else {
                (0, vk::MEMORY_PROPERTY_DEVICE_LOCAL_BIT)
            };
            let memory = match device.allocate(requirements, required, preferred) {
                Ok(m) => m,
                Err(e) => {
                    (device.fns.destroy_buffer)(device.raw, raw, std::ptr::null());
                    return Err(e);
                }
            };
            (device.fns.bind_buffer_memory)(device.raw, raw, memory, 0);
            let mut buffer = Buffer {
                device: device.clone(),
                raw,
                memory,
                mapped: std::ptr::null_mut(),
            };
            if !host_visible {
                return Ok(buffer);
            }
            let mut ptr: *mut c_void = std::ptr::null_mut();
            check(
                (device.fns.map_memory)(device.raw, memory, 0, vk::WHOLE_SIZE, 0, &mut ptr),
                "vkMapMemory",
            )?;
            buffer.mapped = ptr as *mut u8;
            Ok(buffer)
        }
    }
}

// ------------------------------------------------------------------------------------------
// Textures

pub(crate) struct Texture {
    device: Arc<DeviceInner>,
    pub raw: vk::Image,
    /// Zero for swapchain images, which the swapchain owns.
    memory: vk::DeviceMemory,
    owned: bool,
    pub format: TextureFormat,
    pub mip_level_count: u32,
    pub array_layers: u32,
    /// Current layout of every subresource; transitions are whole-texture for now.
    pub layout: Mutex<i32>,
    /// For swapchain images: the semaphore the first submission using the image must wait on.
    pub acquire: Mutex<Option<vk::Semaphore>>,
}

unsafe impl Send for Texture {}
unsafe impl Sync for Texture {}

impl Texture {
    pub fn full_range(&self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: conv::aspect_mask(self.format, TextureAspect::All),
            base_mip_level: 0,
            level_count: self.mip_level_count,
            base_array_layer: 0,
            layer_count: self.array_layers,
        }
    }

    /// Wraps a swapchain image.
    pub(super) fn swapchain_image(
        device: &Arc<DeviceInner>,
        raw: vk::Image,
        format: TextureFormat,
        acquire: vk::Semaphore,
    ) -> Texture {
        Texture {
            device: device.clone(),
            raw,
            memory: 0,
            owned: false,
            format,
            mip_level_count: 1,
            array_layers: 1,
            layout: Mutex::new(vk::IMAGE_LAYOUT_UNDEFINED),
            acquire: Mutex::new(Some(acquire)),
        }
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                (self.device.fns.destroy_image)(self.device.raw, self.raw, std::ptr::null());
                (self.device.fns.free_memory)(self.device.raw, self.memory, std::ptr::null());
            }
        }
    }
}

impl Device {
    /// Creates an image from a validated descriptor.
    pub fn create_texture(&self, desc: &TextureDescriptor<'_>) -> Result<Texture, GpuError> {
        let device = &self.inner;
        let size = desc.size;
        let vk_format = device.adapter.vk_format(desc.format);
        let array_layers = if desc.dimension == TextureDimension::D3 {
            1
        } else {
            size.depth_or_array_layers
        };
        let depth = if desc.dimension == TextureDimension::D3 {
            size.depth_or_array_layers
        } else {
            1
        };
        let cube_compatible = desc.dimension == TextureDimension::D2
            && array_layers >= 6
            && size.width == size.height;
        let info = vk::ImageCreateInfo {
            s_type: vk::STRUCTURE_TYPE_IMAGE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: if cube_compatible {
                vk::IMAGE_CREATE_CUBE_COMPATIBLE_BIT
            } else {
                0
            },
            image_type: match desc.dimension {
                TextureDimension::D1 => vk::IMAGE_TYPE_1D,
                TextureDimension::D2 => vk::IMAGE_TYPE_2D,
                TextureDimension::D3 => vk::IMAGE_TYPE_3D,
            },
            format: vk_format,
            extent: vk::Extent3D {
                width: size.width,
                height: size.height,
                depth,
            },
            mip_levels: desc.mip_level_count,
            array_layers,
            samples: desc.sample_count,
            tiling: vk::IMAGE_TILING_OPTIMAL,
            usage: conv::texture_usage(desc.usage, desc.format),
            sharing_mode: vk::SHARING_MODE_EXCLUSIVE,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
            initial_layout: vk::IMAGE_LAYOUT_UNDEFINED,
        };
        unsafe {
            let mut raw = 0;
            check(
                (device.fns.create_image)(device.raw, &info, std::ptr::null(), &mut raw),
                "vkCreateImage",
            )?;
            let mut requirements = vk::MemoryRequirements::default();
            (device.fns.get_image_memory_requirements)(device.raw, raw, &mut requirements);
            let memory =
                match device.allocate(requirements, 0, vk::MEMORY_PROPERTY_DEVICE_LOCAL_BIT) {
                    Ok(memory) => memory,
                    Err(e) => {
                        (device.fns.destroy_image)(device.raw, raw, std::ptr::null());
                        return Err(e);
                    }
                };
            (device.fns.bind_image_memory)(device.raw, raw, memory, 0);
            Ok(Texture {
                device: device.clone(),
                raw,
                memory,
                owned: true,
                format: desc.format,
                mip_level_count: desc.mip_level_count,
                array_layers,
                layout: Mutex::new(vk::IMAGE_LAYOUT_UNDEFINED),
                acquire: Mutex::new(None),
            })
        }
    }

    /// Creates a view of `texture` from a validated range.
    pub fn create_view(
        &self,
        texture: &Texture,
        desc: &hal::ViewDesc,
    ) -> Result<TextureView, GpuError> {
        let device = &self.inner;
        let info = vk::ImageViewCreateInfo {
            s_type: vk::STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            image: texture.raw,
            view_type: match desc.dimension {
                TextureViewDimension::D1 => vk::IMAGE_VIEW_TYPE_1D,
                TextureViewDimension::D2 => vk::IMAGE_VIEW_TYPE_2D,
                TextureViewDimension::D2Array => vk::IMAGE_VIEW_TYPE_2D_ARRAY,
                TextureViewDimension::Cube => vk::IMAGE_VIEW_TYPE_CUBE,
                TextureViewDimension::CubeArray => vk::IMAGE_VIEW_TYPE_CUBE_ARRAY,
                TextureViewDimension::D3 => vk::IMAGE_VIEW_TYPE_3D,
            },
            format: device.adapter.vk_format(desc.format),
            components: vk::ComponentMapping::default(),
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: conv::aspect_mask(desc.format, desc.aspect),
                base_mip_level: desc.base_mip_level,
                level_count: desc.mip_level_count,
                base_array_layer: desc.base_array_layer,
                layer_count: desc.array_layer_count,
            },
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_image_view)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        check(r, "vkCreateImageView")?;
        Ok(TextureView {
            device: device.clone(),
            raw,
        })
    }
}

pub(crate) struct TextureView {
    device: Arc<DeviceInner>,
    pub raw: vk::ImageView,
}

unsafe impl Send for TextureView {}
unsafe impl Sync for TextureView {}

impl Drop for TextureView {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_image_view)(self.device.raw, self.raw, std::ptr::null())
        };
    }
}

// ------------------------------------------------------------------------------------------
// Samplers

pub(crate) struct Sampler {
    device: Arc<DeviceInner>,
    pub raw: vk::Sampler,
}

unsafe impl Send for Sampler {}
unsafe impl Sync for Sampler {}

impl Drop for Sampler {
    fn drop(&mut self) {
        unsafe { (self.device.fns.destroy_sampler)(self.device.raw, self.raw, std::ptr::null()) };
    }
}

impl Device {
    pub fn create_sampler(&self, desc: &SamplerDescriptor<'_>) -> Result<Sampler, GpuError> {
        let device = &self.inner;
        // Devices without anisotropy clamp to 1, which WebGPU allows ("at most").
        let anisotropy = if device.has_feature_flag(super::FEATURE_SAMPLER_ANISOTROPY) {
            (desc.max_anisotropy as f32)
                .min(device.adapter.properties.limits.max_sampler_anisotropy)
        } else {
            1.0
        };
        let filter = |f: FilterMode| match f {
            FilterMode::Nearest => vk::FILTER_NEAREST,
            FilterMode::Linear => vk::FILTER_LINEAR,
        };
        let address = |a: AddressMode| match a {
            AddressMode::ClampToEdge => vk::SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
            AddressMode::Repeat => vk::SAMPLER_ADDRESS_MODE_REPEAT,
            AddressMode::MirrorRepeat => vk::SAMPLER_ADDRESS_MODE_MIRRORED_REPEAT,
        };
        let info = vk::SamplerCreateInfo {
            s_type: vk::STRUCTURE_TYPE_SAMPLER_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            mag_filter: filter(desc.mag_filter),
            min_filter: filter(desc.min_filter),
            mipmap_mode: match desc.mipmap_filter {
                MipmapFilterMode::Nearest => vk::SAMPLER_MIPMAP_MODE_NEAREST,
                MipmapFilterMode::Linear => vk::SAMPLER_MIPMAP_MODE_LINEAR,
            },
            address_mode_u: address(desc.address_mode_u),
            address_mode_v: address(desc.address_mode_v),
            address_mode_w: address(desc.address_mode_w),
            mip_lod_bias: 0.0,
            anisotropy_enable: (anisotropy > 1.0) as u32,
            max_anisotropy: anisotropy,
            compare_enable: desc.compare.is_some() as u32,
            compare_op: desc.compare.map_or(0, conv::compare),
            min_lod: desc.lod_min_clamp,
            max_lod: desc.lod_max_clamp,
            border_color: vk::BORDER_COLOR_FLOAT_TRANSPARENT_BLACK,
            unnormalized_coordinates: vk::FALSE,
        };
        let mut raw = 0;
        let r =
            unsafe { (device.fns.create_sampler)(device.raw, &info, std::ptr::null(), &mut raw) };
        check(r, "vkCreateSampler")?;
        Ok(Sampler {
            device: device.clone(),
            raw,
        })
    }
}

// ------------------------------------------------------------------------------------------
// Bind group layouts, pipeline layouts and bind groups

pub(crate) struct BindGroupLayout {
    device: Arc<DeviceInner>,
    pub raw: vk::DescriptorSetLayout,
    /// The descriptor type of each entry, in entry order.
    types: Vec<i32>,
}

unsafe impl Send for BindGroupLayout {}
unsafe impl Sync for BindGroupLayout {}

impl Drop for BindGroupLayout {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_descriptor_set_layout)(
                self.device.raw,
                self.raw,
                std::ptr::null(),
            )
        };
    }
}

fn descriptor_type(ty: &BindingType) -> i32 {
    match ty {
        BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            ..
        } => vk::DESCRIPTOR_TYPE_UNIFORM_BUFFER,
        BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            ..
        } => vk::DESCRIPTOR_TYPE_UNIFORM_BUFFER_DYNAMIC,
        BindingType::Buffer {
            has_dynamic_offset: false,
            ..
        } => vk::DESCRIPTOR_TYPE_STORAGE_BUFFER,
        BindingType::Buffer { .. } => vk::DESCRIPTOR_TYPE_STORAGE_BUFFER_DYNAMIC,
        BindingType::Sampler(_) => vk::DESCRIPTOR_TYPE_SAMPLER,
        BindingType::Texture { .. } => vk::DESCRIPTOR_TYPE_SAMPLED_IMAGE,
        BindingType::StorageTexture { .. } => vk::DESCRIPTOR_TYPE_STORAGE_IMAGE,
    }
}

fn stage_flags(stages: ShaderStages) -> vk::Flags {
    let mut flags = 0;
    if stages.contains(ShaderStages::VERTEX) {
        flags |= vk::SHADER_STAGE_VERTEX_BIT;
    }
    if stages.contains(ShaderStages::FRAGMENT) {
        flags |= vk::SHADER_STAGE_FRAGMENT_BIT;
    }
    if stages.contains(ShaderStages::COMPUTE) {
        flags |= vk::SHADER_STAGE_COMPUTE_BIT;
    }
    flags
}

pub(crate) struct PipelineLayout {
    device: Arc<DeviceInner>,
    pub raw: vk::PipelineLayout,
}

unsafe impl Send for PipelineLayout {}
unsafe impl Sync for PipelineLayout {}

impl Drop for PipelineLayout {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_pipeline_layout)(self.device.raw, self.raw, std::ptr::null())
        };
    }
}

pub(crate) struct BindGroup {
    device: Arc<DeviceInner>,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
}

unsafe impl Send for BindGroup {}
unsafe impl Sync for BindGroup {}

impl Drop for BindGroup {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_descriptor_pool)(self.device.raw, self.pool, std::ptr::null())
        };
    }
}

impl Device {
    /// Creates a descriptor set layout from validated entries sorted by binding.
    pub fn create_bind_group_layout(
        &self,
        entries: &[BindGroupLayoutEntry],
    ) -> Result<BindGroupLayout, GpuError> {
        let device = &self.inner;
        let bindings: Vec<vk::DescriptorSetLayoutBinding> = entries
            .iter()
            .map(|e| vk::DescriptorSetLayoutBinding {
                binding: e.binding,
                descriptor_type: descriptor_type(&e.ty),
                descriptor_count: 1,
                stage_flags: stage_flags(e.visibility),
                p_immutable_samplers: std::ptr::null(),
            })
            .collect();
        let info = vk::DescriptorSetLayoutCreateInfo {
            s_type: vk::STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            binding_count: bindings.len() as u32,
            p_bindings: bindings.as_ptr(),
        };
        let mut raw = 0;
        unsafe {
            check(
                (device.fns.create_descriptor_set_layout)(
                    device.raw,
                    &info,
                    std::ptr::null(),
                    &mut raw,
                ),
                "vkCreateDescriptorSetLayout",
            )?;
        }
        Ok(BindGroupLayout {
            device: device.clone(),
            raw,
            types: bindings.iter().map(|b| b.descriptor_type).collect(),
        })
    }

    pub fn create_pipeline_layout(
        &self,
        groups: &[&BindGroupLayout],
    ) -> Result<PipelineLayout, GpuError> {
        let device = &self.inner;
        let set_layouts: Vec<vk::DescriptorSetLayout> = groups.iter().map(|g| g.raw).collect();
        let info = vk::PipelineLayoutCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            set_layout_count: set_layouts.len() as u32,
            p_set_layouts: set_layouts.as_ptr(),
            push_constant_range_count: 0,
            p_push_constant_ranges: std::ptr::null(),
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_pipeline_layout)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        check(r, "vkCreatePipelineLayout")?;
        Ok(PipelineLayout {
            device: device.clone(),
            raw,
        })
    }

    /// Allocates a descriptor set from a pool of its own and writes the resources, given in
    /// layout-entry order, into it.
    pub fn create_bind_group(
        &self,
        layout: &BindGroupLayout,
        entries: &[BindGroupLayoutEntry],
        resources: &[hal::Resource<'_>],
    ) -> Result<BindGroup, GpuError> {
        let device = &self.inner;
        let fns = &device.fns;
        let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
        for &ty in &layout.types {
            match sizes.iter_mut().find(|s| s.descriptor_type == ty) {
                Some(size) => size.descriptor_count += 1,
                None => sizes.push(vk::DescriptorPoolSize {
                    descriptor_type: ty,
                    descriptor_count: 1,
                }),
            }
        }
        let pool_info = vk::DescriptorPoolCreateInfo {
            s_type: vk::STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            max_sets: 1,
            pool_size_count: sizes.len() as u32,
            p_pool_sizes: sizes.as_ptr(),
        };
        unsafe {
            let mut pool = 0;
            check(
                (fns.create_descriptor_pool)(device.raw, &pool_info, std::ptr::null(), &mut pool),
                "vkCreateDescriptorPool",
            )?;
            let alloc = vk::DescriptorSetAllocateInfo {
                s_type: vk::STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO,
                p_next: std::ptr::null(),
                descriptor_pool: pool,
                descriptor_set_count: 1,
                p_set_layouts: &layout.raw,
            };
            let mut set = 0;
            if let Err(e) = check(
                (fns.allocate_descriptor_sets)(device.raw, &alloc, &mut set),
                "vkAllocateDescriptorSets",
            ) {
                (fns.destroy_descriptor_pool)(device.raw, pool, std::ptr::null());
                return Err(e);
            }

            // Infos live in vectors sized up front so the pointers the writes hold stay valid.
            let mut buffers = Vec::with_capacity(resources.len());
            let mut images = Vec::with_capacity(resources.len());
            let mut writes = Vec::with_capacity(resources.len());
            for ((entry, resource), &ty) in entries.iter().zip(resources).zip(&layout.types) {
                let (p_image_info, p_buffer_info) = match resource {
                    hal::Resource::Buffer {
                        buffer,
                        offset,
                        size,
                    } => {
                        buffers.push(vk::DescriptorBufferInfo {
                            buffer: buffer.raw,
                            offset: *offset,
                            range: *size,
                        });
                        (std::ptr::null(), buffers.last().unwrap() as *const _)
                    }
                    hal::Resource::Sampler(sampler) => {
                        images.push(vk::DescriptorImageInfo {
                            sampler: sampler.raw,
                            image_view: 0,
                            image_layout: vk::IMAGE_LAYOUT_UNDEFINED,
                        });
                        (images.last().unwrap() as *const _, std::ptr::null())
                    }
                    // Every texture reached through a bind group is used in the `GENERAL`
                    // layout, so the descriptor stays valid however the texture is used
                    // elsewhere in the same pass.
                    hal::Resource::Texture(view) => {
                        images.push(vk::DescriptorImageInfo {
                            sampler: 0,
                            image_view: view.raw,
                            image_layout: vk::IMAGE_LAYOUT_GENERAL,
                        });
                        (images.last().unwrap() as *const _, std::ptr::null())
                    }
                };
                writes.push(vk::WriteDescriptorSet {
                    s_type: vk::STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET,
                    p_next: std::ptr::null(),
                    dst_set: set,
                    dst_binding: entry.binding,
                    dst_array_element: 0,
                    descriptor_count: 1,
                    descriptor_type: ty,
                    p_image_info,
                    p_buffer_info,
                    p_texel_buffer_view: std::ptr::null(),
                });
            }
            (fns.update_descriptor_sets)(
                device.raw,
                writes.len() as u32,
                writes.as_ptr(),
                0,
                std::ptr::null(),
            );
            Ok(BindGroup {
                device: device.clone(),
                pool,
                set,
            })
        }
    }
}

// ------------------------------------------------------------------------------------------
// Shader modules

pub(crate) struct ShaderModule {
    device: Arc<DeviceInner>,
    pub raw: vk::ShaderModule,
}

unsafe impl Send for ShaderModule {}
unsafe impl Sync for ShaderModule {}

impl Drop for ShaderModule {
    fn drop(&mut self) {
        unsafe {
            (self.device.fns.destroy_shader_module)(self.device.raw, self.raw, std::ptr::null())
        };
    }
}

impl Device {
    /// Lowers a checked module to SPIR-V. A code generation error is returned as `Ok(Err)`,
    /// for the compilation messages.
    pub fn create_shader_module(
        &self,
        module: &shallot_wgsl::Module,
    ) -> Result<Result<ShaderModule, shallot_wgsl::Error>, GpuError> {
        let device = &self.inner;
        let words = match module.to_spirv(&shallot_wgsl::SpirvOptions::default()) {
            Ok(w) => w,
            Err(e) => return Ok(Err(e)),
        };
        let info = vk::ShaderModuleCreateInfo {
            s_type: vk::STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            code_size: words.len() * 4,
            p_code: words.as_ptr(),
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_shader_module)(device.raw, &info, std::ptr::null(), &mut raw)
        };
        check(r, "vkCreateShaderModule")?;
        Ok(Ok(ShaderModule {
            device: device.clone(),
            raw,
        }))
    }
}

// ------------------------------------------------------------------------------------------
// Pipelines

pub(crate) struct RenderPipeline {
    device: Arc<DeviceInner>,
    pub raw: vk::Pipeline,
}

unsafe impl Send for RenderPipeline {}
unsafe impl Sync for RenderPipeline {}

impl Drop for RenderPipeline {
    fn drop(&mut self) {
        unsafe { (self.device.fns.destroy_pipeline)(self.device.raw, self.raw, std::ptr::null()) };
    }
}

pub(crate) struct ComputePipeline {
    device: Arc<DeviceInner>,
    pub raw: vk::Pipeline,
}

unsafe impl Send for ComputePipeline {}
unsafe impl Sync for ComputePipeline {}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        unsafe { (self.device.fns.destroy_pipeline)(self.device.raw, self.raw, std::ptr::null()) };
    }
}

impl Device {
    pub fn create_render_pipeline(
        &self,
        desc: &hal::RenderPipelineDesc<'_>,
    ) -> Result<RenderPipeline, GpuError> {
        let device = &self.inner;
        let mut bindings = Vec::new();
        let mut attributes = Vec::new();
        for (slot, layout) in desc.buffers.iter().enumerate() {
            bindings.push(vk::VertexInputBindingDescription {
                binding: slot as u32,
                stride: layout.array_stride as u32,
                input_rate: match layout.step_mode {
                    VertexStepMode::Vertex => vk::VERTEX_INPUT_RATE_VERTEX,
                    VertexStepMode::Instance => vk::VERTEX_INPUT_RATE_INSTANCE,
                },
            });
            for attribute in layout.attributes {
                attributes.push(vk::VertexInputAttributeDescription {
                    location: attribute.shader_location,
                    binding: slot as u32,
                    format: conv::vertex_format(attribute.format),
                    offset: attribute.offset as u32,
                });
            }
        }
        let mut vk_color_formats = Vec::new();
        let mut blends = Vec::new();
        for target in desc.targets {
            let Some(target) = target else {
                vk_color_formats.push(vk::FORMAT_UNDEFINED);
                blends.push(vk::PipelineColorBlendAttachmentState::default());
                continue;
            };
            vk_color_formats.push(device.adapter.vk_format(target.format));
            let blend = target.blend.unwrap_or_default();
            blends.push(vk::PipelineColorBlendAttachmentState {
                blend_enable: target.blend.is_some() as u32,
                src_color_blend_factor: conv::blend_factor(blend.color.src_factor),
                dst_color_blend_factor: conv::blend_factor(blend.color.dst_factor),
                color_blend_op: conv::blend_op(blend.color.operation),
                src_alpha_blend_factor: conv::blend_factor(blend.alpha.src_factor),
                dst_alpha_blend_factor: conv::blend_factor(blend.alpha.dst_factor),
                alpha_blend_op: conv::blend_op(blend.alpha.operation),
                color_write_mask: target.write_mask.0,
            });
        }
        let strip = matches!(
            desc.primitive.topology,
            PrimitiveTopology::LineStrip | PrimitiveTopology::TriangleStrip
        );

        let entry_names: Vec<CString> = std::iter::once(&desc.vertex.entry.name)
            .chain(desc.fragment.as_ref().map(|f| &f.entry.name))
            .map(|n| CString::new(n.as_str()).unwrap())
            .collect();
        let mut stages = vec![vk::PipelineShaderStageCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            stage: vk::SHADER_STAGE_VERTEX_BIT,
            module: desc.vertex.module.raw,
            p_name: entry_names[0].as_ptr(),
            p_specialization_info: std::ptr::null(),
        }];
        if let Some(f) = &desc.fragment {
            stages.push(vk::PipelineShaderStageCreateInfo {
                s_type: vk::STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                stage: vk::SHADER_STAGE_FRAGMENT_BIT,
                module: f.module.raw,
                p_name: entry_names[1].as_ptr(),
                p_specialization_info: std::ptr::null(),
            });
        }
        let vertex_input = vk::PipelineVertexInputStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            vertex_binding_description_count: bindings.len() as u32,
            p_vertex_binding_descriptions: bindings.as_ptr(),
            vertex_attribute_description_count: attributes.len() as u32,
            p_vertex_attribute_descriptions: attributes.as_ptr(),
        };
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            topology: conv::topology(desc.primitive.topology),
            primitive_restart_enable: (strip && desc.primitive.strip_index_format.is_some()) as u32,
        };
        let viewport = vk::PipelineViewportStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            viewport_count: 1,
            p_viewports: std::ptr::null(),
            scissor_count: 1,
            p_scissors: std::ptr::null(),
        };
        let depth_bias = desc
            .depth_stencil
            .map(|d| (d.depth_bias, d.depth_bias_slope_scale, d.depth_bias_clamp));
        let (bias, slope, clamp) = depth_bias.unwrap_or((0, 0.0, 0.0));
        let rasterization = vk::PipelineRasterizationStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            depth_clamp_enable: vk::FALSE,
            rasterizer_discard_enable: vk::FALSE,
            polygon_mode: vk::POLYGON_MODE_FILL,
            cull_mode: match desc.primitive.cull_mode {
                CullMode::None => vk::CULL_MODE_NONE,
                CullMode::Front => vk::CULL_MODE_FRONT_BIT,
                CullMode::Back => vk::CULL_MODE_BACK_BIT,
            },
            // The flipped viewport keeps WebGPU's winding, so this maps directly.
            front_face: match desc.primitive.front_face {
                FrontFace::Ccw => vk::FRONT_FACE_COUNTER_CLOCKWISE,
                FrontFace::Cw => vk::FRONT_FACE_CLOCKWISE,
            },
            depth_bias_enable: (bias != 0 || slope != 0.0) as u32,
            depth_bias_constant_factor: bias as f32,
            depth_bias_clamp: clamp,
            depth_bias_slope_factor: slope,
            line_width: 1.0,
        };
        let sample_mask = desc.multisample.mask;
        let multisample = vk::PipelineMultisampleStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            rasterization_samples: desc.multisample.count,
            sample_shading_enable: vk::FALSE,
            min_sample_shading: 0.0,
            p_sample_mask: &sample_mask,
            alpha_to_coverage_enable: desc.multisample.alpha_to_coverage_enabled as u32,
            alpha_to_one_enable: vk::FALSE,
        };
        let depth_stencil = desc
            .depth_stencil
            .map(|ds| vk::PipelineDepthStencilStateCreateInfo {
                s_type: vk::STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                depth_test_enable: ds.format.has_depth() as u32,
                depth_write_enable: ds.depth_write_enabled as u32,
                depth_compare_op: conv::compare(ds.depth_compare),
                depth_bounds_test_enable: vk::FALSE,
                stencil_test_enable: ds.format.has_stencil() as u32,
                front: conv::stencil_face(
                    &ds.stencil_front,
                    ds.stencil_read_mask,
                    ds.stencil_write_mask,
                ),
                back: conv::stencil_face(
                    &ds.stencil_back,
                    ds.stencil_read_mask,
                    ds.stencil_write_mask,
                ),
                min_depth_bounds: 0.0,
                max_depth_bounds: 1.0,
            });
        let color_blend = vk::PipelineColorBlendStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            logic_op_enable: vk::FALSE,
            logic_op: 0,
            attachment_count: blends.len() as u32,
            p_attachments: blends.as_ptr(),
            blend_constants: [0.0; 4],
        };
        let dynamic_states = [
            vk::DYNAMIC_STATE_VIEWPORT,
            vk::DYNAMIC_STATE_SCISSOR,
            vk::DYNAMIC_STATE_BLEND_CONSTANTS,
            vk::DYNAMIC_STATE_STENCIL_REFERENCE,
        ];
        let dynamic = vk::PipelineDynamicStateCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            dynamic_state_count: dynamic_states.len() as u32,
            p_dynamic_states: dynamic_states.as_ptr(),
        };
        let ds_format = desc.depth_stencil.map(|d| d.format);
        let vk_ds_format = ds_format
            .map(|f| device.adapter.vk_format(f))
            .unwrap_or(vk::FORMAT_UNDEFINED);
        let rendering = vk::PipelineRenderingCreateInfo {
            s_type: vk::STRUCTURE_TYPE_PIPELINE_RENDERING_CREATE_INFO,
            p_next: std::ptr::null(),
            view_mask: 0,
            color_attachment_count: vk_color_formats.len() as u32,
            p_color_attachment_formats: vk_color_formats.as_ptr(),
            depth_attachment_format: if ds_format.is_some_and(|f| f.has_depth()) {
                vk_ds_format
            } else {
                vk::FORMAT_UNDEFINED
            },
            stencil_attachment_format: if ds_format.is_some_and(|f| f.has_stencil()) {
                vk_ds_format
            } else {
                vk::FORMAT_UNDEFINED
            },
        };
        let info = vk::GraphicsPipelineCreateInfo {
            s_type: vk::STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
            p_next: &rendering as *const _ as *const c_void,
            flags: 0,
            stage_count: stages.len() as u32,
            p_stages: stages.as_ptr(),
            p_vertex_input_state: &vertex_input,
            p_input_assembly_state: &input_assembly,
            p_tessellation_state: std::ptr::null(),
            p_viewport_state: &viewport,
            p_rasterization_state: &rasterization,
            p_multisample_state: &multisample,
            p_depth_stencil_state: depth_stencil
                .as_ref()
                .map_or(std::ptr::null(), |d| d as *const _),
            p_color_blend_state: &color_blend,
            p_dynamic_state: &dynamic,
            layout: desc.layout.raw,
            render_pass: 0,
            subpass: 0,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_graphics_pipelines)(
                device.raw,
                0,
                1,
                &info,
                std::ptr::null(),
                &mut raw,
            )
        };
        check(r, "vkCreateGraphicsPipelines")?;
        Ok(RenderPipeline {
            device: device.clone(),
            raw,
        })
    }

    pub fn create_compute_pipeline(
        &self,
        desc: &hal::ComputePipelineDesc<'_>,
    ) -> Result<ComputePipeline, GpuError> {
        let device = &self.inner;
        let entry_name = CString::new(desc.stage.entry.name.as_str()).unwrap();
        let info = vk::ComputePipelineCreateInfo {
            s_type: vk::STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            stage: vk::PipelineShaderStageCreateInfo {
                s_type: vk::STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: std::ptr::null(),
                flags: 0,
                stage: vk::SHADER_STAGE_COMPUTE_BIT,
                module: desc.stage.module.raw,
                p_name: entry_name.as_ptr(),
                p_specialization_info: std::ptr::null(),
            },
            layout: desc.layout.raw,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut raw = 0;
        let r = unsafe {
            (device.fns.create_compute_pipelines)(
                device.raw,
                0,
                1,
                &info,
                std::ptr::null(),
                &mut raw,
            )
        };
        check(r, "vkCreateComputePipelines")?;
        Ok(ComputePipeline {
            device: device.clone(),
            raw,
        })
    }
}
