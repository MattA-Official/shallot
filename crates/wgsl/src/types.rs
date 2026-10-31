//! WGSL types and their memory layout.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scalar {
    Bool,
    I32,
    U32,
    F32,
    F16,
    AbstractInt,
    AbstractFloat,
}

impl Scalar {
    pub fn is_abstract(self) -> bool {
        matches!(self, Scalar::AbstractInt | Scalar::AbstractFloat)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F16 | Scalar::AbstractFloat)
    }

    pub fn is_integer(self) -> bool {
        matches!(self, Scalar::I32 | Scalar::U32 | Scalar::AbstractInt)
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Scalar::I32 | Scalar::AbstractInt)
    }

    pub fn is_numeric(self) -> bool {
        self != Scalar::Bool
    }

    /// The concrete type an abstract scalar becomes when nothing else decides: `i32` or `f32`.
    pub fn concretize(self) -> Scalar {
        match self {
            Scalar::AbstractInt => Scalar::I32,
            Scalar::AbstractFloat => Scalar::F32,
            other => other,
        }
    }

    /// Whether an abstract value of type `self` converts automatically to `to`.
    pub fn converts_to(self, to: Scalar) -> bool {
        self == to
            || match self {
                Scalar::AbstractInt => to != Scalar::Bool,
                Scalar::AbstractFloat => matches!(to, Scalar::F32 | Scalar::F16),
                _ => false,
            }
    }

    pub fn size(self) -> u32 {
        match self {
            Scalar::F16 => 2,
            Scalar::AbstractInt | Scalar::AbstractFloat => 8,
            _ => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scalar::Bool => "bool",
            Scalar::I32 => "i32",
            Scalar::U32 => "u32",
            Scalar::F32 => "f32",
            Scalar::F16 => "f16",
            Scalar::AbstractInt => "abstract-int",
            Scalar::AbstractFloat => "abstract-float",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressSpace {
    Function,
    Private,
    Workgroup,
    Uniform,
    Storage,
    Handle,
}

impl AddressSpace {
    pub fn name(self) -> &'static str {
        match self {
            AddressSpace::Function => "function",
            AddressSpace::Private => "private",
            AddressSpace::Workgroup => "workgroup",
            AddressSpace::Uniform => "uniform",
            AddressSpace::Storage => "storage",
            AddressSpace::Handle => "handle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureDim {
    D1,
    D2,
    D3,
    Cube,
}

impl TextureDim {
    /// Components of a texel coordinate: cube textures are addressed by a direction.
    pub fn coordinates(self) -> u8 {
        match self {
            TextureDim::D1 => 1,
            TextureDim::D2 => 2,
            TextureDim::D3 | TextureDim::Cube => 3,
        }
    }

    /// Components of `textureDimensions`: a cube face's width and height.
    pub fn extent(self) -> u8 {
        match self {
            TextureDim::D1 => 1,
            TextureDim::D2 | TextureDim::Cube => 2,
            TextureDim::D3 => 3,
        }
    }
}

/// The channel type of a storage texel format, or `None` for a name WGSL does not define.
pub fn texel_format_scalar(format: &str) -> Option<Scalar> {
    Some(match format {
        "rgba8unorm" | "rgba8snorm" | "bgra8unorm" | "rgba16float" | "r32float" | "rg32float"
        | "rgba32float" | "r8unorm" | "rg8unorm" | "r16float" | "rg16float" | "rgb10a2unorm"
        | "rg11b10ufloat" => Scalar::F32,
        "rgba8sint" | "rgba16sint" | "r32sint" | "rg32sint" | "rgba32sint" | "r8sint"
        | "rg8sint" | "r16sint" | "rg16sint" => Scalar::I32,
        "rgba8uint" | "rgba16uint" | "r32uint" | "rg32uint" | "rgba32uint" | "r8uint"
        | "rg8uint" | "r16uint" | "rg16uint" | "rgb10a2uint" => Scalar::U32,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TextureKind {
    Sampled {
        sampled: Scalar,
        multisampled: bool,
    },
    Depth {
        multisampled: bool,
    },
    /// The texel format is kept as its WGSL name, such as `rgba8unorm`.
    Storage {
        format: String,
        access: Access,
    },
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    Scalar(Scalar),
    Vector(u8, Scalar),
    Matrix {
        columns: u8,
        rows: u8,
        scalar: Scalar,
    },
    /// `count` is `None` for runtime-sized arrays.
    Array {
        element: Box<Type>,
        count: Option<u32>,
    },
    /// Index into [`crate::ir::Module::structs`].
    Struct(usize),
    Atomic(Scalar),
    Pointer {
        space: AddressSpace,
        pointee: Box<Type>,
        access: Access,
    },
    Sampler {
        comparison: bool,
    },
    Texture {
        dim: TextureDim,
        arrayed: bool,
        kind: TextureKind,
    },
}

pub const BOOL: Type = Type::Scalar(Scalar::Bool);
pub const I32: Type = Type::Scalar(Scalar::I32);
pub const U32: Type = Type::Scalar(Scalar::U32);
pub const F32: Type = Type::Scalar(Scalar::F32);

impl Type {
    pub fn scalar(&self) -> Option<Scalar> {
        match self {
            Type::Scalar(s) | Type::Vector(_, s) | Type::Atomic(s) => Some(*s),
            Type::Matrix { scalar, .. } => Some(*scalar),
            _ => None,
        }
    }

    /// The scalar type at the leaves of a scalar, vector, matrix or array.
    pub fn leaf_scalar(&self) -> Option<Scalar> {
        match self {
            Type::Array { element, .. } => element.leaf_scalar(),
            other => other.scalar(),
        }
    }

    pub fn is_abstract(&self) -> bool {
        self.leaf_scalar().is_some_and(Scalar::is_abstract)
    }

    pub fn vector_size(&self) -> Option<u8> {
        match self {
            Type::Vector(n, _) => Some(*n),
            _ => None,
        }
    }

    /// Replaces the leaf scalar, keeping the shape: `vec3<abstract-float>` to `vec3<f32>`.
    pub fn with_scalar(&self, scalar: Scalar) -> Type {
        match self {
            Type::Scalar(_) => Type::Scalar(scalar),
            Type::Vector(n, _) => Type::Vector(*n, scalar),
            Type::Matrix { columns, rows, .. } => Type::Matrix {
                columns: *columns,
                rows: *rows,
                scalar,
            },
            Type::Array { element, count } => Type::Array {
                element: Box::new(element.with_scalar(scalar)),
                count: *count,
            },
            other => other.clone(),
        }
    }

    pub fn concretize(&self) -> Type {
        match self.leaf_scalar() {
            Some(s) if s.is_abstract() => self.with_scalar(s.concretize()),
            _ => self.clone(),
        }
    }

    /// Constructible types: those with a zero value that can be built and copied.
    pub fn is_constructible(&self, structs: &[StructType]) -> bool {
        match self {
            Type::Scalar(_) | Type::Vector(..) | Type::Matrix { .. } => true,
            Type::Array { element, count } => count.is_some() && element.is_constructible(structs),
            Type::Struct(i) => structs[*i]
                .members
                .iter()
                .all(|m| m.ty.is_constructible(structs)),
            _ => false,
        }
    }

    /// Host-shareable types can live in uniform and storage buffers.
    pub fn is_host_shareable(&self, structs: &[StructType]) -> bool {
        match self {
            Type::Scalar(s) | Type::Vector(_, s) | Type::Matrix { scalar: s, .. } => {
                s.is_numeric() && !s.is_abstract()
            }
            Type::Atomic(_) => true,
            Type::Array { element, .. } => element.is_host_shareable(structs),
            Type::Struct(i) => structs[*i]
                .members
                .iter()
                .all(|m| m.ty.is_host_shareable(structs)),
            _ => false,
        }
    }

    pub fn display<'a>(&'a self, structs: &'a [StructType]) -> TypeDisplay<'a> {
        TypeDisplay { ty: self, structs }
    }
}

pub struct TypeDisplay<'a> {
    ty: &'a Type,
    structs: &'a [StructType],
}

impl fmt::Display for TypeDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let structs = self.structs;
        match self.ty {
            Type::Void => f.write_str("void"),
            Type::Scalar(s) => f.write_str(s.name()),
            Type::Vector(n, s) => write!(f, "vec{n}<{}>", s.name()),
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => write!(f, "mat{columns}x{rows}<{}>", scalar.name()),
            Type::Array {
                element,
                count: Some(n),
            } => write!(f, "array<{}, {n}>", element.display(structs)),
            Type::Array {
                element,
                count: None,
            } => write!(f, "array<{}>", element.display(structs)),
            Type::Struct(i) => f.write_str(&structs[*i].name),
            Type::Atomic(s) => write!(f, "atomic<{}>", s.name()),
            Type::Pointer { space, pointee, .. } => {
                write!(f, "ptr<{}, {}>", space.name(), pointee.display(structs))
            }
            Type::Sampler { comparison: false } => f.write_str("sampler"),
            Type::Sampler { comparison: true } => f.write_str("sampler_comparison"),
            Type::Texture { dim, arrayed, kind } => {
                let dim = match dim {
                    TextureDim::D1 => "1d",
                    TextureDim::D2 => "2d",
                    TextureDim::D3 => "3d",
                    TextureDim::Cube => "cube",
                };
                let array = if *arrayed { "_array" } else { "" };
                match kind {
                    TextureKind::Sampled {
                        sampled,
                        multisampled: false,
                    } => {
                        write!(f, "texture_{dim}{array}<{}>", sampled.name())
                    }
                    TextureKind::Sampled {
                        sampled,
                        multisampled: true,
                    } => {
                        write!(f, "texture_multisampled_{dim}<{}>", sampled.name())
                    }
                    TextureKind::Depth {
                        multisampled: false,
                    } => write!(f, "texture_depth_{dim}{array}"),
                    TextureKind::Depth { multisampled: true } => {
                        write!(f, "texture_depth_multisampled_{dim}")
                    }
                    TextureKind::Storage { format, .. } => {
                        write!(f, "texture_storage_{dim}{array}<{format}>")
                    }
                    TextureKind::External => f.write_str("texture_external"),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructType {
    pub name: String,
    pub members: Vec<StructMember>,
    /// Alignment and size from WGSL's layout rules (only meaningful when host-shareable).
    pub align: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructMember {
    pub name: String,
    pub ty: Type,
    pub offset: u32,
    pub builtin: Option<Builtin>,
    pub location: Option<u32>,
    pub interpolation: Option<Interpolation>,
}

/// `@builtin` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    VertexIndex,
    InstanceIndex,
    Position,
    FrontFacing,
    FragDepth,
    SampleIndex,
    SampleMask,
    LocalInvocationId,
    LocalInvocationIndex,
    GlobalInvocationId,
    WorkgroupId,
    NumWorkgroups,
}

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "vertex_index" => Builtin::VertexIndex,
            "instance_index" => Builtin::InstanceIndex,
            "position" => Builtin::Position,
            "front_facing" => Builtin::FrontFacing,
            "frag_depth" => Builtin::FragDepth,
            "sample_index" => Builtin::SampleIndex,
            "sample_mask" => Builtin::SampleMask,
            "local_invocation_id" => Builtin::LocalInvocationId,
            "local_invocation_index" => Builtin::LocalInvocationIndex,
            "global_invocation_id" => Builtin::GlobalInvocationId,
            "workgroup_id" => Builtin::WorkgroupId,
            "num_workgroups" => Builtin::NumWorkgroups,
            _ => return None,
        })
    }

    /// The type WGSL requires for this builtin.
    pub fn ty(self) -> Type {
        match self {
            Builtin::VertexIndex
            | Builtin::InstanceIndex
            | Builtin::SampleIndex
            | Builtin::SampleMask
            | Builtin::LocalInvocationIndex => U32,
            Builtin::Position => Type::Vector(4, Scalar::F32),
            Builtin::FrontFacing => BOOL,
            Builtin::FragDepth => F32,
            Builtin::LocalInvocationId
            | Builtin::GlobalInvocationId
            | Builtin::WorkgroupId
            | Builtin::NumWorkgroups => Type::Vector(3, Scalar::U32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interpolation {
    Perspective(Sampling),
    Linear(Sampling),
    Flat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sampling {
    Center,
    Centroid,
    Sample,
}

fn round_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}

/// WGSL `AlignOf` and `SizeOf`.
pub fn align_and_size(ty: &Type, structs: &[StructType]) -> (u32, u32) {
    match ty {
        Type::Scalar(s) | Type::Atomic(s) => (s.size(), s.size()),
        Type::Vector(n, s) => {
            let size = s.size() * *n as u32;
            let align = if *n == 3 { s.size() * 4 } else { size };
            (align, size)
        }
        Type::Matrix {
            columns,
            rows,
            scalar,
        } => {
            let (align, _) = align_and_size(&Type::Vector(*rows, *scalar), structs);
            (align, align * *columns as u32)
        }
        Type::Array { element, count } => {
            let (align, size) = align_and_size(element, structs);
            let stride = round_up(size, align);
            (align, stride * count.unwrap_or(1))
        }
        Type::Struct(i) => (structs[*i].align, structs[*i].size),
        _ => (1, 0),
    }
}

/// The byte distance between consecutive array elements.
pub fn array_stride(element: &Type, structs: &[StructType]) -> u32 {
    let (align, size) = align_and_size(element, structs);
    round_up(size, align)
}

/// Lays out struct members: each member's offset and the struct's alignment and size.
/// `explicit` holds each member's `@align` and `@size` attributes.
pub fn layout_struct(
    members: &mut [StructMember],
    explicit: &[(Option<u32>, Option<u32>)],
    structs: &[StructType],
) -> (u32, u32) {
    let mut offset = 0;
    let mut struct_align = 1;
    for (member, &(align_attr, size_attr)) in members.iter_mut().zip(explicit) {
        let (natural_align, natural_size) = align_and_size(&member.ty, structs);
        let align = align_attr.unwrap_or(natural_align);
        let size = size_attr.unwrap_or(natural_size);
        offset = round_up(offset, align);
        member.offset = offset;
        offset += size;
        struct_align = struct_align.max(align);
    }
    (struct_align, round_up(offset.max(1), struct_align))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(ty: Type) -> StructMember {
        StructMember {
            name: String::new(),
            ty,
            offset: 0,
            builtin: None,
            location: None,
            interpolation: None,
        }
    }

    #[test]
    fn vec3_aligns_to_sixteen() {
        assert_eq!(align_and_size(&Type::Vector(3, Scalar::F32), &[]), (16, 12));
        assert_eq!(align_and_size(&Type::Vector(2, Scalar::F32), &[]), (8, 8));
        let mat = Type::Matrix {
            columns: 3,
            rows: 3,
            scalar: Scalar::F32,
        };
        assert_eq!(align_and_size(&mat, &[]), (16, 48));
    }

    #[test]
    fn struct_layout_follows_wgsl_rules() {
        // struct { a: f32, b: vec3f, c: f32, d: array<vec2f, 2> }
        let mut members = vec![
            member(F32),
            member(Type::Vector(3, Scalar::F32)),
            member(F32),
            member(Type::Array {
                element: Box::new(Type::Vector(2, Scalar::F32)),
                count: Some(2),
            }),
        ];
        let (align, size) = layout_struct(&mut members, &[(None, None); 4], &[]);
        let offsets: Vec<u32> = members.iter().map(|m| m.offset).collect();
        assert_eq!(offsets, vec![0, 16, 28, 32]);
        assert_eq!((align, size), (16, 48));
    }
}
