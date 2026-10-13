//! `any` in WASM builds.
//!
//! A value of type `any` is an `eqref`. Strings, arrays, dictionaries and
//! objects are stored as they are; numbers, `bool`, `char` and enums
//! without data are boxed in a struct `{ kind, value }` (see
//! [`crate::types::Boxes`]), where `kind` tells the Belsk2 type.

use belsk2_syntax::ast::{BinOp, Expr, UnaryOp};
use belsk2_syntax::{FloatKind, IntKind, NumTy, Span, Ty, TypeKind};
use wasm_encoder::{BlockType, HeapType, Instruction as I, ValType};

use super::objects::{self, obj_fn};
use super::{runtime_fn, unsupported, Ctx, FnGen, R};
use crate::helpers::{Asm, Helper};
use crate::types::{any_type, nullable, Boxes};

const BOOL: i32 = 1;
const CHAR: i32 = 2;
/// `3 + i` for the integer type `IntKind::ALL[i]`.
const INT: i32 = 3;
const FLOAT: i32 = 11;
const DOUBLE: i32 = 12;
/// `ENUM + tag` for an enum without data.
const ENUM: i32 = 1000;

fn int_kind(k: IntKind) -> i32 {
    INT + IntKind::ALL.iter().position(|&x| x == k).unwrap_or(0) as i32
}

/// The kind of a boxed value of type `ty`, and which box holds it.
fn kind_of(ctx: &Ctx, ty: &Ty) -> Option<(i32, ValType)> {
    Some(match ty.canonical() {
        Ty::Bool => (BOOL, ValType::I32),
        Ty::Char => (CHAR, ValType::I32),
        Ty::Int(k) => (
            int_kind(k),
            if k.bits() == 64 {
                ValType::I64
            } else {
                ValType::I32
            },
        ),
        Ty::Float(FloatKind::F32) => (FLOAT, ValType::F32),
        Ty::Float(FloatKind::F64) => (DOUBLE, ValType::F64),
        Ty::Named(t) => {
            let c = ctx.classes.get(&t.id())?;
            if c.st.is_some() {
                return None;
            }
            (ENUM + c.tag, ValType::I32)
        }
        _ => return None,
    })
}

fn box_for(b: &Boxes, vt: ValType) -> u32 {
    match vt {
        ValType::I64 => b.i64,
        ValType::F32 => b.f32,
        ValType::F64 => b.f64,
        _ => b.i32,
    }
}

/// Whether an `i32` box of this kind holds an unsigned value.
fn unsigned_test(a: &mut Asm, kind: u32) {
    // bool, char, byte, ushort, uint
    a.ops([
        I::LocalGet(kind),
        I::I32Const(BOOL),
        I::I32Eq,
        I::LocalGet(kind),
        I::I32Const(CHAR),
        I::I32Eq,
        I::I32Or,
        I::LocalGet(kind),
        I::I32Const(int_kind(IntKind::U8)),
        I::I32Eq,
        I::I32Or,
        I::LocalGet(kind),
        I::I32Const(int_kind(IntKind::U16)),
        I::I32Eq,
        I::I32Or,
        I::LocalGet(kind),
        I::I32Const(int_kind(IntKind::U32)),
        I::I32Eq,
        I::I32Or,
    ]);
}

impl FnGen<'_> {
    /// Boxes the value of type `from` on the stack (if it needs a box).
    pub(super) fn box_value(&mut self, from: &Ty, span: Span) -> R<()> {
        let Some((kind, vt)) = kind_of(self.ctx, &self.t(from)) else {
            let vt = self.vt(from, span)?;
            if matches!(vt, ValType::Ref(_)) {
                return Ok(());
            }
            return Err(unsupported(format!("storing {from} in an any is"), span));
        };
        let boxes = self.ctx.types.boxes();
        let tmp = self.a.local(vt);
        self.a.ops([
            I::LocalSet(tmp),
            I::I32Const(kind),
            I::LocalGet(tmp),
            I::StructNew(box_for(&boxes, vt)),
        ]);
        Ok(())
    }

    /// Turns the `any` on the stack into a value of type `to`, trapping if
    /// it is not one.
    pub(super) fn unbox(&mut self, to: &Ty, span: Span) -> R<()> {
        let to = self.t(to);
        let want = self.vt(&to, span)?;
        if let ValType::Ref(r) = want {
            if !to.is_any() {
                self.emit(I::RefCastNullable(r.heap_type));
            }
            return Ok(());
        }
        let boxes = self.ctx.types.boxes();
        let v = self.a.local(any_type());
        self.emit(I::LocalSet(v));
        let Some(target) = to.num() else {
            // bool, char, an enum: the box holds exactly that.
            self.a.ops([
                I::LocalGet(v),
                I::RefCastNonNull(HeapType::Concrete(boxes.i32)),
                I::StructGet {
                    struct_type_index: boxes.i32,
                    field_index: 1,
                },
            ]);
            return Ok(());
        };
        // A string holding a number is parsed first.
        let string = self.ctx.types.string();
        let parse = runtime_fn(self.ctx, "rtParse")?;
        self.a.ops([
            I::LocalGet(v),
            I::RefTestNonNull(HeapType::Concrete(string)),
            I::If(BlockType::Empty),
            I::LocalGet(v),
            I::RefCastNullable(HeapType::Concrete(string)),
            I::Call(parse),
            I::LocalSet(v),
            I::End,
        ]);
        // A number of any type converts to the number wanted.
        self.emit(I::Block(BlockType::Result(want)));
        self.depth += 1;
        for (bt, vt) in [
            (boxes.i32, ValType::I32),
            (boxes.i64, ValType::I64),
            (boxes.f32, ValType::F32),
            (boxes.f64, ValType::F64),
        ] {
            self.a.ops([
                I::LocalGet(v),
                I::RefTestNonNull(HeapType::Concrete(bt)),
                I::If(BlockType::Empty),
            ]);
            let get = [
                I::LocalGet(v),
                I::RefCastNonNull(HeapType::Concrete(bt)),
                I::StructGet {
                    struct_type_index: bt,
                    field_index: 1,
                },
            ];
            match vt {
                ValType::I32 => {
                    let kind = self.a.local(ValType::I32);
                    self.a.ops([
                        I::LocalGet(v),
                        I::RefCastNonNull(HeapType::Concrete(bt)),
                        I::StructGet {
                            struct_type_index: bt,
                            field_index: 0,
                        },
                        I::LocalSet(kind),
                    ]);
                    self.a.ops(get.clone());
                    self.emit(I::I64ExtendI32U);
                    self.a.ops(get);
                    self.emit(I::I64ExtendI32S);
                    unsigned_test(&mut self.a, kind);
                    self.emit(I::Select);
                    self.num_convert(NumTy::Int(IntKind::I64), target);
                }
                ValType::I64 => {
                    self.a.ops(get);
                    self.num_convert(NumTy::Int(IntKind::I64), target);
                }
                ValType::F32 => {
                    self.a.ops(get);
                    self.num_convert(NumTy::Float(FloatKind::F32), target);
                }
                _ => {
                    self.a.ops(get);
                    self.num_convert(NumTy::Float(FloatKind::F64), target);
                }
            }
            self.emit(I::Br(1));
            self.emit(I::End);
        }
        self.emit(I::Unreachable);
        self.emit(I::End);
        self.depth -= 1;
        Ok(())
    }

    /// A binary operator with an `any` operand, checked at run time.
    pub(super) fn any_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> R<()> {
        self.value(lhs, &Ty::Any)?;
        self.value(rhs, &Ty::Any)?;
        self.any_op(op, span)
    }

    /// Applies `op` to the two `any` values on the stack.
    pub(super) fn any_op(&mut self, op: BinOp, span: Span) -> R<()> {
        use BinOp::*;
        let (name, code) = match op {
            Add => ("rtAnyOp", 0),
            Sub => ("rtAnyOp", 1),
            Mul => ("rtAnyOp", 2),
            Div => ("rtAnyOp", 3),
            Rem => ("rtAnyOp", 4),
            BitAnd => ("rtAnyOp", 5),
            BitOr => ("rtAnyOp", 6),
            BitXor => ("rtAnyOp", 7),
            Shl => ("rtAnyOp", 8),
            Shr => ("rtAnyOp", 9),
            Lt => ("rtAnyCmp", 0),
            Gt => ("rtAnyCmp", 1),
            Le => ("rtAnyCmp", 2),
            Ge => ("rtAnyCmp", 3),
            _ => {
                return Err(unsupported(
                    format!("'{}' on an any is", op.symbol()),
                    span,
                ))
            }
        };
        let f = runtime_fn(self.ctx, name)?;
        self.a.ops([I::I32Const(code), I::Call(f)]);
        Ok(())
    }

    /// `-x` and `~x` for an `any`.
    pub(super) fn any_unary(&mut self, op: UnaryOp, inner: &Expr, span: Span) -> R<()> {
        let name = match op {
            UnaryOp::Neg => "rtAnyNeg",
            UnaryOp::BitNot => "rtAnyBitNot",
            UnaryOp::Not => return Err(unsupported("'!' on an any here is".to_string(), span)),
        };
        self.value(inner, &Ty::Any)?;
        let f = runtime_fn(self.ctx, name)?;
        self.emit(I::Call(f));
        Ok(())
    }

    /// Pushes whether the `any` in local `v` is a `t` (`null` is not).
    pub(super) fn any_test(&mut self, v: u32, t: &Ty, span: Span) -> R<()> {
        let t = self.t(t);
        let test = |heap: u32| [I::LocalGet(v), I::RefTestNonNull(HeapType::Concrete(heap))];
        match t.canonical() {
            Ty::Any => self.a.ops([I::LocalGet(v), I::RefIsNull, I::I32Eqz]),
            Ty::String => {
                let s = self.ctx.types.string();
                self.a.ops(test(s));
            }
            Ty::Named(n)
                if self
                    .ctx
                    .classes
                    .get(&n.id())
                    .is_some_and(|c| c.st.is_some()) =>
            {
                let object = self.ctx.object;
                let f = objects::is_fn(self.ctx, &n);
                self.a.ops(test(object));
                self.a.ops([
                    I::If(BlockType::Result(ValType::I32)),
                    I::LocalGet(v),
                    I::RefCastNullable(HeapType::Concrete(object)),
                    I::Call(f),
                    I::Else,
                    I::I32Const(0),
                    I::End,
                ]);
            }
            other => match kind_of(self.ctx, &other) {
                Some((kind, vt)) => {
                    let bt = box_for(&self.ctx.types.boxes(), vt);
                    self.a.ops(test(bt));
                    self.a.ops([
                        I::If(BlockType::Result(ValType::I32)),
                        I::LocalGet(v),
                        I::RefCastNonNull(HeapType::Concrete(bt)),
                        I::StructGet {
                            struct_type_index: bt,
                            field_index: 0,
                        },
                        I::I32Const(kind),
                        I::I32Eq,
                        I::Else,
                        I::I32Const(0),
                        I::End,
                    ]);
                }
                None => match self.vt(&other, span)? {
                    ValType::Ref(r) => self.a.ops([I::LocalGet(v), I::RefTestNonNull(r.heap_type)]),
                    _ => return Err(unsupported(format!("testing an any for {other} is"), span)),
                },
            },
        }
        Ok(())
    }
}

/// `a == b` for two `any` values: the same object or string, or numbers
/// with the same value (`1 == 1L`).
pub fn eq_fn(ctx: &mut Ctx) -> u32 {
    obj_fn(ctx, "AnyEq".to_string(), |ctx| {
        let s = ctx.types.string();
        let boxes = ctx.types.boxes();
        let to_f64 = num_fn(ctx);
        let str_eq = ctx.helper(Helper::StrEq);
        let mut a = Asm::new(vec![any_type(), any_type()], vec![ValType::I32]);
        let ret = |a: &mut Asm, v: i32| {
            a.ops([I::If(BlockType::Empty), I::I32Const(v), I::Return, I::End])
        };
        a.ops([I::LocalGet(0), I::LocalGet(1), I::RefEq]);
        ret(&mut a, 1);
        a.ops([
            I::LocalGet(0),
            I::RefIsNull,
            I::LocalGet(1),
            I::RefIsNull,
            I::I32Or,
        ]);
        ret(&mut a, 0);
        let both = |a: &mut Asm, heap: u32| {
            a.ops([
                I::LocalGet(0),
                I::RefTestNonNull(HeapType::Concrete(heap)),
                I::LocalGet(1),
                I::RefTestNonNull(HeapType::Concrete(heap)),
                I::I32And,
            ]);
        };
        both(&mut a, s);
        a.ops([
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::RefCastNullable(HeapType::Concrete(s)),
            I::LocalGet(1),
            I::RefCastNullable(HeapType::Concrete(s)),
            I::Call(str_eq),
            I::Return,
            I::End,
        ]);
        // Numbers compare by value; bools, chars and enums by kind and
        // value.
        let (k0, k1) = (a.local(ValType::I32), a.local(ValType::I32));
        let base = boxes.base;
        let kind = |a: &mut Asm, v: u32, into: u32| {
            a.ops([
                I::LocalGet(v),
                I::RefTestNonNull(HeapType::Concrete(base)),
                I::If(BlockType::Result(ValType::I32)),
                I::LocalGet(v),
                I::RefCastNonNull(HeapType::Concrete(base)),
                I::StructGet {
                    struct_type_index: base,
                    field_index: 0,
                },
                I::Else,
                I::I32Const(0),
                I::End,
                I::LocalSet(into),
            ]);
        };
        kind(&mut a, 0, k0);
        kind(&mut a, 1, k1);
        a.ops([
            I::LocalGet(k0),
            I::I32Eqz,
            I::LocalGet(k1),
            I::I32Eqz,
            I::I32Or,
        ]);
        ret(&mut a, 0);
        let numeric = |a: &mut Asm, k: u32| {
            a.ops([
                I::LocalGet(k),
                I::I32Const(INT),
                I::I32GeS,
                I::LocalGet(k),
                I::I32Const(DOUBLE),
                I::I32LeS,
                I::I32And,
            ]);
        };
        numeric(&mut a, k0);
        numeric(&mut a, k1);
        a.ops([
            I::I32And,
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::Call(to_f64),
            I::LocalGet(1),
            I::Call(to_f64),
            I::F64Eq,
            I::Return,
            I::End,
            I::LocalGet(k0),
            I::LocalGet(k1),
            I::I32Ne,
        ]);
        ret(&mut a, 0);
        a.ops([
            I::LocalGet(0),
            I::RefCastNonNull(HeapType::Concrete(boxes.i32)),
            I::StructGet {
                struct_type_index: boxes.i32,
                field_index: 1,
            },
            I::LocalGet(1),
            I::RefCastNonNull(HeapType::Concrete(boxes.i32)),
            I::StructGet {
                struct_type_index: boxes.i32,
                field_index: 1,
            },
            I::I32Eq,
        ]);
        Ok(a)
    })
}

/// The boxed number in an `any` as a `double`.
fn num_fn(ctx: &mut Ctx) -> u32 {
    obj_fn(ctx, "AnyF64".to_string(), |ctx| {
        let boxes = ctx.types.boxes();
        let mut a = Asm::new(vec![any_type()], vec![ValType::F64]);
        let kind = a.local(ValType::I32);
        for (bt, vt) in [
            (boxes.i32, ValType::I32),
            (boxes.i64, ValType::I64),
            (boxes.f32, ValType::F32),
            (boxes.f64, ValType::F64),
        ] {
            let cast = [I::LocalGet(0), I::RefCastNonNull(HeapType::Concrete(bt))];
            a.ops([
                I::LocalGet(0),
                I::RefTestNonNull(HeapType::Concrete(bt)),
                I::If(BlockType::Empty),
            ]);
            a.ops(cast.clone());
            a.ops([
                I::StructGet {
                    struct_type_index: bt,
                    field_index: 0,
                },
                I::LocalSet(kind),
            ]);
            a.ops(cast.clone());
            a.op(I::StructGet {
                struct_type_index: bt,
                field_index: 1,
            });
            match vt {
                ValType::I32 => {
                    a.op(I::F64ConvertI32U);
                    a.ops(cast.clone());
                    a.op(I::StructGet {
                        struct_type_index: bt,
                        field_index: 1,
                    });
                    a.op(I::F64ConvertI32S);
                    unsigned_test(&mut a, kind);
                    a.op(I::Select);
                }
                ValType::I64 => {
                    a.op(I::F64ConvertI64U);
                    a.ops(cast.clone());
                    a.op(I::StructGet {
                        struct_type_index: bt,
                        field_index: 1,
                    });
                    a.op(I::F64ConvertI64S);
                    a.ops([
                        I::LocalGet(kind),
                        I::I32Const(int_kind(IntKind::U64)),
                        I::I32Eq,
                        I::Select,
                    ]);
                }
                ValType::F32 => a.op(I::F64PromoteF32),
                _ => {}
            }
            a.ops([I::Return, I::End]);
        }
        a.op(I::F64Const(0.0.into()));
        Ok(a)
    })
}

/// Reserves the function printing an `any`; its body is built when the
/// module is finished, so that it knows every array and dictionary type.
pub fn show_fn(ctx: &mut Ctx) -> u32 {
    if let Some(f) = ctx.show_any {
        return f;
    }
    let s = ctx.types.string();
    let ty = ctx
        .types
        .func(vec![any_type(), ValType::I32], vec![nullable(s)]);
    let f = ctx.add_def(ty);
    ctx.show_any = Some(f);
    f
}

/// Reserves the function telling whether an `any` counts as true: not
/// `null`, zero, `false`, an empty string, array or dictionary. Built when
/// the module is finished, like [`show_fn`].
pub fn truthy_fn(ctx: &mut Ctx) -> u32 {
    if let Some(f) = ctx.truthy_any {
        return f;
    }
    let ty = ctx.types.func(vec![any_type()], vec![ValType::I32]);
    let f = ctx.add_def(ty);
    ctx.truthy_any = Some(f);
    f
}

/// The body of [`truthy_fn`].
pub fn build_truthy(ctx: &mut Ctx) -> R<Asm> {
    let s = ctx.types.string();
    let boxes = ctx.types.boxes();
    let mut a = Asm::new(vec![any_type()], vec![ValType::I32]);
    a.ops([
        I::LocalGet(0),
        I::RefIsNull,
        I::If(BlockType::Empty),
        I::I32Const(0),
        I::Return,
        I::End,
        I::LocalGet(0),
        I::RefTestNonNull(HeapType::Concrete(s)),
        I::If(BlockType::Empty),
        I::LocalGet(0),
        I::RefCastNonNull(HeapType::Concrete(s)),
        I::ArrayLen,
        I::I32Const(0),
        I::I32Ne,
        I::Return,
        I::End,
    ]);
    for (bt, vt) in [
        (boxes.i32, ValType::I32),
        (boxes.i64, ValType::I64),
        (boxes.f32, ValType::F32),
        (boxes.f64, ValType::F64),
    ] {
        let get = |field| {
            [
                I::LocalGet(0),
                I::RefCastNonNull(HeapType::Concrete(bt)),
                I::StructGet {
                    struct_type_index: bt,
                    field_index: field,
                },
            ]
        };
        a.ops([
            I::LocalGet(0),
            I::RefTestNonNull(HeapType::Concrete(bt)),
            I::If(BlockType::Empty),
        ]);
        a.ops(get(1));
        match vt {
            ValType::I32 => {
                // Enums count as true whatever their case.
                a.ops([I::I32Const(0), I::I32Ne]);
                a.ops(get(0));
                a.ops([I::I32Const(ENUM), I::I32GeS, I::I32Or]);
            }
            ValType::I64 => a.ops([I::I64Const(0), I::I64Ne]),
            ValType::F32 => a.ops([I::F32Const(0.0.into()), I::F32Ne]),
            _ => a.ops([I::F64Const(0.0.into()), I::F64Ne]),
        }
        a.ops([I::Return, I::End]);
    }
    for (ty, heap) in ctx.types.containers() {
        let len = match &ty {
            Ty::Array(_) => vec![I::StructGet {
                struct_type_index: heap,
                field_index: 1,
            }],
            Ty::Map(k, v) => super::maps::len_instrs(ctx, k, v)?,
            _ => continue,
        };
        a.ops([
            I::LocalGet(0),
            I::RefTestNonNull(HeapType::Concrete(heap)),
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::RefCastNonNull(HeapType::Concrete(heap)),
        ]);
        a.ops(len);
        a.ops([I::I32Const(0), I::I32Ne, I::Return, I::End]);
    }
    a.op(I::I32Const(1));
    Ok(a)
}

/// The body of [`show_fn`].
pub fn build_show(ctx: &mut Ctx) -> R<Asm> {
    let s = ctx.types.string();
    let sref = nullable(s);
    let boxes = ctx.types.boxes();
    let mut a = Asm::new(vec![any_type(), ValType::I32], vec![sref]);
    let lit = |ctx: &mut Ctx, a: &mut Asm, text: &str| {
        let ops = ctx.string_lit(text);
        a.ops(ops);
    };
    a.ops([I::LocalGet(0), I::RefIsNull, I::If(BlockType::Empty)]);
    lit(ctx, &mut a, "null");
    a.ops([I::Return, I::End]);
    a.ops([
        I::LocalGet(0),
        I::RefTestNonNull(HeapType::Concrete(s)),
        I::If(BlockType::Empty),
        I::LocalGet(0),
        I::RefCastNullable(HeapType::Concrete(s)),
        I::Return,
        I::End,
    ]);
    // Boxed values: by kind.
    let kind = a.local(ValType::I32);
    let (i32v, i64v, f32v, f64v) = (
        a.local(ValType::I32),
        a.local(ValType::I64),
        a.local(ValType::F32),
        a.local(ValType::F64),
    );
    let enums: Vec<(i32, Ty)> = ctx
        .classes
        .values()
        .filter(|c| c.ty.kind == TypeKind::Enum && c.st.is_none())
        .map(|c| (c.tag, Ty::Named(c.ty.clone())))
        .collect();
    let format = super::format_fn(ctx)?;
    for (bt, vt, local) in [
        (boxes.i32, ValType::I32, i32v),
        (boxes.i64, ValType::I64, i64v),
        (boxes.f32, ValType::F32, f32v),
        (boxes.f64, ValType::F64, f64v),
    ] {
        let cast = [I::LocalGet(0), I::RefCastNonNull(HeapType::Concrete(bt))];
        a.ops([
            I::LocalGet(0),
            I::RefTestNonNull(HeapType::Concrete(bt)),
            I::If(BlockType::Empty),
        ]);
        a.ops(cast.clone());
        a.ops([
            I::StructGet {
                struct_type_index: bt,
                field_index: 0,
            },
            I::LocalSet(kind),
        ]);
        a.ops(cast);
        a.ops([
            I::StructGet {
                struct_type_index: bt,
                field_index: 1,
            },
            I::LocalSet(local),
        ]);
        match vt {
            ValType::I32 => {
                a.ops([
                    I::LocalGet(kind),
                    I::I32Const(BOOL),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                    I::LocalGet(local),
                    I::If(BlockType::Result(sref)),
                ]);
                lit(ctx, &mut a, "true");
                a.op(I::Else);
                lit(ctx, &mut a, "false");
                a.ops([I::End, I::Return, I::End]);
                a.ops([
                    I::LocalGet(kind),
                    I::I32Const(CHAR),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                    I::LocalGet(local),
                    I::ArrayNewFixed {
                        array_type_index: s,
                        array_size: 1,
                    },
                    I::Return,
                    I::End,
                ]);
                for (tag, ty) in &enums {
                    let f = objects::show_fn(ctx, ty)?;
                    a.ops([
                        I::LocalGet(kind),
                        I::I32Const(ENUM + tag),
                        I::I32Eq,
                        I::If(BlockType::Empty),
                        I::LocalGet(local),
                        I::LocalGet(1),
                        I::Call(f),
                        I::Return,
                        I::End,
                    ]);
                }
                let to_str = ctx.helper(Helper::IntToStr { unsigned: false });
                a.ops([
                    I::LocalGet(local),
                    I::I64ExtendI32U,
                    I::LocalGet(local),
                    I::I64ExtendI32S,
                ]);
                unsigned_test(&mut a, kind);
                a.ops([I::Select, I::Call(to_str), I::Return]);
            }
            ValType::I64 => {
                let signed = ctx.helper(Helper::IntToStr { unsigned: false });
                let unsigned = ctx.helper(Helper::IntToStr { unsigned: true });
                a.ops([
                    I::LocalGet(kind),
                    I::I32Const(int_kind(IntKind::U64)),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                    I::LocalGet(local),
                    I::Call(unsigned),
                    I::Return,
                    I::End,
                    I::LocalGet(local),
                    I::Call(signed),
                    I::Return,
                ]);
            }
            ValType::F32 => a.ops([
                I::LocalGet(local),
                I::F64PromoteF32,
                I::I32Const(1),
                I::Call(format),
                I::Return,
            ]),
            _ => a.ops([
                I::LocalGet(local),
                I::I32Const(0),
                I::Call(format),
                I::Return,
            ]),
        }
        a.op(I::End);
    }
    // Objects: by their real type.
    if !ctx.classes.is_empty() {
        let object = ctx.object;
        let tag = a.local(ValType::I32);
        let mut kinds: Vec<(i32, u32, Ty)> = ctx
            .classes
            .values()
            .filter(|c| {
                c.st.is_some()
                    && c.ty.kind != TypeKind::Interface
                    && !(c.ty.kind == TypeKind::Class && c.ty.is_abstract())
            })
            .map(|c| (c.tag, c.st.unwrap_or(0), Ty::Named(c.ty.clone())))
            .collect();
        kinds.sort_by_key(|k| k.0);
        a.ops([
            I::LocalGet(0),
            I::RefTestNonNull(HeapType::Concrete(object)),
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::RefCastNonNull(HeapType::Concrete(object)),
            I::StructGet {
                struct_type_index: object,
                field_index: 0,
            },
            I::LocalSet(tag),
        ]);
        for (ctag, st, ty) in kinds {
            let f = objects::show_fn(ctx, &ty)?;
            a.ops([
                I::LocalGet(tag),
                I::I32Const(ctag),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::LocalGet(0),
                I::RefCastNullable(HeapType::Concrete(st)),
                I::LocalGet(1),
                I::Call(f),
                I::Return,
                I::End,
            ]);
        }
        a.op(I::End);
    }
    // Arrays, dictionaries and pairs of every type the module has.
    for (ty, heap) in ctx.types.containers() {
        let f = objects::show_fn(ctx, &ty)?;
        a.ops([
            I::LocalGet(0),
            I::RefTestNonNull(HeapType::Concrete(heap)),
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::RefCastNullable(HeapType::Concrete(heap)),
            I::LocalGet(1),
            I::Call(f),
            I::Return,
            I::End,
        ]);
    }
    lit(ctx, &mut a, "?");
    Ok(a)
}
