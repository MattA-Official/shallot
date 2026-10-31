//! The backend interface under the WebGPU front end.
//!
//! The front end (`crate::frontend`) implements WebGPU's validation, error scopes, mapping and
//! submission tracking once. A backend only creates objects from descriptors the front end has
//! already validated and translates recorded commands. Exactly one backend is compiled in: Metal
//! on macOS, Vulkan elsewhere.
//!
//! Every backend provides the same set of types under the same names, re-exported below. Each
//! object keeps what it was created from alive and releases its native object when dropped.

use crate::*;

mod vulkan;
pub(crate) use vulkan::Buffer;
pub(crate) use vulkan::{
    Adapter, BindGroup, BindGroupLayout, ComputePipeline, Device, Instance, PipelineLayout,
    RenderPipeline, Sampler, ShaderModule, Submission, Surface, Texture, TextureView,
};

/// A texture view's subresource range and interpretation, already validated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewDesc {
    pub format: TextureFormat,
    pub dimension: TextureViewDimension,
    pub aspect: TextureAspect,
    pub base_mip_level: u32,
    pub mip_level_count: u32,
    pub base_array_layer: u32,
    pub array_layer_count: u32,
}

/// One resource of a bind group, in layout-entry order.
pub(crate) enum Resource<'a> {
    Buffer {
        buffer: &'a Buffer,
        offset: u64,
        size: u64,
    },
    Sampler(&'a Sampler),
    Texture(&'a TextureView),
}

/// A shader stage of a pipeline: the backend's module and the reflected entry point.
#[derive(Clone, Copy)]
pub(crate) struct Stage<'a> {
    pub module: &'a ShaderModule,
    /// For backends that compile per pipeline (Metal); Vulkan compiles per module.
    #[allow(dead_code)]
    pub reflection: &'a shallot_wgsl::Module,
    pub entry: &'a shallot_wgsl::EntryPoint,
}

/// A validated render pipeline.
pub(crate) struct RenderPipelineDesc<'a> {
    pub layout: &'a PipelineLayout,
    /// The entries of each bind group layout, by group index, for backends that bind
    /// resources by slot (Metal).
    #[allow(dead_code)]
    pub groups: &'a [&'a [BindGroupLayoutEntry]],
    pub vertex: Stage<'a>,
    pub buffers: &'a [VertexBufferLayout<'a>],
    pub fragment: Option<Stage<'a>>,
    pub targets: &'a [Option<ColorTargetState>],
    pub primitive: PrimitiveState,
    pub depth_stencil: Option<DepthStencilState>,
    pub multisample: MultisampleState,
}

/// A validated compute pipeline.
pub(crate) struct ComputePipelineDesc<'a> {
    pub layout: &'a PipelineLayout,
    /// As for [`RenderPipelineDesc::groups`].
    #[allow(dead_code)]
    pub groups: &'a [&'a [BindGroupLayoutEntry]],
    pub stage: Stage<'a>,
}

/// One buffer-texture copy, already validated. The buffer side is described by its offset
/// and the bytes between rows and images.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TexelCopy {
    pub buffer_offset: u64,
    pub bytes_per_row: u32,
    pub rows_per_image: u32,
    pub mip_level: u32,
    pub origin: Origin3d,
    pub size: Extent3d,
    pub aspect: TextureAspect,
}

/// Where a submission's work stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Pending,
    Done,
    /// The device was lost; the work will never finish.
    Lost,
}

/// A presentable image acquired from a surface.
pub(crate) struct Acquired {
    pub texture: Texture,
    /// Which of the swapchain's images it is, for presenting.
    pub image: u32,
    pub suboptimal: bool,
    /// Changes whenever the swapchain is recreated; presenting an image from an older one does
    /// nothing.
    pub generation: u64,
}

/// Why acquiring a surface image failed.
pub(crate) enum AcquireError {
    Outdated,
    Timeout,
    Lost,
    Other(String),
}

/// The surface image a submission presents, when it presents one.
pub(crate) struct Present<'a> {
    pub texture: &'a std::sync::Arc<crate::frontend::TextureShared>,
    pub surface: &'a Surface,
    pub image: u32,
}
