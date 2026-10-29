//! WebGPU values to Vulkan values.

use super::vk;
use crate::*;

/// The Vulkan format for a WebGPU format. `depth24plus` formats are resolved per adapter,
/// so they are passed in.
pub fn format(format: TextureFormat, depth24: i32, depth24_stencil8: i32) -> i32 {
    use TextureFormat::*;
    match format {
        R8Unorm => vk::FORMAT_R8_UNORM,
        R8Snorm => vk::FORMAT_R8_SNORM,
        R8Uint => vk::FORMAT_R8_UINT,
        R8Sint => vk::FORMAT_R8_SINT,
        R16Uint => vk::FORMAT_R16_UINT,
        R16Sint => vk::FORMAT_R16_SINT,
        R16Float => vk::FORMAT_R16_SFLOAT,
        Rg8Unorm => vk::FORMAT_R8G8_UNORM,
        Rg8Snorm => vk::FORMAT_R8G8_SNORM,
        Rg8Uint => vk::FORMAT_R8G8_UINT,
        Rg8Sint => vk::FORMAT_R8G8_SINT,
        R32Uint => vk::FORMAT_R32_UINT,
        R32Sint => vk::FORMAT_R32_SINT,
        R32Float => vk::FORMAT_R32_SFLOAT,
        Rg16Uint => vk::FORMAT_R16G16_UINT,
        Rg16Sint => vk::FORMAT_R16G16_SINT,
        Rg16Float => vk::FORMAT_R16G16_SFLOAT,
        Rgba8Unorm => vk::FORMAT_R8G8B8A8_UNORM,
        Rgba8UnormSrgb => vk::FORMAT_R8G8B8A8_SRGB,
        Rgba8Snorm => vk::FORMAT_R8G8B8A8_SNORM,
        Rgba8Uint => vk::FORMAT_R8G8B8A8_UINT,
        Rgba8Sint => vk::FORMAT_R8G8B8A8_SINT,
        Bgra8Unorm => vk::FORMAT_B8G8R8A8_UNORM,
        Bgra8UnormSrgb => vk::FORMAT_B8G8R8A8_SRGB,
        Rgb10a2Uint => vk::FORMAT_A2B10G10R10_UINT_PACK32,
        Rgb10a2Unorm => vk::FORMAT_A2B10G10R10_UNORM_PACK32,
        Rg11b10Ufloat => vk::FORMAT_B10G11R11_UFLOAT_PACK32,
        Rgb9e5Ufloat => vk::FORMAT_E5B9G9R9_UFLOAT_PACK32,
        Rg32Uint => vk::FORMAT_R32G32_UINT,
        Rg32Sint => vk::FORMAT_R32G32_SINT,
        Rg32Float => vk::FORMAT_R32G32_SFLOAT,
        Rgba16Uint => vk::FORMAT_R16G16B16A16_UINT,
        Rgba16Sint => vk::FORMAT_R16G16B16A16_SINT,
        Rgba16Float => vk::FORMAT_R16G16B16A16_SFLOAT,
        Rgba32Uint => vk::FORMAT_R32G32B32A32_UINT,
        Rgba32Sint => vk::FORMAT_R32G32B32A32_SINT,
        Rgba32Float => vk::FORMAT_R32G32B32A32_SFLOAT,
        Stencil8 => vk::FORMAT_S8_UINT,
        Depth16Unorm => vk::FORMAT_D16_UNORM,
        Depth24Plus => depth24,
        Depth24PlusStencil8 => depth24_stencil8,
        Depth32Float => vk::FORMAT_D32_SFLOAT,
        Depth32FloatStencil8 => vk::FORMAT_D32_SFLOAT_S8_UINT,
    }
}

/// The WebGPU format for a surface format, if it is one WebGPU can present.
pub fn surface_format(format: i32) -> Option<TextureFormat> {
    Some(match format {
        vk::FORMAT_B8G8R8A8_UNORM => TextureFormat::Bgra8Unorm,
        vk::FORMAT_B8G8R8A8_SRGB => TextureFormat::Bgra8UnormSrgb,
        vk::FORMAT_R8G8B8A8_UNORM => TextureFormat::Rgba8Unorm,
        vk::FORMAT_R8G8B8A8_SRGB => TextureFormat::Rgba8UnormSrgb,
        vk::FORMAT_R16G16B16A16_SFLOAT => TextureFormat::Rgba16Float,
        _ => return None,
    })
}

pub fn vertex_format(format: VertexFormat) -> i32 {
    use VertexFormat::*;
    // Values from VkFormat (checked against vulkan_core.h); the 8- and 16-bit formats are not in vk.rs's list.
    match format {
        Uint8x2 => 20,
        Uint8x4 => 41,
        Sint8x2 => 21,
        Sint8x4 => 42,
        Unorm8x2 => 16,
        Unorm8x4 => 37,
        Snorm8x2 => 17,
        Snorm8x4 => 38,
        Uint16x2 => 81,
        Uint16x4 => 95,
        Sint16x2 => 82,
        Sint16x4 => 96,
        Unorm16x2 => 77,
        Unorm16x4 => 91,
        Snorm16x2 => 78,
        Snorm16x4 => 92,
        Float16x2 => 83,
        Float16x4 => 97,
        Float32 => vk::FORMAT_R32_SFLOAT,
        Float32x2 => vk::FORMAT_R32G32_SFLOAT,
        Float32x3 => vk::FORMAT_R32G32B32_SFLOAT,
        Float32x4 => vk::FORMAT_R32G32B32A32_SFLOAT,
        Uint32 => vk::FORMAT_R32_UINT,
        Uint32x2 => vk::FORMAT_R32G32_UINT,
        Uint32x3 => vk::FORMAT_R32G32B32_UINT,
        Uint32x4 => vk::FORMAT_R32G32B32A32_UINT,
        Sint32 => vk::FORMAT_R32_SINT,
        Sint32x2 => vk::FORMAT_R32G32_SINT,
        Sint32x3 => vk::FORMAT_R32G32B32_SINT,
        Sint32x4 => vk::FORMAT_R32G32B32A32_SINT,
        Unorm10_10_10_2 => vk::FORMAT_A2B10G10R10_UNORM_PACK32,
    }
}

pub fn aspect_mask(format: TextureFormat, aspect: TextureAspect) -> vk::Flags {
    match aspect {
        TextureAspect::DepthOnly => vk::IMAGE_ASPECT_DEPTH_BIT,
        TextureAspect::StencilOnly => vk::IMAGE_ASPECT_STENCIL_BIT,
        TextureAspect::All => {
            let mut mask = 0;
            if format.has_depth() {
                mask |= vk::IMAGE_ASPECT_DEPTH_BIT;
            }
            if format.has_stencil() {
                mask |= vk::IMAGE_ASPECT_STENCIL_BIT;
            }
            if mask == 0 {
                mask = vk::IMAGE_ASPECT_COLOR_BIT;
            }
            mask
        }
    }
}

pub fn buffer_usage(usage: BufferUsage) -> vk::Flags {
    let mut flags = 0;
    // Every buffer can take part in copies internally (staging uploads, readback).
    flags |= vk::BUFFER_USAGE_TRANSFER_SRC_BIT | vk::BUFFER_USAGE_TRANSFER_DST_BIT;
    if usage.contains(BufferUsage::INDEX) {
        flags |= vk::BUFFER_USAGE_INDEX_BUFFER_BIT;
    }
    if usage.contains(BufferUsage::VERTEX) {
        flags |= vk::BUFFER_USAGE_VERTEX_BUFFER_BIT;
    }
    if usage.contains(BufferUsage::UNIFORM) {
        flags |= vk::BUFFER_USAGE_UNIFORM_BUFFER_BIT;
    }
    if usage.contains(BufferUsage::STORAGE) {
        flags |= vk::BUFFER_USAGE_STORAGE_BUFFER_BIT;
    }
    if usage.contains(BufferUsage::INDIRECT) {
        flags |= vk::BUFFER_USAGE_INDIRECT_BUFFER_BIT;
    }
    flags
}

pub fn texture_usage(usage: TextureUsage, format: TextureFormat) -> vk::Flags {
    let mut flags = 0;
    if usage.contains(TextureUsage::COPY_SRC) {
        flags |= vk::IMAGE_USAGE_TRANSFER_SRC_BIT;
    }
    if usage.contains(TextureUsage::COPY_DST) {
        flags |= vk::IMAGE_USAGE_TRANSFER_DST_BIT;
    }
    if usage.contains(TextureUsage::TEXTURE_BINDING) {
        flags |= vk::IMAGE_USAGE_SAMPLED_BIT;
    }
    if usage.contains(TextureUsage::STORAGE_BINDING) {
        flags |= vk::IMAGE_USAGE_STORAGE_BIT;
    }
    if usage.contains(TextureUsage::RENDER_ATTACHMENT) {
        flags |= if format.is_depth_stencil() {
            vk::IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT
        } else {
            vk::IMAGE_USAGE_COLOR_ATTACHMENT_BIT
        };
    }
    flags
}

pub fn topology(topology: PrimitiveTopology) -> i32 {
    match topology {
        PrimitiveTopology::PointList => vk::PRIMITIVE_TOPOLOGY_POINT_LIST,
        PrimitiveTopology::LineList => vk::PRIMITIVE_TOPOLOGY_LINE_LIST,
        PrimitiveTopology::LineStrip => vk::PRIMITIVE_TOPOLOGY_LINE_STRIP,
        PrimitiveTopology::TriangleList => vk::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
        PrimitiveTopology::TriangleStrip => vk::PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP,
    }
}

pub fn compare(f: CompareFunction) -> i32 {
    match f {
        CompareFunction::Never => vk::COMPARE_OP_NEVER,
        CompareFunction::Less => vk::COMPARE_OP_LESS,
        CompareFunction::Equal => vk::COMPARE_OP_EQUAL,
        CompareFunction::LessEqual => vk::COMPARE_OP_LESS_OR_EQUAL,
        CompareFunction::Greater => vk::COMPARE_OP_GREATER,
        CompareFunction::NotEqual => vk::COMPARE_OP_NOT_EQUAL,
        CompareFunction::GreaterEqual => vk::COMPARE_OP_GREATER_OR_EQUAL,
        CompareFunction::Always => vk::COMPARE_OP_ALWAYS,
    }
}

pub fn stencil_op(op: StencilOperation) -> i32 {
    match op {
        StencilOperation::Keep => vk::STENCIL_OP_KEEP,
        StencilOperation::Zero => vk::STENCIL_OP_ZERO,
        StencilOperation::Replace => vk::STENCIL_OP_REPLACE,
        StencilOperation::Invert => vk::STENCIL_OP_INVERT,
        StencilOperation::IncrementClamp => vk::STENCIL_OP_INCREMENT_AND_CLAMP,
        StencilOperation::DecrementClamp => vk::STENCIL_OP_DECREMENT_AND_CLAMP,
        StencilOperation::IncrementWrap => vk::STENCIL_OP_INCREMENT_AND_WRAP,
        StencilOperation::DecrementWrap => vk::STENCIL_OP_DECREMENT_AND_WRAP,
    }
}

pub fn stencil_face(
    face: &StencilFaceState,
    read_mask: u32,
    write_mask: u32,
) -> vk::StencilOpState {
    vk::StencilOpState {
        fail_op: stencil_op(face.fail_op),
        pass_op: stencil_op(face.pass_op),
        depth_fail_op: stencil_op(face.depth_fail_op),
        compare_op: compare(face.compare),
        compare_mask: read_mask,
        write_mask,
        reference: 0,
    }
}

pub fn blend_factor(f: BlendFactor) -> i32 {
    match f {
        BlendFactor::Zero => vk::BLEND_FACTOR_ZERO,
        BlendFactor::One => vk::BLEND_FACTOR_ONE,
        BlendFactor::Src => vk::BLEND_FACTOR_SRC_COLOR,
        BlendFactor::OneMinusSrc => vk::BLEND_FACTOR_ONE_MINUS_SRC_COLOR,
        BlendFactor::SrcAlpha => vk::BLEND_FACTOR_SRC_ALPHA,
        BlendFactor::OneMinusSrcAlpha => vk::BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
        BlendFactor::Dst => vk::BLEND_FACTOR_DST_COLOR,
        BlendFactor::OneMinusDst => vk::BLEND_FACTOR_ONE_MINUS_DST_COLOR,
        BlendFactor::DstAlpha => vk::BLEND_FACTOR_DST_ALPHA,
        BlendFactor::OneMinusDstAlpha => vk::BLEND_FACTOR_ONE_MINUS_DST_ALPHA,
        BlendFactor::SrcAlphaSaturated => vk::BLEND_FACTOR_SRC_ALPHA_SATURATE,
        BlendFactor::Constant => vk::BLEND_FACTOR_CONSTANT_COLOR,
        BlendFactor::OneMinusConstant => vk::BLEND_FACTOR_ONE_MINUS_CONSTANT_COLOR,
    }
}

pub fn blend_op(op: BlendOperation) -> i32 {
    match op {
        BlendOperation::Add => vk::BLEND_OP_ADD,
        BlendOperation::Subtract => vk::BLEND_OP_SUBTRACT,
        BlendOperation::ReverseSubtract => vk::BLEND_OP_REVERSE_SUBTRACT,
        BlendOperation::Min => vk::BLEND_OP_MIN,
        BlendOperation::Max => vk::BLEND_OP_MAX,
    }
}

pub fn clear_color(color: Color, format: TextureFormat) -> vk::ClearValue {
    use TextureFormat::*;
    let c = [color.r, color.g, color.b, color.a];
    match format {
        R8Uint | R16Uint | Rg8Uint | R32Uint | Rg16Uint | Rgba8Uint | Rgb10a2Uint | Rg32Uint
        | Rgba16Uint | Rgba32Uint => vk::ClearValue {
            color_uint: c.map(|v| v as u32),
        },
        R8Sint | R16Sint | Rg8Sint | R32Sint | Rg16Sint | Rgba8Sint | Rg32Sint | Rgba16Sint
        | Rgba32Sint => vk::ClearValue {
            color_int: c.map(|v| v as i32),
        },
        _ => vk::ClearValue {
            color_float: c.map(|v| v as f32),
        },
    }
}

pub fn present_mode(mode: PresentMode) -> i32 {
    match mode {
        PresentMode::Fifo => vk::PRESENT_MODE_FIFO_KHR,
        PresentMode::Mailbox => vk::PRESENT_MODE_MAILBOX_KHR,
        PresentMode::Immediate => vk::PRESENT_MODE_IMMEDIATE_KHR,
    }
}
