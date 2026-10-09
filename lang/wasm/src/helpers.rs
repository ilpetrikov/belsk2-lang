//! Small runtime functions the generated code calls. Each is added to the
//! module only when it is used.

use wasm_encoder::{BlockType, Instruction as I, ValType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Helper {
    /// `int` division that wraps `int.MinValue / -1` instead of trapping.
    DivI32,
    /// The same for `long`.
    DivI64,
    /// The floating-point remainder (`%` on doubles), like C's `fmod`.
    Fmod,
    /// Traps if a `bel` value exceeds 1000; returns the value.
    Bel,
    /// Turns an `i64` code point into a `char`, trapping if it is not a
    /// valid Unicode scalar value.
    Char,
}

impl Helper {
    pub fn signature(self) -> (Vec<ValType>, Vec<ValType>) {
        match self {
            Helper::DivI32 => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            Helper::DivI64 => (vec![ValType::I64, ValType::I64], vec![ValType::I64]),
            Helper::Fmod => (vec![ValType::F64, ValType::F64], vec![ValType::F64]),
            Helper::Bel => (vec![ValType::F64], vec![ValType::F64]),
            Helper::Char => (vec![ValType::I64], vec![ValType::I32]),
        }
    }

    pub fn body(self) -> Vec<I<'static>> {
        match self {
            Helper::DivI32 => vec![
                I::LocalGet(1),
                I::I32Const(-1),
                I::I32Eq,
                I::If(BlockType::Result(ValType::I32)),
                I::I32Const(0),
                I::LocalGet(0),
                I::I32Sub,
                I::Else,
                I::LocalGet(0),
                I::LocalGet(1),
                I::I32DivS,
                I::End,
            ],
            Helper::DivI64 => vec![
                I::LocalGet(1),
                I::I64Const(-1),
                I::I64Eq,
                I::If(BlockType::Result(ValType::I64)),
                I::I64Const(0),
                I::LocalGet(0),
                I::I64Sub,
                I::Else,
                I::LocalGet(0),
                I::LocalGet(1),
                I::I64DivS,
                I::End,
            ],
            // fmod(a, inf) = a for finite a; otherwise a - b * trunc(a / b)
            // with the sign of a.
            Helper::Fmod => vec![
                I::LocalGet(1),
                I::F64Abs,
                I::F64Const(f64::INFINITY.into()),
                I::F64Eq,
                I::LocalGet(0),
                I::F64Abs,
                I::F64Const(f64::INFINITY.into()),
                I::F64Ne,
                I::I32And,
                I::If(BlockType::Result(ValType::F64)),
                I::LocalGet(0),
                I::Else,
                I::LocalGet(0),
                I::LocalGet(0),
                I::LocalGet(1),
                I::F64Div,
                I::F64Trunc,
                I::LocalGet(1),
                I::F64Mul,
                I::F64Sub,
                I::LocalGet(0),
                I::F64Copysign,
                I::End,
            ],
            Helper::Bel => vec![
                I::LocalGet(0),
                I::F64Const(1000.0.into()),
                I::F64Gt,
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
                I::LocalGet(0),
            ],
            // Valid: 0..=0x10FFFF, except the surrogates 0xD800..=0xDFFF.
            Helper::Char => vec![
                I::LocalGet(0),
                I::I64Const(0x10FFFF),
                I::I64GtU,
                I::LocalGet(0),
                I::I64Const(0xD800),
                I::I64Sub,
                I::I64Const(0x800),
                I::I64LtU,
                I::I32Or,
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
                I::LocalGet(0),
                I::I32WrapI64,
            ],
        }
    }
}
