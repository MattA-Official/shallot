//! Samplers, bind group layouts, bind groups and pipeline layouts, including the layouts
//! derived from shader reflection for `layout: "auto"`.
//!
//! Every texture reached through a bind group is used in the `GENERAL` layout, so a view's
//! descriptor stays valid however the texture is used elsewhere in the same pass (a mip chain
//! read and written in one pass, for example). Finer layouts can come with finer tracking.

use super::resource::{label, named, BufferShared, TextureViewShared};
use super::{check, conv, vk, Device, DeviceShared};
use crate::*;
use std::sync::Arc;

// ------------------------------------------------------------------------------------------
// Samplers

pub(crate) struct SamplerShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::Sampler,
    pub comparison: bool,
    /// Whether any filter is linear, which `non-filtering` sampler bindings forbid.
    pub filtering: bool,
    pub label: String,
}

unsafe impl Send for SamplerShared {}
unsafe impl Sync for SamplerShared {}

impl Drop for SamplerShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_sampler)(self.device.raw, self.raw, std::ptr::null())
            };
        }
    }
}

/// `GPUSampler`.
#[derive(Clone)]
pub struct Sampler {
    pub(crate) shared: Arc<SamplerShared>,
}

impl Device {
    /// `createSampler`.
    pub fn create_sampler(&self, desc: &SamplerDescriptor<'_>) -> Sampler {
        let device = &self.shared;
        let name = label(desc.label);
        let filtering = desc.mag_filter == FilterMode::Linear
            || desc.min_filter == FilterMode::Linear
            || desc.mipmap_filter == MipmapFilterMode::Linear;
        let invalid = |message: Option<String>| {
            if let Some(message) = message {
                device.validation(format!("{}: {message}", named("sampler", &name)));
            }
            Sampler {
                shared: Arc::new(SamplerShared {
                    device: device.clone(),
                    raw: 0,
                    comparison: desc.compare.is_some(),
                    filtering,
                    label: name.clone(),
                }),
            }
        };
        if desc.lod_min_clamp < 0.0 || desc.lod_max_clamp < desc.lod_min_clamp {
            return invalid(Some("lod clamps must satisfy 0 <= min <= max".into()));
        }
        if desc.max_anisotropy == 0 {
            return invalid(Some("maxAnisotropy must be at least 1".into()));
        }
        if desc.max_anisotropy > 1
            && (desc.mag_filter != FilterMode::Linear
                || desc.min_filter != FilterMode::Linear
                || desc.mipmap_filter != MipmapFilterMode::Linear)
        {
            return invalid(Some(
                "anisotropic filtering needs every filter to be linear".into(),
            ));
        }
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
        if let Err(e) = check(r, "vkCreateSampler") {
            device.report(e);
            return invalid(None);
        }
        Sampler {
            shared: Arc::new(SamplerShared {
                device: device.clone(),
                raw,
                comparison: desc.compare.is_some(),
                filtering,
                label: name,
            }),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Bind group layouts

pub(crate) struct BindGroupLayoutShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::DescriptorSetLayout,
    /// Sorted by binding number, which is also the order of dynamic offsets.
    pub entries: Vec<BindGroupLayoutEntry>,
    pub label: String,
    pub valid: bool,
}

unsafe impl Send for BindGroupLayoutShared {}
unsafe impl Sync for BindGroupLayoutShared {}

impl Drop for BindGroupLayoutShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_descriptor_set_layout)(
                    self.device.raw,
                    self.raw,
                    std::ptr::null(),
                )
            };
        }
    }
}

impl BindGroupLayoutShared {
    pub fn describe(&self) -> String {
        named("bind group layout", &self.label)
    }

    /// WebGPU's "group-equivalent": the same entries.
    pub fn equivalent(&self, other: &BindGroupLayoutShared) -> bool {
        self.entries == other.entries
    }

    fn create(
        device: &Arc<DeviceShared>,
        entries: &[BindGroupLayoutEntry],
        name: String,
    ) -> Result<Arc<BindGroupLayoutShared>, LayoutError> {
        let v = LayoutError::Validation;
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.binding);
        if entries.windows(2).any(|w| w[0].binding == w[1].binding) {
            return Err(v("binding numbers must be unique".into()));
        }
        let limits = &device.limits;
        let (mut dynamic_uniform, mut dynamic_storage) = (0, 0);
        for entry in &entries {
            if entry.visibility.0 & !0x7 != 0 || entry.binding >= limits.max_bindings_per_bind_group
            {
                return Err(v(format!(
                    "binding {} has an invalid number or visibility",
                    entry.binding
                )));
            }
            match entry.ty {
                BindingType::Buffer {
                    ty,
                    has_dynamic_offset,
                    ..
                } => {
                    if ty == BufferBindingType::Storage
                        && entry.visibility.contains(ShaderStages::VERTEX)
                    {
                        return Err(v(format!(
                            "binding {} is a writable storage buffer visible to the vertex stage",
                            entry.binding
                        )));
                    }
                    if has_dynamic_offset {
                        match ty {
                            BufferBindingType::Uniform => dynamic_uniform += 1,
                            _ => dynamic_storage += 1,
                        }
                    }
                }
                BindingType::Sampler(_) => {}
                BindingType::Texture {
                    sample_type,
                    view_dimension,
                    multisampled,
                } => {
                    if multisampled
                        && (view_dimension != TextureViewDimension::D2
                            || sample_type == TextureSampleType::Float)
                    {
                        return Err(v(format!(
                            "binding {}: multisampled textures are 2D and not filterable",
                            entry.binding
                        )));
                    }
                }
                BindingType::StorageTexture {
                    access,
                    format,
                    view_dimension,
                } => {
                    if matches!(
                        view_dimension,
                        TextureViewDimension::Cube | TextureViewDimension::CubeArray
                    ) {
                        return Err(v(format!(
                            "binding {}: storage textures cannot be cubes",
                            entry.binding
                        )));
                    }
                    if !format.supports_storage(access) {
                        return Err(v(format!(
                            "binding {}: {} is not a {access:?} storage format",
                            entry.binding,
                            format.name()
                        )));
                    }
                    if access != StorageTextureAccess::ReadOnly
                        && entry.visibility.contains(ShaderStages::VERTEX)
                    {
                        return Err(v(format!(
                            "binding {} is a writable storage texture visible to the vertex stage",
                            entry.binding
                        )));
                    }
                }
            }
        }
        if dynamic_uniform > limits.max_dynamic_uniform_buffers_per_pipeline_layout
            || dynamic_storage > limits.max_dynamic_storage_buffers_per_pipeline_layout
        {
            return Err(v("too many dynamic buffer bindings".into()));
        }

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
            )
            .map_err(LayoutError::Device)?;
        }
        Ok(Arc::new(BindGroupLayoutShared {
            device: device.clone(),
            raw,
            entries,
            label: name,
            valid: true,
        }))
    }

    pub(crate) fn invalid(device: &Arc<DeviceShared>, name: String) -> Arc<BindGroupLayoutShared> {
        Arc::new(BindGroupLayoutShared {
            device: device.clone(),
            raw: 0,
            entries: Vec::new(),
            label: name,
            valid: false,
        })
    }
}

/// The layout of a group no shader stage uses.
pub(crate) fn empty_layout(device: &Arc<DeviceShared>) -> Arc<BindGroupLayoutShared> {
    match BindGroupLayoutShared::create(device, &[], String::new()) {
        Ok(layout) => layout,
        Err(LayoutError::Device(e)) => {
            device.report(e);
            BindGroupLayoutShared::invalid(device, String::new())
        }
        Err(LayoutError::Validation(_)) => unreachable!("an empty layout is valid"),
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

pub(crate) fn stage_flags(stages: ShaderStages) -> vk::Flags {
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

enum LayoutError {
    Validation(String),
    Device(GpuError),
}

/// `GPUBindGroupLayout`.
#[derive(Clone)]
pub struct BindGroupLayout {
    pub(crate) shared: Arc<BindGroupLayoutShared>,
}

impl BindGroupLayout {
    /// The entries, sorted by binding number.
    pub fn entries(&self) -> &[BindGroupLayoutEntry] {
        &self.shared.entries
    }
}

impl Device {
    /// `createBindGroupLayout`.
    pub fn create_bind_group_layout(
        &self,
        desc: &BindGroupLayoutDescriptor<'_>,
    ) -> BindGroupLayout {
        let device = &self.shared;
        let name = label(desc.label);
        let shared = match BindGroupLayoutShared::create(device, desc.entries, name.clone()) {
            Ok(shared) => shared,
            Err(LayoutError::Validation(message)) => {
                device.validation(format!("{}: {message}", named("bind group layout", &name)));
                BindGroupLayoutShared::invalid(device, name)
            }
            Err(LayoutError::Device(e)) => {
                device.report(e);
                BindGroupLayoutShared::invalid(device, name)
            }
        };
        BindGroupLayout { shared }
    }
}

// ------------------------------------------------------------------------------------------
// Pipeline layouts

pub(crate) struct PipelineLayoutShared {
    pub device: Arc<DeviceShared>,
    pub raw: vk::PipelineLayout,
    pub groups: Vec<Arc<BindGroupLayoutShared>>,
}

unsafe impl Send for PipelineLayoutShared {}
unsafe impl Sync for PipelineLayoutShared {}

impl Drop for PipelineLayoutShared {
    fn drop(&mut self) {
        if self.raw != 0 {
            unsafe {
                (self.device.fns.destroy_pipeline_layout)(
                    self.device.raw,
                    self.raw,
                    std::ptr::null(),
                )
            };
        }
    }
}

impl PipelineLayoutShared {
    pub(crate) fn create(
        device: &Arc<DeviceShared>,
        groups: Vec<Arc<BindGroupLayoutShared>>,
    ) -> Result<Arc<PipelineLayoutShared>, String> {
        if groups.len() as u32 > device.limits.max_bind_groups {
            return Err(format!(
                "{} bind group layouts exceed the limit of {}",
                groups.len(),
                device.limits.max_bind_groups
            ));
        }
        if let Some(g) = groups.iter().find(|g| !g.valid) {
            return Err(format!("{} is invalid", g.describe()));
        }
        let dynamic = |kind: BufferBindingType| {
            groups
                .iter()
                .flat_map(|g| &g.entries)
                .filter(|e| match e.ty {
                    BindingType::Buffer {
                        ty,
                        has_dynamic_offset: true,
                        ..
                    } => (ty == BufferBindingType::Uniform) == (kind == BufferBindingType::Uniform),
                    _ => false,
                })
                .count() as u32
        };
        if dynamic(BufferBindingType::Uniform)
            > device
                .limits
                .max_dynamic_uniform_buffers_per_pipeline_layout
            || dynamic(BufferBindingType::Storage)
                > device
                    .limits
                    .max_dynamic_storage_buffers_per_pipeline_layout
        {
            return Err("too many dynamic buffer bindings across the layout".into());
        }
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
        if let Err(e) = check(r, "vkCreatePipelineLayout") {
            device.report(e);
        }
        Ok(Arc::new(PipelineLayoutShared {
            device: device.clone(),
            raw,
            groups,
        }))
    }

    /// Checks that the shader stages' bindings fit this layout, as WebGPU requires of an
    /// explicit layout.
    pub(crate) fn check_stages(&self, stages: &[StageReflection<'_>]) -> Result<(), String> {
        for stage in stages {
            for binding in stage.module.bindings_for(stage.entry) {
                let needed = binding_type(&binding.kind)?;
                let entry = self
                    .groups
                    .get(binding.group as usize)
                    .and_then(|g| g.entries.iter().find(|e| e.binding == binding.binding))
                    .ok_or_else(|| {
                        format!(
                            "@group({}) @binding({}) of `{}` is not in the pipeline layout",
                            binding.group, binding.binding, binding.name
                        )
                    })?;
                if !entry.visibility.contains(stage.stage) {
                    return Err(format!(
                        "@group({}) @binding({}) is not visible to the {:?} stage",
                        binding.group, binding.binding, stage.stage
                    ));
                }
                if !layout_satisfies(&entry.ty, &needed) {
                    return Err(format!(
                        "@group({}) @binding({}) of `{}` does not match the layout entry",
                        binding.group, binding.binding, binding.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One shader stage of a pipeline, with its reflection.
pub(crate) struct StageReflection<'m> {
    pub module: &'m shallot_wgsl::Module,
    pub entry: &'m shallot_wgsl::EntryPoint,
    pub stage: ShaderStages,
}

/// Whether a layout entry can serve a binding whose shader-derived type is `needed`.
fn layout_satisfies(entry: &BindingType, needed: &BindingType) -> bool {
    match (entry, needed) {
        (
            BindingType::Buffer {
                ty: a,
                min_binding_size,
                ..
            },
            BindingType::Buffer {
                ty: b,
                min_binding_size: shader_size,
                ..
            },
        ) => {
            // A read-only shader binding is satisfied by a writable layout entry too.
            let ty_ok = a == b
                || (*a == BufferBindingType::Storage && *b == BufferBindingType::ReadOnlyStorage);
            ty_ok && (*min_binding_size == 0 || min_binding_size >= shader_size)
        }
        (BindingType::Sampler(a), BindingType::Sampler(b)) => {
            (*a == SamplerBindingType::Comparison) == (*b == SamplerBindingType::Comparison)
        }
        (
            BindingType::Texture {
                sample_type: a,
                view_dimension: da,
                multisampled: ma,
            },
            BindingType::Texture {
                sample_type: b,
                view_dimension: db,
                multisampled: mb,
            },
        ) => {
            let sample_ok = a == b
                || (*b == TextureSampleType::Float && *a == TextureSampleType::UnfilterableFloat)
                || (*b == TextureSampleType::Depth && *a == TextureSampleType::Depth);
            sample_ok && da == db && ma == mb
        }
        (
            BindingType::StorageTexture {
                access: a,
                format: fa,
                view_dimension: da,
            },
            BindingType::StorageTexture {
                access: b,
                format: fb,
                view_dimension: db,
            },
        ) => a == b && fa == fb && da == db,
        _ => false,
    }
}

/// `GPUPipelineLayout`.
#[derive(Clone)]
pub struct PipelineLayout {
    pub(crate) shared: Arc<PipelineLayoutShared>,
}

impl Device {
    /// `createPipelineLayout`.
    pub fn create_pipeline_layout(&self, desc: &PipelineLayoutDescriptor<'_>) -> PipelineLayout {
        let device = &self.shared;
        let groups = desc
            .bind_group_layouts
            .iter()
            .map(|g| g.shared.clone())
            .collect();
        match PipelineLayoutShared::create(device, groups) {
            Ok(shared) => PipelineLayout { shared },
            Err(message) => {
                device.validation(format!(
                    "{}: {message}",
                    named("pipeline layout", &label(desc.label))
                ));
                PipelineLayout {
                    shared: Arc::new(PipelineLayoutShared {
                        device: device.clone(),
                        raw: 0,
                        groups: Vec::new(),
                    }),
                }
            }
        }
    }
}

/// The layout-entry type a WGSL resource declaration needs.
fn binding_type(kind: &shallot_wgsl::ResourceKind) -> Result<BindingType, String> {
    use shallot_wgsl::ir::TextureSample;
    use shallot_wgsl::ResourceKind as R;
    Ok(match kind {
        R::UniformBuffer { min_size } => BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: *min_size,
        },
        R::StorageBuffer {
            read_only,
            min_size,
        } => BindingType::Buffer {
            ty: if *read_only {
                BufferBindingType::ReadOnlyStorage
            } else {
                BufferBindingType::Storage
            },
            has_dynamic_offset: false,
            min_binding_size: *min_size,
        },
        R::Sampler { comparison } => BindingType::Sampler(if *comparison {
            SamplerBindingType::Comparison
        } else {
            SamplerBindingType::Filtering
        }),
        R::Texture {
            dim,
            arrayed,
            sample,
            multisampled,
        } => BindingType::Texture {
            sample_type: match sample {
                TextureSample::Float if *multisampled => TextureSampleType::UnfilterableFloat,
                TextureSample::Float => TextureSampleType::Float,
                TextureSample::Depth => TextureSampleType::Depth,
                TextureSample::Sint => TextureSampleType::Sint,
                TextureSample::Uint => TextureSampleType::Uint,
            },
            view_dimension: view_dimension(*dim, *arrayed),
            multisampled: *multisampled,
        },
        R::StorageTexture {
            dim,
            arrayed,
            format,
            access,
        } => BindingType::StorageTexture {
            access: match access {
                shallot_wgsl::types::Access::Read => StorageTextureAccess::ReadOnly,
                shallot_wgsl::types::Access::Write => StorageTextureAccess::WriteOnly,
                shallot_wgsl::types::Access::ReadWrite => StorageTextureAccess::ReadWrite,
            },
            format: TextureFormat::from_name(format)
                .ok_or_else(|| format!("unsupported storage texture format `{format}`"))?,
            view_dimension: view_dimension(*dim, *arrayed),
        },
        R::ExternalTexture => return Err("external textures are not supported".into()),
    })
}

fn view_dimension(dim: shallot_wgsl::types::TextureDim, arrayed: bool) -> TextureViewDimension {
    use shallot_wgsl::types::TextureDim as D;
    match (dim, arrayed) {
        (D::D1, _) => TextureViewDimension::D1,
        (D::D2, false) => TextureViewDimension::D2,
        (D::D2, true) => TextureViewDimension::D2Array,
        (D::D3, _) => TextureViewDimension::D3,
        (D::Cube, false) => TextureViewDimension::Cube,
        (D::Cube, true) => TextureViewDimension::CubeArray,
    }
}

/// The layout WebGPU's `layout: "auto"` derives: one bind group layout per group the stages
/// use, holding the union of their bindings with the stages that see each.
pub(crate) fn auto_layout(
    device: &Arc<DeviceShared>,
    stages: &[StageReflection<'_>],
) -> Result<Arc<PipelineLayoutShared>, String> {
    let mut groups: Vec<Vec<BindGroupLayoutEntry>> = Vec::new();
    for stage in stages {
        for binding in stage.module.bindings_for(stage.entry) {
            let ty = binding_type(&binding.kind)?;
            let group = binding.group as usize;
            if group >= device.limits.max_bind_groups as usize {
                return Err(format!(
                    "@group({group}) exceeds the limit of {} bind groups",
                    device.limits.max_bind_groups
                ));
            }
            if groups.len() <= group {
                groups.resize(group + 1, Vec::new());
            }
            match groups[group]
                .iter_mut()
                .find(|e| e.binding == binding.binding)
            {
                Some(entry) => {
                    entry.visibility |= stage.stage;
                    // Stages may declare a buffer with different sizes; keep the largest.
                    match (&mut entry.ty, ty) {
                        (
                            BindingType::Buffer {
                                ty: a,
                                min_binding_size,
                                ..
                            },
                            BindingType::Buffer {
                                ty: b,
                                min_binding_size: size,
                                ..
                            },
                        ) if *a == b => *min_binding_size = (*min_binding_size).max(size),
                        (existing, ty) if *existing == ty => {}
                        _ => {
                            return Err(format!(
                            "@group({group}) @binding({}) is declared differently by two stages",
                            binding.binding
                        ))
                        }
                    }
                }
                None => groups[group].push(BindGroupLayoutEntry {
                    binding: binding.binding,
                    visibility: stage.stage,
                    ty,
                }),
            }
        }
    }
    let layouts = groups
        .iter()
        .map(
            |entries| match BindGroupLayoutShared::create(device, entries, String::new()) {
                Ok(layout) => Ok(layout),
                Err(LayoutError::Validation(message)) => Err(message),
                Err(LayoutError::Device(e)) => {
                    device.report(e);
                    Ok(BindGroupLayoutShared::invalid(device, String::new()))
                }
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    PipelineLayoutShared::create(device, layouts)
}

// ------------------------------------------------------------------------------------------
// Bind groups

/// What a bind group holds at one binding.
pub(crate) enum Bound {
    Buffer {
        buffer: Arc<BufferShared>,
        offset: u64,
        size: u64,
    },
    Sampler(Arc<SamplerShared>),
    Texture(Arc<TextureViewShared>),
}

pub(crate) struct BindGroupShared {
    pub device: Arc<DeviceShared>,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    pub layout: Arc<BindGroupLayoutShared>,
    /// In layout-entry order.
    pub resources: Vec<Bound>,
    pub label: String,
    pub valid: bool,
}

unsafe impl Send for BindGroupShared {}
unsafe impl Sync for BindGroupShared {}

impl Drop for BindGroupShared {
    fn drop(&mut self) {
        if self.pool != 0 {
            unsafe {
                (self.device.fns.destroy_descriptor_pool)(
                    self.device.raw,
                    self.pool,
                    std::ptr::null(),
                )
            };
        }
    }
}

impl BindGroupShared {
    pub fn describe(&self) -> String {
        named("bind group", &self.label)
    }

    /// Checks at submit time that nothing the group binds has been destroyed.
    pub fn usable(&self) -> Result<(), String> {
        for resource in &self.resources {
            match resource {
                Bound::Buffer { buffer, .. } => buffer.usable()?,
                Bound::Texture(view) => view.texture.usable()?,
                Bound::Sampler(_) => {}
            }
        }
        Ok(())
    }

    /// The bound buffers whose bindings use dynamic offsets, in offset order, with the
    /// binding's offset and size.
    pub fn dynamic_buffers(
        &self,
    ) -> impl Iterator<Item = (&Arc<BufferShared>, u64, u64, BufferBindingType)> {
        self.layout
            .entries
            .iter()
            .zip(&self.resources)
            .filter_map(|(entry, resource)| match (entry.ty, resource) {
                (
                    BindingType::Buffer {
                        ty,
                        has_dynamic_offset: true,
                        ..
                    },
                    Bound::Buffer {
                        buffer,
                        offset,
                        size,
                    },
                ) => Some((buffer, *offset, *size, ty)),
                _ => None,
            })
    }
}

/// `GPUBindGroup`.
#[derive(Clone)]
pub struct BindGroup {
    pub(crate) shared: Arc<BindGroupShared>,
}

impl Device {
    /// `createBindGroup`.
    pub fn create_bind_group(&self, desc: &BindGroupDescriptor<'_>) -> BindGroup {
        let device = &self.shared;
        let name = label(desc.label);
        let invalid = || BindGroup {
            shared: Arc::new(BindGroupShared {
                device: device.clone(),
                pool: 0,
                set: 0,
                layout: desc.layout.shared.clone(),
                resources: Vec::new(),
                label: name.clone(),
                valid: false,
            }),
        };
        let resources = match self.bind_group_resources(desc) {
            Ok(resources) => resources,
            Err(message) => {
                device.validation(format!("{}: {message}", named("bind group", &name)));
                return invalid();
            }
        };
        match unsafe { write_descriptor_set(device, &desc.layout.shared, &resources) } {
            Ok((pool, set)) => BindGroup {
                shared: Arc::new(BindGroupShared {
                    device: device.clone(),
                    pool,
                    set,
                    layout: desc.layout.shared.clone(),
                    resources,
                    label: name,
                    valid: true,
                }),
            },
            Err(e) => {
                device.report(e);
                invalid()
            }
        }
    }

    /// Validates the entries against the layout and returns them in layout order.
    fn bind_group_resources(&self, desc: &BindGroupDescriptor<'_>) -> Result<Vec<Bound>, String> {
        let layout = &desc.layout.shared;
        let limits = &self.shared.limits;
        if !layout.valid {
            return Err(format!("{} is invalid", layout.describe()));
        }
        if desc.entries.len() != layout.entries.len() {
            return Err(format!(
                "has {} entries but its layout has {}",
                desc.entries.len(),
                layout.entries.len()
            ));
        }
        let mut resources = Vec::with_capacity(layout.entries.len());
        for entry in &layout.entries {
            let b = entry.binding;
            let mut matching = desc.entries.iter().filter(|e| e.binding == b);
            let (Some(given), None) = (matching.next(), matching.next()) else {
                return Err(format!("needs exactly one entry for binding {b}"));
            };
            let resource = match (&entry.ty, &given.resource) {
                (
                    BindingType::Buffer {
                        ty,
                        min_binding_size,
                        ..
                    },
                    BindingResource::Buffer(binding),
                ) => {
                    let buffer = &binding.buffer.shared;
                    buffer.usable()?;
                    let size = binding
                        .size
                        .unwrap_or(buffer.size.saturating_sub(binding.offset));
                    let (usage, alignment, max) = match ty {
                        BufferBindingType::Uniform => (
                            BufferUsage::UNIFORM,
                            limits.min_uniform_buffer_offset_alignment,
                            limits.max_uniform_buffer_binding_size,
                        ),
                        _ => (
                            BufferUsage::STORAGE,
                            limits.min_storage_buffer_offset_alignment,
                            limits.max_storage_buffer_binding_size,
                        ),
                    };
                    if !buffer.usage.contains(usage) {
                        return Err(format!(
                            "binding {b}: {} lacks {usage:?} usage",
                            buffer.describe()
                        ));
                    }
                    if !binding.offset.is_multiple_of(alignment as u64) {
                        return Err(format!(
                            "binding {b}: offset must be a multiple of {alignment}"
                        ));
                    }
                    if size == 0 || binding.offset + size > buffer.size {
                        return Err(format!("binding {b}: the range is empty or out of bounds"));
                    }
                    if size > max {
                        return Err(format!(
                            "binding {b}: {size} bytes exceed the binding size limit"
                        ));
                    }
                    if usage == BufferUsage::STORAGE && !size.is_multiple_of(4) {
                        return Err(format!(
                            "binding {b}: storage bindings are a multiple of 4 bytes"
                        ));
                    }
                    if size < *min_binding_size {
                        return Err(format!(
                            "binding {b}: {size} bytes are less than minBindingSize {min_binding_size}"
                        ));
                    }
                    Bound::Buffer {
                        buffer: buffer.clone(),
                        offset: binding.offset,
                        size,
                    }
                }
                (BindingType::Sampler(kind), BindingResource::Sampler(sampler)) => {
                    let s = &sampler.shared;
                    if s.raw == 0 {
                        return Err(format!(
                            "binding {b}: {} is invalid",
                            named("sampler", &s.label)
                        ));
                    }
                    let ok = match kind {
                        SamplerBindingType::Comparison => s.comparison,
                        SamplerBindingType::Filtering => !s.comparison,
                        SamplerBindingType::NonFiltering => !s.comparison && !s.filtering,
                    };
                    if !ok {
                        return Err(format!(
                            "binding {b}: the sampler does not match a {kind:?} binding"
                        ));
                    }
                    Bound::Sampler(s.clone())
                }
                (
                    BindingType::Texture {
                        sample_type,
                        view_dimension,
                        multisampled,
                    },
                    BindingResource::TextureView(view),
                ) => {
                    let v = &view.shared;
                    check_view(v, b, TextureUsage::TEXTURE_BINDING, *view_dimension)?;
                    if (v.texture.sample_count > 1) != *multisampled {
                        return Err(format!(
                            "binding {b}: the view's sample count does not match"
                        ));
                    }
                    let format = match v.aspect {
                        TextureAspect::StencilOnly => TextureFormat::Stencil8,
                        _ => v.format,
                    };
                    if v.format.has_depth()
                        && v.format.has_stencil()
                        && v.aspect == TextureAspect::All
                    {
                        return Err(format!(
                            "binding {b}: a depth-stencil view must select one aspect"
                        ));
                    }
                    if !format.supports_sample_type(*sample_type) {
                        return Err(format!(
                            "binding {b}: {} cannot be bound as {sample_type:?}",
                            format.name()
                        ));
                    }
                    Bound::Texture(v.clone())
                }
                (
                    BindingType::StorageTexture {
                        format,
                        view_dimension,
                        ..
                    },
                    BindingResource::TextureView(view),
                ) => {
                    let v = &view.shared;
                    check_view(v, b, TextureUsage::STORAGE_BINDING, *view_dimension)?;
                    if v.format != *format {
                        return Err(format!(
                            "binding {b}: the view's format is not {}",
                            format.name()
                        ));
                    }
                    if v.mip_level_count != 1 {
                        return Err(format!(
                            "binding {b}: storage texture views have one mip level"
                        ));
                    }
                    Bound::Texture(v.clone())
                }
                _ => {
                    return Err(format!(
                        "binding {b}: the resource does not match the layout"
                    ))
                }
            };
            resources.push(resource);
        }
        Ok(resources)
    }
}

fn check_view(
    view: &TextureViewShared,
    binding: u32,
    usage: TextureUsage,
    dimension: TextureViewDimension,
) -> Result<(), String> {
    if !view.valid {
        return Err(format!("binding {binding}: {} is invalid", view.describe()));
    }
    view.texture.usable()?;
    if !view.texture.usage.contains(usage) {
        return Err(format!(
            "binding {binding}: {} lacks {usage:?} usage",
            view.texture.describe()
        ));
    }
    if view.dimension != dimension {
        return Err(format!(
            "binding {binding}: the view is {:?}, the layout wants {dimension:?}",
            view.dimension
        ));
    }
    Ok(())
}

/// Allocates a descriptor set from a pool of its own and writes the resources into it.
unsafe fn write_descriptor_set(
    device: &DeviceShared,
    layout: &BindGroupLayoutShared,
    resources: &[Bound],
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), GpuError> {
    let fns = &device.fns;
    let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
    for entry in &layout.entries {
        let ty = descriptor_type(&entry.ty);
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
    for (entry, resource) in layout.entries.iter().zip(resources) {
        let (p_image_info, p_buffer_info) = match resource {
            Bound::Buffer {
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
            Bound::Sampler(sampler) => {
                images.push(vk::DescriptorImageInfo {
                    sampler: sampler.raw,
                    image_view: 0,
                    image_layout: vk::IMAGE_LAYOUT_UNDEFINED,
                });
                (images.last().unwrap() as *const _, std::ptr::null())
            }
            Bound::Texture(view) => {
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
            descriptor_type: descriptor_type(&entry.ty),
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
    Ok((pool, set))
}
