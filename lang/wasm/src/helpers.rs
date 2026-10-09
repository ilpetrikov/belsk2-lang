//! Runtime functions the generated code calls. Each is added to the module
//! only when it is used.

use belsk2_syntax::{Error, FloatKind, Ty};
use wasm_encoder::{BlockType, Instruction as I, ValType};

use crate::codegen::Ctx;
use crate::types::nullable;

/// A function body under construction.
pub struct Asm {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    pub locals: Vec<ValType>,
    pub code: Vec<I<'static>>,
}

impl Asm {
    pub fn new(params: Vec<ValType>, results: Vec<ValType>) -> Self {
        Asm {
            params,
            results,
            locals: Vec::new(),
            code: Vec::new(),
        }
    }

    pub fn local(&mut self, t: ValType) -> u32 {
        self.locals.push(t);
        (self.params.len() + self.locals.len() - 1) as u32
    }

    pub fn op(&mut self, ins: I<'static>) {
        self.code.push(ins);
    }

    pub fn ops(&mut self, ins: impl IntoIterator<Item = I<'static>>) {
        self.code.extend(ins);
    }
}

#[derive(Debug, Clone, PartialEq)]
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
    /// Concatenates two non-null strings.
    Concat,
    /// The string itself, or `"null"`.
    StrOrNull,
    /// String equality (`null` equals only `null`).
    StrEq,
    /// Compares two strings: -1, 0 or 1.
    StrCmp,
    /// `substr(s, start, length)` with `i64` bounds, clamped like the
    /// interpreter does.
    Substr,
    /// Formats an `i64` (as unsigned for `ulong`).
    IntToStr { unsigned: bool },
    /// `a[i]` with a bounds check.
    ArrGet(Ty),
    /// `a[i] = v` with a bounds check.
    ArrSet(Ty),
    /// `push(a, v)`: returns `a`.
    ArrPush(Ty),
    /// `pop(a)`: traps on an empty array.
    ArrPop(Ty),
    /// Element-wise equality.
    ArrEq(Ty),
}

/// Builds the body of a helper.
pub fn build(ctx: &mut Ctx, h: &Helper) -> Result<Asm, Error> {
    use ValType::{F64, I32, I64};
    let s = ctx.types.string();
    let sref = nullable(s);
    let mut a;
    match h {
        Helper::DivI32 | Helper::DivI64 => {
            let wide = *h == Helper::DivI64;
            let t = if wide { I64 } else { I32 };
            a = Asm::new(vec![t, t], vec![t]);
            if wide {
                a.ops([I::LocalGet(1), I::I64Const(-1), I::I64Eq]);
            } else {
                a.ops([I::LocalGet(1), I::I32Const(-1), I::I32Eq]);
            }
            a.op(I::If(BlockType::Result(t)));
            if wide {
                a.ops([I::I64Const(0), I::LocalGet(0), I::I64Sub]);
            } else {
                a.ops([I::I32Const(0), I::LocalGet(0), I::I32Sub]);
            }
            a.ops([I::Else, I::LocalGet(0), I::LocalGet(1)]);
            a.op(if wide { I::I64DivS } else { I::I32DivS });
            a.op(I::End);
        }
        // fmod(a, inf) = a for finite a; otherwise a - b * trunc(a / b)
        // with the sign of a.
        Helper::Fmod => {
            a = Asm::new(vec![F64, F64], vec![F64]);
            a.ops([
                I::LocalGet(1),
                I::F64Abs,
                I::F64Const(f64::INFINITY.into()),
                I::F64Eq,
                I::LocalGet(0),
                I::F64Abs,
                I::F64Const(f64::INFINITY.into()),
                I::F64Ne,
                I::I32And,
                I::If(BlockType::Result(F64)),
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
            ]);
        }
        Helper::Bel => {
            a = Asm::new(vec![F64], vec![F64]);
            a.ops([
                I::LocalGet(0),
                I::F64Const(1000.0.into()),
                I::F64Gt,
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
                I::LocalGet(0),
            ]);
        }
        // Valid: 0..=0x10FFFF, except the surrogates 0xD800..=0xDFFF.
        Helper::Char => {
            a = Asm::new(vec![I64], vec![I32]);
            a.ops([
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
            ]);
        }
        Helper::Concat => {
            a = Asm::new(vec![sref, sref], vec![sref]);
            let out = a.local(sref);
            a.ops([
                I::LocalGet(0),
                I::ArrayLen,
                I::LocalGet(1),
                I::ArrayLen,
                I::I32Add,
                I::ArrayNewDefault(s),
                I::LocalSet(out),
            ]);
            let copy = I::ArrayCopy {
                array_type_index_dst: s,
                array_type_index_src: s,
            };
            a.ops([
                I::LocalGet(out),
                I::I32Const(0),
                I::LocalGet(0),
                I::I32Const(0),
                I::LocalGet(0),
                I::ArrayLen,
                copy.clone(),
            ]);
            a.ops([
                I::LocalGet(out),
                I::LocalGet(0),
                I::ArrayLen,
                I::LocalGet(1),
                I::I32Const(0),
                I::LocalGet(1),
                I::ArrayLen,
                copy,
                I::LocalGet(out),
            ]);
        }
        Helper::StrOrNull => {
            a = Asm::new(vec![sref], vec![sref]);
            a.ops([I::LocalGet(0), I::RefIsNull, I::If(BlockType::Result(sref))]);
            a.ops(ctx.string_lit("null"));
            a.ops([I::Else, I::LocalGet(0), I::End]);
        }
        Helper::StrEq => {
            a = Asm::new(vec![sref, sref], vec![I32]);
            seq_eq(
                &mut a,
                |a, i| {
                    a.ops([I::LocalGet(0), I::LocalGet(i), I::ArrayGet(s)]);
                    a.ops([I::LocalGet(1), I::LocalGet(i), I::ArrayGet(s), I::I32Eq]);
                },
                |a| a.op(I::ArrayLen),
            );
        }
        Helper::StrCmp => {
            a = Asm::new(vec![sref, sref], vec![I32]);
            let (la, lb, n, i, x, y) = (
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
            );
            a.ops([
                I::LocalGet(0),
                I::ArrayLen,
                I::LocalSet(la),
                I::LocalGet(1),
                I::ArrayLen,
                I::LocalSet(lb),
                I::LocalGet(la),
                I::LocalGet(lb),
                I::LocalGet(la),
                I::LocalGet(lb),
                I::I32LtU,
                I::Select,
                I::LocalSet(n),
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(0),
                I::LocalGet(i),
                I::ArrayGet(s),
                I::LocalSet(x),
                I::LocalGet(1),
                I::LocalGet(i),
                I::ArrayGet(s),
                I::LocalSet(y),
                I::LocalGet(x),
                I::LocalGet(y),
                I::I32LtU,
                I::If(BlockType::Empty),
                I::I32Const(-1),
                I::Return,
                I::End,
                I::LocalGet(x),
                I::LocalGet(y),
                I::I32GtU,
                I::If(BlockType::Empty),
                I::I32Const(1),
                I::Return,
                I::End,
                I::LocalGet(i),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(i),
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(la),
                I::LocalGet(lb),
                I::I32GtU,
                I::LocalGet(la),
                I::LocalGet(lb),
                I::I32LtU,
                I::I32Sub,
            ]);
        }
        Helper::Substr => {
            a = Asm::new(vec![sref, I64, I64], vec![sref]);
            let (n, st, ln, out) = (a.local(I64), a.local(I64), a.local(I64), a.local(sref));
            a.ops([
                I::LocalGet(0),
                I::ArrayLen,
                I::I64ExtendI32U,
                I::LocalSet(n),
                I::LocalGet(1),
                I::LocalSet(st),
                I::LocalGet(st),
                I::I64Const(0),
                I::I64LtS,
                I::If(BlockType::Empty),
                I::I64Const(0),
                I::LocalSet(st),
                I::End,
                I::LocalGet(st),
                I::LocalGet(n),
                I::I64GtS,
                I::If(BlockType::Empty),
                I::LocalGet(n),
                I::LocalSet(st),
                I::End,
                I::LocalGet(2),
                I::LocalSet(ln),
                I::LocalGet(ln),
                I::I64Const(0),
                I::I64LtS,
                I::If(BlockType::Empty),
                I::I64Const(0),
                I::LocalSet(ln),
                I::End,
                I::LocalGet(ln),
                I::LocalGet(n),
                I::LocalGet(st),
                I::I64Sub,
                I::I64GtS,
                I::If(BlockType::Empty),
                I::LocalGet(n),
                I::LocalGet(st),
                I::I64Sub,
                I::LocalSet(ln),
                I::End,
                I::LocalGet(ln),
                I::I32WrapI64,
                I::ArrayNewDefault(s),
                I::LocalSet(out),
                I::LocalGet(out),
                I::I32Const(0),
                I::LocalGet(0),
                I::LocalGet(st),
                I::I32WrapI64,
                I::LocalGet(ln),
                I::I32WrapI64,
                I::ArrayCopy {
                    array_type_index_dst: s,
                    array_type_index_src: s,
                },
                I::LocalGet(out),
            ]);
        }
        Helper::IntToStr { unsigned } => {
            a = Asm::new(vec![I64], vec![sref]);
            let (buf, pos, neg, u, out) = (
                a.local(sref),
                a.local(I32),
                a.local(I32),
                a.local(I64),
                a.local(sref),
            );
            a.ops([
                I::I32Const(20),
                I::ArrayNewDefault(s),
                I::LocalSet(buf),
                I::I32Const(20),
                I::LocalSet(pos),
                I::LocalGet(0),
                I::LocalSet(u),
            ]);
            if !unsigned {
                a.ops([
                    I::LocalGet(0),
                    I::I64Const(0),
                    I::I64LtS,
                    I::LocalTee(neg),
                    I::If(BlockType::Empty),
                    I::I64Const(0),
                    I::LocalGet(0),
                    I::I64Sub,
                    I::LocalSet(u),
                    I::End,
                ]);
            }
            let dec = [
                I::LocalGet(pos),
                I::I32Const(1),
                I::I32Sub,
                I::LocalSet(pos),
            ];
            a.op(I::Loop(BlockType::Empty));
            a.ops(dec.clone());
            a.ops([
                I::LocalGet(buf),
                I::LocalGet(pos),
                I::LocalGet(u),
                I::I64Const(10),
                I::I64RemU,
                I::I32WrapI64,
                I::I32Const(48),
                I::I32Add,
                I::ArraySet(s),
                I::LocalGet(u),
                I::I64Const(10),
                I::I64DivU,
                I::LocalTee(u),
                I::I64Const(0),
                I::I64Ne,
                I::BrIf(0),
                I::End,
            ]);
            if !unsigned {
                a.ops([I::LocalGet(neg), I::If(BlockType::Empty)]);
                a.ops(dec);
                a.ops([
                    I::LocalGet(buf),
                    I::LocalGet(pos),
                    I::I32Const(45),
                    I::ArraySet(s),
                    I::End,
                ]);
            }
            a.ops([
                I::I32Const(20),
                I::LocalGet(pos),
                I::I32Sub,
                I::ArrayNewDefault(s),
                I::LocalSet(out),
                I::LocalGet(out),
                I::I32Const(0),
                I::LocalGet(buf),
                I::LocalGet(pos),
                I::I32Const(20),
                I::LocalGet(pos),
                I::I32Sub,
                I::ArrayCopy {
                    array_type_index_dst: s,
                    array_type_index_src: s,
                },
                I::LocalGet(out),
            ]);
        }
        Helper::ArrGet(elem)
        | Helper::ArrSet(elem)
        | Helper::ArrPush(elem)
        | Helper::ArrPop(elem)
        | Helper::ArrEq(elem) => {
            let Some(at) = ctx.types.array(elem) else {
                return Err(Error::compile(format!(
                    "arrays of {elem} are not supported in WASM builds yet"
                )));
            };
            let aref = nullable(at.obj);
            let data = I::StructGet {
                struct_type_index: at.obj,
                field_index: 0,
            };
            let len = I::StructGet {
                struct_type_index: at.obj,
                field_index: 1,
            };
            let check = [
                I::LocalGet(1),
                I::LocalGet(0),
                len.clone(),
                I::I32GeU,
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
            ];
            match h {
                Helper::ArrGet(_) => {
                    a = Asm::new(vec![aref, I32], vec![at.elem]);
                    a.ops(check);
                    a.ops([I::LocalGet(0), data, I::LocalGet(1), I::ArrayGet(at.raw)]);
                }
                Helper::ArrSet(_) => {
                    a = Asm::new(vec![aref, I32, at.elem], vec![]);
                    a.ops(check);
                    a.ops([
                        I::LocalGet(0),
                        data,
                        I::LocalGet(1),
                        I::LocalGet(2),
                        I::ArraySet(at.raw),
                    ]);
                }
                Helper::ArrPush(_) => {
                    a = Asm::new(vec![aref, at.elem], vec![aref]);
                    let (n, d, new) = (
                        a.local(I32),
                        a.local(nullable(at.raw)),
                        a.local(nullable(at.raw)),
                    );
                    a.ops([
                        I::LocalGet(0),
                        len,
                        I::LocalSet(n),
                        I::LocalGet(0),
                        data,
                        I::LocalSet(d),
                        I::LocalGet(n),
                        I::LocalGet(d),
                        I::ArrayLen,
                        I::I32Eq,
                        I::If(BlockType::Empty),
                        I::LocalGet(n),
                        I::I32Const(2),
                        I::I32Mul,
                        I::I32Const(4),
                        I::I32Add,
                        I::ArrayNewDefault(at.raw),
                        I::LocalSet(new),
                        I::LocalGet(new),
                        I::I32Const(0),
                        I::LocalGet(d),
                        I::I32Const(0),
                        I::LocalGet(n),
                        I::ArrayCopy {
                            array_type_index_dst: at.raw,
                            array_type_index_src: at.raw,
                        },
                        I::LocalGet(0),
                        I::LocalGet(new),
                        I::RefAsNonNull,
                        I::StructSet {
                            struct_type_index: at.obj,
                            field_index: 0,
                        },
                        I::LocalGet(new),
                        I::LocalSet(d),
                        I::End,
                        I::LocalGet(d),
                        I::LocalGet(n),
                        I::LocalGet(1),
                        I::ArraySet(at.raw),
                        I::LocalGet(0),
                        I::LocalGet(n),
                        I::I32Const(1),
                        I::I32Add,
                        I::StructSet {
                            struct_type_index: at.obj,
                            field_index: 1,
                        },
                        I::LocalGet(0),
                    ]);
                }
                Helper::ArrPop(_) => {
                    a = Asm::new(vec![aref], vec![at.elem]);
                    let n = a.local(I32);
                    a.ops([
                        I::LocalGet(0),
                        len,
                        I::LocalTee(n),
                        I::I32Eqz,
                        I::If(BlockType::Empty),
                        I::Unreachable,
                        I::End,
                        I::LocalGet(0),
                        I::LocalGet(n),
                        I::I32Const(1),
                        I::I32Sub,
                        I::LocalTee(n),
                        I::StructSet {
                            struct_type_index: at.obj,
                            field_index: 1,
                        },
                        I::LocalGet(0),
                        data,
                        I::LocalGet(n),
                        I::ArrayGet(at.raw),
                    ]);
                }
                _ => {
                    a = Asm::new(vec![aref, aref], vec![I32]);
                    let eq: Vec<I<'static>> = match elem.canonical() {
                        Ty::String => vec![I::Call(ctx.helper(Helper::StrEq))],
                        Ty::Array(inner) => {
                            vec![I::Call(ctx.helper(Helper::ArrEq((*inner).clone())))]
                        }
                        Ty::Float(FloatKind::F32) => vec![I::F32Eq],
                        Ty::Float(FloatKind::F64) => vec![I::F64Eq],
                        Ty::Int(k) if k.bits() == 64 => vec![I::I64Eq],
                        _ => vec![I::I32Eq],
                    };
                    seq_eq(
                        &mut a,
                        |a, i| {
                            a.ops([I::LocalGet(0), data.clone(), I::LocalGet(i)]);
                            a.op(I::ArrayGet(at.raw));
                            a.ops([I::LocalGet(1), data.clone(), I::LocalGet(i)]);
                            a.op(I::ArrayGet(at.raw));
                            a.ops(eq.iter().cloned());
                        },
                        |a| a.op(len.clone()),
                    );
                }
            }
        }
    }
    Ok(a)
}

/// Equality of two sequences in locals 0 and 1: the same reference, or
/// both non-null with equal lengths (`len` reads one) and equal items
/// (`item_eq` leaves an `i32` for item `i`).
fn seq_eq(a: &mut Asm, item_eq: impl Fn(&mut Asm, u32), len: impl Fn(&mut Asm)) {
    let (n, i) = (a.local(ValType::I32), a.local(ValType::I32));
    let ret = |a: &mut Asm, v: i32| {
        a.ops([I::If(BlockType::Empty), I::I32Const(v), I::Return, I::End]);
    };
    a.ops([I::LocalGet(0), I::LocalGet(1), I::RefEq]);
    ret(a, 1);
    a.ops([
        I::LocalGet(0),
        I::RefIsNull,
        I::LocalGet(1),
        I::RefIsNull,
        I::I32Or,
    ]);
    ret(a, 0);
    a.op(I::LocalGet(0));
    len(a);
    a.op(I::LocalTee(n));
    a.op(I::LocalGet(1));
    len(a);
    a.op(I::I32Ne);
    ret(a, 0);
    a.op(I::Loop(BlockType::Empty));
    a.ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU]);
    ret(a, 1);
    item_eq(a, i);
    a.op(I::I32Eqz);
    ret(a, 0);
    a.ops([
        I::LocalGet(i),
        I::I32Const(1),
        I::I32Add,
        I::LocalSet(i),
        I::Br(0),
        I::End,
        I::Unreachable,
    ]);
}
