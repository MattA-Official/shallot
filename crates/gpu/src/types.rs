//! Enums, flags and descriptors, mirroring the WebGPU IDL with Rust naming.

use crate::{Buffer, PipelineLayout, ShaderModule, TextureView};
use std::ops::{BitAnd, BitOr, BitOrAssign};

macro_rules! flags {
    ($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* $flag:ident = $value:expr;)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name(pub u32);

        impl $name {
            pub const NONE: $name = $name(0);
            $($(#[$fmeta])* pub const $flag: $name = $name($value);)*

            pub fn contains(self, other: $name) -> bool {
                self.0 & other.0 == other.0
            }

            pub fn intersects(self, other: $name) -> bool {
                self.0 & other.0 != 0
            }

            pub fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl BitOr for $name {
            type Output = $name;
            fn bitor(self, rhs: $name) -> $name {
                $name(self.0 | rhs.0)
            }
        }

        impl BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: $name) {
                self.0 |= rhs.0;
            }
        }

        impl BitAnd for $name {
            type Output = $name;
            fn bitand(self, rhs: $name) -> $name {
                $name(self.0 & rhs.0)
            }
        }
    };
}

flags! {
    /// `GPUBufferUsage`.
    BufferUsage {
        MAP_READ = 0x0001;
        MAP_WRITE = 0x0002;
        COPY_SRC = 0x0004;
        COPY_DST = 0x0008;
        INDEX = 0x0010;
        VERTEX = 0x0020;
        UNIFORM = 0x0040;
        STORAGE = 0x0080;
        INDIRECT = 0x0100;
        QUERY_RESOLVE = 0x0200;
    }
}

flags! {
    /// `GPUTextureUsage`.
    TextureUsage {
        COPY_SRC = 0x01;
        COPY_DST = 0x02;
        TEXTURE_BINDING = 0x04;
        STORAGE_BINDING = 0x08;
        RENDER_ATTACHMENT = 0x10;
    }
}

flags! {
    /// `GPUShaderStage`.
    ShaderStages {
        VERTEX = 0x1;
        FRAGMENT = 0x2;
        COMPUTE = 0x4;
    }
}

flags! {
    /// `GPUColorWrite`.
    ColorWrites {
        RED = 0x1;
        GREEN = 0x2;
        BLUE = 0x4;
        ALPHA = 0x8;
        ALL = 0xf;
    }
}

flags! {
    /// `GPUMapMode`.
    MapMode {
        READ = 0x1;
        WRITE = 0x2;
    }
}

/// `GPUTextureFormat`, for the formats the engine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    R8Unorm,
    R8Snorm,
    R8Uint,
    R8Sint,
    R16Uint,
    R16Sint,
    R16Float,
    Rg8Unorm,
    Rg8Snorm,
    Rg8Uint,
    Rg8Sint,
    R32Uint,
    R32Sint,
    R32Float,
    Rg16Uint,
    Rg16Sint,
    Rg16Float,
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Rgba8Snorm,
    Rgba8Uint,
    Rgba8Sint,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    Rgb10a2Uint,
    Rgb10a2Unorm,
    Rg11b10Ufloat,
    Rgb9e5Ufloat,
    Rg32Uint,
    Rg32Sint,
    Rg32Float,
    Rgba16Uint,
    Rgba16Sint,
    Rgba16Float,
    Rgba32Uint,
    Rgba32Sint,
    Rgba32Float,
    Stencil8,
    Depth16Unorm,
    Depth24Plus,
    Depth24PlusStencil8,
    Depth32Float,
    Depth32FloatStencil8,
}

impl TextureFormat {
    pub fn has_depth(self) -> bool {
        use TextureFormat::*;
        matches!(
            self,
            Depth16Unorm | Depth24Plus | Depth24PlusStencil8 | Depth32Float | Depth32FloatStencil8
        )
    }

    pub fn has_stencil(self) -> bool {
        use TextureFormat::*;
        matches!(self, Stencil8 | Depth24PlusStencil8 | Depth32FloatStencil8)
    }

    pub fn is_depth_stencil(self) -> bool {
        self.has_depth() || self.has_stencil()
    }

    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            TextureFormat::Rgba8UnormSrgb | TextureFormat::Bgra8UnormSrgb
        )
    }

    /// Bytes per texel for copyable colour formats; `None` for depth and stencil formats
    /// whose copy size depends on the aspect.
    pub fn block_size(self) -> Option<u32> {
        use TextureFormat::*;
        Some(match self {
            R8Unorm | R8Snorm | R8Uint | R8Sint | Stencil8 => 1,
            R16Uint | R16Sint | R16Float | Rg8Unorm | Rg8Snorm | Rg8Uint | Rg8Sint
            | Depth16Unorm => 2,
            R32Uint | R32Sint | R32Float | Rg16Uint | Rg16Sint | Rg16Float | Rgba8Unorm
            | Rgba8UnormSrgb | Rgba8Snorm | Rgba8Uint | Rgba8Sint | Bgra8Unorm | Bgra8UnormSrgb
            | Rgb10a2Uint | Rgb10a2Unorm | Rg11b10Ufloat | Rgb9e5Ufloat | Depth32Float => 4,
            Rg32Uint | Rg32Sint | Rg32Float | Rgba16Uint | Rgba16Sint | Rgba16Float => 8,
            Rgba32Uint | Rgba32Sint | Rgba32Float => 16,
            Depth24Plus | Depth24PlusStencil8 | Depth32FloatStencil8 => return None,
        })
    }

    /// The WebGPU spelling, such as `"rgba8unorm-srgb"`.
    pub fn name(self) -> &'static str {
        use TextureFormat::*;
        match self {
            R8Unorm => "r8unorm",
            R8Snorm => "r8snorm",
            R8Uint => "r8uint",
            R8Sint => "r8sint",
            R16Uint => "r16uint",
            R16Sint => "r16sint",
            R16Float => "r16float",
            Rg8Unorm => "rg8unorm",
            Rg8Snorm => "rg8snorm",
            Rg8Uint => "rg8uint",
            Rg8Sint => "rg8sint",
            R32Uint => "r32uint",
            R32Sint => "r32sint",
            R32Float => "r32float",
            Rg16Uint => "rg16uint",
            Rg16Sint => "rg16sint",
            Rg16Float => "rg16float",
            Rgba8Unorm => "rgba8unorm",
            Rgba8UnormSrgb => "rgba8unorm-srgb",
            Rgba8Snorm => "rgba8snorm",
            Rgba8Uint => "rgba8uint",
            Rgba8Sint => "rgba8sint",
            Bgra8Unorm => "bgra8unorm",
            Bgra8UnormSrgb => "bgra8unorm-srgb",
            Rgb10a2Uint => "rgb10a2uint",
            Rgb10a2Unorm => "rgb10a2unorm",
            Rg11b10Ufloat => "rg11b10ufloat",
            Rgb9e5Ufloat => "rgb9e5ufloat",
            Rg32Uint => "rg32uint",
            Rg32Sint => "rg32sint",
            Rg32Float => "rg32float",
            Rgba16Uint => "rgba16uint",
            Rgba16Sint => "rgba16sint",
            Rgba16Float => "rgba16float",
            Rgba32Uint => "rgba32uint",
            Rgba32Sint => "rgba32sint",
            Rgba32Float => "rgba32float",
            Stencil8 => "stencil8",
            Depth16Unorm => "depth16unorm",
            Depth24Plus => "depth24plus",
            Depth24PlusStencil8 => "depth24plus-stencil8",
            Depth32Float => "depth32float",
            Depth32FloatStencil8 => "depth32float-stencil8",
        }
    }
}

/// `GPUVertexFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VertexFormat {
    Uint8x2,
    Uint8x4,
    Sint8x2,
    Sint8x4,
    Unorm8x2,
    Unorm8x4,
    Snorm8x2,
    Snorm8x4,
    Uint16x2,
    Uint16x4,
    Sint16x2,
    Sint16x4,
    Unorm16x2,
    Unorm16x4,
    Snorm16x2,
    Snorm16x4,
    Float16x2,
    Float16x4,
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Uint32,
    Uint32x2,
    Uint32x3,
    Uint32x4,
    Sint32,
    Sint32x2,
    Sint32x3,
    Sint32x4,
    Unorm10_10_10_2,
}

impl VertexFormat {
    pub fn size(self) -> u64 {
        use VertexFormat::*;
        match self {
            Uint8x2 | Sint8x2 | Unorm8x2 | Snorm8x2 => 2,
            Uint8x4 | Sint8x4 | Unorm8x4 | Snorm8x4 | Uint16x2 | Sint16x2 | Unorm16x2
            | Snorm16x2 | Float16x2 | Float32 | Uint32 | Sint32 | Unorm10_10_10_2 => 4,
            Uint16x4 | Sint16x4 | Unorm16x4 | Snorm16x4 | Float16x4 | Float32x2 | Uint32x2
            | Sint32x2 => 8,
            Float32x3 | Uint32x3 | Sint32x3 => 12,
            Float32x4 | Uint32x4 | Sint32x4 => 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextureDimension {
    D1,
    #[default]
    D2,
    D3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureViewDimension {
    D1,
    D2,
    D2Array,
    Cube,
    CubeArray,
    D3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextureAspect {
    #[default]
    All,
    StencilOnly,
    DepthOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    #[default]
    TriangleList,
    TriangleStrip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexFormat {
    Uint16,
    Uint32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrontFace {
    #[default]
    Ccw,
    Cw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CullMode {
    #[default]
    None,
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareFunction {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StencilOperation {
    #[default]
    Keep,
    Zero,
    Replace,
    Invert,
    IncrementClamp,
    DecrementClamp,
    IncrementWrap,
    DecrementWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendFactor {
    Zero,
    One,
    Src,
    OneMinusSrc,
    SrcAlpha,
    OneMinusSrcAlpha,
    Dst,
    OneMinusDst,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturated,
    Constant,
    OneMinusConstant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendOperation {
    #[default]
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VertexStepMode {
    #[default]
    Vertex,
    Instance,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
}

/// `GPULoadOp`, with the clear value carried by `Clear` as in `GPURenderPassColorAttachment`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp<V> {
    Clear(V),
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreOp {
    #[default]
    Store,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerPreference {
    #[default]
    None,
    LowPower,
    HighPerformance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentMode {
    /// Waits for vertical blank, like the browser's `requestAnimationFrame` pacing.
    #[default]
    Fifo,
    Mailbox,
    Immediate,
}

/// `GPUCanvasAlphaMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompositeAlphaMode {
    #[default]
    Opaque,
    Premultiplied,
}

/// `GPUFeatureName`, for the features the engine requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    IndirectFirstInstance,
    TimestampQuery,
    Depth32FloatStencil8,
    Bgra8UnormStorage,
}

/// `GPUSupportedLimits`, the subset the engine reads. Defaults are WebGPU's default limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_texture_dimension_1d: u32,
    pub max_texture_dimension_2d: u32,
    pub max_texture_dimension_3d: u32,
    pub max_texture_array_layers: u32,
    pub max_bind_groups: u32,
    pub max_bindings_per_bind_group: u32,
    pub max_sampled_textures_per_shader_stage: u32,
    pub max_samplers_per_shader_stage: u32,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub max_uniform_buffers_per_shader_stage: u32,
    pub max_uniform_buffer_binding_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub min_uniform_buffer_offset_alignment: u32,
    pub min_storage_buffer_offset_alignment: u32,
    pub max_vertex_buffers: u32,
    pub max_buffer_size: u64,
    pub max_vertex_attributes: u32,
    pub max_vertex_buffer_array_stride: u32,
    pub max_inter_stage_shader_variables: u32,
    pub max_color_attachments: u32,
    pub max_color_attachment_bytes_per_sample: u32,
    pub max_compute_workgroup_storage_size: u32,
    pub max_compute_invocations_per_workgroup: u32,
    pub max_compute_workgroup_size_x: u32,
    pub max_compute_workgroup_size_y: u32,
    pub max_compute_workgroup_size_z: u32,
    pub max_compute_workgroups_per_dimension: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_texture_dimension_1d: 8192,
            max_texture_dimension_2d: 8192,
            max_texture_dimension_3d: 2048,
            max_texture_array_layers: 256,
            max_bind_groups: 4,
            max_bindings_per_bind_group: 1000,
            max_sampled_textures_per_shader_stage: 16,
            max_samplers_per_shader_stage: 16,
            max_storage_buffers_per_shader_stage: 8,
            max_storage_textures_per_shader_stage: 4,
            max_uniform_buffers_per_shader_stage: 12,
            max_uniform_buffer_binding_size: 65536,
            max_storage_buffer_binding_size: 134_217_728,
            min_uniform_buffer_offset_alignment: 256,
            min_storage_buffer_offset_alignment: 256,
            max_vertex_buffers: 8,
            max_buffer_size: 268_435_456,
            max_vertex_attributes: 16,
            max_vertex_buffer_array_stride: 2048,
            max_inter_stage_shader_variables: 16,
            max_color_attachments: 8,
            max_color_attachment_bytes_per_sample: 32,
            max_compute_workgroup_storage_size: 16384,
            max_compute_invocations_per_workgroup: 256,
            max_compute_workgroup_size_x: 256,
            max_compute_workgroup_size_y: 256,
            max_compute_workgroup_size_z: 64,
            max_compute_workgroups_per_dimension: 65535,
        }
    }
}

/// `GPUAdapterInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterInfo {
    pub vendor: String,
    pub architecture: String,
    pub device: String,
    pub description: String,
    /// Whether this is a CPU implementation such as lavapipe or SwiftShader.
    pub is_fallback_adapter: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Extent3d {
    pub width: u32,
    pub height: u32,
    pub depth_or_array_layers: u32,
}

impl Extent3d {
    pub const fn new(width: u32, height: u32, depth_or_array_layers: u32) -> Self {
        Self {
            width,
            height,
            depth_or_array_layers,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Origin3d {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

// ------------------------------------------------------------------------------------------
// Descriptors

#[derive(Debug, Clone, Default)]
pub struct InstanceDescriptor {
    /// Enable `VK_LAYER_KHRONOS_validation` when it is installed. Defaults to on in debug
    /// builds when the `SHALLOT_GPU_VALIDATION` environment variable is set.
    pub validation: bool,
}

#[derive(Clone, Copy, Default)]
pub struct RequestAdapterOptions<'a> {
    pub power_preference: PowerPreference,
    pub force_fallback_adapter: bool,
    pub compatible_surface: Option<&'a crate::Surface>,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceDescriptor<'a> {
    pub label: Option<&'a str>,
    pub required_features: &'a [Feature],
    pub required_limits: Option<Limits>,
}

#[derive(Debug, Clone, Default)]
pub struct BufferDescriptor<'a> {
    pub label: Option<&'a str>,
    pub size: u64,
    pub usage: BufferUsage,
    pub mapped_at_creation: bool,
}

#[derive(Debug, Clone)]
pub struct TextureDescriptor<'a> {
    pub label: Option<&'a str>,
    pub size: Extent3d,
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub dimension: TextureDimension,
    pub format: TextureFormat,
    pub usage: TextureUsage,
    pub view_formats: &'a [TextureFormat],
}

#[derive(Debug, Clone, Default)]
pub struct TextureViewDescriptor<'a> {
    pub label: Option<&'a str>,
    pub format: Option<TextureFormat>,
    pub dimension: Option<TextureViewDimension>,
    pub aspect: TextureAspect,
    pub base_mip_level: u32,
    pub mip_level_count: Option<u32>,
    pub base_array_layer: u32,
    pub array_layer_count: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ShaderModuleDescriptor<'a> {
    pub label: Option<&'a str>,
    pub code: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilationMessageType {
    Error,
    Warning,
    Info,
}

/// One entry of `GPUCompilationInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilationMessage {
    pub message: String,
    pub kind: CompilationMessageType,
    /// One-based, as in WebGPU.
    pub line_num: u32,
    pub line_pos: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexAttribute {
    pub format: VertexFormat,
    pub offset: u64,
    pub shader_location: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexBufferLayout<'a> {
    pub array_stride: u64,
    pub step_mode: VertexStepMode,
    pub attributes: &'a [VertexAttribute],
}

#[derive(Clone)]
pub struct VertexState<'a> {
    pub module: &'a ShaderModule,
    /// `None` picks the module's only vertex entry point.
    pub entry_point: Option<&'a str>,
    pub buffers: &'a [VertexBufferLayout<'a>],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendComponent {
    pub operation: BlendOperation,
    pub src_factor: BlendFactor,
    pub dst_factor: BlendFactor,
}

impl Default for BlendComponent {
    fn default() -> Self {
        Self {
            operation: BlendOperation::Add,
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::Zero,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BlendState {
    pub color: BlendComponent,
    pub alpha: BlendComponent,
}

impl BlendState {
    pub const ALPHA_BLENDING: BlendState = BlendState {
        color: BlendComponent {
            operation: BlendOperation::Add,
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
        },
        alpha: BlendComponent {
            operation: BlendOperation::Add,
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
        },
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTargetState {
    pub format: TextureFormat,
    pub blend: Option<BlendState>,
    pub write_mask: ColorWrites,
}

impl From<TextureFormat> for ColorTargetState {
    fn from(format: TextureFormat) -> Self {
        Self {
            format,
            blend: None,
            write_mask: ColorWrites::ALL,
        }
    }
}

#[derive(Clone)]
pub struct FragmentState<'a> {
    pub module: &'a ShaderModule,
    pub entry_point: Option<&'a str>,
    pub targets: &'a [Option<ColorTargetState>],
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PrimitiveState {
    pub topology: PrimitiveTopology,
    pub strip_index_format: Option<IndexFormat>,
    pub front_face: FrontFace,
    pub cull_mode: CullMode,
    pub unclipped_depth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StencilFaceState {
    pub compare: CompareFunction,
    pub fail_op: StencilOperation,
    pub depth_fail_op: StencilOperation,
    pub pass_op: StencilOperation,
}

impl Default for StencilFaceState {
    fn default() -> Self {
        Self {
            compare: CompareFunction::Always,
            fail_op: StencilOperation::Keep,
            depth_fail_op: StencilOperation::Keep,
            pass_op: StencilOperation::Keep,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthStencilState {
    pub format: TextureFormat,
    pub depth_write_enabled: bool,
    pub depth_compare: CompareFunction,
    pub stencil_front: StencilFaceState,
    pub stencil_back: StencilFaceState,
    pub stencil_read_mask: u32,
    pub stencil_write_mask: u32,
    pub depth_bias: i32,
    pub depth_bias_slope_scale: f32,
    pub depth_bias_clamp: f32,
}

impl DepthStencilState {
    pub fn depth(
        format: TextureFormat,
        depth_write_enabled: bool,
        depth_compare: CompareFunction,
    ) -> Self {
        Self {
            format,
            depth_write_enabled,
            depth_compare,
            stencil_front: StencilFaceState::default(),
            stencil_back: StencilFaceState::default(),
            stencil_read_mask: !0,
            stencil_write_mask: !0,
            depth_bias: 0,
            depth_bias_slope_scale: 0.0,
            depth_bias_clamp: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultisampleState {
    pub count: u32,
    pub mask: u32,
    pub alpha_to_coverage_enabled: bool,
}

impl Default for MultisampleState {
    fn default() -> Self {
        Self {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        }
    }
}

/// `layout: "auto"` or an explicit pipeline layout.
#[derive(Clone, Default)]
pub enum PipelineLayoutChoice<'a> {
    #[default]
    Auto,
    Layout(&'a PipelineLayout),
}

#[derive(Clone)]
pub struct RenderPipelineDescriptor<'a> {
    pub label: Option<&'a str>,
    pub layout: PipelineLayoutChoice<'a>,
    pub vertex: VertexState<'a>,
    pub primitive: PrimitiveState,
    pub depth_stencil: Option<DepthStencilState>,
    pub multisample: MultisampleState,
    pub fragment: Option<FragmentState<'a>>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandEncoderDescriptor<'a> {
    pub label: Option<&'a str>,
}

#[derive(Clone)]
pub struct RenderPassColorAttachment<'a> {
    pub view: &'a TextureView,
    pub resolve_target: Option<&'a TextureView>,
    pub load_op: LoadOp<Color>,
    pub store_op: StoreOp,
}

#[derive(Clone)]
pub struct RenderPassDepthStencilAttachment<'a> {
    pub view: &'a TextureView,
    /// `None` leaves the aspect untouched (WebGPU's absent `depthLoadOp`).
    pub depth_ops: Option<(LoadOp<f32>, StoreOp)>,
    pub depth_read_only: bool,
    pub stencil_ops: Option<(LoadOp<u32>, StoreOp)>,
    pub stencil_read_only: bool,
}

#[derive(Clone, Default)]
pub struct RenderPassDescriptor<'a> {
    pub label: Option<&'a str>,
    pub color_attachments: &'a [Option<RenderPassColorAttachment<'a>>],
    pub depth_stencil_attachment: Option<RenderPassDepthStencilAttachment<'a>>,
}

/// `GPUTexelCopyBufferInfo`.
#[derive(Clone)]
pub struct TexelCopyBufferInfo<'a> {
    pub buffer: &'a Buffer,
    pub layout: TexelCopyBufferLayout,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TexelCopyBufferLayout {
    pub offset: u64,
    pub bytes_per_row: Option<u32>,
    pub rows_per_image: Option<u32>,
}

/// `GPUTexelCopyTextureInfo`.
#[derive(Clone)]
pub struct TexelCopyTextureInfo<'a> {
    pub texture: &'a crate::Texture,
    pub mip_level: u32,
    pub origin: Origin3d,
    pub aspect: TextureAspect,
}

/// `GPUCanvasConfiguration`, sized explicitly since there is no canvas element.
#[derive(Debug, Clone)]
pub struct SurfaceConfiguration {
    pub format: TextureFormat,
    pub usage: TextureUsage,
    pub width: u32,
    pub height: u32,
    pub present_mode: PresentMode,
    pub alpha_mode: CompositeAlphaMode,
}
