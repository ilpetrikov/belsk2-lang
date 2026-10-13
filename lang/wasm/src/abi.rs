//! The canonical ABI of the component model for Belsk2 values: how a value
//! of a WIT type is flattened into core values (to pass it as parameters
//! and results) and laid out in linear memory (in lists and in results
//! that do not fit in one core value).
//!
//! Records are Belsk2 structs `{ tag, fields... }`; enums are the `i32`
//! index of their case; variants (also `option` and `result`) are enums
//! with data: a case struct `{ tag, case, payload }` per case.

use belsk2_project::{WitTy, WitType, WitTypeKind};
use belsk2_syntax::{Error, FloatKind, IntKind, Ty};
use wasm_encoder::{BlockType, HeapType, Instruction as I, MemArg, ValType};

use crate::codegen::{Ctx, R};
use crate::helpers::{Asm, Helper};
use crate::types::nullable;

pub fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

/// The core type both of two cases' values fit in (canonical ABI `join`).
fn join(a: ValType, b: ValType) -> ValType {
    match (a, b) {
        _ if a == b => a,
        (ValType::I32, ValType::F32) | (ValType::F32, ValType::I32) => ValType::I32,
        _ => ValType::I64,
    }
}

/// The payloads of a variant's cases, joined position by position.
fn joined(cases: &[(String, Option<(String, WitTy)>)]) -> Vec<ValType> {
    let mut out: Vec<ValType> = Vec::new();
    for (_, payload) in cases {
        let Some((_, t)) = payload else {
            continue;
        };
        for (i, v) in flat(t).into_iter().enumerate() {
            match out.get_mut(i) {
                Some(slot) => *slot = join(*slot, v),
                None => out.push(v),
            }
        }
    }
    out
}

/// The core types a value of `t` is passed as.
pub fn flat(t: &WitTy) -> Vec<ValType> {
    match t {
        WitTy::Int(k) if k.bits() == 64 => vec![ValType::I64],
        WitTy::Float(FloatKind::F32) => vec![ValType::F32],
        WitTy::Float(FloatKind::F64) => vec![ValType::F64],
        WitTy::String | WitTy::List(_) => vec![ValType::I32, ValType::I32],
        WitTy::Named(n) => match &n.kind {
            WitTypeKind::Record(fields) => fields.iter().flat_map(|(_, t)| flat(t)).collect(),
            WitTypeKind::Enum(_) => vec![ValType::I32],
            WitTypeKind::Variant(cases) => {
                let mut out = vec![ValType::I32];
                out.extend(joined(cases));
                out
            }
        },
        _ => vec![ValType::I32],
    }
}

fn align_to(n: i32, align: i32) -> i32 {
    (n + align - 1) / align * align
}

/// Size of the discriminant of an enum or variant with `n` cases.
fn discriminant(n: usize) -> i32 {
    if n <= 1 << 8 {
        1
    } else if n <= 1 << 16 {
        2
    } else {
        4
    }
}

/// Where the payload of a variant starts, and the size and alignment of
/// the variant.
fn variant_layout(cases: &[(String, Option<(String, WitTy)>)]) -> (i32, i32, i32) {
    let d = discriminant(cases.len());
    let (mut size, mut align) = (0, 1);
    for (_, payload) in cases {
        if let Some((_, t)) = payload {
            let (s, a) = size_align(t);
            size = size.max(s);
            align = align.max(a);
        }
    }
    let offset = align_to(d, align);
    let total_align = align.max(d);
    (offset, align_to(offset + size, total_align), total_align)
}

/// Offsets of a record's fields, and its size and alignment.
fn record_layout(fields: &[(String, WitTy)]) -> (Vec<i32>, i32, i32) {
    let (mut offset, mut align) = (0, 1);
    let mut offsets = Vec::new();
    for (_, t) in fields {
        let (s, a) = size_align(t);
        offset = align_to(offset, a);
        offsets.push(offset);
        offset += s;
        align = align.max(a);
    }
    (offsets, align_to(offset, align), align)
}

/// Size and alignment of `t` stored in memory.
pub fn size_align(t: &WitTy) -> (i32, i32) {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::I8 | IntKind::U8) => (1, 1),
        WitTy::Int(IntKind::I16 | IntKind::U16) => (2, 2),
        WitTy::Int(IntKind::I64 | IntKind::U64) | WitTy::Float(FloatKind::F64) => (8, 8),
        WitTy::String | WitTy::List(_) => (8, 4),
        WitTy::Named(n) => match &n.kind {
            WitTypeKind::Record(fields) => {
                let (_, size, align) = record_layout(fields);
                (size, align)
            }
            WitTypeKind::Enum(cases) => {
                let d = discriminant(cases.len());
                (d, d)
            }
            WitTypeKind::Variant(cases) => {
                let (_, size, align) = variant_layout(cases);
                (size, align)
            }
        },
        _ => (4, 4),
    }
}

/// The run-time layout of the Belsk2 type of `n`: its tag, struct and the
/// struct of each case.
struct Layout {
    tag: i32,
    st: u32,
    cases: Vec<u32>,
}

fn layout(ctx: &Ctx, n: &WitType) -> R<Layout> {
    let named = match n.ty() {
        Some(Ty::Named(r)) => Some(r),
        _ => None,
    };
    named
        .and_then(|r| ctx.class_layout(r.id()))
        .map(|(tag, st, cases)| Layout {
            tag,
            st: st.unwrap_or(0),
            cases,
        })
        .ok_or_else(|| {
            Error::compile(format!(
                "internal compiler error: the WIT type '{}' has no declaration",
                n.wit
            ))
        })
}

/// The instruction storing a value of a primitive type.
fn store_op(t: &WitTy, offset: u64) -> I<'static> {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::I8 | IntKind::U8) => I::I32Store8(mem(offset)),
        WitTy::Int(IntKind::I16 | IntKind::U16) => I::I32Store16(mem(offset)),
        WitTy::Int(IntKind::I64 | IntKind::U64) => I::I64Store(mem(offset)),
        WitTy::Float(FloatKind::F32) => I::F32Store(mem(offset)),
        WitTy::Float(FloatKind::F64) => I::F64Store(mem(offset)),
        _ => I::I32Store(mem(offset)),
    }
}

/// The instruction loading a value of a primitive type.
fn load_op(t: &WitTy, offset: u64) -> I<'static> {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::U8) => I::I32Load8U(mem(offset)),
        WitTy::Int(IntKind::I8) => I::I32Load8S(mem(offset)),
        WitTy::Int(IntKind::U16) => I::I32Load16U(mem(offset)),
        WitTy::Int(IntKind::I16) => I::I32Load16S(mem(offset)),
        WitTy::Int(IntKind::I64 | IntKind::U64) => I::I64Load(mem(offset)),
        WitTy::Float(FloatKind::F32) => I::F32Load(mem(offset)),
        WitTy::Float(FloatKind::F64) => I::F64Load(mem(offset)),
        _ => I::I32Load(mem(offset)),
    }
}

fn discriminant_store(n: usize, offset: u64) -> I<'static> {
    match discriminant(n) {
        1 => I::I32Store8(mem(offset)),
        2 => I::I32Store16(mem(offset)),
        _ => I::I32Store(mem(offset)),
    }
}

fn discriminant_load(n: usize, offset: u64) -> I<'static> {
    match discriminant(n) {
        1 => I::I32Load8U(mem(offset)),
        2 => I::I32Load16U(mem(offset)),
        _ => I::I32Load(mem(offset)),
    }
}

/// Converts a core value of type `from` (a joined variant slot) on the
/// stack to `to` (the case's own type).
fn narrow(a: &mut Asm, from: ValType, to: ValType) {
    match (from, to) {
        _ if from == to => {}
        (ValType::I32, ValType::F32) => a.op(I::F32ReinterpretI32),
        (ValType::I64, ValType::I32) => a.op(I::I32WrapI64),
        (ValType::I64, ValType::F32) => a.ops([I::I32WrapI64, I::F32ReinterpretI32]),
        (ValType::I64, ValType::F64) => a.op(I::F64ReinterpretI64),
        _ => {}
    }
}

/// The reverse of [`narrow`].
fn widen(a: &mut Asm, from: ValType, to: ValType) {
    match (from, to) {
        _ if from == to => {}
        (ValType::F32, ValType::I32) => a.op(I::I32ReinterpretF32),
        (ValType::I32, ValType::I64) => a.op(I::I64ExtendI32U),
        (ValType::F32, ValType::I64) => a.ops([I::I32ReinterpretF32, I::I64ExtendI32U]),
        (ValType::F64, ValType::I64) => a.op(I::I64ReinterpretF64),
        _ => {}
    }
}

/// Turns the flattened values of `t` on the stack into a Belsk2 value.
pub fn lift(ctx: &mut Ctx, a: &mut Asm, t: &WitTy) -> R<()> {
    match t {
        WitTy::Bool => a.ops([I::I32Const(0), I::I32Ne]),
        WitTy::String => a.op(I::Call(ctx.helper(Helper::LiftStr))),
        WitTy::List(elem) => a.op(I::Call(ctx.helper(Helper::LiftList((**elem).clone())))),
        WitTy::Named(n) if !matches!(n.kind, WitTypeKind::Enum(_)) => {
            let types = flat(t);
            let locals: Vec<u32> = types.iter().map(|&v| a.local(v)).collect();
            for &l in locals.iter().rev() {
                a.op(I::LocalSet(l));
            }
            lift_from(ctx, a, n, &locals, &types)?;
        }
        _ => {}
    }
    Ok(())
}

/// Builds the value of `n` from its flattened values in `locals`.
fn lift_from(ctx: &mut Ctx, a: &mut Asm, n: &WitType, locals: &[u32], types: &[ValType]) -> R<()> {
    let lay = layout(ctx, n)?;
    match &n.kind {
        WitTypeKind::Record(fields) => {
            a.op(I::I32Const(lay.tag));
            let mut at = 0;
            for (_, t) in fields {
                let count = flat(t).len();
                for &l in locals.iter().skip(at).take(count) {
                    a.op(I::LocalGet(l));
                }
                at += count;
                lift(ctx, a, t)?;
            }
            a.op(I::StructNew(lay.st));
        }
        WitTypeKind::Enum(_) => {
            if let Some(&l) = locals.first() {
                a.op(I::LocalGet(l));
            }
        }
        WitTypeKind::Variant(cases) => {
            let Some(&disc) = locals.first() else {
                return Ok(());
            };
            a.op(I::Block(BlockType::Result(nullable(lay.st))));
            for (k, (_, payload)) in cases.iter().enumerate() {
                let cst = lay.cases.get(k).copied().unwrap_or(0);
                a.ops([
                    I::LocalGet(disc),
                    I::I32Const(k as i32),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                    I::I32Const(lay.tag),
                    I::I32Const(k as i32),
                ]);
                if let Some((_, t)) = payload {
                    for (i, v) in flat(t).into_iter().enumerate() {
                        let (Some(&l), Some(&slot)) = (locals.get(i + 1), types.get(i + 1)) else {
                            continue;
                        };
                        a.op(I::LocalGet(l));
                        narrow(a, slot, v);
                    }
                    lift(ctx, a, t)?;
                }
                a.ops([I::StructNew(cst), I::Br(1), I::End]);
            }
            a.ops([I::Unreachable, I::End]);
        }
    }
    Ok(())
}

/// Turns the Belsk2 value of `t` on the stack into its flattened values.
pub fn lower(ctx: &mut Ctx, a: &mut Asm, t: &WitTy) -> R<()> {
    match t {
        WitTy::String => a.op(I::Call(ctx.helper(Helper::LowerStr))),
        WitTy::List(elem) => a.op(I::Call(ctx.helper(Helper::LowerList((**elem).clone())))),
        WitTy::Named(n) => {
            let lay = layout(ctx, n)?;
            match &n.kind {
                WitTypeKind::Record(fields) => {
                    let v = a.local(nullable(lay.st));
                    a.op(I::LocalSet(v));
                    for (i, (_, t)) in fields.iter().enumerate() {
                        a.ops([
                            I::LocalGet(v),
                            I::StructGet {
                                struct_type_index: lay.st,
                                field_index: i as u32 + 1,
                            },
                        ]);
                        lower(ctx, a, t)?;
                    }
                }
                WitTypeKind::Enum(_) => {}
                WitTypeKind::Variant(cases) => {
                    let slots = joined(cases);
                    let v = a.local(nullable(lay.st));
                    let disc = a.local(ValType::I32);
                    let outs: Vec<u32> = slots.iter().map(|&s| a.local(s)).collect();
                    a.ops([
                        I::LocalTee(v),
                        I::StructGet {
                            struct_type_index: lay.st,
                            field_index: 1,
                        },
                        I::LocalSet(disc),
                    ]);
                    for (k, (_, payload)) in cases.iter().enumerate() {
                        let Some((_, t)) = payload else {
                            continue;
                        };
                        let cst = lay.cases.get(k).copied().unwrap_or(0);
                        a.ops([
                            I::LocalGet(disc),
                            I::I32Const(k as i32),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                            I::LocalGet(v),
                            I::RefCastNonNull(HeapType::Concrete(cst)),
                            I::StructGet {
                                struct_type_index: cst,
                                field_index: 2,
                            },
                        ]);
                        lower(ctx, a, t)?;
                        let own = flat(t);
                        let temps: Vec<u32> = own.iter().map(|&v| a.local(v)).collect();
                        for &l in temps.iter().rev() {
                            a.op(I::LocalSet(l));
                        }
                        for (i, (&l, &v)) in temps.iter().zip(&own).enumerate() {
                            let (Some(&out), Some(&slot)) = (outs.get(i), slots.get(i)) else {
                                continue;
                            };
                            a.op(I::LocalGet(l));
                            widen(a, v, slot);
                            a.op(I::LocalSet(out));
                        }
                        a.op(I::End);
                    }
                    a.op(I::LocalGet(disc));
                    for &o in &outs {
                        a.op(I::LocalGet(o));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Stores the Belsk2 value of `t` on the stack at `addr + offset`.
pub fn store(ctx: &mut Ctx, a: &mut Asm, t: &WitTy, addr: u32, offset: u64) -> R<()> {
    match t {
        WitTy::String | WitTy::List(_) => {
            lower(ctx, a, t)?;
            let (p, l) = (a.local(ValType::I32), a.local(ValType::I32));
            a.ops([
                I::LocalSet(l),
                I::LocalSet(p),
                I::LocalGet(addr),
                I::LocalGet(p),
                I::I32Store(mem(offset)),
                I::LocalGet(addr),
                I::LocalGet(l),
                I::I32Store(mem(offset + 4)),
            ]);
        }
        WitTy::Named(n) => {
            let lay = layout(ctx, n)?;
            match &n.kind {
                WitTypeKind::Record(fields) => {
                    let (offsets, ..) = record_layout(fields);
                    let v = a.local(nullable(lay.st));
                    a.op(I::LocalSet(v));
                    for (i, ((_, t), fo)) in fields.iter().zip(offsets).enumerate() {
                        a.ops([
                            I::LocalGet(v),
                            I::StructGet {
                                struct_type_index: lay.st,
                                field_index: i as u32 + 1,
                            },
                        ]);
                        store(ctx, a, t, addr, offset + fo as u64)?;
                    }
                }
                WitTypeKind::Enum(cases) => {
                    let v = a.local(ValType::I32);
                    a.ops([
                        I::LocalSet(v),
                        I::LocalGet(addr),
                        I::LocalGet(v),
                        discriminant_store(cases.len(), offset),
                    ]);
                }
                WitTypeKind::Variant(cases) => {
                    let (po, ..) = variant_layout(cases);
                    let v = a.local(nullable(lay.st));
                    let disc = a.local(ValType::I32);
                    a.ops([
                        I::LocalTee(v),
                        I::StructGet {
                            struct_type_index: lay.st,
                            field_index: 1,
                        },
                        I::LocalSet(disc),
                        I::LocalGet(addr),
                        I::LocalGet(disc),
                        discriminant_store(cases.len(), offset),
                    ]);
                    for (k, (_, payload)) in cases.iter().enumerate() {
                        let Some((_, t)) = payload else {
                            continue;
                        };
                        let cst = lay.cases.get(k).copied().unwrap_or(0);
                        a.ops([
                            I::LocalGet(disc),
                            I::I32Const(k as i32),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                            I::LocalGet(v),
                            I::RefCastNonNull(HeapType::Concrete(cst)),
                            I::StructGet {
                                struct_type_index: cst,
                                field_index: 2,
                            },
                        ]);
                        store(ctx, a, t, addr, offset + po as u64)?;
                        a.op(I::End);
                    }
                }
            }
        }
        _ => {
            let vt = flat(t).first().copied().unwrap_or(ValType::I32);
            let v = a.local(vt);
            a.ops([
                I::LocalSet(v),
                I::LocalGet(addr),
                I::LocalGet(v),
                store_op(t, offset),
            ]);
        }
    }
    Ok(())
}

/// Pushes the Belsk2 value of `t` stored at `addr + offset`.
pub fn load(ctx: &mut Ctx, a: &mut Asm, t: &WitTy, addr: u32, offset: u64) -> R<()> {
    match t {
        WitTy::String | WitTy::List(_) => {
            a.ops([
                I::LocalGet(addr),
                I::I32Load(mem(offset)),
                I::LocalGet(addr),
                I::I32Load(mem(offset + 4)),
            ]);
            lift(ctx, a, t)?;
        }
        WitTy::Named(n) => {
            let lay = layout(ctx, n)?;
            match &n.kind {
                WitTypeKind::Record(fields) => {
                    let (offsets, ..) = record_layout(fields);
                    a.op(I::I32Const(lay.tag));
                    for ((_, t), fo) in fields.iter().zip(offsets) {
                        load(ctx, a, t, addr, offset + fo as u64)?;
                    }
                    a.op(I::StructNew(lay.st));
                }
                WitTypeKind::Enum(cases) => {
                    a.ops([I::LocalGet(addr), discriminant_load(cases.len(), offset)]);
                }
                WitTypeKind::Variant(cases) => {
                    let (po, ..) = variant_layout(cases);
                    let disc = a.local(ValType::I32);
                    a.ops([
                        I::LocalGet(addr),
                        discriminant_load(cases.len(), offset),
                        I::LocalSet(disc),
                        I::Block(BlockType::Result(nullable(lay.st))),
                    ]);
                    for (k, (_, payload)) in cases.iter().enumerate() {
                        let cst = lay.cases.get(k).copied().unwrap_or(0);
                        a.ops([
                            I::LocalGet(disc),
                            I::I32Const(k as i32),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                            I::I32Const(lay.tag),
                            I::I32Const(k as i32),
                        ]);
                        if let Some((_, t)) = payload {
                            load(ctx, a, t, addr, offset + po as u64)?;
                        }
                        a.ops([I::StructNew(cst), I::Br(1), I::End]);
                    }
                    a.ops([I::Unreachable, I::End]);
                }
            }
        }
        _ => {
            a.ops([I::LocalGet(addr), load_op(t, offset)]);
            if matches!(t, WitTy::Bool) {
                a.ops([I::I32Const(0), I::I32Ne]);
            }
        }
    }
    Ok(())
}
