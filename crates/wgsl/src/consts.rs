//! Constant values and constant evaluation.

use crate::ast::{BinaryOp, UnaryOp};
use crate::types::{Scalar, StructType, Type};

/// A constant. Integers of every width are held as `i64` and floats as `f64`; the expression's
/// type says which WGSL type the value has, and values are kept in that type's range.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Composite(Vec<ConstValue>),
}

impl ConstValue {
    pub fn zero(ty: &Type, structs: &[StructType]) -> ConstValue {
        match ty {
            Type::Scalar(Scalar::Bool) => ConstValue::Bool(false),
            Type::Scalar(s) if s.is_float() => ConstValue::Float(0.0),
            Type::Scalar(_) => ConstValue::Int(0),
            Type::Vector(n, s) => {
                ConstValue::Composite(vec![
                    ConstValue::zero(&Type::Scalar(*s), structs);
                    *n as usize
                ])
            }
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => {
                ConstValue::Composite(vec![
                    ConstValue::zero(&Type::Vector(*rows, *scalar), structs);
                    *columns as usize
                ])
            }
            Type::Array { element, count } => {
                ConstValue::Composite(vec![
                    ConstValue::zero(element, structs);
                    count.unwrap_or(0) as usize
                ])
            }
            Type::Struct(i) => ConstValue::Composite(
                structs[*i]
                    .members
                    .iter()
                    .map(|m| ConstValue::zero(&m.ty, structs))
                    .collect(),
            ),
            _ => ConstValue::Int(0),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            ConstValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn components(&self) -> &[ConstValue] {
        match self {
            ConstValue::Composite(parts) => parts,
            other => std::slice::from_ref(other),
        }
    }
}

/// Converts a scalar constant to `to`, as an explicit conversion (`f32(x)`) or an automatic
/// abstract conversion would. Fails when the value does not fit.
pub fn convert_scalar(value: &ConstValue, from: Scalar, to: Scalar) -> Result<ConstValue, String> {
    let out = match (value, to) {
        (ConstValue::Bool(b), Scalar::Bool) => ConstValue::Bool(*b),
        (ConstValue::Bool(b), s) if s.is_float() => ConstValue::Float(*b as i64 as f64),
        (ConstValue::Bool(b), _) => ConstValue::Int(*b as i64),
        (ConstValue::Int(v), Scalar::Bool) => ConstValue::Bool(*v != 0),
        (ConstValue::Int(v), s) if s.is_float() => ConstValue::Float(round_float(*v as f64, s)),
        (ConstValue::Int(v), s) => {
            let v = if from.is_abstract() {
                check_int_range(*v, s)?
            } else {
                // Same-width integer conversions reinterpret the bits.
                wrap_int(*v, s)
            };
            ConstValue::Int(v)
        }
        (ConstValue::Float(v), Scalar::Bool) => ConstValue::Bool(*v != 0.0),
        (ConstValue::Float(v), s) if s.is_float() => {
            if from == Scalar::AbstractFloat && v.is_finite() && !fits_float(*v, s) {
                return Err(format!("value {v} cannot be represented as {}", s.name()));
            }
            ConstValue::Float(round_float(*v, s))
        }
        (ConstValue::Float(v), s) => {
            if from.is_abstract() {
                return Err(format!(
                    "abstract float {v} does not convert to {}",
                    s.name()
                ));
            }
            // WGSL float-to-integer conversion clamps to the target range.
            let (min, max) = int_range(s);
            ConstValue::Int((v.trunc() as i64).clamp(min, max))
        }
        (ConstValue::Composite(_), _) => {
            return Err("cannot convert a composite to a scalar".into())
        }
    };
    Ok(out)
}

/// Converts a constant of type `from` to type `to`, component by component.
pub fn convert(
    value: &ConstValue,
    from: &Type,
    to: &Type,
    structs: &[StructType],
) -> Result<ConstValue, String> {
    match (from, to) {
        (Type::Scalar(a), Type::Scalar(b)) => convert_scalar(value, *a, *b),
        (Type::Vector(_, a), Type::Vector(_, b)) => {
            let parts = value
                .components()
                .iter()
                .map(|c| convert_scalar(c, *a, *b))
                .collect::<Result<_, _>>()?;
            Ok(ConstValue::Composite(parts))
        }
        (
            Type::Matrix {
                rows, scalar: a, ..
            },
            Type::Matrix { scalar: b, .. },
        ) => {
            let (col_from, col_to) = (Type::Vector(*rows, *a), Type::Vector(*rows, *b));
            let parts = value
                .components()
                .iter()
                .map(|c| convert(c, &col_from, &col_to, structs))
                .collect::<Result<_, _>>()?;
            Ok(ConstValue::Composite(parts))
        }
        (Type::Array { element: a, .. }, Type::Array { element: b, .. }) => {
            let parts = value
                .components()
                .iter()
                .map(|c| convert(c, a, b, structs))
                .collect::<Result<_, _>>()?;
            Ok(ConstValue::Composite(parts))
        }
        _ if from == to => Ok(value.clone()),
        _ => Err(format!(
            "cannot convert {} to {}",
            from.display(structs),
            to.display(structs)
        )),
    }
}

fn int_range(s: Scalar) -> (i64, i64) {
    match s {
        Scalar::I32 => (i32::MIN as i64, i32::MAX as i64),
        Scalar::U32 => (0, u32::MAX as i64),
        _ => (i64::MIN, i64::MAX),
    }
}

fn check_int_range(v: i64, s: Scalar) -> Result<i64, String> {
    let (min, max) = int_range(s);
    if v < min || v > max {
        Err(format!("value {v} does not fit in {}", s.name()))
    } else {
        Ok(v)
    }
}

fn wrap_int(v: i64, s: Scalar) -> i64 {
    match s {
        Scalar::I32 => v as i32 as i64,
        Scalar::U32 => v as u32 as i64,
        _ => v,
    }
}

fn fits_float(v: f64, s: Scalar) -> bool {
    match s {
        Scalar::F32 => v.abs() <= f32::MAX as f64,
        Scalar::F16 => v.abs() <= 65504.0,
        _ => true,
    }
}

fn round_float(v: f64, s: Scalar) -> f64 {
    match s {
        Scalar::F32 | Scalar::F16 => v as f32 as f64,
        _ => v,
    }
}

pub fn unary(op: UnaryOp, value: &ConstValue, ty: &Type) -> Result<ConstValue, String> {
    let scalar = ty.leaf_scalar().ok_or("not a numeric constant")?;
    map(value, &mut |v| match (op, v) {
        (UnaryOp::Neg, ConstValue::Int(i)) => {
            let r = i.checked_neg().ok_or("integer overflow")?;
            finish_int(r, scalar)
        }
        (UnaryOp::Neg, ConstValue::Float(f)) => Ok(ConstValue::Float(-f)),
        (UnaryOp::Not, ConstValue::Bool(b)) => Ok(ConstValue::Bool(!b)),
        (UnaryOp::BitNot, ConstValue::Int(i)) => Ok(ConstValue::Int(wrap_int(!i, scalar))),
        _ => Err("unsupported constant operation".into()),
    })
}

fn map(
    value: &ConstValue,
    f: &mut dyn FnMut(&ConstValue) -> Result<ConstValue, String>,
) -> Result<ConstValue, String> {
    match value {
        ConstValue::Composite(parts) => Ok(ConstValue::Composite(
            parts.iter().map(|p| map(p, f)).collect::<Result<_, _>>()?,
        )),
        scalar => f(scalar),
    }
}

/// Integer results: abstract and `i32`/`u32` results that overflow are errors in constant
/// expressions, as WGSL requires.
fn finish_int(v: i64, s: Scalar) -> Result<ConstValue, String> {
    check_int_range(v, s)
        .map(ConstValue::Int)
        .map_err(|_| "integer overflow in constant expression".into())
}

fn finish_float(v: f64, s: Scalar) -> Result<ConstValue, String> {
    if !v.is_finite() || !fits_float(v, s) {
        return Err("float overflow in constant expression".into());
    }
    Ok(ConstValue::Float(round_float(v, s)))
}

/// Evaluates `a op b` for scalar or same-shape vector operands of one scalar type. Mixed
/// scalar and vector operands must have been splatted first.
pub fn binary(
    op: BinaryOp,
    a: &ConstValue,
    b: &ConstValue,
    operand: &Type,
) -> Result<ConstValue, String> {
    let scalar = operand
        .leaf_scalar()
        .ok_or("not a scalar or vector constant")?;
    match (a, b) {
        (ConstValue::Composite(xs), ConstValue::Composite(ys)) => {
            if matches!(
                operand,
                Type::Matrix { .. } | Type::Array { .. } | Type::Struct(_)
            ) {
                return Err("unsupported constant operation".into());
            }
            if xs.len() != ys.len() {
                return Err("mismatched vector sizes".into());
            }
            let parts: Vec<ConstValue> = xs
                .iter()
                .zip(ys)
                .map(|(x, y)| binary(op, x, y, &Type::Scalar(scalar)))
                .collect::<Result<_, _>>()?;
            Ok(ConstValue::Composite(parts))
        }
        (ConstValue::Int(x), ConstValue::Int(y)) => int_binary(op, *x, *y, scalar),
        (ConstValue::Float(x), ConstValue::Float(y)) => float_binary(op, *x, *y, scalar),
        (ConstValue::Bool(x), ConstValue::Bool(y)) => Ok(ConstValue::Bool(match op {
            BinaryOp::And | BinaryOp::BitAnd => *x && *y,
            BinaryOp::Or | BinaryOp::BitOr => *x || *y,
            BinaryOp::Eq => x == y,
            BinaryOp::Ne => x != y,
            _ => return Err("unsupported boolean operation".into()),
        })),
        _ => Err("mismatched constant operands".into()),
    }
}

fn int_binary(op: BinaryOp, x: i64, y: i64, s: Scalar) -> Result<ConstValue, String> {
    use BinaryOp::*;
    let compare = |b: bool| Ok(ConstValue::Bool(b));
    match op {
        Eq => return compare(x == y),
        Ne => return compare(x != y),
        Lt => return compare(x < y),
        Le => return compare(x <= y),
        Gt => return compare(x > y),
        Ge => return compare(x >= y),
        _ => {}
    }
    let bits = if s == Scalar::AbstractInt { 64 } else { 32 };
    let r = match op {
        Add => x.checked_add(y),
        Sub => x.checked_sub(y),
        Mul => x.checked_mul(y),
        Div if y == 0 => return Err("division by zero in constant expression".into()),
        Rem if y == 0 => return Err("remainder by zero in constant expression".into()),
        Div => x.checked_div(y),
        Rem => x.checked_rem(y),
        BitAnd => Some(x & y),
        BitOr => Some(x | y),
        BitXor => Some(x ^ y),
        Shl | Shr if y < 0 || y >= bits => {
            return Err("shift amount out of range in constant expression".into())
        }
        Shl => {
            let r = wrap_int(x << y, s);
            return Ok(ConstValue::Int(r));
        }
        Shr if s == Scalar::U32 => Some(((x as u32) >> y) as i64),
        Shr => Some(x >> y),
        _ => return Err("unsupported integer operation".into()),
    };
    finish_int(r.ok_or("integer overflow in constant expression")?, s)
}

fn float_binary(op: BinaryOp, x: f64, y: f64, s: Scalar) -> Result<ConstValue, String> {
    use BinaryOp::*;
    let compare = |b: bool| Ok(ConstValue::Bool(b));
    let r = match op {
        Eq => return compare(x == y),
        Ne => return compare(x != y),
        Lt => return compare(x < y),
        Le => return compare(x <= y),
        Gt => return compare(x > y),
        Ge => return compare(x >= y),
        Add => x + y,
        Sub => x - y,
        Mul => x * y,
        Div => x / y,
        Rem => x % y,
        _ => return Err("unsupported float operation".into()),
    };
    finish_float(r, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_int_range_checks() {
        assert!(
            convert_scalar(&ConstValue::Int(1 << 40), Scalar::AbstractInt, Scalar::I32).is_err()
        );
        assert_eq!(
            convert_scalar(&ConstValue::Int(-1), Scalar::I32, Scalar::U32).unwrap(),
            ConstValue::Int(u32::MAX as i64)
        );
        assert_eq!(
            convert_scalar(&ConstValue::Float(-3.7), Scalar::F32, Scalar::I32).unwrap(),
            ConstValue::Int(-3)
        );
    }

    #[test]
    fn integer_overflow_is_an_error() {
        let max = ConstValue::Int(i32::MAX as i64);
        assert!(binary(BinaryOp::Add, &max, &ConstValue::Int(1), &crate::types::I32).is_err());
        assert!(binary(BinaryOp::Div, &max, &ConstValue::Int(0), &crate::types::I32).is_err());
        let ok = binary(
            BinaryOp::Add,
            &ConstValue::Int(2),
            &ConstValue::Int(3),
            &crate::types::U32,
        );
        assert_eq!(ok.unwrap(), ConstValue::Int(5));
    }

    #[test]
    fn vectors_fold_componentwise() {
        let a = ConstValue::Composite(vec![ConstValue::Float(1.0), ConstValue::Float(2.0)]);
        let b = ConstValue::Composite(vec![ConstValue::Float(0.5), ConstValue::Float(0.5)]);
        let ty = Type::Vector(2, Scalar::F32);
        let r = binary(BinaryOp::Mul, &a, &b, &ty).unwrap();
        assert_eq!(
            r,
            ConstValue::Composite(vec![ConstValue::Float(0.5), ConstValue::Float(1.0)])
        );
    }
}
