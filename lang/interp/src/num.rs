//! Numeric conversions and arithmetic with C# semantics: fixed-width
//! integers that wrap on overflow, `float` rounded to 32 bits, binary
//! numeric promotion.

use belsk2_syntax::ast::BinOp;
use belsk2_syntax::{Error, FloatKind, IntKind, NumTy, Result, Ty};

use crate::value::Value;

/// A number taken out of a [`Value`]; `char` counts as `ushort`.
#[derive(Debug, Clone, Copy)]
enum N {
    I(i128),
    F(f64),
}

fn num_of(v: &Value) -> Option<(NumTy, N)> {
    match v {
        Value::Int(i, k) => Some((NumTy::Int(*k), N::I(*i))),
        Value::Float(f, k) => Some((NumTy::Float(*k), N::F(*f))),
        Value::Char(c) => Some((NumTy::Int(IntKind::U16), N::I(u32::from(*c).into()))),
        _ => None,
    }
}

fn to_f64(n: N) -> f64 {
    match n {
        N::I(i) => i as f64,
        N::F(f) => f,
    }
}

fn make(n: N, ty: NumTy) -> Value {
    match ty {
        NumTy::Int(k) => Value::Int(
            k.wrap(match n {
                N::I(i) => i,
                // Truncates toward zero; NaN becomes 0, huge values saturate.
                N::F(f) => f as i128,
            }),
            k,
        ),
        NumTy::Float(k) => Value::Float(k.round(to_f64(n)), k),
    }
}

fn parse_str(s: &str, target: &Ty) -> Result<N> {
    let t = s.trim();
    if let Ok(i) = t.parse::<i128>() {
        return Ok(N::I(i));
    }
    match t.parse::<f64>() {
        Ok(f) => Ok(N::F(f)),
        Err(_) => Err(Error::runtime(format!(
            "cannot convert \"{s}\" to {target}"
        ))),
    }
}

fn char_from(code: i128) -> Result<Value> {
    u32::try_from(code)
        .ok()
        .and_then(char::from_u32)
        .map(Value::Char)
        .ok_or_else(|| Error::runtime(format!("{code} is not a valid char")))
}

/// An explicit conversion, as performed by `int(x)`, `byte(x)`,
/// `double(x)`, `char(x)` and by conversions the checker inserted.
/// Integer conversions wrap like C# unchecked casts.
pub fn convert(v: Value, to: &Ty) -> Result<Value> {
    let fail = |v: &Value| Error::type_error(format!("cannot convert {} to {to}", v.type_name()));
    let target = to.canonical();
    if let Some(ty) = target.num() {
        let n = match &v {
            Value::Bool(b) => N::I(i128::from(*b)),
            Value::String(s) => {
                let n = parse_str(s, to)?;
                if let (N::I(i), NumTy::Int(k)) = (n, ty) {
                    if !k.fits(i) {
                        return Err(Error::runtime(format!(
                            "{i} is out of range for {}",
                            k.name()
                        )));
                    }
                }
                n
            }
            other => num_of(other).ok_or_else(|| fail(other))?.1,
        };
        return Ok(make(n, ty));
    }
    match (target, v) {
        (Ty::Any, v) => Ok(v),
        (Ty::Char, Value::Char(c)) => Ok(Value::Char(c)),
        (Ty::Char, Value::String(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(Value::Char(c)),
                _ => Err(Error::runtime(format!("\"{s}\" is not a single character"))),
            }
        }
        (Ty::Char, v) => match num_of(&v) {
            Some((_, n)) => char_from(match n {
                N::I(i) => i,
                N::F(f) => f as i128,
            }),
            None => Err(fail(&v)),
        },
        (Ty::String, Value::Char(c)) => Ok(Value::String(c.to_string())),
        (t, v) if v.fits(&t) => Ok(v),
        (_, v) => Err(fail(&v)),
    }
}

/// Converts a value being stored in a place of type `ty` (variable,
/// parameter, return value). Statically typed values arrive already
/// converted; values of type `any` are checked here and may only be
/// converted without losing information.
pub fn coerce(ty: &Ty, v: Value, name: &str) -> Result<Value> {
    // Type parameters are not known at run time; they accept anything.
    if ty.has_params() {
        let erased = ty.substitute(&|_| Some(Ty::Any));
        return coerce(&erased, v, name);
    }
    let mismatch = |v: &Value| {
        Error::type_error(format!(
            "cannot store {} value in {name} of type {ty}",
            v.type_name()
        ))
    };
    if ty.is_any() {
        return Ok(v);
    }
    if matches!(v, Value::Null) {
        return if ty.nullable() {
            Ok(v)
        } else {
            Err(Error::type_error(format!(
                "{name} of type {ty} cannot be null"
            )))
        };
    }
    let target = ty.canonical();
    let v = if let (Some(to), Some((from, n))) = (target.num(), num_of(&v)) {
        let lossless = from.widens_to(to)
            || match (n, to) {
                (N::I(i), NumTy::Int(k)) => k.fits(i),
                (N::I(_), NumTy::Float(_)) => true,
                (N::F(_), NumTy::Float(_)) => true,
                (N::F(_), NumTy::Int(_)) => false,
            };
        if !lossless || matches!(v, Value::Char(_)) && !NumTy::Int(IntKind::U16).widens_to(to) {
            return Err(mismatch(&v));
        }
        make(n, to)
    } else {
        match (&target, v) {
            (Ty::String, Value::Char(c)) => Value::String(c.to_string()),
            (Ty::Char, Value::String(s)) if s.chars().count() == 1 => {
                convert(Value::String(s), &Ty::Char)?
            }
            (t, v) if v.fits(t) => v,
            (_, v) => return Err(mismatch(&v)),
        }
    };
    if let (Ty::Bel, Value::Float(f, _)) = (ty, &v) {
        if *f > 1000.0 {
            return Err(Error::type_error(format!(
                "bel value {} exceeds the maximum of 1000 for {name}",
                crate::value::format_float(*f, FloatKind::F64)
            )));
        }
    }
    Ok(v)
}

fn int_shift(op: BinOp, v: i128, count: i128, k: IntKind) -> i128 {
    let mask = if k.bits() == 64 { 63 } else { 31 };
    let count = (count & mask) as u32;
    match op {
        BinOp::Shl => k.wrap(v.wrapping_shl(count)),
        _ => k.wrap(v >> count),
    }
}

/// Arithmetic, bitwise and shift operators on numbers (and chars, which
/// count as `ushort`). Returns `None` if the operands are not numbers.
pub fn arith(op: BinOp, l: &Value, r: &Value) -> Option<Result<Value>> {
    let ((lt, ln), (rt, rn)) = (num_of(l)?, num_of(r)?);
    let mismatch = || {
        Error::type_error(format!(
            "cannot apply '{}' to {} and {}",
            op.symbol(),
            l.type_name(),
            r.type_name()
        ))
    };
    if matches!(op, BinOp::Shl | BinOp::Shr) {
        let (NumTy::Int(k), N::I(v), N::I(count)) = (lt, ln, rn) else {
            return Some(Err(mismatch()));
        };
        let k = NumTy::int_promoted(k);
        return Some(Ok(Value::Int(int_shift(op, v, count, k), k)));
    }
    let Some(common) = NumTy::promote(lt, rt) else {
        return Some(Err(mismatch()));
    };
    Some(match common {
        NumTy::Int(k) => {
            let (a, b) = match (make(ln, common), make(rn, common)) {
                (Value::Int(a, _), Value::Int(b, _)) => (a, b),
                _ => return Some(Err(mismatch())),
            };
            let v = match op {
                BinOp::Add => a.wrapping_add(b),
                BinOp::Sub => a.wrapping_sub(b),
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::Div | BinOp::Rem if b == 0 => {
                    return Some(Err(Error::runtime("division by zero")))
                }
                BinOp::Div => a.wrapping_div(b),
                BinOp::Rem => a.wrapping_rem(b),
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                BinOp::BitXor => a ^ b,
                _ => return Some(Err(mismatch())),
            };
            Ok(Value::Int(k.wrap(v), k))
        }
        NumTy::Float(k) => {
            let (a, b) = (k.round(to_f64(ln)), k.round(to_f64(rn)));
            let v = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                // Floating-point division by zero gives infinity, as in C#.
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                _ => return Some(Err(mismatch())),
            };
            Ok(Value::Float(k.round(v), k))
        }
    })
}

/// Compares two numbers (or chars). `None` if either is not a number or
/// one is NaN.
pub fn compare(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
    let ((_, a), (_, b)) = (num_of(l)?, num_of(r)?);
    match (a, b) {
        (N::I(a), N::I(b)) => Some(a.cmp(&b)),
        (a, b) => to_f64(a).partial_cmp(&to_f64(b)),
    }
}

/// Unary `-`: small integers become `int`, `uint` becomes `long`.
pub fn negate(v: &Value) -> Result<Value> {
    let Some((ty, n)) = num_of(v) else {
        return Err(Error::type_error(format!(
            "cannot negate {}",
            v.type_name()
        )));
    };
    let Some(result) = ty.negated() else {
        return Err(Error::type_error(format!(
            "cannot negate {}",
            v.type_name()
        )));
    };
    Ok(match n {
        N::I(i) => make(N::I(-i), result),
        N::F(f) => make(N::F(-f), result),
    })
}

/// Unary `~`.
pub fn bit_not(v: &Value) -> Result<Value> {
    match num_of(v) {
        Some((NumTy::Int(k), N::I(i))) => {
            let k = NumTy::int_promoted(k);
            Ok(Value::Int(k.wrap(!i), k))
        }
        _ => Err(Error::type_error(format!(
            "cannot apply '~' to {}",
            v.type_name()
        ))),
    }
}

/// Numeric equality across types (`1 == 1.0`).
pub fn num_eq(l: &Value, r: &Value) -> Option<bool> {
    compare(l, r).map(|o| o.is_eq()).or_else(|| {
        // NaN is never equal to anything.
        num_of(l).and(num_of(r)).map(|_| false)
    })
}
