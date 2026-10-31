//! SPIR-V 1.3 back end for Vulkan.

use crate::ast::{BinaryOp, UnaryOp};
use crate::consts::ConstValue;
use crate::ir::*;
use crate::types::*;
use crate::{Error, Span};
use std::collections::{BTreeSet, HashMap};

type Result<T> = std::result::Result<T, Error>;

/// Options for [`Module::to_spirv`].
#[derive(Debug, Clone)]
pub struct SpirvOptions {
    /// Emit `OpName` and `OpMemberName` for functions, variables and structs.
    pub debug_names: bool,
}

impl Default for SpirvOptions {
    fn default() -> Self {
        Self { debug_names: true }
    }
}

impl Module {
    /// Lowers the whole module, every entry point included, to one SPIR-V binary. Each entry
    /// point keeps its WGSL name.
    pub fn to_spirv(&self, options: &SpirvOptions) -> Result<Vec<u32>> {
        Writer::new(self, options).write()
    }
}

/// Every type a uniform or storage buffer holds, through struct members and array elements.
/// A type that is also used elsewhere (a workgroup array of the same type, a local copy of a
/// buffer struct) shares the laid-out SPIR-V type; separating those needs a second, layout-free
/// type and member-wise copies, which this writer does not do yet.
fn laid_out_types(module: &Module) -> std::collections::HashSet<Type> {
    fn visit(ty: &Type, module: &Module, out: &mut std::collections::HashSet<Type>) {
        let ty = if ty.is_abstract() {
            ty.concretize()
        } else {
            ty.clone()
        };
        match &ty {
            Type::Array { element, .. } => visit(element, module, out),
            Type::Struct(i) => {
                for member in &module.structs[*i].members {
                    visit(&member.ty, module, out);
                }
            }
            _ => {}
        }
        out.insert(ty);
    }
    let mut out = std::collections::HashSet::new();
    for global in &module.globals {
        if matches!(global.space, AddressSpace::Uniform | AddressSpace::Storage) {
            visit(&global.ty, module, &mut out);
        }
    }
    out
}

fn unsupported<T>(what: &str) -> Result<T> {
    Err(Error::new(
        format!("{what} is not supported by the SPIR-V back end yet"),
        Span::default(),
    ))
}

mod op {
    pub const NAME: u16 = 5;
    pub const MEMBER_NAME: u16 = 6;
    pub const EXT_INST_IMPORT: u16 = 11;
    pub const EXT_INST: u16 = 12;
    pub const MEMORY_MODEL: u16 = 14;
    pub const ENTRY_POINT: u16 = 15;
    pub const EXECUTION_MODE: u16 = 16;
    pub const CAPABILITY: u16 = 17;
    pub const TYPE_VOID: u16 = 19;
    pub const TYPE_BOOL: u16 = 20;
    pub const TYPE_INT: u16 = 21;
    pub const TYPE_FLOAT: u16 = 22;
    pub const TYPE_VECTOR: u16 = 23;
    pub const TYPE_MATRIX: u16 = 24;
    pub const TYPE_ARRAY: u16 = 28;
    pub const TYPE_RUNTIME_ARRAY: u16 = 29;
    pub const TYPE_STRUCT: u16 = 30;
    pub const TYPE_IMAGE: u16 = 25;
    pub const TYPE_SAMPLER: u16 = 26;
    pub const TYPE_SAMPLED_IMAGE: u16 = 27;
    pub const TYPE_POINTER: u16 = 32;
    pub const TYPE_FUNCTION: u16 = 33;
    pub const CONSTANT_TRUE: u16 = 41;
    pub const CONSTANT_FALSE: u16 = 42;
    pub const CONSTANT: u16 = 43;
    pub const CONSTANT_COMPOSITE: u16 = 44;
    pub const FUNCTION: u16 = 54;
    pub const FUNCTION_PARAMETER: u16 = 55;
    pub const FUNCTION_END: u16 = 56;
    pub const FUNCTION_CALL: u16 = 57;
    pub const VARIABLE: u16 = 59;
    pub const LOAD: u16 = 61;
    pub const STORE: u16 = 62;
    pub const ACCESS_CHAIN: u16 = 65;
    pub const ARRAY_LENGTH: u16 = 68;
    pub const DECORATE: u16 = 71;
    pub const MEMBER_DECORATE: u16 = 72;
    pub const VECTOR_EXTRACT_DYNAMIC: u16 = 77;
    pub const VECTOR_SHUFFLE: u16 = 79;
    pub const COMPOSITE_CONSTRUCT: u16 = 80;
    pub const COMPOSITE_EXTRACT: u16 = 81;
    pub const TRANSPOSE: u16 = 84;
    pub const SAMPLED_IMAGE: u16 = 86;
    pub const IMAGE_SAMPLE_IMPLICIT_LOD: u16 = 87;
    pub const IMAGE_SAMPLE_EXPLICIT_LOD: u16 = 88;
    pub const IMAGE_SAMPLE_DREF_IMPLICIT_LOD: u16 = 89;
    pub const IMAGE_SAMPLE_DREF_EXPLICIT_LOD: u16 = 90;
    pub const IMAGE_FETCH: u16 = 95;
    pub const IMAGE_GATHER: u16 = 96;
    pub const IMAGE_DREF_GATHER: u16 = 97;
    pub const IMAGE_READ: u16 = 98;
    pub const IMAGE_WRITE: u16 = 99;
    pub const IMAGE_QUERY_SIZE_LOD: u16 = 103;
    pub const IMAGE_QUERY_SIZE: u16 = 104;
    pub const IMAGE_QUERY_LEVELS: u16 = 106;
    pub const IMAGE_QUERY_SAMPLES: u16 = 107;
    pub const CONVERT_F_TO_U: u16 = 109;
    pub const CONVERT_F_TO_S: u16 = 110;
    pub const CONVERT_S_TO_F: u16 = 111;
    pub const CONVERT_U_TO_F: u16 = 112;
    pub const F_CONVERT: u16 = 115;
    pub const BITCAST: u16 = 124;
    pub const S_NEGATE: u16 = 126;
    pub const F_NEGATE: u16 = 127;
    pub const I_ADD: u16 = 128;
    pub const F_ADD: u16 = 129;
    pub const I_SUB: u16 = 130;
    pub const F_SUB: u16 = 131;
    pub const I_MUL: u16 = 132;
    pub const F_MUL: u16 = 133;
    pub const U_DIV: u16 = 134;
    pub const S_DIV: u16 = 135;
    pub const F_DIV: u16 = 136;
    pub const U_MOD: u16 = 137;
    pub const S_REM: u16 = 138;
    pub const F_REM: u16 = 140;
    pub const MATRIX_TIMES_SCALAR: u16 = 143;
    pub const VECTOR_TIMES_MATRIX: u16 = 144;
    pub const MATRIX_TIMES_VECTOR: u16 = 145;
    pub const MATRIX_TIMES_MATRIX: u16 = 146;
    pub const DOT: u16 = 148;
    pub const ANY: u16 = 154;
    pub const ALL: u16 = 155;
    pub const LOGICAL_EQUAL: u16 = 164;
    pub const LOGICAL_NOT_EQUAL: u16 = 165;
    pub const LOGICAL_OR: u16 = 166;
    pub const LOGICAL_AND: u16 = 167;
    pub const LOGICAL_NOT: u16 = 168;
    pub const SELECT: u16 = 169;
    pub const I_EQUAL: u16 = 170;
    pub const I_NOT_EQUAL: u16 = 171;
    pub const U_GREATER_THAN: u16 = 172;
    pub const S_GREATER_THAN: u16 = 173;
    pub const U_GREATER_THAN_EQUAL: u16 = 174;
    pub const S_GREATER_THAN_EQUAL: u16 = 175;
    pub const U_LESS_THAN: u16 = 176;
    pub const S_LESS_THAN: u16 = 177;
    pub const U_LESS_THAN_EQUAL: u16 = 178;
    pub const S_LESS_THAN_EQUAL: u16 = 179;
    pub const F_ORD_EQUAL: u16 = 180;
    pub const F_UNORD_NOT_EQUAL: u16 = 183;
    pub const F_ORD_LESS_THAN: u16 = 184;
    pub const F_ORD_GREATER_THAN: u16 = 186;
    pub const F_ORD_LESS_THAN_EQUAL: u16 = 188;
    pub const F_ORD_GREATER_THAN_EQUAL: u16 = 190;
    pub const SHIFT_RIGHT_LOGICAL: u16 = 194;
    pub const SHIFT_RIGHT_ARITHMETIC: u16 = 195;
    pub const SHIFT_LEFT_LOGICAL: u16 = 196;
    pub const BITWISE_OR: u16 = 197;
    pub const BITWISE_XOR: u16 = 198;
    pub const BITWISE_AND: u16 = 199;
    pub const NOT: u16 = 200;
    pub const BIT_FIELD_INSERT: u16 = 201;
    pub const BIT_FIELD_S_EXTRACT: u16 = 202;
    pub const BIT_FIELD_U_EXTRACT: u16 = 203;
    pub const BIT_REVERSE: u16 = 204;
    pub const BIT_COUNT: u16 = 205;
    pub const DPDX: u16 = 207;
    pub const DPDY: u16 = 208;
    pub const FWIDTH: u16 = 209;
    pub const CONTROL_BARRIER: u16 = 224;
    pub const PHI: u16 = 245;
    pub const LOOP_MERGE: u16 = 246;
    pub const SELECTION_MERGE: u16 = 247;
    pub const LABEL: u16 = 248;
    pub const BRANCH: u16 = 249;
    pub const BRANCH_CONDITIONAL: u16 = 250;
    pub const SWITCH: u16 = 251;
    pub const KILL: u16 = 252;
    pub const RETURN: u16 = 253;
    pub const RETURN_VALUE: u16 = 254;
    pub const UNREACHABLE: u16 = 255;
}

mod glsl {
    pub const ROUND_EVEN: u32 = 2;
    pub const TRUNC: u32 = 3;
    pub const F_ABS: u32 = 4;
    pub const S_ABS: u32 = 5;
    pub const F_SIGN: u32 = 6;
    pub const S_SIGN: u32 = 7;
    pub const FLOOR: u32 = 8;
    pub const CEIL: u32 = 9;
    pub const FRACT: u32 = 10;
    pub const RADIANS: u32 = 11;
    pub const DEGREES: u32 = 12;
    pub const SIN: u32 = 13;
    pub const COS: u32 = 14;
    pub const TAN: u32 = 15;
    pub const ASIN: u32 = 16;
    pub const ACOS: u32 = 17;
    pub const ATAN: u32 = 18;
    pub const SINH: u32 = 19;
    pub const COSH: u32 = 20;
    pub const TANH: u32 = 21;
    pub const ASINH: u32 = 22;
    pub const ACOSH: u32 = 23;
    pub const ATANH: u32 = 24;
    pub const ATAN2: u32 = 25;
    pub const POW: u32 = 26;
    pub const EXP: u32 = 27;
    pub const LOG: u32 = 28;
    pub const EXP2: u32 = 29;
    pub const LOG2: u32 = 30;
    pub const SQRT: u32 = 31;
    pub const INVERSE_SQRT: u32 = 32;
    pub const DETERMINANT: u32 = 33;
    pub const F_MIN: u32 = 37;
    pub const U_MIN: u32 = 38;
    pub const S_MIN: u32 = 39;
    pub const F_MAX: u32 = 40;
    pub const U_MAX: u32 = 41;
    pub const S_MAX: u32 = 42;
    pub const F_CLAMP: u32 = 43;
    pub const U_CLAMP: u32 = 44;
    pub const S_CLAMP: u32 = 45;
    pub const F_MIX: u32 = 46;
    pub const STEP: u32 = 48;
    pub const SMOOTH_STEP: u32 = 49;
    pub const FMA: u32 = 50;
    pub const PACK_SNORM_4X8: u32 = 54;
    pub const PACK_UNORM_4X8: u32 = 55;
    pub const PACK_SNORM_2X16: u32 = 56;
    pub const PACK_UNORM_2X16: u32 = 57;
    pub const PACK_HALF_2X16: u32 = 58;
    pub const UNPACK_SNORM_2X16: u32 = 60;
    pub const UNPACK_UNORM_2X16: u32 = 61;
    pub const UNPACK_HALF_2X16: u32 = 62;
    pub const UNPACK_SNORM_4X8: u32 = 63;
    pub const UNPACK_UNORM_4X8: u32 = 64;
    pub const LENGTH: u32 = 66;
    pub const DISTANCE: u32 = 67;
    pub const CROSS: u32 = 68;
    pub const NORMALIZE: u32 = 69;
    pub const FACE_FORWARD: u32 = 70;
    pub const REFLECT: u32 = 71;
    pub const REFRACT: u32 = 72;
    pub const FIND_I_LSB: u32 = 73;
    pub const FIND_S_MSB: u32 = 74;
    pub const FIND_U_MSB: u32 = 75;
}

mod class {
    pub const UNIFORM_CONSTANT: u32 = 0;
    pub const INPUT: u32 = 1;
    pub const UNIFORM: u32 = 2;
    pub const OUTPUT: u32 = 3;
    pub const WORKGROUP: u32 = 4;
    pub const PRIVATE: u32 = 6;
    pub const FUNCTION: u32 = 7;
    pub const STORAGE_BUFFER: u32 = 12;
}

mod decoration {
    pub const BLOCK: u32 = 2;
    pub const COL_MAJOR: u32 = 5;
    pub const ARRAY_STRIDE: u32 = 6;
    pub const MATRIX_STRIDE: u32 = 7;
    pub const BUILT_IN: u32 = 11;
    pub const NO_PERSPECTIVE: u32 = 13;
    pub const FLAT: u32 = 14;
    pub const CENTROID: u32 = 16;
    pub const SAMPLE: u32 = 17;
    pub const NON_WRITABLE: u32 = 24;
    pub const NON_READABLE: u32 = 25;
    pub const LOCATION: u32 = 30;
    pub const BINDING: u32 = 33;
    pub const DESCRIPTOR_SET: u32 = 34;
    pub const OFFSET: u32 = 35;
}

mod builtin {
    pub const POSITION: u32 = 0;
    pub const FRAG_COORD: u32 = 15;
    pub const FRONT_FACING: u32 = 17;
    pub const SAMPLE_ID: u32 = 18;
    pub const SAMPLE_MASK: u32 = 20;
    pub const FRAG_DEPTH: u32 = 22;
    pub const NUM_WORKGROUPS: u32 = 24;
    pub const WORKGROUP_ID: u32 = 26;
    pub const LOCAL_INVOCATION_ID: u32 = 27;
    pub const GLOBAL_INVOCATION_ID: u32 = 28;
    pub const LOCAL_INVOCATION_INDEX: u32 = 29;
    pub const VERTEX_INDEX: u32 = 42;
    pub const INSTANCE_INDEX: u32 = 43;
}

const CAPABILITY_SHADER: u32 = 1;
const CAPABILITY_IMAGE_GATHER_EXTENDED: u32 = 25;
const CAPABILITY_SAMPLE_RATE_SHADING: u32 = 35;
const CAPABILITY_SAMPLED_1D: u32 = 43;
const CAPABILITY_IMAGE_1D: u32 = 44;
const CAPABILITY_SAMPLED_CUBE_ARRAY: u32 = 45;
const CAPABILITY_STORAGE_IMAGE_EXTENDED_FORMATS: u32 = 49;
const CAPABILITY_IMAGE_QUERY: u32 = 50;
const CAPABILITY_STORAGE_IMAGE_READ_WITHOUT_FORMAT: u32 = 55;
const CAPABILITY_STORAGE_IMAGE_WRITE_WITHOUT_FORMAT: u32 = 56;

/// Image operand mask bits, in the order their operands follow.
mod image_operand {
    pub const BIAS: u32 = 0x1;
    pub const LOD: u32 = 0x2;
    pub const GRAD: u32 = 0x4;
    pub const CONST_OFFSET: u32 = 0x8;
    pub const SAMPLE: u32 = 0x40;
}

/// The SPIR-V `ImageFormat` of a WGSL storage texel format, and whether it needs the
/// `StorageImageExtendedFormats` capability. `bgra8unorm` has no SPIR-V format and is declared
/// `Unknown`, read and written without a format.
fn image_format(format: &str) -> (u32, bool) {
    match format {
        "rgba32float" => (1, false),
        "rgba16float" => (2, false),
        "r32float" => (3, false),
        "rgba8unorm" => (4, false),
        "rgba8snorm" => (5, false),
        "rg32float" => (6, true),
        "rg16float" => (7, true),
        "rg11b10ufloat" => (8, true),
        "r16float" => (9, true),
        "rgb10a2unorm" => (11, true),
        "rg8unorm" => (13, true),
        "r8unorm" => (15, true),
        "rgba32sint" => (21, false),
        "rgba16sint" => (22, false),
        "rgba8sint" => (23, false),
        "r32sint" => (24, false),
        "rg32sint" => (25, true),
        "rg16sint" => (26, true),
        "rg8sint" => (27, true),
        "r16sint" => (28, true),
        "r8sint" => (29, true),
        "rgba32uint" => (30, false),
        "rgba16uint" => (31, false),
        "rgba8uint" => (32, false),
        "r32uint" => (33, false),
        "rgb10a2uint" => (34, true),
        "rg32uint" => (35, true),
        "rg16uint" => (36, true),
        "rg8uint" => (37, true),
        "r16uint" => (38, true),
        "r8uint" => (39, true),
        _ => (0, false),
    }
}

fn encode(buf: &mut Vec<u32>, opcode: u16, operands: &[u32]) {
    buf.push(((operands.len() as u32 + 1) << 16) | opcode as u32);
    buf.extend_from_slice(operands);
}

fn string_words(s: &str) -> Vec<u32> {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    bytes
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn storage_class(space: AddressSpace) -> u32 {
    match space {
        AddressSpace::Function => class::FUNCTION,
        AddressSpace::Private => class::PRIVATE,
        AddressSpace::Workgroup => class::WORKGROUP,
        AddressSpace::Uniform => class::UNIFORM,
        AddressSpace::Storage => class::STORAGE_BUFFER,
        AddressSpace::Handle => class::UNIFORM_CONSTANT,
    }
}

struct GlobalVar {
    id: u32,
    /// Buffer globals are wrapped in a `Block` struct; access goes through member 0.
    wrapped: bool,
}

/// A branch target for `break` together with whether anything branched to it.
struct Target {
    label: u32,
    used: bool,
}

#[derive(Default)]
struct FnState {
    vars: Vec<u32>,
    body: Vec<u32>,
    locals: Vec<u32>,
    params: Vec<u32>,
    current: u32,
    terminated: bool,
    breaks: Vec<Target>,
    continues: Vec<u32>,
}

struct Writer<'m> {
    module: &'m Module,
    options: &'m SpirvOptions,
    next_id: u32,
    capabilities: BTreeSet<u32>,
    glsl: u32,
    entry_points: Vec<u32>,
    execution_modes: Vec<u32>,
    debug: Vec<u32>,
    annotations: Vec<u32>,
    declarations: Vec<u32>,
    functions: Vec<u32>,
    types: HashMap<Type, u32>,
    pointers: HashMap<(u32, u32), u32>,
    function_types: HashMap<Vec<u32>, u32>,
    constants: HashMap<(u16, Vec<u32>), u32>,
    globals: Vec<GlobalVar>,
    function_ids: Vec<u32>,
    current_fn: usize,
    /// Struct types already decorated `Block`.
    blocks: BTreeSet<u32>,
    /// Types reachable from uniform and storage buffers, the only ones given an explicit
    /// layout: Vulkan forbids `Offset` and `ArrayStride` on types in other storage classes.
    laid_out: std::collections::HashSet<Type>,
    /// `OpTypeSampledImage` for each image type.
    sampled_images: HashMap<u32, u32>,
    f: FnState,
}

impl<'m> Writer<'m> {
    fn new(module: &'m Module, options: &'m SpirvOptions) -> Self {
        Writer {
            module,
            options,
            next_id: 1,
            capabilities: BTreeSet::from([CAPABILITY_SHADER]),
            glsl: 0,
            entry_points: Vec::new(),
            execution_modes: Vec::new(),
            debug: Vec::new(),
            annotations: Vec::new(),
            declarations: Vec::new(),
            functions: Vec::new(),
            types: HashMap::new(),
            pointers: HashMap::new(),
            function_types: HashMap::new(),
            constants: HashMap::new(),
            globals: Vec::new(),
            function_ids: Vec::new(),
            current_fn: 0,
            blocks: BTreeSet::new(),
            laid_out: laid_out_types(module),
            sampled_images: HashMap::new(),
            f: FnState::default(),
        }
    }

    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn name(&mut self, id: u32, name: &str) {
        if self.options.debug_names {
            let mut operands = vec![id];
            operands.extend(string_words(name));
            encode(&mut self.debug, op::NAME, &operands);
        }
    }

    fn decorate(&mut self, id: u32, decoration: u32, args: &[u32]) {
        let mut operands = vec![id, decoration];
        operands.extend_from_slice(args);
        encode(&mut self.annotations, op::DECORATE, &operands);
    }

    fn member_decorate(&mut self, id: u32, member: u32, decoration: u32, args: &[u32]) {
        let mut operands = vec![id, member, decoration];
        operands.extend_from_slice(args);
        encode(&mut self.annotations, op::MEMBER_DECORATE, &operands);
    }

    fn write(mut self) -> Result<Vec<u32>> {
        self.glsl = self.id();
        for _ in &self.module.functions {
            let id = self.id();
            self.function_ids.push(id);
        }
        for (i, global) in self.module.globals.iter().enumerate() {
            self.global(i, global)?;
        }
        for i in 0..self.module.functions.len() {
            self.function(i)?;
        }
        for entry in &self.module.entry_points {
            self.entry_point(entry)?;
        }

        let mut out = vec![0x0723_0203, 0x0001_0300, 0, self.next_id, 0];
        for &cap in &self.capabilities {
            encode(&mut out, op::CAPABILITY, &[cap]);
        }
        let mut import = vec![self.glsl];
        import.extend(string_words("GLSL.std.450"));
        encode(&mut out, op::EXT_INST_IMPORT, &import);
        encode(&mut out, op::MEMORY_MODEL, &[0, 1]);
        out.extend(&self.entry_points);
        out.extend(&self.execution_modes);
        out.extend(&self.debug);
        out.extend(&self.annotations);
        out.extend(&self.declarations);
        out.extend(&self.functions);
        Ok(out)
    }

    // ---------------------------------------------------------------------------------------
    // Types and constants

    fn type_id(&mut self, ty: &Type) -> Result<u32> {
        let ty = if ty.is_abstract() {
            ty.concretize()
        } else {
            ty.clone()
        };
        if let Type::Atomic(s) = &ty {
            return self.type_id(&Type::Scalar(*s));
        }
        // WGSL types that differ only in what SPIR-V does not encode share one declaration.
        match &ty {
            Type::Sampler { comparison: true } => {
                return self.type_id(&Type::Sampler { comparison: false })
            }
            Type::Texture {
                dim,
                arrayed,
                kind: TextureKind::Storage { format, access },
            } if *access != Access::ReadWrite => {
                return self.type_id(&Type::Texture {
                    dim: *dim,
                    arrayed: *arrayed,
                    kind: TextureKind::Storage {
                        format: format.clone(),
                        access: Access::ReadWrite,
                    },
                })
            }
            _ => {}
        }
        if let Some(&id) = self.types.get(&ty) {
            return Ok(id);
        }
        let id = match &ty {
            Type::Void => {
                let id = self.id();
                encode(&mut self.declarations, op::TYPE_VOID, &[id]);
                id
            }
            Type::Scalar(s) => {
                let id = self.id();
                match s {
                    Scalar::Bool => encode(&mut self.declarations, op::TYPE_BOOL, &[id]),
                    Scalar::I32 => encode(&mut self.declarations, op::TYPE_INT, &[id, 32, 1]),
                    Scalar::U32 => encode(&mut self.declarations, op::TYPE_INT, &[id, 32, 0]),
                    Scalar::F32 => encode(&mut self.declarations, op::TYPE_FLOAT, &[id, 32]),
                    Scalar::F16 => return unsupported("f16"),
                    _ => unreachable!(),
                }
                id
            }
            Type::Vector(n, s) => {
                let scalar = self.type_id(&Type::Scalar(*s))?;
                let id = self.id();
                encode(
                    &mut self.declarations,
                    op::TYPE_VECTOR,
                    &[id, scalar, *n as u32],
                );
                id
            }
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => {
                let column = self.type_id(&Type::Vector(*rows, *scalar))?;
                let id = self.id();
                encode(
                    &mut self.declarations,
                    op::TYPE_MATRIX,
                    &[id, column, *columns as u32],
                );
                id
            }
            Type::Array { element, count } => {
                let element_id = self.type_id(element)?;
                let id = self.id();
                match count {
                    Some(n) => {
                        let length = self.constant(&ConstValue::Int(*n as i64), &U32)?;
                        encode(
                            &mut self.declarations,
                            op::TYPE_ARRAY,
                            &[id, element_id, length],
                        );
                    }
                    None => encode(
                        &mut self.declarations,
                        op::TYPE_RUNTIME_ARRAY,
                        &[id, element_id],
                    ),
                }
                if self.laid_out.contains(&ty) {
                    let stride = array_stride(element, &self.module.structs);
                    self.decorate(id, decoration::ARRAY_STRIDE, &[stride]);
                }
                id
            }
            Type::Struct(i) => {
                let st = &self.module.structs[*i];
                let members: Vec<u32> = st
                    .members
                    .iter()
                    .map(|m| self.type_id(&m.ty))
                    .collect::<Result<_>>()?;
                let id = self.id();
                let mut operands = vec![id];
                operands.extend(&members);
                encode(&mut self.declarations, op::TYPE_STRUCT, &operands);
                self.name(id, &st.name.clone());
                let shareable = self.laid_out.contains(&ty);
                for (m, member) in st.members.iter().enumerate() {
                    if self.options.debug_names {
                        let mut operands = vec![id, m as u32];
                        operands.extend(string_words(&member.name));
                        encode(&mut self.debug, op::MEMBER_NAME, &operands);
                    }
                    if shareable {
                        self.member_layout(id, m as u32, &member.ty, member.offset);
                    }
                }
                id
            }
            Type::Pointer { space, pointee, .. } => {
                let pointee = self.type_id(pointee)?;
                self.pointer(storage_class(*space), pointee)
            }
            Type::Sampler { .. } => {
                let id = self.id();
                encode(&mut self.declarations, op::TYPE_SAMPLER, &[id]);
                id
            }
            Type::Texture { dim, arrayed, kind } => self.image_type(*dim, *arrayed, kind)?,
            Type::Atomic(_) => unreachable!(),
        };
        self.types.insert(ty, id);
        Ok(id)
    }

    fn member_layout(&mut self, struct_id: u32, member: u32, ty: &Type, offset: u32) {
        self.member_decorate(struct_id, member, decoration::OFFSET, &[offset]);
        let mut inner = ty;
        while let Type::Array { element, .. } = inner {
            inner = element;
        }
        if let Type::Matrix { rows, scalar, .. } = inner {
            let stride = array_stride(&Type::Vector(*rows, *scalar), &self.module.structs);
            self.member_decorate(struct_id, member, decoration::COL_MAJOR, &[]);
            self.member_decorate(struct_id, member, decoration::MATRIX_STRIDE, &[stride]);
        }
    }

    fn pointer(&mut self, class: u32, pointee: u32) -> u32 {
        if let Some(&id) = self.pointers.get(&(class, pointee)) {
            return id;
        }
        let id = self.id();
        encode(
            &mut self.declarations,
            op::TYPE_POINTER,
            &[id, class, pointee],
        );
        self.pointers.insert((class, pointee), id);
        id
    }

    fn pointer_to(&mut self, space: AddressSpace, ty: &Type) -> Result<u32> {
        let pointee = self.type_id(ty)?;
        Ok(self.pointer(storage_class(space), pointee))
    }

    fn function_type(&mut self, ret: u32, params: &[u32]) -> u32 {
        let mut key = vec![ret];
        key.extend_from_slice(params);
        if let Some(&id) = self.function_types.get(&key) {
            return id;
        }
        let id = self.id();
        let mut operands = vec![id];
        operands.extend(&key);
        encode(&mut self.declarations, op::TYPE_FUNCTION, &operands);
        self.function_types.insert(key, id);
        id
    }

    fn cached_constant(&mut self, opcode: u16, ty: u32, words: &[u32]) -> u32 {
        let mut key = vec![ty];
        key.extend_from_slice(words);
        if let Some(&id) = self.constants.get(&(opcode, key.clone())) {
            return id;
        }
        let id = self.id();
        let mut operands = vec![ty, id];
        operands.extend_from_slice(words);
        encode(&mut self.declarations, opcode, &operands);
        self.constants.insert((opcode, key), id);
        id
    }

    fn constant(&mut self, value: &ConstValue, ty: &Type) -> Result<u32> {
        let ty = if ty.is_abstract() {
            ty.concretize()
        } else {
            ty.clone()
        };
        let type_id = self.type_id(&ty)?;
        match (&ty, value) {
            (Type::Scalar(Scalar::Bool), ConstValue::Bool(b)) => {
                let opcode = if *b {
                    op::CONSTANT_TRUE
                } else {
                    op::CONSTANT_FALSE
                };
                Ok(self.cached_constant(opcode, type_id, &[]))
            }
            (Type::Scalar(Scalar::F32), ConstValue::Float(v)) => {
                Ok(self.cached_constant(op::CONSTANT, type_id, &[(*v as f32).to_bits()]))
            }
            (Type::Scalar(Scalar::F32), ConstValue::Int(v)) => {
                Ok(self.cached_constant(op::CONSTANT, type_id, &[(*v as f32).to_bits()]))
            }
            (Type::Scalar(_), ConstValue::Int(v)) => {
                Ok(self.cached_constant(op::CONSTANT, type_id, &[*v as u32]))
            }
            (_, ConstValue::Composite(parts)) => {
                let element_types: Vec<Type> = match &ty {
                    Type::Vector(n, s) => vec![Type::Scalar(*s); *n as usize],
                    Type::Matrix {
                        columns,
                        rows,
                        scalar,
                    } => vec![Type::Vector(*rows, *scalar); *columns as usize],
                    Type::Array { element, count } => {
                        vec![(**element).clone(); count.unwrap_or(0) as usize]
                    }
                    Type::Struct(i) => self.module.structs[*i]
                        .members
                        .iter()
                        .map(|m| m.ty.clone())
                        .collect(),
                    _ => return unsupported("this constant"),
                };
                if element_types.len() != parts.len() {
                    return Err(Error::new("malformed composite constant", Span::default()));
                }
                let ids: Vec<u32> = parts
                    .iter()
                    .zip(&element_types)
                    .map(|(p, t)| self.constant(p, t))
                    .collect::<Result<_>>()?;
                Ok(self.cached_constant(op::CONSTANT_COMPOSITE, type_id, &ids))
            }
            _ => Err(Error::new("malformed constant", Span::default())),
        }
    }

    fn u32_const(&mut self, v: u32) -> u32 {
        self.constant(&ConstValue::Int(v as i64), &U32).unwrap()
    }

    // ---------------------------------------------------------------------------------------
    // Module scope

    fn global(&mut self, index: usize, global: &Global) -> Result<()> {
        let class = storage_class(global.space);
        let buffer = matches!(global.space, AddressSpace::Uniform | AddressSpace::Storage);
        let read_only = global.access == Access::Read && global.space == AddressSpace::Storage;
        // Structs ending in a runtime-sized array can only live in storage buffers, so they are
        // decorated as the block themselves; every other buffer type is wrapped in one.
        let direct_block = match &global.ty {
            Type::Struct(i) => {
                matches!(self.module.structs[*i].members.last(), Some(m) if matches!(m.ty, Type::Array { count: None, .. }))
            }
            _ => false,
        };
        let wrapped = buffer && !direct_block;
        let pointee = if wrapped {
            let inner = self.type_id(&global.ty)?;
            let block = self.id();
            encode(&mut self.declarations, op::TYPE_STRUCT, &[block, inner]);
            self.decorate(block, decoration::BLOCK, &[]);
            self.member_layout(block, 0, &global.ty, 0);
            if read_only {
                self.member_decorate(block, 0, decoration::NON_WRITABLE, &[]);
            }
            self.name(block, &format!("{}_block", global.name));
            block
        } else if direct_block {
            let id = self.type_id(&global.ty)?;
            if self.blocks.insert(id) {
                self.decorate(id, decoration::BLOCK, &[]);
            }
            if read_only {
                let Type::Struct(i) = &global.ty else {
                    unreachable!()
                };
                for m in 0..self.module.structs[*i].members.len() {
                    self.member_decorate(id, m as u32, decoration::NON_WRITABLE, &[]);
                }
            }
            id
        } else {
            self.type_id(&global.ty)?
        };
        let pointer = self.pointer(class, pointee);
        let init = match global.space {
            AddressSpace::Private => {
                let zero = ConstValue::zero(&global.ty, &self.module.structs);
                let value = global
                    .init
                    .as_ref()
                    .and_then(|e| e.as_const())
                    .unwrap_or(&zero)
                    .clone();
                Some(self.constant(&value, &global.ty)?)
            }
            _ => None,
        };
        let id = self.id();
        let mut operands = vec![pointer, id, class];
        operands.extend(init);
        encode(&mut self.declarations, op::VARIABLE, &operands);
        self.name(id, &global.name);
        if let Some((group, binding)) = global.group_binding {
            self.decorate(id, decoration::DESCRIPTOR_SET, &[group]);
            self.decorate(id, decoration::BINDING, &[binding]);
        }
        if let Type::Texture {
            kind: TextureKind::Storage { access, format },
            ..
        } = &global.ty
        {
            // Formats SPIR-V cannot name (bgra8unorm) are read and written without one.
            if image_format(format).0 == 0 {
                if *access != Access::Write {
                    self.capabilities
                        .insert(CAPABILITY_STORAGE_IMAGE_READ_WITHOUT_FORMAT);
                }
                if *access != Access::Read {
                    self.capabilities
                        .insert(CAPABILITY_STORAGE_IMAGE_WRITE_WITHOUT_FORMAT);
                }
            }
            match access {
                Access::Read => self.decorate(id, decoration::NON_WRITABLE, &[]),
                Access::Write => self.decorate(id, decoration::NON_READABLE, &[]),
                Access::ReadWrite => {}
            }
        }
        debug_assert_eq!(self.globals.len(), index);
        self.globals.push(GlobalVar { id, wrapped });
        Ok(())
    }

    fn function(&mut self, index: usize) -> Result<()> {
        let function = &self.module.functions[index];
        let ret = self.type_id(&function.ret)?;
        let param_types: Vec<u32> = function
            .params
            .iter()
            .map(|p| self.type_id(&p.ty))
            .collect::<Result<_>>()?;
        let fn_type = self.function_type(ret, &param_types);
        let id = self.function_ids[index];
        // An entry point's body is called by a wrapper that carries the WGSL name. Naming both
        // alike makes SPIRV-Cross (MoltenVK) emit two MSL functions of the same name and
        // parameters, which Metal rejects.
        if self.module.entry_points.iter().any(|e| e.function == index) {
            self.name(id, &format!("{}_body", function.name));
        } else {
            self.name(id, &function.name);
        }
        self.current_fn = index;

        let mut head = Vec::new();
        encode(&mut head, op::FUNCTION, &[ret, id, 0, fn_type]);
        self.f = FnState::default();
        for (p, &ty) in function.params.iter().zip(&param_types) {
            let param = self.id();
            encode(&mut head, op::FUNCTION_PARAMETER, &[ty, param]);
            self.name(param, &p.name);
            self.f.params.push(param);
        }
        let entry = self.id();
        encode(&mut head, op::LABEL, &[entry]);
        self.f.current = entry;

        for local in &function.locals {
            if local.mutable {
                let pointer = self.pointer_to(AddressSpace::Function, &local.ty)?;
                let var = self.id();
                encode(
                    &mut self.f.vars,
                    op::VARIABLE,
                    &[pointer, var, class::FUNCTION],
                );
                self.name(var, &local.name);
                self.f.locals.push(var);
            } else {
                self.f.locals.push(0);
            }
        }

        self.block(&function.body)?;
        if !self.f.terminated {
            if function.ret == Type::Void {
                encode(&mut self.f.body, op::RETURN, &[]);
            } else {
                encode(&mut self.f.body, op::UNREACHABLE, &[]);
            }
        }
        let state = std::mem::take(&mut self.f);
        self.functions.extend(head);
        self.functions.extend(state.vars);
        self.functions.extend(state.body);
        encode(&mut self.functions, op::FUNCTION_END, &[]);
        Ok(())
    }

    fn interface_var(&mut self, class: u32, ty: &Type, io: IoBinding, stage: Stage) -> Result<u32> {
        let sample_mask = io == IoBinding::Builtin(Builtin::SampleMask);
        let var_ty = if sample_mask {
            Type::Array {
                element: Box::new(U32),
                count: Some(1),
            }
        } else {
            ty.clone()
        };
        let pointee = self.type_id(&var_ty)?;
        let pointer = self.pointer(class, pointee);
        let id = self.id();
        encode(&mut self.declarations, op::VARIABLE, &[pointer, id, class]);
        match io {
            IoBinding::Builtin(b) => {
                let value = match b {
                    Builtin::Position if stage == Stage::Fragment => builtin::FRAG_COORD,
                    Builtin::Position => builtin::POSITION,
                    Builtin::VertexIndex => builtin::VERTEX_INDEX,
                    Builtin::InstanceIndex => builtin::INSTANCE_INDEX,
                    Builtin::FrontFacing => builtin::FRONT_FACING,
                    Builtin::FragDepth => builtin::FRAG_DEPTH,
                    Builtin::SampleIndex => {
                        self.capabilities.insert(CAPABILITY_SAMPLE_RATE_SHADING);
                        builtin::SAMPLE_ID
                    }
                    Builtin::SampleMask => builtin::SAMPLE_MASK,
                    Builtin::LocalInvocationId => builtin::LOCAL_INVOCATION_ID,
                    Builtin::LocalInvocationIndex => builtin::LOCAL_INVOCATION_INDEX,
                    Builtin::GlobalInvocationId => builtin::GLOBAL_INVOCATION_ID,
                    Builtin::WorkgroupId => builtin::WORKGROUP_ID,
                    Builtin::NumWorkgroups => builtin::NUM_WORKGROUPS,
                };
                self.decorate(id, decoration::BUILT_IN, &[value]);
                if b == Builtin::SampleIndex && class == class::INPUT {
                    self.decorate(id, decoration::FLAT, &[]);
                }
            }
            IoBinding::Location {
                location,
                interpolation,
            } => {
                self.decorate(id, decoration::LOCATION, &[location]);
                // Vertex inputs take no interpolation decorations.
                let varying = !(stage == Stage::Vertex && class == class::INPUT);
                if varying {
                    let integer = ty.scalar().is_some_and(|s| s.is_integer());
                    match interpolation {
                        Some(Interpolation::Flat) => self.decorate(id, decoration::FLAT, &[]),
                        _ if integer => self.decorate(id, decoration::FLAT, &[]),
                        Some(Interpolation::Linear(s)) => {
                            self.decorate(id, decoration::NO_PERSPECTIVE, &[]);
                            self.sampling(id, s);
                        }
                        Some(Interpolation::Perspective(s)) => self.sampling(id, s),
                        None => {}
                    }
                }
            }
        }
        Ok(id)
    }

    fn sampling(&mut self, id: u32, sampling: Sampling) {
        match sampling {
            Sampling::Center => {}
            Sampling::Centroid => self.decorate(id, decoration::CENTROID, &[]),
            Sampling::Sample => {
                self.capabilities.insert(CAPABILITY_SAMPLE_RATE_SHADING);
                self.decorate(id, decoration::SAMPLE, &[]);
            }
        }
    }

    /// Loads an interface variable, reading element 0 of the array `SampleMask` is declared as.
    fn load_interface(&mut self, var: u32, ty: &Type, io: IoBinding, class: u32) -> Result<u32> {
        let ty_id = self.type_id(ty)?;
        let pointer = if io == IoBinding::Builtin(Builtin::SampleMask) {
            let element = self.pointer(class, ty_id);
            let zero = self.u32_const(0);
            let id = self.id();
            encode(
                &mut self.f.body,
                op::ACCESS_CHAIN,
                &[element, id, var, zero],
            );
            id
        } else {
            var
        };
        let id = self.id();
        encode(&mut self.f.body, op::LOAD, &[ty_id, id, pointer]);
        Ok(id)
    }

    fn store_interface(&mut self, var: u32, value: u32, ty: &Type, io: IoBinding) -> Result<()> {
        let pointer = if io == IoBinding::Builtin(Builtin::SampleMask) {
            let ty_id = self.type_id(ty)?;
            let element = self.pointer(class::OUTPUT, ty_id);
            let zero = self.u32_const(0);
            let id = self.id();
            encode(
                &mut self.f.body,
                op::ACCESS_CHAIN,
                &[element, id, var, zero],
            );
            id
        } else {
            var
        };
        encode(&mut self.f.body, op::STORE, &[pointer, value]);
        Ok(())
    }

    fn entry_point(&mut self, entry: &EntryPoint) -> Result<()> {
        let function = &self.module.functions[entry.function];
        let void = self.type_id(&Type::Void)?;
        let fn_type = self.function_type(void, &[]);
        let id = self.id();
        let mut head = Vec::new();
        encode(&mut head, op::FUNCTION, &[void, id, 0, fn_type]);
        let label = self.id();
        encode(&mut head, op::LABEL, &[label]);
        self.f = FnState {
            current: label,
            ..Default::default()
        };
        let mut interface = Vec::new();

        let mut args = Vec::new();
        for param in &function.params {
            match (param.io, &param.ty) {
                (Some(io), ty) => {
                    let var = self.interface_var(class::INPUT, ty, io, entry.stage)?;
                    interface.push(var);
                    args.push(self.load_interface(var, ty, io, class::INPUT)?);
                }
                (None, Type::Struct(i)) => {
                    let members = self.module.structs[*i].members.clone();
                    let mut parts = Vec::new();
                    for m in &members {
                        let io = member_io(m);
                        let var = self.interface_var(class::INPUT, &m.ty, io, entry.stage)?;
                        interface.push(var);
                        parts.push(self.load_interface(var, &m.ty, io, class::INPUT)?);
                    }
                    let ty = self.type_id(&param.ty)?;
                    let value = self.id();
                    let mut operands = vec![ty, value];
                    operands.extend(parts);
                    encode(&mut self.f.body, op::COMPOSITE_CONSTRUCT, &operands);
                    args.push(value);
                }
                _ => unreachable!("checked entry-point parameter"),
            }
        }

        let ret = self.type_id(&function.ret)?;
        let result = self.id();
        let mut operands = vec![ret, result, self.function_ids[entry.function]];
        operands.extend(args);
        encode(&mut self.f.body, op::FUNCTION_CALL, &operands);

        match (function.ret_io, &function.ret) {
            (_, Type::Void) => {}
            (Some(io), ty) => {
                let var = self.interface_var(class::OUTPUT, ty, io, entry.stage)?;
                interface.push(var);
                self.store_interface(var, result, ty, io)?;
            }
            (None, Type::Struct(i)) => {
                let members = self.module.structs[*i].members.clone();
                for (m, member) in members.iter().enumerate() {
                    let io = member_io(member);
                    let var = self.interface_var(class::OUTPUT, &member.ty, io, entry.stage)?;
                    interface.push(var);
                    let ty = self.type_id(&member.ty)?;
                    let value = self.id();
                    encode(
                        &mut self.f.body,
                        op::COMPOSITE_EXTRACT,
                        &[ty, value, result, m as u32],
                    );
                    self.store_interface(var, value, &member.ty, io)?;
                }
            }
            _ => unreachable!("checked entry-point return"),
        }
        encode(&mut self.f.body, op::RETURN, &[]);
        let state = std::mem::take(&mut self.f);
        self.functions.extend(head);
        self.functions.extend(state.vars);
        self.functions.extend(state.body);
        encode(&mut self.functions, op::FUNCTION_END, &[]);

        let model = match entry.stage {
            Stage::Vertex => 0,
            Stage::Fragment => 4,
            Stage::Compute => 5,
        };
        let mut operands = vec![model, id];
        operands.extend(string_words(&entry.name));
        operands.extend(interface);
        encode(&mut self.entry_points, op::ENTRY_POINT, &operands);
        match entry.stage {
            Stage::Fragment => {
                encode(&mut self.execution_modes, op::EXECUTION_MODE, &[id, 7]);
                if entry
                    .outputs
                    .iter()
                    .any(|o| o.io == IoBinding::Builtin(Builtin::FragDepth))
                {
                    encode(&mut self.execution_modes, op::EXECUTION_MODE, &[id, 12]);
                }
            }
            Stage::Compute => {
                let [x, y, z] = entry.workgroup_size;
                encode(
                    &mut self.execution_modes,
                    op::EXECUTION_MODE,
                    &[id, 17, x, y, z],
                );
            }
            Stage::Vertex => {}
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Statements

    fn emit(&mut self, opcode: u16, operands: &[u32]) {
        encode(&mut self.f.body, opcode, operands);
    }

    fn label(&mut self, id: u32) {
        self.emit(op::LABEL, &[id]);
        self.f.current = id;
        self.f.terminated = false;
    }

    fn terminate(&mut self, opcode: u16, operands: &[u32]) {
        self.emit(opcode, operands);
        self.f.terminated = true;
    }

    fn branch(&mut self, target: u32) {
        self.terminate(op::BRANCH, &[target]);
    }

    fn block(&mut self, stmts: &[Stmt]) -> Result<()> {
        for stmt in stmts {
            if self.f.terminated {
                // Statements after a return, break, continue or discard are unreachable.
                break;
            }
            self.stmt(stmt)?;
        }
        Ok(())
    }

    /// Ends a merge block nothing branches to.
    fn finish_merge(&mut self, reached: bool) {
        if !reached {
            self.terminate(op::UNREACHABLE, &[]);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Let(i, value) => {
                let id = self.expr(value)?;
                self.f.locals[*i] = id;
            }
            Stmt::Var(i, init) => {
                let ty = &self.module.functions[self.current_fn].locals[*i].ty;
                let value = match init {
                    Some(e) => self.expr(e)?,
                    None => {
                        let zero = ConstValue::zero(ty, &self.module.structs);
                        self.constant(&zero, ty)?
                    }
                };
                let var = self.f.locals[*i];
                self.emit(op::STORE, &[var, value]);
            }
            Stmt::Store { target, value } => {
                let pointer = self.pointer_of(target)?;
                let value = self.expr(value)?;
                self.emit(op::STORE, &[pointer, value]);
            }
            Stmt::Update {
                target,
                op: binop,
                value,
            } => {
                let pointer = self.pointer_of(target)?;
                let ty = self.type_id(&target.ty)?;
                let current = self.id();
                self.emit(op::LOAD, &[ty, current, pointer]);
                let rhs = self.expr(value)?;
                let result =
                    self.binary_ids(*binop, &target.ty, current, &value.ty, rhs, &target.ty)?;
                self.emit(op::STORE, &[pointer, result]);
            }
            Stmt::Eval(e) => {
                self.expr(e)?;
            }
            Stmt::Return(value) => match value {
                Some(e) => {
                    let id = self.expr(e)?;
                    self.terminate(op::RETURN_VALUE, &[id]);
                }
                None => self.terminate(op::RETURN, &[]),
            },
            Stmt::If {
                cond,
                then,
                otherwise,
            } => {
                let c = self.expr(cond)?;
                let then_label = self.id();
                let merge = self.id();
                let else_label = if otherwise.is_empty() {
                    merge
                } else {
                    self.id()
                };
                self.emit(op::SELECTION_MERGE, &[merge, 0]);
                self.terminate(op::BRANCH_CONDITIONAL, &[c, then_label, else_label]);
                let mut reached = otherwise.is_empty();
                self.label(then_label);
                self.block(then)?;
                if !self.f.terminated {
                    self.branch(merge);
                    reached = true;
                }
                if !otherwise.is_empty() {
                    self.label(else_label);
                    self.block(otherwise)?;
                    if !self.f.terminated {
                        self.branch(merge);
                        reached = true;
                    }
                }
                self.label(merge);
                self.finish_merge(reached);
            }
            Stmt::Switch { selector, cases } => {
                let sel = self.expr(selector)?;
                let merge = self.id();
                let labels: Vec<u32> = cases.iter().map(|_| self.id()).collect();
                let default = cases
                    .iter()
                    .position(|c| c.selectors.contains(&None))
                    .unwrap();
                let mut operands = vec![sel, labels[default]];
                for (case, &label) in cases.iter().zip(&labels) {
                    for value in case.selectors.iter().flatten() {
                        operands.push(*value as u32);
                        operands.push(label);
                    }
                }
                self.emit(op::SELECTION_MERGE, &[merge, 0]);
                self.terminate(op::SWITCH, &operands);
                self.f.breaks.push(Target {
                    label: merge,
                    used: false,
                });
                let mut reached = false;
                for (case, &label) in cases.iter().zip(&labels) {
                    self.label(label);
                    self.block(&case.body)?;
                    if !self.f.terminated {
                        self.branch(merge);
                        reached = true;
                    }
                }
                reached |= self.f.breaks.pop().unwrap().used;
                self.label(merge);
                self.finish_merge(reached);
            }
            Stmt::Loop {
                body,
                continuing,
                break_if,
            } => {
                let header = self.id();
                let body_label = self.id();
                let continue_label = self.id();
                let merge = self.id();
                self.branch(header);
                self.label(header);
                self.emit(op::LOOP_MERGE, &[merge, continue_label, 0]);
                self.branch(body_label);
                self.label(body_label);
                self.f.breaks.push(Target {
                    label: merge,
                    used: false,
                });
                self.f.continues.push(continue_label);
                self.block(body)?;
                if !self.f.terminated {
                    self.branch(continue_label);
                }
                self.f.continues.pop();
                self.label(continue_label);
                self.block(continuing)?;
                let mut reached = self.f.breaks.pop().unwrap().used;
                match break_if {
                    Some(cond) => {
                        let c = self.expr(cond)?;
                        self.terminate(op::BRANCH_CONDITIONAL, &[c, merge, header]);
                        reached = true;
                    }
                    None => self.branch(header),
                }
                self.label(merge);
                self.finish_merge(reached);
            }
            Stmt::Break => {
                let target = self.f.breaks.last_mut().expect("checked break");
                target.used = true;
                let label = target.label;
                self.branch(label);
            }
            Stmt::Continue => {
                let label = *self.f.continues.last().expect("checked continue");
                self.branch(label);
            }
            Stmt::Discard => self.terminate(op::KILL, &[]),
            Stmt::Block(stmts) => self.block(stmts)?,
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Expressions

    fn pointer_of(&mut self, e: &Expr) -> Result<u32> {
        let (space, _) = e.reference.expect("pointer_of a value");
        match &e.kind {
            ExprKind::Local(i) => Ok(self.f.locals[*i]),
            ExprKind::Global(i) => {
                let global = &self.globals[*i];
                let (var, wrapped) = (global.id, global.wrapped);
                if !wrapped {
                    return Ok(var);
                }
                let pointer = self.pointer_to(space, &e.ty)?;
                let zero = self.u32_const(0);
                let id = self.id();
                self.emit(op::ACCESS_CHAIN, &[pointer, id, var, zero]);
                Ok(id)
            }
            ExprKind::Member(base, index) => {
                let base = self.pointer_of(base)?;
                let pointer = self.pointer_to(space, &e.ty)?;
                let index = self.u32_const(*index);
                let id = self.id();
                self.emit(op::ACCESS_CHAIN, &[pointer, id, base, index]);
                Ok(id)
            }
            ExprKind::Index(base, index) => {
                let base = self.pointer_of(base)?;
                let index = self.expr(index)?;
                let pointer = self.pointer_to(space, &e.ty)?;
                let id = self.id();
                self.emit(op::ACCESS_CHAIN, &[pointer, id, base, index]);
                Ok(id)
            }
            ExprKind::Deref(pointer) => self.expr(pointer),
            _ => unreachable!("not a reference expression"),
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<u32> {
        if e.reference.is_some() {
            // References are only read through `Load`; reaching one here means the checker
            // passed memory where a pointer value is expected (`&x`).
            return self.pointer_of(e);
        }
        let ty = self.type_id(&e.ty)?;
        match &e.kind {
            ExprKind::Const(value) => self.constant(value, &e.ty),
            ExprKind::Local(i) => Ok(self.f.locals[*i]),
            ExprKind::Param(i) => Ok(self.f.params[*i]),
            ExprKind::Global(_) => unreachable!("globals are references"),
            ExprKind::Load(inner) => {
                let pointer = self.pointer_of(inner)?;
                let id = self.id();
                self.emit(op::LOAD, &[ty, id, pointer]);
                Ok(id)
            }
            ExprKind::Unary(unop, operand) => {
                let x = self.expr(operand)?;
                let scalar = scalar_of(&e.ty);
                let opcode = match unop {
                    UnaryOp::Neg if scalar.is_float() => op::F_NEGATE,
                    UnaryOp::Neg => op::S_NEGATE,
                    UnaryOp::Not => op::LOGICAL_NOT,
                    UnaryOp::BitNot => op::NOT,
                    _ => unreachable!(),
                };
                let id = self.id();
                self.emit(opcode, &[ty, id, x]);
                Ok(id)
            }
            ExprKind::Binary(binop @ (BinaryOp::And | BinaryOp::Or), lhs, rhs) => {
                let l = self.expr(lhs)?;
                let start = self.f.current;
                let rhs_label = self.id();
                let merge = self.id();
                self.emit(op::SELECTION_MERGE, &[merge, 0]);
                let targets = if *binop == BinaryOp::And {
                    [rhs_label, merge]
                } else {
                    [merge, rhs_label]
                };
                self.terminate(op::BRANCH_CONDITIONAL, &[l, targets[0], targets[1]]);
                self.label(rhs_label);
                let r = self.expr(rhs)?;
                let rhs_end = self.f.current;
                self.branch(merge);
                self.label(merge);
                let id = self.id();
                self.emit(op::PHI, &[ty, id, l, start, r, rhs_end]);
                Ok(id)
            }
            ExprKind::Binary(binop, lhs, rhs) => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                self.binary_ids(*binop, &lhs.ty, l, &rhs.ty, r, &e.ty)
            }
            ExprKind::Construct(parts) => {
                let ids: Vec<u32> = parts.iter().map(|p| self.expr(p)).collect::<Result<_>>()?;
                let id = self.id();
                let mut operands = vec![ty, id];
                operands.extend(ids);
                self.emit(op::COMPOSITE_CONSTRUCT, &operands);
                Ok(id)
            }
            ExprKind::Splat(scalar) => {
                let x = self.expr(scalar)?;
                Ok(self.splat_id(ty, x, e.ty.vector_size().unwrap()))
            }
            ExprKind::Convert(arg) => {
                let x = self.expr(arg)?;
                self.convert_ids(&arg.ty, x, &e.ty)
            }
            ExprKind::Bitcast(arg) => {
                let x = self.expr(arg)?;
                let id = self.id();
                self.emit(op::BITCAST, &[ty, id, x]);
                Ok(id)
            }
            ExprKind::Call(f, args) => {
                let ids: Vec<u32> = args.iter().map(|a| self.expr(a)).collect::<Result<_>>()?;
                let id = self.id();
                let mut operands = vec![ty, id, self.function_ids[*f]];
                operands.extend(ids);
                self.emit(op::FUNCTION_CALL, &operands);
                Ok(id)
            }
            ExprKind::Builtin(f, args) => self.builtin(*f, args, &e.ty),
            ExprKind::Member(base, index) => {
                let b = self.expr(base)?;
                let id = self.id();
                self.emit(op::COMPOSITE_EXTRACT, &[ty, id, b, *index]);
                Ok(id)
            }
            ExprKind::Swizzle(base, components) => {
                let b = self.expr(base)?;
                let id = self.id();
                if components.len() == 1 {
                    self.emit(op::COMPOSITE_EXTRACT, &[ty, id, b, components[0] as u32]);
                } else {
                    let mut operands = vec![ty, id, b, b];
                    operands.extend(components.iter().map(|&c| c as u32));
                    self.emit(op::VECTOR_SHUFFLE, &operands);
                }
                Ok(id)
            }
            ExprKind::Index(base, index) => {
                let b = self.expr(base)?;
                if let Some(ConstValue::Int(i)) = index.as_const() {
                    let id = self.id();
                    self.emit(op::COMPOSITE_EXTRACT, &[ty, id, b, *i as u32]);
                    return Ok(id);
                }
                let i = self.expr(index)?;
                if let Type::Vector(..) = base.ty {
                    let id = self.id();
                    self.emit(op::VECTOR_EXTRACT_DYNAMIC, &[ty, id, b, i]);
                    return Ok(id);
                }
                // Dynamic indexing of an array or matrix value goes through a temporary.
                let pointer_ty = self.pointer_to(AddressSpace::Function, &base.ty)?;
                let temp = self.id();
                encode(
                    &mut self.f.vars,
                    op::VARIABLE,
                    &[pointer_ty, temp, class::FUNCTION],
                );
                self.emit(op::STORE, &[temp, b]);
                let element_pointer = self.pointer_to(AddressSpace::Function, &e.ty)?;
                let pointer = self.id();
                self.emit(op::ACCESS_CHAIN, &[element_pointer, pointer, temp, i]);
                let id = self.id();
                self.emit(op::LOAD, &[ty, id, pointer]);
                Ok(id)
            }
            ExprKind::AddrOf(inner) => self.pointer_of(inner),
            ExprKind::Deref(_) => unreachable!("dereferences are references"),
        }
    }

    fn splat_id(&mut self, vector_ty: u32, x: u32, n: u8) -> u32 {
        let id = self.id();
        let mut operands = vec![vector_ty, id];
        operands.extend(std::iter::repeat_n(x, n as usize));
        self.emit(op::COMPOSITE_CONSTRUCT, &operands);
        id
    }

    /// A constant scalar, splatted to `like`'s vector size.
    fn constant_like(&mut self, value: ConstValue, like: &Type) -> Result<u32> {
        let value = match like.vector_size() {
            Some(n) => ConstValue::Composite(vec![value; n as usize]),
            None => value,
        };
        self.constant(&value, like)
    }

    fn convert_ids(&mut self, from: &Type, x: u32, to: &Type) -> Result<u32> {
        if let (
            Type::Matrix {
                columns,
                rows,
                scalar: a,
            },
            Type::Matrix { scalar: b, .. },
        ) = (from, to)
        {
            let (col_from, col_to) = (Type::Vector(*rows, *a), Type::Vector(*rows, *b));
            let col_from_id = self.type_id(&col_from)?;
            let mut parts = Vec::new();
            for c in 0..*columns as u32 {
                let column = self.id();
                self.emit(op::COMPOSITE_EXTRACT, &[col_from_id, column, x, c]);
                parts.push(self.convert_ids(&col_from, column, &col_to)?);
            }
            let ty = self.type_id(to)?;
            let id = self.id();
            let mut operands = vec![ty, id];
            operands.extend(parts);
            self.emit(op::COMPOSITE_CONSTRUCT, &operands);
            return Ok(id);
        }
        let (a, b) = (scalar_of(from), scalar_of(to));
        let ty = self.type_id(to)?;
        let id = self.id();
        match (a, b) {
            _ if a == b => return Ok(x),
            (Scalar::Bool, _) => {
                let one = if b.is_float() {
                    ConstValue::Float(1.0)
                } else {
                    ConstValue::Int(1)
                };
                let zero = if b.is_float() {
                    ConstValue::Float(0.0)
                } else {
                    ConstValue::Int(0)
                };
                let one = self.constant_like(one, to)?;
                let zero = self.constant_like(zero, to)?;
                self.emit(op::SELECT, &[ty, id, x, one, zero]);
            }
            (_, Scalar::Bool) => {
                let zero = if a.is_float() {
                    ConstValue::Float(0.0)
                } else {
                    ConstValue::Int(0)
                };
                let zero = self.constant_like(zero, from)?;
                let opcode = if a.is_float() {
                    op::F_UNORD_NOT_EQUAL
                } else {
                    op::I_NOT_EQUAL
                };
                self.emit(opcode, &[ty, id, x, zero]);
            }
            (Scalar::F32 | Scalar::F16, Scalar::F32 | Scalar::F16) => {
                self.emit(op::F_CONVERT, &[ty, id, x])
            }
            (Scalar::I32, _) if b.is_float() => self.emit(op::CONVERT_S_TO_F, &[ty, id, x]),
            (Scalar::U32, _) if b.is_float() => self.emit(op::CONVERT_U_TO_F, &[ty, id, x]),
            (_, Scalar::I32) if a.is_float() => self.emit(op::CONVERT_F_TO_S, &[ty, id, x]),
            (_, Scalar::U32) if a.is_float() => self.emit(op::CONVERT_F_TO_U, &[ty, id, x]),
            _ => self.emit(op::BITCAST, &[ty, id, x]),
        }
        Ok(id)
    }

    fn binary_ids(
        &mut self,
        binop: BinaryOp,
        lty: &Type,
        l: u32,
        rty: &Type,
        r: u32,
        result: &Type,
    ) -> Result<u32> {
        use BinaryOp::*;
        let ty = self.type_id(result)?;
        let id = self.id();
        match (lty, rty) {
            (Type::Matrix { .. }, Type::Matrix { .. }) if binop == Mul => {
                self.emit(op::MATRIX_TIMES_MATRIX, &[ty, id, l, r]);
                return Ok(id);
            }
            (
                Type::Matrix {
                    columns,
                    rows,
                    scalar,
                },
                Type::Matrix { .. },
            ) => {
                let column = Type::Vector(*rows, *scalar);
                let column_id = self.type_id(&column)?;
                let opcode = if binop == Add { op::F_ADD } else { op::F_SUB };
                let mut parts = Vec::new();
                for c in 0..*columns as u32 {
                    let (a, b, out) = (self.id(), self.id(), self.id());
                    self.emit(op::COMPOSITE_EXTRACT, &[column_id, a, l, c]);
                    self.emit(op::COMPOSITE_EXTRACT, &[column_id, b, r, c]);
                    self.emit(opcode, &[column_id, out, a, b]);
                    parts.push(out);
                }
                let mut operands = vec![ty, id];
                operands.extend(parts);
                self.emit(op::COMPOSITE_CONSTRUCT, &operands);
                return Ok(id);
            }
            (Type::Matrix { .. }, Type::Vector(..)) => {
                self.emit(op::MATRIX_TIMES_VECTOR, &[ty, id, l, r]);
                return Ok(id);
            }
            (Type::Vector(..), Type::Matrix { .. }) => {
                self.emit(op::VECTOR_TIMES_MATRIX, &[ty, id, l, r]);
                return Ok(id);
            }
            (Type::Matrix { .. }, Type::Scalar(_)) => {
                self.emit(op::MATRIX_TIMES_SCALAR, &[ty, id, l, r]);
                return Ok(id);
            }
            (Type::Scalar(_), Type::Matrix { .. }) => {
                self.emit(op::MATRIX_TIMES_SCALAR, &[ty, id, r, l]);
                return Ok(id);
            }
            _ => {}
        }
        let scalar = scalar_of(lty);
        let float = scalar.is_float();
        let signed = scalar.is_signed();
        let boolean = scalar == Scalar::Bool;
        let opcode = match binop {
            Add if float => op::F_ADD,
            Add => op::I_ADD,
            Sub if float => op::F_SUB,
            Sub => op::I_SUB,
            Mul if float => op::F_MUL,
            Mul => op::I_MUL,
            Div if float => op::F_DIV,
            Rem if float => op::F_REM,
            Div | Rem => {
                // WGSL defines integer division by zero (and i32::MIN / -1) to divide by one.
                let divisor = self.safe_divisor(lty, l, r, signed)?;
                let opcode = match (binop, signed) {
                    (Div, true) => op::S_DIV,
                    (Div, false) => op::U_DIV,
                    (_, true) => op::S_REM,
                    (_, false) => op::U_MOD,
                };
                self.emit(opcode, &[ty, id, l, divisor]);
                return Ok(id);
            }
            BitAnd if boolean => op::LOGICAL_AND,
            BitAnd => op::BITWISE_AND,
            BitOr if boolean => op::LOGICAL_OR,
            BitOr => op::BITWISE_OR,
            BitXor => op::BITWISE_XOR,
            Shl | Shr => {
                // Shift amounts are taken modulo the bit width.
                let mask = self.constant_like(ConstValue::Int(31), rty)?;
                let rty_id = self.type_id(rty)?;
                let masked = self.id();
                self.emit(op::BITWISE_AND, &[rty_id, masked, r, mask]);
                let opcode = match (binop, signed) {
                    (Shl, _) => op::SHIFT_LEFT_LOGICAL,
                    (_, true) => op::SHIFT_RIGHT_ARITHMETIC,
                    (_, false) => op::SHIFT_RIGHT_LOGICAL,
                };
                self.emit(opcode, &[ty, id, l, masked]);
                return Ok(id);
            }
            Eq if boolean => op::LOGICAL_EQUAL,
            Eq if float => op::F_ORD_EQUAL,
            Eq => op::I_EQUAL,
            Ne if boolean => op::LOGICAL_NOT_EQUAL,
            Ne if float => op::F_UNORD_NOT_EQUAL,
            Ne => op::I_NOT_EQUAL,
            Lt if float => op::F_ORD_LESS_THAN,
            Lt if signed => op::S_LESS_THAN,
            Lt => op::U_LESS_THAN,
            Le if float => op::F_ORD_LESS_THAN_EQUAL,
            Le if signed => op::S_LESS_THAN_EQUAL,
            Le => op::U_LESS_THAN_EQUAL,
            Gt if float => op::F_ORD_GREATER_THAN,
            Gt if signed => op::S_GREATER_THAN,
            Gt => op::U_GREATER_THAN,
            Ge if float => op::F_ORD_GREATER_THAN_EQUAL,
            Ge if signed => op::S_GREATER_THAN_EQUAL,
            Ge => op::U_GREATER_THAN_EQUAL,
            And => op::LOGICAL_AND,
            Or => op::LOGICAL_OR,
        };
        self.emit(opcode, &[ty, id, l, r]);
        Ok(id)
    }

    fn safe_divisor(&mut self, ty: &Type, l: u32, r: u32, signed: bool) -> Result<u32> {
        let ty_id = self.type_id(ty)?;
        let bool_ty = self.type_id(&ty.with_scalar(Scalar::Bool))?;
        let zero = self.constant_like(ConstValue::Int(0), ty)?;
        let one = self.constant_like(ConstValue::Int(1), ty)?;
        let mut bad = self.id();
        self.emit(op::I_EQUAL, &[bool_ty, bad, r, zero]);
        if signed {
            let min = self.constant_like(ConstValue::Int(i32::MIN as i64), ty)?;
            let minus_one = self.constant_like(ConstValue::Int(-1), ty)?;
            let (is_min, is_minus_one, overflow, either) =
                (self.id(), self.id(), self.id(), self.id());
            self.emit(op::I_EQUAL, &[bool_ty, is_min, l, min]);
            self.emit(op::I_EQUAL, &[bool_ty, is_minus_one, r, minus_one]);
            self.emit(op::LOGICAL_AND, &[bool_ty, overflow, is_min, is_minus_one]);
            self.emit(op::LOGICAL_OR, &[bool_ty, either, bad, overflow]);
            bad = either;
        }
        let id = self.id();
        self.emit(op::SELECT, &[ty_id, id, bad, one, r]);
        Ok(id)
    }

    fn ext(&mut self, ty: u32, instruction: u32, args: &[u32]) -> u32 {
        let id = self.id();
        let mut operands = vec![ty, id, self.glsl, instruction];
        operands.extend_from_slice(args);
        self.emit(op::EXT_INST, &operands);
        id
    }

    fn builtin(&mut self, f: BuiltinFn, args: &[Expr], ret: &Type) -> Result<u32> {
        use BuiltinFn::*;
        if f == ArrayLength {
            return self.array_length(&args[0]);
        }
        if f.is_texture() {
            return self.texture(f, args, ret);
        }
        let ids: Vec<u32> = args.iter().map(|a| self.expr(a)).collect::<Result<_>>()?;
        let ty = self.type_id(ret)?;
        let arg_ty = args.first().map(|a| a.ty.clone()).unwrap_or(Type::Void);
        let scalar = scalar_of(&arg_ty);
        let (float, signed) = (scalar.is_float(), scalar.is_signed());
        let simple = |i: u32| Some(i);
        let instruction = match f {
            Abs if float => simple(glsl::F_ABS),
            Abs if signed => simple(glsl::S_ABS),
            Abs => return Ok(ids[0]),
            Acos => simple(glsl::ACOS),
            Acosh => simple(glsl::ACOSH),
            Asin => simple(glsl::ASIN),
            Asinh => simple(glsl::ASINH),
            Atan => simple(glsl::ATAN),
            Atanh => simple(glsl::ATANH),
            Atan2 => simple(glsl::ATAN2),
            Ceil => simple(glsl::CEIL),
            Clamp if float => simple(glsl::F_CLAMP),
            Clamp if signed => simple(glsl::S_CLAMP),
            Clamp => simple(glsl::U_CLAMP),
            Cos => simple(glsl::COS),
            Cosh => simple(glsl::COSH),
            Degrees => simple(glsl::DEGREES),
            Exp => simple(glsl::EXP),
            Exp2 => simple(glsl::EXP2),
            Floor => simple(glsl::FLOOR),
            Fma => simple(glsl::FMA),
            Fract => simple(glsl::FRACT),
            InverseSqrt => simple(glsl::INVERSE_SQRT),
            Log => simple(glsl::LOG),
            Log2 => simple(glsl::LOG2),
            Max if float => simple(glsl::F_MAX),
            Max if signed => simple(glsl::S_MAX),
            Max => simple(glsl::U_MAX),
            Min if float => simple(glsl::F_MIN),
            Min if signed => simple(glsl::S_MIN),
            Min => simple(glsl::U_MIN),
            Mix => simple(glsl::F_MIX),
            Pow => simple(glsl::POW),
            Radians => simple(glsl::RADIANS),
            Round => simple(glsl::ROUND_EVEN),
            Saturate => {
                let zero = self.constant_like(ConstValue::Float(0.0), &arg_ty)?;
                let one = self.constant_like(ConstValue::Float(1.0), &arg_ty)?;
                return Ok(self.ext(ty, glsl::F_CLAMP, &[ids[0], zero, one]));
            }
            Sign if float => simple(glsl::F_SIGN),
            Sign => simple(glsl::S_SIGN),
            Sin => simple(glsl::SIN),
            Sinh => simple(glsl::SINH),
            Smoothstep => simple(glsl::SMOOTH_STEP),
            Sqrt => simple(glsl::SQRT),
            Step => simple(glsl::STEP),
            Tan => simple(glsl::TAN),
            Tanh => simple(glsl::TANH),
            Trunc => simple(glsl::TRUNC),
            Cross => simple(glsl::CROSS),
            Determinant => simple(glsl::DETERMINANT),
            Distance => simple(glsl::DISTANCE),
            FaceForward => simple(glsl::FACE_FORWARD),
            Length => simple(glsl::LENGTH),
            Normalize => simple(glsl::NORMALIZE),
            Reflect => simple(glsl::REFLECT),
            Refract => simple(glsl::REFRACT),
            FirstTrailingBit => simple(glsl::FIND_I_LSB),
            FirstLeadingBit if signed => simple(glsl::FIND_S_MSB),
            FirstLeadingBit => simple(glsl::FIND_U_MSB),
            Pack4x8Snorm => simple(glsl::PACK_SNORM_4X8),
            Pack4x8Unorm => simple(glsl::PACK_UNORM_4X8),
            Pack2x16Snorm => simple(glsl::PACK_SNORM_2X16),
            Pack2x16Unorm => simple(glsl::PACK_UNORM_2X16),
            Pack2x16Float => simple(glsl::PACK_HALF_2X16),
            Unpack4x8Snorm => simple(glsl::UNPACK_SNORM_4X8),
            Unpack4x8Unorm => simple(glsl::UNPACK_UNORM_4X8),
            Unpack2x16Snorm => simple(glsl::UNPACK_SNORM_2X16),
            Unpack2x16Unorm => simple(glsl::UNPACK_UNORM_2X16),
            Unpack2x16Float => simple(glsl::UNPACK_HALF_2X16),
            _ => None,
        };
        if let Some(instruction) = instruction {
            return Ok(self.ext(ty, instruction, &ids));
        }
        let core = |this: &mut Self, opcode: u16, operands: &[u32]| {
            let id = this.id();
            let mut all = vec![ty, id];
            all.extend_from_slice(operands);
            this.emit(opcode, &all);
            id
        };
        Ok(match f {
            Dot if float => core(self, op::DOT, &ids),
            Dot => {
                let n = arg_ty.vector_size().unwrap();
                let product_ty = self.type_id(&arg_ty)?;
                let product = self.id();
                self.emit(op::I_MUL, &[product_ty, product, ids[0], ids[1]]);
                let mut sum = 0;
                for c in 0..n as u32 {
                    let part = self.id();
                    self.emit(op::COMPOSITE_EXTRACT, &[ty, part, product, c]);
                    sum = if c == 0 {
                        part
                    } else {
                        let next = self.id();
                        self.emit(op::I_ADD, &[ty, next, sum, part]);
                        next
                    };
                }
                sum
            }
            Transpose => core(self, op::TRANSPOSE, &ids),
            All => core(self, op::ALL, &ids),
            Any => core(self, op::ANY, &ids),
            Select => {
                let mut cond = ids[2];
                if let (Some(n), Type::Scalar(_)) = (ret.vector_size(), &args[2].ty) {
                    let bool_vec = self.type_id(&Type::Vector(n, Scalar::Bool))?;
                    cond = self.splat_id(bool_vec, cond, n);
                }
                core(self, op::SELECT, &[cond, ids[1], ids[0]])
            }
            CountOneBits => core(self, op::BIT_COUNT, &ids),
            ReverseBits => core(self, op::BIT_REVERSE, &ids),
            ExtractBits if signed => core(self, op::BIT_FIELD_S_EXTRACT, &ids),
            ExtractBits => core(self, op::BIT_FIELD_U_EXTRACT, &ids),
            InsertBits => core(self, op::BIT_FIELD_INSERT, &ids),
            Dpdx => core(self, op::DPDX, &ids),
            Dpdy => core(self, op::DPDY, &ids),
            Fwidth => core(self, op::FWIDTH, &ids),
            WorkgroupBarrier | StorageBarrier => {
                let workgroup = self.u32_const(2);
                let execution = workgroup;
                let (scope, semantics) = if f == WorkgroupBarrier {
                    (workgroup, self.u32_const(0x8 | 0x100))
                } else {
                    (self.u32_const(1), self.u32_const(0x8 | 0x40))
                };
                self.emit(op::CONTROL_BARRIER, &[execution, scope, semantics]);
                0
            }
            _ => return unsupported(&format!("{f:?}")),
        })
    }

    // ---------------------------------------------------------------------------------------
    // Textures

    fn image_type(&mut self, dim: TextureDim, arrayed: bool, kind: &TextureKind) -> Result<u32> {
        let (scalar, depth, multisampled, sampled, format) = match kind {
            TextureKind::Sampled {
                sampled,
                multisampled,
            } => (*sampled, 0, *multisampled, 1, 0),
            TextureKind::Depth { multisampled } => (Scalar::F32, 1, *multisampled, 1, 0),
            TextureKind::Storage { format, .. } => {
                let (spirv, extended) = image_format(format);
                if extended {
                    self.capabilities
                        .insert(CAPABILITY_STORAGE_IMAGE_EXTENDED_FORMATS);
                }
                (texel_format_scalar(format).unwrap(), 0, false, 2, spirv)
            }
            TextureKind::External => return unsupported("external textures"),
        };
        let spirv_dim = match dim {
            TextureDim::D1 => {
                self.capabilities.insert(if sampled == 1 {
                    CAPABILITY_SAMPLED_1D
                } else {
                    CAPABILITY_IMAGE_1D
                });
                0
            }
            TextureDim::D2 => 1,
            TextureDim::D3 => 2,
            TextureDim::Cube => {
                if arrayed {
                    self.capabilities.insert(CAPABILITY_SAMPLED_CUBE_ARRAY);
                }
                3
            }
        };
        let scalar_ty = self.type_id(&Type::Scalar(scalar))?;
        let id = self.id();
        encode(
            &mut self.declarations,
            op::TYPE_IMAGE,
            &[
                id,
                scalar_ty,
                spirv_dim,
                depth,
                arrayed as u32,
                multisampled as u32,
                sampled,
                format,
            ],
        );
        Ok(id)
    }

    fn sampled_image_type(&mut self, image: u32) -> u32 {
        if let Some(&id) = self.sampled_images.get(&image) {
            return id;
        }
        let id = self.id();
        encode(&mut self.declarations, op::TYPE_SAMPLED_IMAGE, &[id, image]);
        self.sampled_images.insert(image, id);
        id
    }

    /// Emits `opcode` with a result of type `ty` and returns the result id.
    fn op(&mut self, opcode: u16, ty: u32, operands: &[u32]) -> u32 {
        let id = self.id();
        let mut all = vec![ty, id];
        all.extend_from_slice(operands);
        self.emit(opcode, &all);
        id
    }

    /// Appends the array index to a coordinate, converted to the coordinate's scalar type.
    fn with_layer(
        &mut self,
        coords: u32,
        coords_ty: &Type,
        layer: Option<(u32, &Type)>,
    ) -> Result<(u32, Type)> {
        let Some((layer, layer_ty)) = layer else {
            return Ok((coords, coords_ty.clone()));
        };
        let scalar = scalar_of(coords_ty);
        let layer = self.convert_ids(layer_ty, layer, &Type::Scalar(scalar))?;
        let n = coords_ty.vector_size().unwrap_or(1) + 1;
        let ty = Type::Vector(n, scalar);
        let ty_id = self.type_id(&ty)?;
        Ok((
            self.op(op::COMPOSITE_CONSTRUCT, ty_id, &[coords, layer]),
            ty,
        ))
    }

    fn u32_min(&mut self, ty: &Type, a: u32, b: u32) -> Result<u32> {
        let ty_id = self.type_id(ty)?;
        Ok(self.ext(ty_id, glsl::U_MIN, &[a, b]))
    }

    /// `x - 1` for a `u32` scalar or vector, used to turn sizes and counts into maximum indices.
    fn minus_one(&mut self, ty: &Type, x: u32) -> Result<u32> {
        let one = self.constant_like(ConstValue::Int(1), ty)?;
        let ty_id = self.type_id(ty)?;
        Ok(self.op(op::I_SUB, ty_id, &[x, one]))
    }

    /// Clamps `x` (an `i32` or `u32` scalar or vector) to `0..=max` as `u32`: negative values
    /// wrap to large unsigned ones and clamp to `max`, which WebGPU's robustness rules allow.
    fn clamp_index(&mut self, x: u32, x_ty: &Type, max: u32) -> Result<(u32, Type)> {
        let unsigned = x_ty.with_scalar(Scalar::U32);
        let x = self.convert_ids(x_ty, x, &unsigned)?;
        Ok((self.u32_min(&unsigned, x, max)?, unsigned))
    }

    fn texture(&mut self, f: BuiltinFn, args: &[Expr], ret: &Type) -> Result<u32> {
        use BuiltinFn::*;
        let all_ids: Vec<u32> = args.iter().map(|a| self.expr(a)).collect::<Result<_>>()?;
        let ret_id = self.type_id(ret)?;
        // textureGather on a colour texture leads with the component.
        let skip = (f == TextureGather && matches!(args[0].ty, Type::Scalar(_))) as usize;
        let (args, ids) = (&args[skip..], &all_ids[skip..]);
        let Type::Texture { dim, arrayed, kind } = &args[0].ty else {
            unreachable!("checked texture argument")
        };
        let (dim, arrayed, kind) = (*dim, *arrayed, kind.clone());
        let image = ids[0];
        let image_ty = self.type_id(&args[0].ty)?;
        let depth = matches!(kind, TextureKind::Depth { .. });
        let multisampled = matches!(
            kind,
            TextureKind::Sampled {
                multisampled: true,
                ..
            } | TextureKind::Depth { multisampled: true }
        );
        let storage = matches!(kind, TextureKind::Storage { .. });
        let has_levels = !storage && !multisampled;
        let size_ty = match dim.extent() + arrayed as u8 {
            1 => U32,
            n => Type::Vector(n, Scalar::U32),
        };

        match f {
            TextureSample
            | TextureSampleBias
            | TextureSampleLevel
            | TextureSampleGrad
            | TextureSampleCompare
            | TextureSampleCompareLevel
            | TextureGather
            | TextureGatherCompare => {
                let sampled_ty = self.sampled_image_type(image_ty);
                let sampled = self.op(op::SAMPLED_IMAGE, sampled_ty, &[image, ids[1]]);
                let layer = arrayed.then(|| (ids[3], &args[3].ty));
                let (coords, _) = self.with_layer(ids[2], &args[2].ty, layer)?;
                let mut next = 3 + arrayed as usize;
                let mut mask = 0;
                let mut operands = Vec::new();
                let mut dref = 0;
                match f {
                    TextureSampleBias => {
                        mask |= image_operand::BIAS;
                        operands.push(ids[next]);
                        next += 1;
                    }
                    TextureSampleLevel => {
                        let lod = self.convert_ids(&args[next].ty, ids[next], &F32)?;
                        mask |= image_operand::LOD;
                        operands.push(lod);
                        next += 1;
                    }
                    TextureSampleGrad => {
                        mask |= image_operand::GRAD;
                        operands.extend([ids[next], ids[next + 1]]);
                        next += 2;
                    }
                    TextureSampleCompare | TextureGatherCompare => {
                        dref = ids[next];
                        next += 1;
                    }
                    TextureSampleCompareLevel => {
                        dref = ids[next];
                        next += 1;
                        mask |= image_operand::LOD;
                        operands.push(self.constant(&ConstValue::Float(0.0), &F32)?);
                    }
                    _ => {}
                }
                if next < args.len() {
                    mask |= image_operand::CONST_OFFSET;
                    operands.push(ids[next]);
                    if matches!(f, TextureGather | TextureGatherCompare) {
                        self.capabilities.insert(CAPABILITY_IMAGE_GATHER_EXTENDED);
                    }
                }
                let mut tail = Vec::new();
                if mask != 0 {
                    tail.push(mask);
                    tail.extend(operands);
                }
                let operands = |head: &[u32]| -> Vec<u32> {
                    let mut all = head.to_vec();
                    all.extend(&tail);
                    all
                };
                Ok(match f {
                    TextureGather => {
                        let component = if skip == 1 {
                            // A constant: the checker requires one, and SPIR-V does too.
                            all_ids[0]
                        } else {
                            self.u32_const(0)
                        };
                        self.op(
                            op::IMAGE_GATHER,
                            ret_id,
                            &operands(&[sampled, coords, component]),
                        )
                    }
                    TextureGatherCompare => self.op(
                        op::IMAGE_DREF_GATHER,
                        ret_id,
                        &operands(&[sampled, coords, dref]),
                    ),
                    TextureSampleCompare => self.op(
                        op::IMAGE_SAMPLE_DREF_IMPLICIT_LOD,
                        ret_id,
                        &operands(&[sampled, coords, dref]),
                    ),
                    TextureSampleCompareLevel => self.op(
                        op::IMAGE_SAMPLE_DREF_EXPLICIT_LOD,
                        ret_id,
                        &operands(&[sampled, coords, dref]),
                    ),
                    _ => {
                        let opcode = if matches!(f, TextureSample | TextureSampleBias) {
                            op::IMAGE_SAMPLE_IMPLICIT_LOD
                        } else {
                            op::IMAGE_SAMPLE_EXPLICIT_LOD
                        };
                        let vec4 = self.type_id(&Type::Vector(4, Scalar::F32))?;
                        let texel = self.op(opcode, vec4, &operands(&[sampled, coords]));
                        if depth {
                            self.op(op::COMPOSITE_EXTRACT, ret_id, &[texel, 0])
                        } else {
                            texel
                        }
                    }
                })
            }
            TextureLoad | TextureStore => {
                self.capabilities.insert(CAPABILITY_IMAGE_QUERY);
                let layer = arrayed.then(|| (ids[2], &args[2].ty));
                let (coords, coords_ty) = self.with_layer(ids[1], &args[1].ty, layer)?;
                let next = 2 + arrayed as usize;
                let size_id = self.type_id(&size_ty)?;
                let u32_ty = self.type_id(&U32)?;
                let mut image_operands = Vec::new();
                let size = if has_levels {
                    let levels = self.op(op::IMAGE_QUERY_LEVELS, u32_ty, &[image]);
                    let max_level = self.minus_one(&U32, levels)?;
                    let (level, _) = self.clamp_index(ids[next], &args[next].ty, max_level)?;
                    image_operands = vec![image_operand::LOD, level];
                    self.op(op::IMAGE_QUERY_SIZE_LOD, size_id, &[image, level])
                } else {
                    if multisampled {
                        let samples = self.op(op::IMAGE_QUERY_SAMPLES, u32_ty, &[image]);
                        let max_sample = self.minus_one(&U32, samples)?;
                        let (sample, _) =
                            self.clamp_index(ids[next], &args[next].ty, max_sample)?;
                        image_operands = vec![image_operand::SAMPLE, sample];
                    }
                    self.op(op::IMAGE_QUERY_SIZE, size_id, &[image])
                };
                let max = self.minus_one(&size_ty, size)?;
                let (coords, _) = self.clamp_index(coords, &coords_ty, max)?;
                if f == TextureStore {
                    let value = ids[args.len() - 1];
                    self.emit(op::IMAGE_WRITE, &[image, coords, value]);
                    return Ok(0);
                }
                let mut operands = vec![image, coords];
                operands.extend(image_operands);
                if storage {
                    return Ok(self.op(op::IMAGE_READ, ret_id, &operands));
                }
                let texel_ty = if depth {
                    self.type_id(&Type::Vector(4, Scalar::F32))?
                } else {
                    ret_id
                };
                let texel = self.op(op::IMAGE_FETCH, texel_ty, &operands);
                Ok(if depth {
                    self.op(op::COMPOSITE_EXTRACT, ret_id, &[texel, 0])
                } else {
                    texel
                })
            }
            TextureDimensions | TextureNumLayers => {
                self.capabilities.insert(CAPABILITY_IMAGE_QUERY);
                let size_id = self.type_id(&size_ty)?;
                let size = if has_levels {
                    let level = match ids.get(1) {
                        Some(&level) => {
                            let u32_ty = self.type_id(&U32)?;
                            let levels = self.op(op::IMAGE_QUERY_LEVELS, u32_ty, &[image]);
                            let max_level = self.minus_one(&U32, levels)?;
                            self.clamp_index(level, &args[1].ty, max_level)?.0
                        }
                        None => self.u32_const(0),
                    };
                    self.op(op::IMAGE_QUERY_SIZE_LOD, size_id, &[image, level])
                } else {
                    self.op(op::IMAGE_QUERY_SIZE, size_id, &[image])
                };
                let n = size_ty.vector_size().unwrap_or(1) as u32;
                Ok(match (f, arrayed) {
                    (TextureNumLayers, _) => self.op(op::COMPOSITE_EXTRACT, ret_id, &[size, n - 1]),
                    (_, false) => size,
                    (_, true) if n == 2 => self.op(op::COMPOSITE_EXTRACT, ret_id, &[size, 0]),
                    (_, true) => {
                        let mut operands = vec![size, size];
                        operands.extend(0..n - 1);
                        self.op(op::VECTOR_SHUFFLE, ret_id, &operands)
                    }
                })
            }
            TextureNumLevels => {
                self.capabilities.insert(CAPABILITY_IMAGE_QUERY);
                Ok(self.op(op::IMAGE_QUERY_LEVELS, ret_id, &[image]))
            }
            TextureNumSamples => {
                self.capabilities.insert(CAPABILITY_IMAGE_QUERY);
                Ok(self.op(op::IMAGE_QUERY_SAMPLES, ret_id, &[image]))
            }
            _ => unreachable!(),
        }
    }

    fn array_length(&mut self, pointer: &Expr) -> Result<u32> {
        let ExprKind::AddrOf(target) = &pointer.kind else {
            return unsupported("arrayLength of a pointer parameter");
        };
        let u32_ty = self.type_id(&U32)?;
        let id = self.id();
        match &target.kind {
            ExprKind::Global(g) => {
                let var = self.globals[*g].id;
                self.emit(op::ARRAY_LENGTH, &[u32_ty, id, var, 0]);
            }
            ExprKind::Member(base, member) => {
                let base = self.pointer_of(base)?;
                self.emit(op::ARRAY_LENGTH, &[u32_ty, id, base, *member]);
            }
            _ => return unsupported("arrayLength of this expression"),
        }
        Ok(id)
    }
}

fn member_io(member: &StructMember) -> IoBinding {
    match (member.builtin, member.location) {
        (Some(b), _) => IoBinding::Builtin(b),
        (None, Some(location)) => IoBinding::Location {
            location,
            interpolation: member.interpolation,
        },
        _ => unreachable!("checked entry-point struct"),
    }
}
