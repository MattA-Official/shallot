//! Hand-written Vulkan declarations for the subset `shallot-gpu` calls.
//!
//! Transcribed from `vulkan_core.h` 1.3.275, `vulkan_xlib.h`, `vulkan_win32.h` and the Vulkan specification.
//! Struct sizes are asserted in the tests against the sizes the C headers give on 64-bit
//! targets. Grows with use.

#![allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::upper_case_acronyms
)]

use std::ffi::{c_char, c_void, CStr};

pub type Bool32 = u32;
pub type DeviceSize = u64;
pub type Flags = u32;
pub type Flags64 = u64;
pub type Result = i32;

pub type Instance = *mut c_void;
pub type PhysicalDevice = *mut c_void;
pub type Device = *mut c_void;
pub type Queue = *mut c_void;
pub type CommandBuffer = *mut c_void;

pub type Buffer = u64;
pub type Image = u64;
pub type ImageView = u64;
pub type DeviceMemory = u64;
pub type ShaderModule = u64;
pub type PipelineLayout = u64;
pub type Pipeline = u64;
pub type PipelineCache = u64;
pub type DescriptorSetLayout = u64;
pub type DescriptorPool = u64;
pub type DescriptorSet = u64;
pub type Sampler = u64;
pub type RenderPass = u64;
pub type CommandPool = u64;
pub type Fence = u64;
pub type Semaphore = u64;
pub type SurfaceKHR = u64;
pub type SwapchainKHR = u64;

pub const NULL_HANDLE: u64 = 0;
pub const TRUE: Bool32 = 1;
pub const FALSE: Bool32 = 0;
pub const WHOLE_SIZE: u64 = !0;
pub const QUEUE_FAMILY_IGNORED: u32 = !0;

pub const fn make_api_version(major: u32, minor: u32, patch: u32) -> u32 {
    (major << 22) | (minor << 12) | patch
}
pub const API_VERSION_1_1: u32 = make_api_version(1, 1, 0);
pub const API_VERSION_1_3: u32 = make_api_version(1, 3, 0);

pub fn version_major(v: u32) -> u32 {
    (v >> 22) & 0x7f
}
pub fn version_minor(v: u32) -> u32 {
    (v >> 12) & 0x3ff
}

// VkResult
pub const SUCCESS: Result = 0;
pub const NOT_READY: Result = 1;
pub const TIMEOUT: Result = 2;
pub const INCOMPLETE: Result = 5;
pub const SUBOPTIMAL_KHR: Result = 1000001003;
pub const ERROR_OUT_OF_HOST_MEMORY: Result = -1;
pub const ERROR_OUT_OF_DEVICE_MEMORY: Result = -2;
pub const ERROR_DEVICE_LOST: Result = -4;
pub const ERROR_SURFACE_LOST_KHR: Result = -1000000000;
pub const ERROR_OUT_OF_DATE_KHR: Result = -1000001004;

// VkStructureType
pub const STRUCTURE_TYPE_APPLICATION_INFO: i32 = 0;
pub const STRUCTURE_TYPE_INSTANCE_CREATE_INFO: i32 = 1;
pub const STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO: i32 = 2;
pub const STRUCTURE_TYPE_DEVICE_CREATE_INFO: i32 = 3;
pub const STRUCTURE_TYPE_SUBMIT_INFO: i32 = 4;
pub const STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO: i32 = 5;
pub const STRUCTURE_TYPE_FENCE_CREATE_INFO: i32 = 8;
pub const STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO: i32 = 9;
pub const STRUCTURE_TYPE_BUFFER_CREATE_INFO: i32 = 12;
pub const STRUCTURE_TYPE_IMAGE_CREATE_INFO: i32 = 14;
pub const STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO: i32 = 15;
pub const STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO: i32 = 16;
pub const STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO: i32 = 18;
pub const STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: i32 = 19;
pub const STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: i32 = 20;
pub const STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO: i32 = 22;
pub const STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: i32 = 23;
pub const STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: i32 = 24;
pub const STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO: i32 = 25;
pub const STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: i32 = 26;
pub const STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO: i32 = 27;
pub const STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO: i32 = 28;
pub const STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO: i32 = 29;
pub const STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO: i32 = 30;
pub const STRUCTURE_TYPE_SAMPLER_CREATE_INFO: i32 = 31;
pub const STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO: i32 = 32;
pub const STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO: i32 = 33;
pub const STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO: i32 = 34;
pub const STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET: i32 = 35;
pub const STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: i32 = 39;
pub const STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: i32 = 40;
pub const STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: i32 = 42;
pub const STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2: i32 = 1000059000;
pub const STRUCTURE_TYPE_PHYSICAL_DEVICE_DYNAMIC_RENDERING_FEATURES: i32 = 1000044003;
pub const STRUCTURE_TYPE_PHYSICAL_DEVICE_SYNCHRONIZATION_2_FEATURES: i32 = 1000314007;
pub const STRUCTURE_TYPE_RENDERING_INFO: i32 = 1000044000;
pub const STRUCTURE_TYPE_RENDERING_ATTACHMENT_INFO: i32 = 1000044001;
pub const STRUCTURE_TYPE_PIPELINE_RENDERING_CREATE_INFO: i32 = 1000044002;
pub const STRUCTURE_TYPE_MEMORY_BARRIER_2: i32 = 1000314000;
pub const STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2: i32 = 1000314002;
pub const STRUCTURE_TYPE_DEPENDENCY_INFO: i32 = 1000314003;
pub const STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR: i32 = 1000001000;
pub const STRUCTURE_TYPE_PRESENT_INFO_KHR: i32 = 1000001001;
pub const STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR: i32 = 1000004000;
pub const STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR: i32 = 1000009000;

// Queue, memory and usage flags.
pub const QUEUE_GRAPHICS_BIT: Flags = 0x1;
pub const QUEUE_COMPUTE_BIT: Flags = 0x2;
pub const MEMORY_PROPERTY_DEVICE_LOCAL_BIT: Flags = 0x1;
pub const MEMORY_PROPERTY_HOST_VISIBLE_BIT: Flags = 0x2;
pub const MEMORY_PROPERTY_HOST_COHERENT_BIT: Flags = 0x4;
pub const MEMORY_PROPERTY_HOST_CACHED_BIT: Flags = 0x8;

pub const BUFFER_USAGE_TRANSFER_SRC_BIT: Flags = 0x1;
pub const BUFFER_USAGE_TRANSFER_DST_BIT: Flags = 0x2;
pub const BUFFER_USAGE_UNIFORM_BUFFER_BIT: Flags = 0x10;
pub const BUFFER_USAGE_STORAGE_BUFFER_BIT: Flags = 0x20;
pub const BUFFER_USAGE_INDEX_BUFFER_BIT: Flags = 0x40;
pub const BUFFER_USAGE_VERTEX_BUFFER_BIT: Flags = 0x80;
pub const BUFFER_USAGE_INDIRECT_BUFFER_BIT: Flags = 0x100;

pub const IMAGE_USAGE_TRANSFER_SRC_BIT: Flags = 0x1;
pub const IMAGE_USAGE_TRANSFER_DST_BIT: Flags = 0x2;
pub const IMAGE_USAGE_SAMPLED_BIT: Flags = 0x4;
pub const IMAGE_USAGE_STORAGE_BIT: Flags = 0x8;
pub const IMAGE_USAGE_COLOR_ATTACHMENT_BIT: Flags = 0x10;
pub const IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT: Flags = 0x20;

pub const FORMAT_FEATURE_COLOR_ATTACHMENT_BIT: Flags = 0x80;
pub const FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT: Flags = 0x200;

pub const IMAGE_ASPECT_COLOR_BIT: Flags = 0x1;
pub const IMAGE_ASPECT_DEPTH_BIT: Flags = 0x2;
pub const IMAGE_ASPECT_STENCIL_BIT: Flags = 0x4;

pub const SHADER_STAGE_VERTEX_BIT: Flags = 0x1;
pub const SHADER_STAGE_FRAGMENT_BIT: Flags = 0x10;
pub const SHADER_STAGE_COMPUTE_BIT: Flags = 0x20;

pub const COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT: Flags = 0x2;
pub const COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: Flags = 0x1;
pub const FENCE_CREATE_SIGNALED_BIT: Flags = 0x1;

pub const COLOR_COMPONENT_ALL: Flags = 0xf;
pub const CULL_MODE_NONE: Flags = 0;
pub const CULL_MODE_FRONT_BIT: Flags = 0x1;
pub const CULL_MODE_BACK_BIT: Flags = 0x2;

pub const SURFACE_TRANSFORM_IDENTITY_BIT_KHR: Flags = 0x1;
pub const COMPOSITE_ALPHA_OPAQUE_BIT_KHR: Flags = 0x1;
pub const COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR: Flags = 0x2;
pub const COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR: Flags = 0x4;
pub const COMPOSITE_ALPHA_INHERIT_BIT_KHR: Flags = 0x8;

// Synchronization2 pipeline stages and access flags.
pub const PIPELINE_STAGE_2_ALL_COMMANDS_BIT: Flags64 = 0x10000;
pub const ACCESS_2_MEMORY_READ_BIT: Flags64 = 0x8000;
pub const ACCESS_2_MEMORY_WRITE_BIT: Flags64 = 0x10000;
pub const PIPELINE_STAGE_ALL_COMMANDS_BIT: Flags = 0x10000;

// Enums.
pub const IMAGE_LAYOUT_UNDEFINED: i32 = 0;
pub const IMAGE_LAYOUT_GENERAL: i32 = 1;
pub const IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: i32 = 2;
pub const IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL: i32 = 3;
pub const IMAGE_LAYOUT_DEPTH_STENCIL_READ_ONLY_OPTIMAL: i32 = 4;
pub const IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL: i32 = 5;
pub const IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: i32 = 6;
pub const IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL: i32 = 7;
pub const IMAGE_LAYOUT_PRESENT_SRC_KHR: i32 = 1000001002;

pub const IMAGE_TYPE_1D: i32 = 0;
pub const IMAGE_TYPE_2D: i32 = 1;
pub const IMAGE_TYPE_3D: i32 = 2;
pub const IMAGE_VIEW_TYPE_1D: i32 = 0;
pub const IMAGE_VIEW_TYPE_2D: i32 = 1;
pub const IMAGE_VIEW_TYPE_3D: i32 = 2;
pub const IMAGE_VIEW_TYPE_CUBE: i32 = 3;
pub const IMAGE_VIEW_TYPE_1D_ARRAY: i32 = 4;
pub const IMAGE_VIEW_TYPE_2D_ARRAY: i32 = 5;
pub const IMAGE_VIEW_TYPE_CUBE_ARRAY: i32 = 6;
pub const IMAGE_TILING_OPTIMAL: i32 = 0;
pub const IMAGE_CREATE_CUBE_COMPATIBLE_BIT: Flags = 0x10;
pub const SHARING_MODE_EXCLUSIVE: i32 = 0;
pub const COMPONENT_SWIZZLE_IDENTITY: i32 = 0;

pub const PIPELINE_BIND_POINT_GRAPHICS: i32 = 0;
pub const PIPELINE_BIND_POINT_COMPUTE: i32 = 1;

pub const FILTER_NEAREST: i32 = 0;
pub const FILTER_LINEAR: i32 = 1;
pub const SAMPLER_MIPMAP_MODE_NEAREST: i32 = 0;
pub const SAMPLER_MIPMAP_MODE_LINEAR: i32 = 1;
pub const SAMPLER_ADDRESS_MODE_REPEAT: i32 = 0;
pub const SAMPLER_ADDRESS_MODE_MIRRORED_REPEAT: i32 = 1;
pub const SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE: i32 = 2;
pub const BORDER_COLOR_FLOAT_TRANSPARENT_BLACK: i32 = 0;

pub const DESCRIPTOR_TYPE_SAMPLER: i32 = 0;
pub const DESCRIPTOR_TYPE_SAMPLED_IMAGE: i32 = 2;
pub const DESCRIPTOR_TYPE_STORAGE_IMAGE: i32 = 3;
pub const DESCRIPTOR_TYPE_UNIFORM_BUFFER: i32 = 6;
pub const DESCRIPTOR_TYPE_STORAGE_BUFFER: i32 = 7;
pub const DESCRIPTOR_TYPE_UNIFORM_BUFFER_DYNAMIC: i32 = 8;
pub const DESCRIPTOR_TYPE_STORAGE_BUFFER_DYNAMIC: i32 = 9;
pub const COMMAND_BUFFER_LEVEL_PRIMARY: i32 = 0;
pub const VERTEX_INPUT_RATE_VERTEX: i32 = 0;
pub const VERTEX_INPUT_RATE_INSTANCE: i32 = 1;
pub const INDEX_TYPE_UINT16: i32 = 0;
pub const INDEX_TYPE_UINT32: i32 = 1;

pub const PRIMITIVE_TOPOLOGY_POINT_LIST: i32 = 0;
pub const PRIMITIVE_TOPOLOGY_LINE_LIST: i32 = 1;
pub const PRIMITIVE_TOPOLOGY_LINE_STRIP: i32 = 2;
pub const PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: i32 = 3;
pub const PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP: i32 = 4;

pub const POLYGON_MODE_FILL: i32 = 0;
pub const FRONT_FACE_COUNTER_CLOCKWISE: i32 = 0;
pub const FRONT_FACE_CLOCKWISE: i32 = 1;

pub const COMPARE_OP_NEVER: i32 = 0;
pub const COMPARE_OP_LESS: i32 = 1;
pub const COMPARE_OP_EQUAL: i32 = 2;
pub const COMPARE_OP_LESS_OR_EQUAL: i32 = 3;
pub const COMPARE_OP_GREATER: i32 = 4;
pub const COMPARE_OP_NOT_EQUAL: i32 = 5;
pub const COMPARE_OP_GREATER_OR_EQUAL: i32 = 6;
pub const COMPARE_OP_ALWAYS: i32 = 7;

pub const STENCIL_OP_KEEP: i32 = 0;
pub const STENCIL_OP_ZERO: i32 = 1;
pub const STENCIL_OP_REPLACE: i32 = 2;
pub const STENCIL_OP_INCREMENT_AND_CLAMP: i32 = 3;
pub const STENCIL_OP_DECREMENT_AND_CLAMP: i32 = 4;
pub const STENCIL_OP_INVERT: i32 = 5;
pub const STENCIL_OP_INCREMENT_AND_WRAP: i32 = 6;
pub const STENCIL_OP_DECREMENT_AND_WRAP: i32 = 7;

pub const BLEND_FACTOR_ZERO: i32 = 0;
pub const BLEND_FACTOR_ONE: i32 = 1;
pub const BLEND_FACTOR_SRC_COLOR: i32 = 2;
pub const BLEND_FACTOR_ONE_MINUS_SRC_COLOR: i32 = 3;
pub const BLEND_FACTOR_DST_COLOR: i32 = 4;
pub const BLEND_FACTOR_ONE_MINUS_DST_COLOR: i32 = 5;
pub const BLEND_FACTOR_SRC_ALPHA: i32 = 6;
pub const BLEND_FACTOR_ONE_MINUS_SRC_ALPHA: i32 = 7;
pub const BLEND_FACTOR_DST_ALPHA: i32 = 8;
pub const BLEND_FACTOR_ONE_MINUS_DST_ALPHA: i32 = 9;
pub const BLEND_FACTOR_CONSTANT_COLOR: i32 = 10;
pub const BLEND_FACTOR_ONE_MINUS_CONSTANT_COLOR: i32 = 11;
pub const BLEND_FACTOR_SRC_ALPHA_SATURATE: i32 = 14;

pub const BLEND_OP_ADD: i32 = 0;
pub const BLEND_OP_SUBTRACT: i32 = 1;
pub const BLEND_OP_REVERSE_SUBTRACT: i32 = 2;
pub const BLEND_OP_MIN: i32 = 3;
pub const BLEND_OP_MAX: i32 = 4;

pub const DYNAMIC_STATE_VIEWPORT: i32 = 0;
pub const DYNAMIC_STATE_SCISSOR: i32 = 1;
pub const DYNAMIC_STATE_BLEND_CONSTANTS: i32 = 4;
pub const DYNAMIC_STATE_STENCIL_REFERENCE: i32 = 8;

pub const ATTACHMENT_LOAD_OP_LOAD: i32 = 0;
pub const ATTACHMENT_LOAD_OP_CLEAR: i32 = 1;
pub const ATTACHMENT_LOAD_OP_DONT_CARE: i32 = 2;
pub const ATTACHMENT_STORE_OP_STORE: i32 = 0;
pub const ATTACHMENT_STORE_OP_DONT_CARE: i32 = 1;
pub const RESOLVE_MODE_NONE: Flags = 0;
pub const RESOLVE_MODE_AVERAGE_BIT: Flags = 0x2;

pub const PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU: i32 = 1;
pub const PHYSICAL_DEVICE_TYPE_DISCRETE_GPU: i32 = 2;
pub const PHYSICAL_DEVICE_TYPE_VIRTUAL_GPU: i32 = 3;
pub const PHYSICAL_DEVICE_TYPE_CPU: i32 = 4;

pub const PRESENT_MODE_IMMEDIATE_KHR: i32 = 0;
pub const PRESENT_MODE_MAILBOX_KHR: i32 = 1;
pub const PRESENT_MODE_FIFO_KHR: i32 = 2;
pub const PRESENT_MODE_FIFO_RELAXED_KHR: i32 = 3;
pub const COLOR_SPACE_SRGB_NONLINEAR_KHR: i32 = 0;

// VkFormat
pub const FORMAT_UNDEFINED: i32 = 0;
pub const FORMAT_R8_UNORM: i32 = 9;
pub const FORMAT_R8_SNORM: i32 = 10;
pub const FORMAT_R8_UINT: i32 = 13;
pub const FORMAT_R8_SINT: i32 = 14;
pub const FORMAT_R8G8_UNORM: i32 = 16;
pub const FORMAT_R8G8_SNORM: i32 = 17;
pub const FORMAT_R8G8_UINT: i32 = 20;
pub const FORMAT_R8G8_SINT: i32 = 21;
pub const FORMAT_R8G8B8A8_UNORM: i32 = 37;
pub const FORMAT_R8G8B8A8_SNORM: i32 = 38;
pub const FORMAT_R8G8B8A8_UINT: i32 = 41;
pub const FORMAT_R8G8B8A8_SINT: i32 = 42;
pub const FORMAT_R8G8B8A8_SRGB: i32 = 43;
pub const FORMAT_B8G8R8A8_UNORM: i32 = 44;
pub const FORMAT_B8G8R8A8_SRGB: i32 = 50;
pub const FORMAT_A2B10G10R10_UNORM_PACK32: i32 = 64;
pub const FORMAT_A2B10G10R10_UINT_PACK32: i32 = 68;
pub const FORMAT_R16_UNORM: i32 = 70;
pub const FORMAT_R16_UINT: i32 = 74;
pub const FORMAT_R16_SINT: i32 = 75;
pub const FORMAT_R16_SFLOAT: i32 = 76;
pub const FORMAT_R16G16_UINT: i32 = 81;
pub const FORMAT_R16G16_SINT: i32 = 82;
pub const FORMAT_R16G16_SFLOAT: i32 = 83;
pub const FORMAT_R16G16B16A16_UINT: i32 = 95;
pub const FORMAT_R16G16B16A16_SINT: i32 = 96;
pub const FORMAT_R16G16B16A16_SFLOAT: i32 = 97;
pub const FORMAT_R32_UINT: i32 = 98;
pub const FORMAT_R32_SINT: i32 = 99;
pub const FORMAT_R32_SFLOAT: i32 = 100;
pub const FORMAT_R32G32_UINT: i32 = 101;
pub const FORMAT_R32G32_SINT: i32 = 102;
pub const FORMAT_R32G32_SFLOAT: i32 = 103;
pub const FORMAT_R32G32B32_UINT: i32 = 104;
pub const FORMAT_R32G32B32_SINT: i32 = 105;
pub const FORMAT_R32G32B32_SFLOAT: i32 = 106;
pub const FORMAT_R32G32B32A32_UINT: i32 = 107;
pub const FORMAT_R32G32B32A32_SINT: i32 = 108;
pub const FORMAT_R32G32B32A32_SFLOAT: i32 = 109;
pub const FORMAT_B10G11R11_UFLOAT_PACK32: i32 = 122;
pub const FORMAT_E5B9G9R9_UFLOAT_PACK32: i32 = 123;
pub const FORMAT_D16_UNORM: i32 = 124;
pub const FORMAT_X8_D24_UNORM_PACK32: i32 = 125;
pub const FORMAT_D32_SFLOAT: i32 = 126;
pub const FORMAT_S8_UINT: i32 = 127;
pub const FORMAT_D24_UNORM_S8_UINT: i32 = 129;
pub const FORMAT_D32_SFLOAT_S8_UINT: i32 = 130;
pub const FORMAT_R16G16_UNORM: i32 = 77;
pub const FORMAT_R16G16B16A16_UNORM: i32 = 91;
pub const FORMAT_R8G8_SRGB: i32 = 22;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Extent2D {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Extent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Offset2D {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Offset3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Rect2D {
    pub offset: Offset2D,
    pub extent: Extent2D,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[repr(C)]
pub struct ApplicationInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub p_application_name: *const c_char,
    pub application_version: u32,
    pub p_engine_name: *const c_char,
    pub engine_version: u32,
    pub api_version: u32,
}

#[repr(C)]
pub struct InstanceCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub p_application_info: *const ApplicationInfo,
    pub enabled_layer_count: u32,
    pub pp_enabled_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub pp_enabled_extension_names: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExtensionProperties {
    pub extension_name: [c_char; 256],
    pub spec_version: u32,
}

impl ExtensionProperties {
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.extension_name.as_ptr()) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LayerProperties {
    pub layer_name: [c_char; 256],
    pub spec_version: u32,
    pub implementation_version: u32,
    pub description: [c_char; 256],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PhysicalDeviceLimits {
    pub max_image_dimension_1d: u32,
    pub max_image_dimension_2d: u32,
    pub max_image_dimension_3d: u32,
    pub max_image_dimension_cube: u32,
    pub max_image_array_layers: u32,
    pub max_texel_buffer_elements: u32,
    pub max_uniform_buffer_range: u32,
    pub max_storage_buffer_range: u32,
    pub max_push_constants_size: u32,
    pub max_memory_allocation_count: u32,
    pub max_sampler_allocation_count: u32,
    pub buffer_image_granularity: DeviceSize,
    pub sparse_address_space_size: DeviceSize,
    pub max_bound_descriptor_sets: u32,
    pub max_per_stage_descriptor_samplers: u32,
    pub max_per_stage_descriptor_uniform_buffers: u32,
    pub max_per_stage_descriptor_storage_buffers: u32,
    pub max_per_stage_descriptor_sampled_images: u32,
    pub max_per_stage_descriptor_storage_images: u32,
    pub max_per_stage_descriptor_input_attachments: u32,
    pub max_per_stage_resources: u32,
    pub max_descriptor_set_samplers: u32,
    pub max_descriptor_set_uniform_buffers: u32,
    pub max_descriptor_set_uniform_buffers_dynamic: u32,
    pub max_descriptor_set_storage_buffers: u32,
    pub max_descriptor_set_storage_buffers_dynamic: u32,
    pub max_descriptor_set_sampled_images: u32,
    pub max_descriptor_set_storage_images: u32,
    pub max_descriptor_set_input_attachments: u32,
    pub max_vertex_input_attributes: u32,
    pub max_vertex_input_bindings: u32,
    pub max_vertex_input_attribute_offset: u32,
    pub max_vertex_input_binding_stride: u32,
    pub max_vertex_output_components: u32,
    pub max_tessellation_generation_level: u32,
    pub max_tessellation_patch_size: u32,
    pub max_tessellation_control_per_vertex_input_components: u32,
    pub max_tessellation_control_per_vertex_output_components: u32,
    pub max_tessellation_control_per_patch_output_components: u32,
    pub max_tessellation_control_total_output_components: u32,
    pub max_tessellation_evaluation_input_components: u32,
    pub max_tessellation_evaluation_output_components: u32,
    pub max_geometry_shader_invocations: u32,
    pub max_geometry_input_components: u32,
    pub max_geometry_output_components: u32,
    pub max_geometry_output_vertices: u32,
    pub max_geometry_total_output_components: u32,
    pub max_fragment_input_components: u32,
    pub max_fragment_output_attachments: u32,
    pub max_fragment_dual_src_attachments: u32,
    pub max_fragment_combined_output_resources: u32,
    pub max_compute_shared_memory_size: u32,
    pub max_compute_work_group_count: [u32; 3],
    pub max_compute_work_group_invocations: u32,
    pub max_compute_work_group_size: [u32; 3],
    pub sub_pixel_precision_bits: u32,
    pub sub_texel_precision_bits: u32,
    pub mipmap_precision_bits: u32,
    pub max_draw_indexed_index_value: u32,
    pub max_draw_indirect_count: u32,
    pub max_sampler_lod_bias: f32,
    pub max_sampler_anisotropy: f32,
    pub max_viewports: u32,
    pub max_viewport_dimensions: [u32; 2],
    pub viewport_bounds_range: [f32; 2],
    pub viewport_sub_pixel_bits: u32,
    pub min_memory_map_alignment: usize,
    pub min_texel_buffer_offset_alignment: DeviceSize,
    pub min_uniform_buffer_offset_alignment: DeviceSize,
    pub min_storage_buffer_offset_alignment: DeviceSize,
    pub min_texel_offset: i32,
    pub max_texel_offset: u32,
    pub min_texel_gather_offset: i32,
    pub max_texel_gather_offset: u32,
    pub min_interpolation_offset: f32,
    pub max_interpolation_offset: f32,
    pub sub_pixel_interpolation_offset_bits: u32,
    pub max_framebuffer_width: u32,
    pub max_framebuffer_height: u32,
    pub max_framebuffer_layers: u32,
    pub framebuffer_color_sample_counts: Flags,
    pub framebuffer_depth_sample_counts: Flags,
    pub framebuffer_stencil_sample_counts: Flags,
    pub framebuffer_no_attachments_sample_counts: Flags,
    pub max_color_attachments: u32,
    pub sampled_image_color_sample_counts: Flags,
    pub sampled_image_integer_sample_counts: Flags,
    pub sampled_image_depth_sample_counts: Flags,
    pub sampled_image_stencil_sample_counts: Flags,
    pub storage_image_sample_counts: Flags,
    pub max_sample_mask_words: u32,
    pub timestamp_compute_and_graphics: Bool32,
    pub timestamp_period: f32,
    pub max_clip_distances: u32,
    pub max_cull_distances: u32,
    pub max_combined_clip_and_cull_distances: u32,
    pub discrete_queue_priorities: u32,
    pub point_size_range: [f32; 2],
    pub line_width_range: [f32; 2],
    pub point_size_granularity: f32,
    pub line_width_granularity: f32,
    pub strict_lines: Bool32,
    pub standard_sample_locations: Bool32,
    pub optimal_buffer_copy_offset_alignment: DeviceSize,
    pub optimal_buffer_copy_row_pitch_alignment: DeviceSize,
    pub non_coherent_atom_size: DeviceSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PhysicalDeviceSparseProperties {
    pub residency_standard_2d_block_shape: Bool32,
    pub residency_standard_2d_multisample_block_shape: Bool32,
    pub residency_standard_3d_block_shape: Bool32,
    pub residency_aligned_mip_size: Bool32,
    pub residency_non_resident_strict: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PhysicalDeviceProperties {
    pub api_version: u32,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_type: i32,
    pub device_name: [c_char; 256],
    pub pipeline_cache_uuid: [u8; 16],
    pub limits: PhysicalDeviceLimits,
    pub sparse_properties: PhysicalDeviceSparseProperties,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct QueueFamilyProperties {
    pub queue_flags: Flags,
    pub queue_count: u32,
    pub timestamp_valid_bits: u32,
    pub min_image_transfer_granularity: Extent3D,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemoryType {
    pub property_flags: Flags,
    pub heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemoryHeap {
    pub size: DeviceSize,
    pub flags: Flags,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PhysicalDeviceMemoryProperties {
    pub memory_type_count: u32,
    pub memory_types: [MemoryType; 32],
    pub memory_heap_count: u32,
    pub memory_heaps: [MemoryHeap; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FormatProperties {
    pub linear_tiling_features: Flags,
    pub optimal_tiling_features: Flags,
    pub buffer_features: Flags,
}

/// `VkPhysicalDeviceFeatures`: 55 booleans, kept as an array.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PhysicalDeviceFeatures {
    pub flags: [Bool32; 55],
}

#[repr(C)]
pub struct PhysicalDeviceFeatures2 {
    pub s_type: i32,
    pub p_next: *mut c_void,
    pub features: PhysicalDeviceFeatures,
}

#[repr(C)]
pub struct PhysicalDeviceDynamicRenderingFeatures {
    pub s_type: i32,
    pub p_next: *mut c_void,
    pub dynamic_rendering: Bool32,
}

#[repr(C)]
pub struct PhysicalDeviceSynchronization2Features {
    pub s_type: i32,
    pub p_next: *mut c_void,
    pub synchronization2: Bool32,
}

#[repr(C)]
pub struct DeviceQueueCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub queue_family_index: u32,
    pub queue_count: u32,
    pub p_queue_priorities: *const f32,
}

#[repr(C)]
pub struct DeviceCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub queue_create_info_count: u32,
    pub p_queue_create_infos: *const DeviceQueueCreateInfo,
    pub enabled_layer_count: u32,
    pub pp_enabled_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub pp_enabled_extension_names: *const *const c_char,
    pub p_enabled_features: *const PhysicalDeviceFeatures,
}

#[repr(C)]
pub struct MemoryAllocateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub allocation_size: DeviceSize,
    pub memory_type_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemoryRequirements {
    pub size: DeviceSize,
    pub alignment: DeviceSize,
    pub memory_type_bits: u32,
}

#[repr(C)]
pub struct BufferCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub size: DeviceSize,
    pub usage: Flags,
    pub sharing_mode: i32,
    pub queue_family_index_count: u32,
    pub p_queue_family_indices: *const u32,
}

#[repr(C)]
pub struct ImageCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub image_type: i32,
    pub format: i32,
    pub extent: Extent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: Flags,
    pub tiling: i32,
    pub usage: Flags,
    pub sharing_mode: i32,
    pub queue_family_index_count: u32,
    pub p_queue_family_indices: *const u32,
    pub initial_layout: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ComponentMapping {
    pub r: i32,
    pub g: i32,
    pub b: i32,
    pub a: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ImageSubresourceRange {
    pub aspect_mask: Flags,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ImageSubresourceLayers {
    pub aspect_mask: Flags,
    pub mip_level: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

#[repr(C)]
pub struct ImageViewCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub image: Image,
    pub view_type: i32,
    pub format: i32,
    pub components: ComponentMapping,
    pub subresource_range: ImageSubresourceRange,
}

#[repr(C)]
pub struct ShaderModuleCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub code_size: usize,
    pub p_code: *const u32,
}

#[repr(C)]
pub struct PipelineLayoutCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub set_layout_count: u32,
    pub p_set_layouts: *const DescriptorSetLayout,
    pub push_constant_range_count: u32,
    pub p_push_constant_ranges: *const c_void,
}

#[repr(C)]
pub struct PipelineShaderStageCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub stage: Flags,
    pub module: ShaderModule,
    pub p_name: *const c_char,
    pub p_specialization_info: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub input_rate: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: i32,
    pub offset: u32,
}

#[repr(C)]
pub struct PipelineVertexInputStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub vertex_binding_description_count: u32,
    pub p_vertex_binding_descriptions: *const VertexInputBindingDescription,
    pub vertex_attribute_description_count: u32,
    pub p_vertex_attribute_descriptions: *const VertexInputAttributeDescription,
}

#[repr(C)]
pub struct PipelineInputAssemblyStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub topology: i32,
    pub primitive_restart_enable: Bool32,
}

#[repr(C)]
pub struct PipelineViewportStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub viewport_count: u32,
    pub p_viewports: *const Viewport,
    pub scissor_count: u32,
    pub p_scissors: *const Rect2D,
}

#[repr(C)]
pub struct PipelineRasterizationStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub depth_clamp_enable: Bool32,
    pub rasterizer_discard_enable: Bool32,
    pub polygon_mode: i32,
    pub cull_mode: Flags,
    pub front_face: i32,
    pub depth_bias_enable: Bool32,
    pub depth_bias_constant_factor: f32,
    pub depth_bias_clamp: f32,
    pub depth_bias_slope_factor: f32,
    pub line_width: f32,
}

#[repr(C)]
pub struct PipelineMultisampleStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub rasterization_samples: Flags,
    pub sample_shading_enable: Bool32,
    pub min_sample_shading: f32,
    pub p_sample_mask: *const u32,
    pub alpha_to_coverage_enable: Bool32,
    pub alpha_to_one_enable: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct StencilOpState {
    pub fail_op: i32,
    pub pass_op: i32,
    pub depth_fail_op: i32,
    pub compare_op: i32,
    pub compare_mask: u32,
    pub write_mask: u32,
    pub reference: u32,
}

#[repr(C)]
pub struct PipelineDepthStencilStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub depth_test_enable: Bool32,
    pub depth_write_enable: Bool32,
    pub depth_compare_op: i32,
    pub depth_bounds_test_enable: Bool32,
    pub stencil_test_enable: Bool32,
    pub front: StencilOpState,
    pub back: StencilOpState,
    pub min_depth_bounds: f32,
    pub max_depth_bounds: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PipelineColorBlendAttachmentState {
    pub blend_enable: Bool32,
    pub src_color_blend_factor: i32,
    pub dst_color_blend_factor: i32,
    pub color_blend_op: i32,
    pub src_alpha_blend_factor: i32,
    pub dst_alpha_blend_factor: i32,
    pub alpha_blend_op: i32,
    pub color_write_mask: Flags,
}

#[repr(C)]
pub struct PipelineColorBlendStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub logic_op_enable: Bool32,
    pub logic_op: i32,
    pub attachment_count: u32,
    pub p_attachments: *const PipelineColorBlendAttachmentState,
    pub blend_constants: [f32; 4],
}

#[repr(C)]
pub struct PipelineDynamicStateCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub dynamic_state_count: u32,
    pub p_dynamic_states: *const i32,
}

#[repr(C)]
pub struct PipelineRenderingCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub view_mask: u32,
    pub color_attachment_count: u32,
    pub p_color_attachment_formats: *const i32,
    pub depth_attachment_format: i32,
    pub stencil_attachment_format: i32,
}

#[repr(C)]
pub struct GraphicsPipelineCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub stage_count: u32,
    pub p_stages: *const PipelineShaderStageCreateInfo,
    pub p_vertex_input_state: *const PipelineVertexInputStateCreateInfo,
    pub p_input_assembly_state: *const PipelineInputAssemblyStateCreateInfo,
    pub p_tessellation_state: *const c_void,
    pub p_viewport_state: *const PipelineViewportStateCreateInfo,
    pub p_rasterization_state: *const PipelineRasterizationStateCreateInfo,
    pub p_multisample_state: *const PipelineMultisampleStateCreateInfo,
    pub p_depth_stencil_state: *const PipelineDepthStencilStateCreateInfo,
    pub p_color_blend_state: *const PipelineColorBlendStateCreateInfo,
    pub p_dynamic_state: *const PipelineDynamicStateCreateInfo,
    pub layout: PipelineLayout,
    pub render_pass: RenderPass,
    pub subpass: u32,
    pub base_pipeline_handle: Pipeline,
    pub base_pipeline_index: i32,
}

#[repr(C)]
pub struct CommandPoolCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub queue_family_index: u32,
}

#[repr(C)]
pub struct CommandBufferAllocateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub command_pool: CommandPool,
    pub level: i32,
    pub command_buffer_count: u32,
}

#[repr(C)]
pub struct CommandBufferBeginInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub p_inheritance_info: *const c_void,
}

/// `VkClearValue`: four floats, ints or uints, or depth and stencil.
#[repr(C)]
#[derive(Clone, Copy)]
pub union ClearValue {
    pub color_float: [f32; 4],
    pub color_int: [i32; 4],
    pub color_uint: [u32; 4],
    pub depth_stencil: ClearDepthStencilValue,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ClearDepthStencilValue {
    pub depth: f32,
    pub stencil: u32,
}

#[repr(C)]
pub struct RenderingAttachmentInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub image_view: ImageView,
    pub image_layout: i32,
    pub resolve_mode: Flags,
    pub resolve_image_view: ImageView,
    pub resolve_image_layout: i32,
    pub load_op: i32,
    pub store_op: i32,
    pub clear_value: ClearValue,
}

#[repr(C)]
pub struct RenderingInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub render_area: Rect2D,
    pub layer_count: u32,
    pub view_mask: u32,
    pub color_attachment_count: u32,
    pub p_color_attachments: *const RenderingAttachmentInfo,
    pub p_depth_attachment: *const RenderingAttachmentInfo,
    pub p_stencil_attachment: *const RenderingAttachmentInfo,
}

#[repr(C)]
pub struct MemoryBarrier2 {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub src_stage_mask: Flags64,
    pub src_access_mask: Flags64,
    pub dst_stage_mask: Flags64,
    pub dst_access_mask: Flags64,
}

#[repr(C)]
pub struct ImageMemoryBarrier2 {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub src_stage_mask: Flags64,
    pub src_access_mask: Flags64,
    pub dst_stage_mask: Flags64,
    pub dst_access_mask: Flags64,
    pub old_layout: i32,
    pub new_layout: i32,
    pub src_queue_family_index: u32,
    pub dst_queue_family_index: u32,
    pub image: Image,
    pub subresource_range: ImageSubresourceRange,
}

#[repr(C)]
pub struct DependencyInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub dependency_flags: Flags,
    pub memory_barrier_count: u32,
    pub p_memory_barriers: *const MemoryBarrier2,
    pub buffer_memory_barrier_count: u32,
    pub p_buffer_memory_barriers: *const c_void,
    pub image_memory_barrier_count: u32,
    pub p_image_memory_barriers: *const ImageMemoryBarrier2,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BufferCopy {
    pub src_offset: DeviceSize,
    pub dst_offset: DeviceSize,
    pub size: DeviceSize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BufferImageCopy {
    pub buffer_offset: DeviceSize,
    pub buffer_row_length: u32,
    pub buffer_image_height: u32,
    pub image_subresource: ImageSubresourceLayers,
    pub image_offset: Offset3D,
    pub image_extent: Extent3D,
}

#[repr(C)]
pub struct FenceCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
}

#[repr(C)]
pub struct SemaphoreCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
}

#[repr(C)]
pub struct SubmitInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub wait_semaphore_count: u32,
    pub p_wait_semaphores: *const Semaphore,
    pub p_wait_dst_stage_mask: *const Flags,
    pub command_buffer_count: u32,
    pub p_command_buffers: *const CommandBuffer,
    pub signal_semaphore_count: u32,
    pub p_signal_semaphores: *const Semaphore,
}

#[repr(C)]
pub struct XlibSurfaceCreateInfoKHR {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub dpy: *mut c_void,
    pub window: std::ffi::c_ulong,
}

#[repr(C)]
pub struct Win32SurfaceCreateInfoKHR {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub hinstance: *mut c_void,
    pub hwnd: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SurfaceCapabilitiesKHR {
    pub min_image_count: u32,
    pub max_image_count: u32,
    pub current_extent: Extent2D,
    pub min_image_extent: Extent2D,
    pub max_image_extent: Extent2D,
    pub max_image_array_layers: u32,
    pub supported_transforms: Flags,
    pub current_transform: Flags,
    pub supported_composite_alpha: Flags,
    pub supported_usage_flags: Flags,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SurfaceFormatKHR {
    pub format: i32,
    pub color_space: i32,
}

#[repr(C)]
pub struct SwapchainCreateInfoKHR {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub surface: SurfaceKHR,
    pub min_image_count: u32,
    pub image_format: i32,
    pub image_color_space: i32,
    pub image_extent: Extent2D,
    pub image_array_layers: u32,
    pub image_usage: Flags,
    pub image_sharing_mode: i32,
    pub queue_family_index_count: u32,
    pub p_queue_family_indices: *const u32,
    pub pre_transform: Flags,
    pub composite_alpha: Flags,
    pub present_mode: i32,
    pub clipped: Bool32,
    pub old_swapchain: SwapchainKHR,
}

#[repr(C)]
pub struct PresentInfoKHR {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub wait_semaphore_count: u32,
    pub p_wait_semaphores: *const Semaphore,
    pub swapchain_count: u32,
    pub p_swapchains: *const SwapchainKHR,
    pub p_image_indices: *const u32,
    pub p_results: *mut Result,
}

#[repr(C)]
pub struct SamplerCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub mag_filter: i32,
    pub min_filter: i32,
    pub mipmap_mode: i32,
    pub address_mode_u: i32,
    pub address_mode_v: i32,
    pub address_mode_w: i32,
    pub mip_lod_bias: f32,
    pub anisotropy_enable: Bool32,
    pub max_anisotropy: f32,
    pub compare_enable: Bool32,
    pub compare_op: i32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border_color: i32,
    pub unnormalized_coordinates: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptor_type: i32,
    pub descriptor_count: u32,
    pub stage_flags: Flags,
    pub p_immutable_samplers: *const Sampler,
}

#[repr(C)]
pub struct DescriptorSetLayoutCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub binding_count: u32,
    pub p_bindings: *const DescriptorSetLayoutBinding,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorPoolSize {
    pub descriptor_type: i32,
    pub descriptor_count: u32,
}

#[repr(C)]
pub struct DescriptorPoolCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub max_sets: u32,
    pub pool_size_count: u32,
    pub p_pool_sizes: *const DescriptorPoolSize,
}

#[repr(C)]
pub struct DescriptorSetAllocateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub descriptor_pool: DescriptorPool,
    pub descriptor_set_count: u32,
    pub p_set_layouts: *const DescriptorSetLayout,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorBufferInfo {
    pub buffer: Buffer,
    pub offset: DeviceSize,
    pub range: DeviceSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorImageInfo {
    pub sampler: Sampler,
    pub image_view: ImageView,
    pub image_layout: i32,
}

#[repr(C)]
pub struct WriteDescriptorSet {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub dst_set: DescriptorSet,
    pub dst_binding: u32,
    pub dst_array_element: u32,
    pub descriptor_count: u32,
    pub descriptor_type: i32,
    pub p_image_info: *const DescriptorImageInfo,
    pub p_buffer_info: *const DescriptorBufferInfo,
    pub p_texel_buffer_view: *const c_void,
}

#[repr(C)]
pub struct ComputePipelineCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub flags: Flags,
    pub stage: PipelineShaderStageCreateInfo,
    pub layout: PipelineLayout,
    pub base_pipeline_handle: Pipeline,
    pub base_pipeline_index: i32,
}

pub type PFN_vkGetInstanceProcAddr =
    unsafe extern "system" fn(Instance, *const c_char) -> *const c_void;

/// Declares a table of Vulkan entry points and a loader that resolves each by name.
macro_rules! fn_table {
    ($table:ident { $($field:ident = $symbol:literal : fn($($arg:ty),* $(,)?) $(-> $ret:ty)?;)* }) => {
        pub struct $table {
            $(pub $field: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
        }

        impl $table {
            /// Resolves every entry point through `load`, failing with the first missing name.
            ///
            /// # Safety
            /// `load` must return pointers to functions with the declared signatures.
            pub unsafe fn load(
                mut load: impl FnMut(&CStr) -> *const c_void,
            ) -> std::result::Result<Self, &'static str> {
                Ok(Self {
                    $($field: {
                        let name = CStr::from_bytes_with_nul(concat!($symbol, "\0").as_bytes()).unwrap();
                        let ptr = load(name);
                        if ptr.is_null() {
                            return Err($symbol);
                        }
                        std::mem::transmute::<*const c_void, unsafe extern "system" fn($($arg),*) $(-> $ret)?>(ptr)
                    },)*
                })
            }
        }
    };
}

fn_table!(EntryFns {
    create_instance = "vkCreateInstance":
        fn(*const InstanceCreateInfo, *const c_void, *mut Instance) -> Result;
    enumerate_instance_extension_properties = "vkEnumerateInstanceExtensionProperties":
        fn(*const c_char, *mut u32, *mut ExtensionProperties) -> Result;
    enumerate_instance_layer_properties = "vkEnumerateInstanceLayerProperties":
        fn(*mut u32, *mut LayerProperties) -> Result;
});

fn_table!(InstanceFns {
    destroy_instance = "vkDestroyInstance": fn(Instance, *const c_void);
    enumerate_physical_devices = "vkEnumeratePhysicalDevices": fn(Instance, *mut u32, *mut PhysicalDevice) -> Result;
    get_physical_device_properties = "vkGetPhysicalDeviceProperties":
        fn(PhysicalDevice, *mut PhysicalDeviceProperties);
    get_physical_device_queue_family_properties = "vkGetPhysicalDeviceQueueFamilyProperties":
        fn(PhysicalDevice, *mut u32, *mut QueueFamilyProperties);
    get_physical_device_memory_properties = "vkGetPhysicalDeviceMemoryProperties":
        fn(PhysicalDevice, *mut PhysicalDeviceMemoryProperties);
    get_physical_device_format_properties = "vkGetPhysicalDeviceFormatProperties":
        fn(PhysicalDevice, i32, *mut FormatProperties);
    get_physical_device_features2 = "vkGetPhysicalDeviceFeatures2": fn(PhysicalDevice, *mut PhysicalDeviceFeatures2);
    enumerate_device_extension_properties = "vkEnumerateDeviceExtensionProperties":
        fn(PhysicalDevice, *const c_char, *mut u32, *mut ExtensionProperties) -> Result;
    create_device = "vkCreateDevice":
        fn(PhysicalDevice, *const DeviceCreateInfo, *const c_void, *mut Device) -> Result;
    get_device_proc_addr = "vkGetDeviceProcAddr": fn(Device, *const c_char) -> *const c_void;
});

fn_table!(SurfaceFns {
    destroy_surface = "vkDestroySurfaceKHR": fn(Instance, SurfaceKHR, *const c_void);
    get_physical_device_surface_support = "vkGetPhysicalDeviceSurfaceSupportKHR":
        fn(PhysicalDevice, u32, SurfaceKHR, *mut Bool32) -> Result;
    get_physical_device_surface_capabilities = "vkGetPhysicalDeviceSurfaceCapabilitiesKHR":
        fn(PhysicalDevice, SurfaceKHR, *mut SurfaceCapabilitiesKHR) -> Result;
    get_physical_device_surface_formats = "vkGetPhysicalDeviceSurfaceFormatsKHR":
        fn(PhysicalDevice, SurfaceKHR, *mut u32, *mut SurfaceFormatKHR) -> Result;
    get_physical_device_surface_present_modes = "vkGetPhysicalDeviceSurfacePresentModesKHR":
        fn(PhysicalDevice, SurfaceKHR, *mut u32, *mut i32) -> Result;
});

fn_table!(XlibSurfaceFns {
    create_xlib_surface = "vkCreateXlibSurfaceKHR":
        fn(Instance, *const XlibSurfaceCreateInfoKHR, *const c_void, *mut SurfaceKHR) -> Result;
});

fn_table!(Win32SurfaceFns {
    create_win32_surface = "vkCreateWin32SurfaceKHR":
        fn(Instance, *const Win32SurfaceCreateInfoKHR, *const c_void, *mut SurfaceKHR) -> Result;
});

fn_table!(DeviceFns {
    destroy_device = "vkDestroyDevice": fn(Device, *const c_void);
    get_device_queue = "vkGetDeviceQueue": fn(Device, u32, u32, *mut Queue);
    queue_submit = "vkQueueSubmit": fn(Queue, u32, *const SubmitInfo, Fence) -> Result;
    queue_wait_idle = "vkQueueWaitIdle": fn(Queue) -> Result;
    device_wait_idle = "vkDeviceWaitIdle": fn(Device) -> Result;
    allocate_memory = "vkAllocateMemory":
        fn(Device, *const MemoryAllocateInfo, *const c_void, *mut DeviceMemory) -> Result;
    free_memory = "vkFreeMemory": fn(Device, DeviceMemory, *const c_void);
    map_memory = "vkMapMemory": fn(Device, DeviceMemory, DeviceSize, DeviceSize, Flags, *mut *mut c_void) -> Result;
    unmap_memory = "vkUnmapMemory": fn(Device, DeviceMemory);
    bind_buffer_memory = "vkBindBufferMemory": fn(Device, Buffer, DeviceMemory, DeviceSize) -> Result;
    bind_image_memory = "vkBindImageMemory": fn(Device, Image, DeviceMemory, DeviceSize) -> Result;
    get_buffer_memory_requirements = "vkGetBufferMemoryRequirements": fn(Device, Buffer, *mut MemoryRequirements);
    get_image_memory_requirements = "vkGetImageMemoryRequirements": fn(Device, Image, *mut MemoryRequirements);
    create_fence = "vkCreateFence": fn(Device, *const FenceCreateInfo, *const c_void, *mut Fence) -> Result;
    destroy_fence = "vkDestroyFence": fn(Device, Fence, *const c_void);
    get_fence_status = "vkGetFenceStatus": fn(Device, Fence) -> Result;
    wait_for_fences = "vkWaitForFences": fn(Device, u32, *const Fence, Bool32, u64) -> Result;
    create_semaphore = "vkCreateSemaphore":
        fn(Device, *const SemaphoreCreateInfo, *const c_void, *mut Semaphore) -> Result;
    destroy_semaphore = "vkDestroySemaphore": fn(Device, Semaphore, *const c_void);
    create_buffer = "vkCreateBuffer": fn(Device, *const BufferCreateInfo, *const c_void, *mut Buffer) -> Result;
    destroy_buffer = "vkDestroyBuffer": fn(Device, Buffer, *const c_void);
    create_image = "vkCreateImage": fn(Device, *const ImageCreateInfo, *const c_void, *mut Image) -> Result;
    destroy_image = "vkDestroyImage": fn(Device, Image, *const c_void);
    create_image_view = "vkCreateImageView":
        fn(Device, *const ImageViewCreateInfo, *const c_void, *mut ImageView) -> Result;
    destroy_image_view = "vkDestroyImageView": fn(Device, ImageView, *const c_void);
    create_shader_module = "vkCreateShaderModule":
        fn(Device, *const ShaderModuleCreateInfo, *const c_void, *mut ShaderModule) -> Result;
    destroy_shader_module = "vkDestroyShaderModule": fn(Device, ShaderModule, *const c_void);
    create_pipeline_layout = "vkCreatePipelineLayout":
        fn(Device, *const PipelineLayoutCreateInfo, *const c_void, *mut PipelineLayout) -> Result;
    destroy_pipeline_layout = "vkDestroyPipelineLayout": fn(Device, PipelineLayout, *const c_void);
    create_graphics_pipelines = "vkCreateGraphicsPipelines":
        fn(Device, PipelineCache, u32, *const GraphicsPipelineCreateInfo, *const c_void, *mut Pipeline) -> Result;
    destroy_pipeline = "vkDestroyPipeline": fn(Device, Pipeline, *const c_void);
    create_command_pool = "vkCreateCommandPool":
        fn(Device, *const CommandPoolCreateInfo, *const c_void, *mut CommandPool) -> Result;
    destroy_command_pool = "vkDestroyCommandPool": fn(Device, CommandPool, *const c_void);
    allocate_command_buffers = "vkAllocateCommandBuffers":
        fn(Device, *const CommandBufferAllocateInfo, *mut CommandBuffer) -> Result;
    free_command_buffers = "vkFreeCommandBuffers": fn(Device, CommandPool, u32, *const CommandBuffer);
    begin_command_buffer = "vkBeginCommandBuffer": fn(CommandBuffer, *const CommandBufferBeginInfo) -> Result;
    end_command_buffer = "vkEndCommandBuffer": fn(CommandBuffer) -> Result;
    cmd_bind_pipeline = "vkCmdBindPipeline": fn(CommandBuffer, i32, Pipeline);
    cmd_set_viewport = "vkCmdSetViewport": fn(CommandBuffer, u32, u32, *const Viewport);
    cmd_set_scissor = "vkCmdSetScissor": fn(CommandBuffer, u32, u32, *const Rect2D);
    cmd_set_blend_constants = "vkCmdSetBlendConstants": fn(CommandBuffer, *const [f32; 4]);
    cmd_set_stencil_reference = "vkCmdSetStencilReference": fn(CommandBuffer, Flags, u32);
    cmd_bind_vertex_buffers = "vkCmdBindVertexBuffers":
        fn(CommandBuffer, u32, u32, *const Buffer, *const DeviceSize);
    cmd_bind_index_buffer = "vkCmdBindIndexBuffer": fn(CommandBuffer, Buffer, DeviceSize, i32);
    cmd_draw = "vkCmdDraw": fn(CommandBuffer, u32, u32, u32, u32);
    cmd_draw_indexed = "vkCmdDrawIndexed": fn(CommandBuffer, u32, u32, u32, i32, u32);
    cmd_copy_buffer = "vkCmdCopyBuffer": fn(CommandBuffer, Buffer, Buffer, u32, *const BufferCopy);
    cmd_copy_buffer_to_image = "vkCmdCopyBufferToImage":
        fn(CommandBuffer, Buffer, Image, i32, u32, *const BufferImageCopy);
    cmd_copy_image_to_buffer = "vkCmdCopyImageToBuffer":
        fn(CommandBuffer, Image, i32, Buffer, u32, *const BufferImageCopy);
    create_sampler = "vkCreateSampler":
        fn(Device, *const SamplerCreateInfo, *const c_void, *mut Sampler) -> Result;
    destroy_sampler = "vkDestroySampler": fn(Device, Sampler, *const c_void);
    create_descriptor_set_layout = "vkCreateDescriptorSetLayout":
        fn(Device, *const DescriptorSetLayoutCreateInfo, *const c_void, *mut DescriptorSetLayout) -> Result;
    destroy_descriptor_set_layout = "vkDestroyDescriptorSetLayout":
        fn(Device, DescriptorSetLayout, *const c_void);
    create_descriptor_pool = "vkCreateDescriptorPool":
        fn(Device, *const DescriptorPoolCreateInfo, *const c_void, *mut DescriptorPool) -> Result;
    destroy_descriptor_pool = "vkDestroyDescriptorPool": fn(Device, DescriptorPool, *const c_void);
    allocate_descriptor_sets = "vkAllocateDescriptorSets":
        fn(Device, *const DescriptorSetAllocateInfo, *mut DescriptorSet) -> Result;
    update_descriptor_sets = "vkUpdateDescriptorSets":
        fn(Device, u32, *const WriteDescriptorSet, u32, *const c_void);
    create_compute_pipelines = "vkCreateComputePipelines":
        fn(Device, PipelineCache, u32, *const ComputePipelineCreateInfo, *const c_void, *mut Pipeline) -> Result;
    cmd_bind_descriptor_sets = "vkCmdBindDescriptorSets":
        fn(CommandBuffer, i32, PipelineLayout, u32, u32, *const DescriptorSet, u32, *const u32);
    cmd_dispatch = "vkCmdDispatch": fn(CommandBuffer, u32, u32, u32);
    cmd_dispatch_indirect = "vkCmdDispatchIndirect": fn(CommandBuffer, Buffer, DeviceSize);
    cmd_draw_indirect = "vkCmdDrawIndirect": fn(CommandBuffer, Buffer, DeviceSize, u32, u32);
    cmd_draw_indexed_indirect = "vkCmdDrawIndexedIndirect": fn(CommandBuffer, Buffer, DeviceSize, u32, u32);
});

// Entry points promoted to core in Vulkan 1.3, loaded under their core or KHR names.
fn_table!(RenderingFns {
    cmd_begin_rendering = "vkCmdBeginRendering": fn(CommandBuffer, *const RenderingInfo);
    cmd_end_rendering = "vkCmdEndRendering": fn(CommandBuffer);
    cmd_pipeline_barrier2 = "vkCmdPipelineBarrier2": fn(CommandBuffer, *const DependencyInfo);
});

fn_table!(SwapchainFns {
    create_swapchain = "vkCreateSwapchainKHR":
        fn(Device, *const SwapchainCreateInfoKHR, *const c_void, *mut SwapchainKHR) -> Result;
    destroy_swapchain = "vkDestroySwapchainKHR": fn(Device, SwapchainKHR, *const c_void);
    get_swapchain_images = "vkGetSwapchainImagesKHR": fn(Device, SwapchainKHR, *mut u32, *mut Image) -> Result;
    acquire_next_image = "vkAcquireNextImageKHR":
        fn(Device, SwapchainKHR, u64, Semaphore, Fence, *mut u32) -> Result;
    queue_present = "vkQueuePresentKHR": fn(Queue, *const PresentInfoKHR) -> Result;
});

/// Calls a Vulkan enumeration function twice: once for the count, once for the items.
pub unsafe fn enumerate<T: Copy>(
    mut f: impl FnMut(*mut u32, *mut T) -> Result,
    zero: T,
) -> std::result::Result<Vec<T>, Result> {
    loop {
        let mut count = 0;
        let r = f(&mut count, std::ptr::null_mut());
        if r < 0 {
            return Err(r);
        }
        let mut items = vec![zero; count as usize];
        let r = f(&mut count, items.as_mut_ptr());
        if r == INCOMPLETE {
            continue;
        }
        if r < 0 {
            return Err(r);
        }
        items.truncate(count as usize);
        return Ok(items);
    }
}

pub fn result_name(r: Result) -> &'static str {
    match r {
        SUCCESS => "VK_SUCCESS",
        NOT_READY => "VK_NOT_READY",
        TIMEOUT => "VK_TIMEOUT",
        SUBOPTIMAL_KHR => "VK_SUBOPTIMAL_KHR",
        ERROR_OUT_OF_HOST_MEMORY => "VK_ERROR_OUT_OF_HOST_MEMORY",
        ERROR_OUT_OF_DEVICE_MEMORY => "VK_ERROR_OUT_OF_DEVICE_MEMORY",
        -3 => "VK_ERROR_INITIALIZATION_FAILED",
        ERROR_DEVICE_LOST => "VK_ERROR_DEVICE_LOST",
        -5 => "VK_ERROR_MEMORY_MAP_FAILED",
        -6 => "VK_ERROR_LAYER_NOT_PRESENT",
        -7 => "VK_ERROR_EXTENSION_NOT_PRESENT",
        -8 => "VK_ERROR_FEATURE_NOT_PRESENT",
        -9 => "VK_ERROR_INCOMPATIBLE_DRIVER",
        ERROR_SURFACE_LOST_KHR => "VK_ERROR_SURFACE_LOST_KHR",
        ERROR_OUT_OF_DATE_KHR => "VK_ERROR_OUT_OF_DATE_KHR",
        _ => "VkResult error",
    }
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// Sizes from `sizeof` on x86_64 Linux with vulkan_core.h 1.3.275.
    #[test]
    fn struct_sizes_match_the_headers() {
        assert_eq!(size_of::<ApplicationInfo>(), 48);
        assert_eq!(size_of::<InstanceCreateInfo>(), 64);
        assert_eq!(size_of::<ExtensionProperties>(), 260);
        assert_eq!(size_of::<LayerProperties>(), 520);
        assert_eq!(size_of::<PhysicalDeviceLimits>(), 504);
        assert_eq!(size_of::<PhysicalDeviceProperties>(), 824);
        assert_eq!(size_of::<QueueFamilyProperties>(), 24);
        assert_eq!(size_of::<PhysicalDeviceMemoryProperties>(), 520);
        assert_eq!(size_of::<PhysicalDeviceFeatures>(), 220);
        assert_eq!(size_of::<PhysicalDeviceFeatures2>(), 240);
        assert_eq!(size_of::<DeviceQueueCreateInfo>(), 40);
        assert_eq!(size_of::<DeviceCreateInfo>(), 72);
        assert_eq!(size_of::<MemoryAllocateInfo>(), 32);
        assert_eq!(size_of::<MemoryRequirements>(), 24);
        assert_eq!(size_of::<BufferCreateInfo>(), 56);
        assert_eq!(size_of::<ImageCreateInfo>(), 88);
        assert_eq!(size_of::<ImageViewCreateInfo>(), 80);
        assert_eq!(size_of::<ShaderModuleCreateInfo>(), 40);
        assert_eq!(size_of::<PipelineLayoutCreateInfo>(), 48);
        assert_eq!(size_of::<PipelineShaderStageCreateInfo>(), 48);
        assert_eq!(size_of::<PipelineVertexInputStateCreateInfo>(), 48);
        assert_eq!(size_of::<PipelineInputAssemblyStateCreateInfo>(), 32);
        assert_eq!(size_of::<PipelineViewportStateCreateInfo>(), 48);
        assert_eq!(size_of::<PipelineRasterizationStateCreateInfo>(), 64);
        assert_eq!(size_of::<PipelineMultisampleStateCreateInfo>(), 48);
        assert_eq!(size_of::<PipelineDepthStencilStateCreateInfo>(), 104);
        assert_eq!(size_of::<PipelineColorBlendStateCreateInfo>(), 56);
        assert_eq!(size_of::<PipelineDynamicStateCreateInfo>(), 32);
        assert_eq!(size_of::<PipelineRenderingCreateInfo>(), 40);
        assert_eq!(size_of::<GraphicsPipelineCreateInfo>(), 144);
        assert_eq!(size_of::<CommandBufferAllocateInfo>(), 32);
        assert_eq!(size_of::<RenderingAttachmentInfo>(), 72);
        assert_eq!(size_of::<RenderingInfo>(), 72);
        assert_eq!(size_of::<ImageMemoryBarrier2>(), 96);
        assert_eq!(size_of::<MemoryBarrier2>(), 48);
        assert_eq!(size_of::<DependencyInfo>(), 64);
        assert_eq!(size_of::<BufferImageCopy>(), 56);
        assert_eq!(size_of::<SubmitInfo>(), 72);
        // `Window` is an `unsigned long`, which is 64 bits on the Unix targets Xlib runs on.
        #[cfg(unix)]
        assert_eq!(size_of::<XlibSurfaceCreateInfoKHR>(), 40);
        assert_eq!(size_of::<Win32SurfaceCreateInfoKHR>(), 40);
        assert_eq!(size_of::<SurfaceCapabilitiesKHR>(), 52);
        assert_eq!(size_of::<SwapchainCreateInfoKHR>(), 104);
        assert_eq!(size_of::<PresentInfoKHR>(), 64);
        assert_eq!(size_of::<PhysicalDeviceDynamicRenderingFeatures>(), 24);
        assert_eq!(size_of::<PhysicalDeviceSynchronization2Features>(), 24);
        assert_eq!(size_of::<CommandPoolCreateInfo>(), 24);
        assert_eq!(size_of::<CommandBufferBeginInfo>(), 32);
        assert_eq!(size_of::<PipelineColorBlendAttachmentState>(), 32);
        assert_eq!(size_of::<StencilOpState>(), 28);
        assert_eq!(size_of::<FormatProperties>(), 12);
        assert_eq!(size_of::<ClearValue>(), 16);
        assert_eq!(size_of::<SamplerCreateInfo>(), 80);
        assert_eq!(size_of::<DescriptorSetLayoutBinding>(), 24);
        assert_eq!(size_of::<DescriptorSetLayoutCreateInfo>(), 32);
        assert_eq!(size_of::<DescriptorPoolSize>(), 8);
        assert_eq!(size_of::<DescriptorPoolCreateInfo>(), 40);
        assert_eq!(size_of::<DescriptorSetAllocateInfo>(), 40);
        assert_eq!(size_of::<DescriptorBufferInfo>(), 24);
        assert_eq!(size_of::<DescriptorImageInfo>(), 24);
        assert_eq!(size_of::<WriteDescriptorSet>(), 64);
        assert_eq!(size_of::<ComputePipelineCreateInfo>(), 96);
    }
}
