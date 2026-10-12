//! `Dictionary<K, V>` in WASM builds (see [`crate::types::MapType`]).
//!
//! Lookups search the keys one by one while there are few of them; from
//! [`INDEXED`] entries on, a hash table of positions is built and kept up
//! to date (removing an entry drops it; the next lookup builds it again).

use belsk2_syntax::{Error, Ty};
use wasm_encoder::{BlockType, Instruction as I, ValType};

use super::objects::{eq_instrs, obj_fn};
use super::{Ctx, R};
use crate::helpers::{Asm, Helper};
use crate::types::{nullable, MapType};

/// From this many entries on, lookups use the hash table.
const INDEXED: i32 = 8;

fn map_type(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<MapType> {
    ctx.types.map(k, v).ok_or_else(|| {
        Error::compile(format!(
            "dictionaries with keys of type {k} (or values of type {v}) are not supported in WASM builds yet"
        ))
    })
}

fn key(name: &str, k: &Ty, v: &Ty) -> String {
    format!("Map{name}:{k:?}|{v:?}")
}

/// Instructions turning the key on the stack into an `i32` hash.
fn hash(ctx: &mut Ctx, k: &Ty) -> R<Vec<I<'static>>> {
    const MIX: i32 = 0x9E37_79B1_u32 as i32;
    Ok(match k.canonical() {
        Ty::String => vec![I::Call(str_hash(ctx))],
        Ty::Int(kind) if kind.bits() == 64 => vec![I::Call(hash64(ctx))],
        Ty::Int(_) | Ty::Bool | Ty::Char => vec![I::I32Const(MIX), I::I32Mul],
        Ty::Named(t) if ctx.classes.get(&t.id()).is_some_and(|c| c.st.is_none()) => {
            vec![I::I32Const(MIX), I::I32Mul]
        }
        other => {
            return Err(Error::compile(format!(
                "dictionary keys of type {other} are not supported in WASM builds yet"
            )))
        }
    })
}

fn hash64(ctx: &mut Ctx) -> u32 {
    obj_fn(ctx, "Hash64".to_string(), |_| {
        let mut a = Asm::new(vec![ValType::I64], vec![ValType::I32]);
        a.ops([
            I::LocalGet(0),
            I::LocalGet(0),
            I::I64Const(32),
            I::I64ShrU,
            I::I64Xor,
            I::I32WrapI64,
            I::I32Const(0x9E37_79B1_u32 as i32),
            I::I32Mul,
        ]);
        Ok(a)
    })
}

/// FNV-1a over the code points of a string.
fn str_hash(ctx: &mut Ctx) -> u32 {
    obj_fn(ctx, "StrHash".to_string(), |ctx| {
        let s = ctx.types.string();
        let mut a = Asm::new(vec![nullable(s)], vec![ValType::I32]);
        let (h, i, n) = (
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(ValType::I32),
        );
        a.ops([
            I::LocalGet(0),
            I::RefIsNull,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
            I::I32Const(0x811C_9DC5_u32 as i32),
            I::LocalSet(h),
            I::LocalGet(0),
            I::ArrayLen,
            I::LocalSet(n),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(i),
            I::LocalGet(n),
            I::I32GeU,
            I::BrIf(1),
            I::LocalGet(h),
            I::LocalGet(0),
            I::LocalGet(i),
            I::ArrayGet(s),
            I::I32Xor,
            I::I32Const(16_777_619),
            I::I32Mul,
            I::LocalSet(h),
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
            I::LocalGet(h),
        ]);
        Ok(a)
    })
}

/// `{}`: an empty dictionary.
pub fn new_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    Ok(obj_fn(ctx, key("New", k, v), move |ctx| {
        let table = ctx.types.string();
        let mut a = Asm::new(Vec::new(), vec![nullable(m.st)]);
        for arr in [m.keys, m.values] {
            a.ops([
                I::I32Const(4),
                I::ArrayNewDefault(arr.raw),
                I::I32Const(0),
                I::StructNew(arr.obj),
            ]);
        }
        a.ops([
            I::RefNull(wasm_encoder::HeapType::Concrete(table)),
            I::StructNew(m.st),
        ]);
        Ok(a)
    }))
}

/// The instructions reading field `i` of the dictionary in local `map`.
fn field(m: &MapType, map: u32, i: u32) -> [I<'static>; 2] {
    [
        I::LocalGet(map),
        I::StructGet {
            struct_type_index: m.st,
            field_index: i,
        },
    ]
}

/// Builds the hash table of a dictionary.
fn rehash_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let k2 = k.clone();
    Ok(obj_fn(ctx, key("Rehash", k, v), move |ctx| {
        let table_ty = ctx.types.string();
        let h_ops = hash(ctx, &k2)?;
        let mut a = Asm::new(vec![nullable(m.st)], Vec::new());
        let (n, size, table, i, h) = (
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(nullable(table_ty)),
            a.local(ValType::I32),
            a.local(ValType::I32),
        );
        let keys_data = I::StructGet {
            struct_type_index: m.keys.obj,
            field_index: 0,
        };
        a.ops(field(&m, 0, 0));
        a.ops([
            I::StructGet {
                struct_type_index: m.keys.obj,
                field_index: 1,
            },
            I::LocalSet(n),
            I::I32Const(16),
            I::LocalSet(size),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(size),
            I::LocalGet(n),
            I::I32Const(2),
            I::I32Mul,
            I::I32GtU,
            I::BrIf(1),
            I::LocalGet(size),
            I::I32Const(2),
            I::I32Mul,
            I::LocalSet(size),
            I::Br(0),
            I::End,
            I::End,
            I::LocalGet(size),
            I::ArrayNewDefault(table_ty),
            I::LocalSet(table),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(i),
            I::LocalGet(n),
            I::I32GeU,
            I::BrIf(1),
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([keys_data, I::LocalGet(i), I::ArrayGet(m.keys.raw)]);
        a.ops(h_ops);
        a.ops([
            I::LocalGet(size),
            I::I32Const(1),
            I::I32Sub,
            I::I32And,
            I::LocalSet(h),
        ]);
        probe_free(&mut a, table, table_ty, h, size);
        a.ops([
            I::LocalGet(table),
            I::LocalGet(h),
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::ArraySet(table_ty),
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
            I::LocalGet(0),
            I::LocalGet(table),
            I::StructSet {
                struct_type_index: m.st,
                field_index: 2,
            },
        ]);
        Ok(a)
    }))
}

/// Moves `h` forward to a free slot of `table` (of `size` slots).
fn probe_free(a: &mut Asm, table: u32, table_ty: u32, h: u32, size: u32) {
    a.ops([
        I::Block(BlockType::Empty),
        I::Loop(BlockType::Empty),
        I::LocalGet(table),
        I::LocalGet(h),
        I::ArrayGet(table_ty),
        I::I32Eqz,
        I::BrIf(1),
        I::LocalGet(h),
        I::I32Const(1),
        I::I32Add,
        I::LocalGet(size),
        I::I32Const(1),
        I::I32Sub,
        I::I32And,
        I::LocalSet(h),
        I::Br(0),
        I::End,
        I::End,
    ]);
}

/// `(map, key) -> position` of the key, or -1.
fn find_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let rehash = rehash_fn(ctx, k, v)?;
    let k2 = k.clone();
    Ok(obj_fn(ctx, key("Find", k, v), move |ctx| {
        let table_ty = ctx.types.string();
        let h_ops = hash(ctx, &k2)?;
        let eq = eq_instrs(ctx, &k2)?;
        let mut a = Asm::new(vec![nullable(m.st), m.keys.elem], vec![ValType::I32]);
        let (n, table, i, h, mask, slot) = (
            a.local(ValType::I32),
            a.local(nullable(table_ty)),
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(ValType::I32),
        );
        let data = I::StructGet {
            struct_type_index: m.keys.obj,
            field_index: 0,
        };
        a.ops(field(&m, 0, 0));
        a.ops([
            I::StructGet {
                struct_type_index: m.keys.obj,
                field_index: 1,
            },
            I::LocalSet(n),
        ]);
        a.ops(field(&m, 0, 2));
        a.ops([I::LocalTee(table), I::RefIsNull, I::If(BlockType::Empty)]);
        // Few entries: look at each.
        a.ops([
            I::LocalGet(n),
            I::I32Const(INDEXED),
            I::I32LtU,
            I::If(BlockType::Empty),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(i),
            I::LocalGet(n),
            I::I32GeU,
            I::BrIf(1),
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([
            data.clone(),
            I::LocalGet(i),
            I::ArrayGet(m.keys.raw),
            I::LocalGet(1),
        ]);
        a.ops(eq.clone());
        a.ops([
            I::If(BlockType::Empty),
            I::LocalGet(i),
            I::Return,
            I::End,
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
            I::I32Const(-1),
            I::Return,
            I::End,
            I::LocalGet(0),
            I::Call(rehash),
        ]);
        a.ops(field(&m, 0, 2));
        a.ops([I::LocalSet(table), I::End]);
        a.ops([
            I::LocalGet(table),
            I::ArrayLen,
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(mask),
            I::LocalGet(1),
        ]);
        a.ops(h_ops);
        a.ops([
            I::LocalGet(mask),
            I::I32And,
            I::LocalSet(h),
            I::Loop(BlockType::Empty),
            I::LocalGet(table),
            I::LocalGet(h),
            I::ArrayGet(table_ty),
            I::LocalTee(slot),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(-1),
            I::Return,
            I::End,
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([
            data,
            I::LocalGet(slot),
            I::I32Const(1),
            I::I32Sub,
            I::ArrayGet(m.keys.raw),
            I::LocalGet(1),
        ]);
        a.ops(eq);
        a.ops([
            I::If(BlockType::Empty),
            I::LocalGet(slot),
            I::I32Const(1),
            I::I32Sub,
            I::Return,
            I::End,
            I::LocalGet(h),
            I::I32Const(1),
            I::I32Add,
            I::LocalGet(mask),
            I::I32And,
            I::LocalSet(h),
            I::Br(0),
            I::End,
            I::Unreachable,
        ]);
        Ok(a)
    }))
}

/// `m[key]`: traps if the key is missing.
pub fn get_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let find = find_fn(ctx, k, v)?;
    Ok(obj_fn(ctx, key("Get", k, v), move |_| {
        let mut a = Asm::new(vec![nullable(m.st), m.keys.elem], vec![m.values.elem]);
        let pos = a.local(ValType::I32);
        a.ops([
            I::LocalGet(0),
            I::LocalGet(1),
            I::Call(find),
            I::LocalTee(pos),
            I::I32Const(0),
            I::I32LtS,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
        ]);
        a.ops(field(&m, 0, 1));
        a.ops([
            I::StructGet {
                struct_type_index: m.values.obj,
                field_index: 0,
            },
            I::LocalGet(pos),
            I::ArrayGet(m.values.raw),
        ]);
        Ok(a)
    }))
}

/// `has(m, key)`
pub fn has_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let find = find_fn(ctx, k, v)?;
    Ok(obj_fn(ctx, key("Has", k, v), move |_| {
        let mut a = Asm::new(vec![nullable(m.st), m.keys.elem], vec![ValType::I32]);
        a.ops([
            I::LocalGet(0),
            I::LocalGet(1),
            I::Call(find),
            I::I32Const(0),
            I::I32GeS,
        ]);
        Ok(a)
    }))
}

/// `m[key] = value`: replaces the value, or adds the entry at the end.
pub fn set_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let find = find_fn(ctx, k, v)?;
    let (k2, v2) = (k.clone(), v.clone());
    Ok(obj_fn(ctx, key("Set", k, v), move |ctx| {
        let table_ty = ctx.types.string();
        let h_ops = hash(ctx, &k2)?;
        let push_k = ctx.helper(Helper::ArrPush(k2.clone()));
        let push_v = ctx.helper(Helper::ArrPush(v2.clone()));
        let mut a = Asm::new(vec![nullable(m.st), m.keys.elem, m.values.elem], Vec::new());
        let (pos, n, table, h, size) = (
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(nullable(table_ty)),
            a.local(ValType::I32),
            a.local(ValType::I32),
        );
        a.ops([
            I::LocalGet(0),
            I::LocalGet(1),
            I::Call(find),
            I::LocalTee(pos),
            I::I32Const(0),
            I::I32GeS,
            I::If(BlockType::Empty),
        ]);
        a.ops(field(&m, 0, 1));
        a.ops([
            I::StructGet {
                struct_type_index: m.values.obj,
                field_index: 0,
            },
            I::LocalGet(pos),
            I::LocalGet(2),
            I::ArraySet(m.values.raw),
            I::Return,
            I::End,
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([
            I::StructGet {
                struct_type_index: m.keys.obj,
                field_index: 1,
            },
            I::LocalSet(n),
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([I::LocalGet(1), I::Call(push_k), I::Drop]);
        a.ops(field(&m, 0, 1));
        a.ops([I::LocalGet(2), I::Call(push_v), I::Drop]);
        // Keep the hash table up to date (or drop it once it is too full).
        a.ops(field(&m, 0, 2));
        a.ops([
            I::LocalTee(table),
            I::RefIsNull,
            I::If(BlockType::Empty),
            I::Return,
            I::End,
            I::LocalGet(table),
            I::ArrayLen,
            I::LocalSet(size),
            I::LocalGet(n),
            I::I32Const(1),
            I::I32Add,
            I::I32Const(2),
            I::I32Mul,
            I::LocalGet(size),
            I::I32GtU,
            I::If(BlockType::Empty),
            I::LocalGet(0),
            I::RefNull(wasm_encoder::HeapType::Concrete(table_ty)),
            I::StructSet {
                struct_type_index: m.st,
                field_index: 2,
            },
            I::Return,
            I::End,
            I::LocalGet(1),
        ]);
        a.ops(h_ops);
        a.ops([
            I::LocalGet(size),
            I::I32Const(1),
            I::I32Sub,
            I::I32And,
            I::LocalSet(h),
        ]);
        probe_free(&mut a, table, table_ty, h, size);
        a.ops([
            I::LocalGet(table),
            I::LocalGet(h),
            I::LocalGet(n),
            I::I32Const(1),
            I::I32Add,
            I::ArraySet(table_ty),
        ]);
        Ok(a)
    }))
}

/// `remove(m, key)`: whether the key was there.
pub fn remove_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let find = find_fn(ctx, k, v)?;
    Ok(obj_fn(ctx, key("Remove", k, v), move |ctx| {
        let table_ty = ctx.types.string();
        let mut a = Asm::new(vec![nullable(m.st), m.keys.elem], vec![ValType::I32]);
        let (pos, n) = (a.local(ValType::I32), a.local(ValType::I32));
        a.ops([
            I::LocalGet(0),
            I::LocalGet(1),
            I::Call(find),
            I::LocalTee(pos),
            I::I32Const(0),
            I::I32LtS,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
        a.ops(field(&m, 0, 0));
        a.ops([
            I::StructGet {
                struct_type_index: m.keys.obj,
                field_index: 1,
            },
            I::LocalSet(n),
        ]);
        for (i, arr) in [(0, m.keys), (1, m.values)] {
            let data = I::StructGet {
                struct_type_index: arr.obj,
                field_index: 0,
            };
            // data[pos..n-1] = data[pos+1..n]
            a.ops(field(&m, 0, i));
            a.ops([data.clone(), I::LocalGet(pos)]);
            a.ops(field(&m, 0, i));
            a.ops([
                data,
                I::LocalGet(pos),
                I::I32Const(1),
                I::I32Add,
                I::LocalGet(n),
                I::LocalGet(pos),
                I::I32Sub,
                I::I32Const(1),
                I::I32Sub,
                I::ArrayCopy {
                    array_type_index_dst: arr.raw,
                    array_type_index_src: arr.raw,
                },
            ]);
            a.ops(field(&m, 0, i));
            a.ops([
                I::LocalGet(n),
                I::I32Const(1),
                I::I32Sub,
                I::StructSet {
                    struct_type_index: arr.obj,
                    field_index: 1,
                },
            ]);
        }
        a.ops([
            I::LocalGet(0),
            I::RefNull(wasm_encoder::HeapType::Concrete(table_ty)),
            I::StructSet {
                struct_type_index: m.st,
                field_index: 2,
            },
            I::I32Const(1),
        ]);
        Ok(a)
    }))
}

/// `keys(m)` or `values(m)`: a new array.
pub fn list_fn(ctx: &mut Ctx, k: &Ty, v: &Ty, values: bool) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let name = if values { "Values" } else { "Keys" };
    Ok(obj_fn(ctx, key(name, k, v), move |_| {
        let (i, arr) = if values { (1, m.values) } else { (0, m.keys) };
        let mut a = Asm::new(vec![nullable(m.st)], vec![nullable(arr.obj)]);
        let (n, data) = (a.local(ValType::I32), a.local(nullable(arr.raw)));
        a.ops(field(&m, 0, i));
        a.ops([
            I::StructGet {
                struct_type_index: arr.obj,
                field_index: 1,
            },
            I::LocalTee(n),
            I::ArrayNewDefault(arr.raw),
            I::LocalSet(data),
            I::LocalGet(data),
            I::I32Const(0),
        ]);
        a.ops(field(&m, 0, i));
        a.ops([
            I::StructGet {
                struct_type_index: arr.obj,
                field_index: 0,
            },
            I::I32Const(0),
            I::LocalGet(n),
            I::ArrayCopy {
                array_type_index_dst: arr.raw,
                array_type_index_src: arr.raw,
            },
            I::LocalGet(data),
            I::RefAsNonNull,
            I::LocalGet(n),
            I::StructNew(arr.obj),
        ]);
        Ok(a)
    }))
}

/// Instructions giving the number of entries of the dictionary on the
/// stack.
pub fn len_instrs(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<Vec<I<'static>>> {
    let m = map_type(ctx, k, v)?;
    Ok(vec![
        I::StructGet {
            struct_type_index: m.st,
            field_index: 0,
        },
        I::StructGet {
            struct_type_index: m.keys.obj,
            field_index: 1,
        },
    ])
}

/// `a == b`: the same entries in the same order.
pub fn eq_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let m = map_type(ctx, k, v)?;
    let (k2, v2) = (k.clone(), v.clone());
    Ok(obj_fn(ctx, key("Eq", k, v), move |ctx| {
        let keq = eq_instrs(ctx, &k2)?;
        let veq = eq_instrs(ctx, &v2)?;
        let mut a = Asm::new(vec![nullable(m.st), nullable(m.st)], vec![ValType::I32]);
        let (n, i) = (a.local(ValType::I32), a.local(ValType::I32));
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
        let len = |a: &mut Asm, map: u32| {
            a.ops(field(&m, map, 0));
            a.op(I::StructGet {
                struct_type_index: m.keys.obj,
                field_index: 1,
            });
        };
        len(&mut a, 0);
        a.op(I::LocalTee(n));
        len(&mut a, 1);
        a.op(I::I32Ne);
        ret(&mut a, 0);
        a.ops([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(i),
            I::LocalGet(n),
            I::I32GeU,
            I::BrIf(1),
        ]);
        for (f, arr, eq) in [(0, m.keys, &keq), (1, m.values, &veq)] {
            for map in [0, 1] {
                a.ops(field(&m, map, f));
                a.ops([
                    I::StructGet {
                        struct_type_index: arr.obj,
                        field_index: 0,
                    },
                    I::LocalGet(i),
                    I::ArrayGet(arr.raw),
                ]);
            }
            a.ops(eq.iter().cloned());
            a.op(I::I32Eqz);
            ret(&mut a, 0);
        }
        a.ops([
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
            I::I32Const(1),
        ]);
        Ok(a)
    }))
}

/// Equality of two `KeyValuePair`s.
pub fn pair_eq_fn(ctx: &mut Ctx, k: &Ty, v: &Ty) -> R<u32> {
    let st = ctx.types.pair(k, v).ok_or_else(|| {
        Error::compile("KeyValuePair of these types is not supported in WASM builds yet")
    })?;
    let (k2, v2) = (k.clone(), v.clone());
    Ok(obj_fn(ctx, key("PairEq", k, v), move |ctx| {
        let mut a = Asm::new(vec![nullable(st), nullable(st)], vec![ValType::I32]);
        for (i, t) in [(0, &k2), (1, &v2)] {
            let eq = eq_instrs(ctx, t)?;
            let get = I::StructGet {
                struct_type_index: st,
                field_index: i,
            };
            a.ops([I::LocalGet(0), get.clone(), I::LocalGet(1), get]);
            a.ops(eq);
            a.ops([
                I::I32Eqz,
                I::If(BlockType::Empty),
                I::I32Const(0),
                I::Return,
                I::End,
            ]);
        }
        a.op(I::I32Const(1));
        Ok(a)
    }))
}
