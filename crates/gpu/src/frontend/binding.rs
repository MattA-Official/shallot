//! Samplers, bind group layouts, bind groups and pipeline layouts, including the layouts
//! derived from shader reflection for `layout: "auto"`.
//!
//! Every texture reached through a bind group is used in the `GENERAL` layout, so a view's
//! descriptor stays valid however the texture is used elsewhere in the same pass (a mip chain
//! read and written in one pass, for example). Finer layouts can come with finer tracking.

use super::resource::{label, named, BufferShared, TextureViewShared};
use super::{Device, DeviceShared};
use crate::hal;
use crate::*;
use std::sync::Arc;

// ------------------------------------------------------------------------------------------
// Samplers

pub(crate) struct SamplerShared {
    /// `None` for an invalid sampler.
    pub raw: Option<hal::Sampler>,
    pub comparison: bool,
    /// Whether any filter is linear, which `non-filtering` sampler bindings forbid.
    pub filtering: bool,
    pub label: String,
}

impl SamplerShared {
    /// The backend sampler of a sampler that has passed validation.
    pub fn hal(&self) -> &hal::Sampler {
        self.raw.as_ref().expect("a validated sampler")
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
                    raw: None,
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
        match device.raw.create_sampler(desc) {
            Ok(raw) => Sampler {
                shared: Arc::new(SamplerShared {
                    raw: Some(raw),
                    comparison: desc.compare.is_some(),
                    filtering,
                    label: name,
                }),
            },
            Err(e) => {
                device.report(e);
                invalid(None)
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Bind group layouts

pub(crate) struct BindGroupLayoutShared {
    /// `None` for an invalid layout.
    pub raw: Option<hal::BindGroupLayout>,
    /// Sorted by binding number, which is also the order of dynamic offsets.
    pub entries: Vec<BindGroupLayoutEntry>,
    pub label: String,
    pub valid: bool,
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

        let raw = device
            .raw
            .create_bind_group_layout(&entries)
            .map_err(LayoutError::Device)?;
        Ok(Arc::new(BindGroupLayoutShared {
            raw: Some(raw),
            entries,
            label: name,
            valid: true,
        }))
    }

    /// The backend layout of a layout that has passed validation.
    pub fn hal(&self) -> &hal::BindGroupLayout {
        self.raw.as_ref().expect("a validated bind group layout")
    }

    pub(crate) fn invalid(name: String) -> Arc<BindGroupLayoutShared> {
        Arc::new(BindGroupLayoutShared {
            raw: None,
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
            BindGroupLayoutShared::invalid(String::new())
        }
        Err(LayoutError::Validation(_)) => unreachable!("an empty layout is valid"),
    }
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
                BindGroupLayoutShared::invalid(name)
            }
            Err(LayoutError::Device(e)) => {
                device.report(e);
                BindGroupLayoutShared::invalid(name)
            }
        };
        BindGroupLayout { shared }
    }
}

// ------------------------------------------------------------------------------------------
// Pipeline layouts

pub(crate) struct PipelineLayoutShared {
    /// `None` if the backend failed to create it.
    pub raw: Option<hal::PipelineLayout>,
    pub groups: Vec<Arc<BindGroupLayoutShared>>,
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
        let layouts: Vec<&hal::BindGroupLayout> = groups.iter().map(|g| g.hal()).collect();
        let raw = match device.raw.create_pipeline_layout(&layouts) {
            Ok(raw) => Some(raw),
            Err(e) => {
                device.report(e);
                None
            }
        };
        Ok(Arc::new(PipelineLayoutShared { raw, groups }))
    }

    /// The backend layout of a layout that was created.
    pub fn hal(&self) -> &hal::PipelineLayout {
        self.raw.as_ref().expect("a created pipeline layout")
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
                        raw: None,
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
                    Ok(BindGroupLayoutShared::invalid(String::new()))
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
    /// `None` for an invalid group.
    pub raw: Option<hal::BindGroup>,
    pub layout: Arc<BindGroupLayoutShared>,
    /// In layout-entry order.
    pub resources: Vec<Bound>,
    pub label: String,
    pub valid: bool,
}

impl BindGroupShared {
    /// The backend group of a group that has passed validation.
    pub fn hal(&self) -> &hal::BindGroup {
        self.raw.as_ref().expect("a validated bind group")
    }

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
                raw: None,
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
        let layout = &desc.layout.shared;
        let hal_resources: Vec<hal::Resource<'_>> = resources
            .iter()
            .map(|r| match r {
                Bound::Buffer {
                    buffer,
                    offset,
                    size,
                } => hal::Resource::Buffer {
                    buffer: buffer.hal(),
                    offset: *offset,
                    size: *size,
                },
                Bound::Sampler(sampler) => hal::Resource::Sampler(sampler.hal()),
                Bound::Texture(view) => hal::Resource::Texture(view.hal()),
            })
            .collect();
        let created = device
            .raw
            .create_bind_group(layout.hal(), &layout.entries, &hal_resources);
        drop(hal_resources);
        match created {
            Ok(raw) => BindGroup {
                shared: Arc::new(BindGroupShared {
                    raw: Some(raw),
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
                    if s.raw.is_none() {
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
