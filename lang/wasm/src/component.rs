//! WebAssembly components: glue between a WIT world and Belsk2 code.
//!
//! The component model passes strings and lists through linear memory
//! (the canonical ABI), while Belsk2 keeps them in GC arrays. So the core
//! module gets a memory with a bump allocator (`cabi_realloc`) and, for
//! every function of the world, a wrapper that copies values between the
//! two:
//!
//! - an export becomes a core function with the flattened signature that
//!   lifts its arguments, calls the Belsk2 function and lowers the result;
//! - an import becomes a Belsk2-callable function that lowers its
//!   arguments, calls the host and lifts the result.
//!
//! The allocator is reset after every call of an export (or in its
//! `cabi_post_*` function when the result lives in memory), so memory does
//! not grow over time.

use belsk2_project::{WitFunc, WitTy, World};
use belsk2_syntax::{Error, FloatKind, IntKind, Span, Ty};
use wasm_encoder::{BlockType, ExportKind, Instruction as I, MemArg, ValType};

use crate::codegen::{Ctx, FnRef, R};
use crate::helpers::{Asm, Helper};
use crate::types::nullable;
use crate::Unit;

/// Where an export's result goes when it does not fit in one value.
const RET_AREA: i32 = 0;
/// Start of the heap (after the return area).
const HEAP_BASE: i32 = 8;
/// Most flattened parameters a function may have (canonical ABI).
const MAX_FLAT_PARAMS: usize = 16;

fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

/// The core types a value of `t` is passed as.
fn flat(t: &WitTy) -> Vec<ValType> {
    match t {
        WitTy::Int(k) if k.bits() == 64 => vec![ValType::I64],
        WitTy::Float(FloatKind::F32) => vec![ValType::F32],
        WitTy::Float(FloatKind::F64) => vec![ValType::F64],
        WitTy::String | WitTy::List(_) => vec![ValType::I32, ValType::I32],
        _ => vec![ValType::I32],
    }
}

/// Size and alignment of `t` stored in memory (as a list element).
fn size_align(t: &WitTy) -> (i32, i32) {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::I8 | IntKind::U8) => (1, 1),
        WitTy::Int(IntKind::I16 | IntKind::U16) => (2, 2),
        WitTy::Int(IntKind::I64 | IntKind::U64) | WitTy::Float(FloatKind::F64) => (8, 8),
        WitTy::String | WitTy::List(_) => (8, 4),
        _ => (4, 4),
    }
}

/// Records, enums and variants are not supported in WASM builds yet.
fn named(f: &WitFunc) -> bool {
    fn has(t: &WitTy) -> bool {
        match t {
            WitTy::Named(_) => true,
            WitTy::List(e) => has(e),
            _ => false,
        }
    }
    f.params.iter().any(|(_, t)| has(t)) || f.result.as_ref().is_some_and(has)
}

fn in_memory(t: &WitTy) -> bool {
    matches!(t, WitTy::String | WitTy::List(_))
}

/// The name of a function in the core module (the "legacy" naming of
/// `wit-component`).
fn core_name(f: &WitFunc) -> String {
    match &f.interface {
        Some(i) => format!("{}#{}", i.key, f.wit_name),
        None => f.wit_name.clone(),
    }
}

/// Sets up the memory, the allocator and the imports of the world (and
/// of WASI with `wasi`). Must run before any other function is defined.
pub fn imports(ctx: &mut Ctx, world: Option<&World>, wasi: bool) {
    ctx.memory = true;
    let mut core = Vec::new();
    for f in world.iter().flat_map(|w| w.imports.iter()) {
        if let Some(why) = &f.unsupported {
            ctx.unavailable.insert(f.name.clone(), why.clone());
            continue;
        }
        if named(f) {
            ctx.unavailable.insert(
                f.name.clone(),
                "WASM builds do not support WIT records and variants yet".to_string(),
            );
            continue;
        }
        let mut params: Vec<ValType> = f.params.iter().flat_map(|(_, t)| flat(t)).collect();
        let mut results = f.result.as_ref().map(flat).unwrap_or_default();
        if results.len() > 1 {
            params.push(ValType::I32);
            results = Vec::new();
        }
        if params.len() > MAX_FLAT_PARAMS {
            ctx.unavailable.insert(
                f.name.clone(),
                "it has too many parameters for the canonical ABI".to_string(),
            );
            continue;
        }
        let module = f
            .interface
            .as_ref()
            .map_or_else(|| "$root".to_string(), |i| i.key.clone());
        let ty = ctx.types.func(params, results);
        core.push((f, ctx.import(&module, &f.wit_name, ty)));
    }

    ctx.heap_base = HEAP_BASE;
    if wasi {
        crate::wasi::imports(ctx);
    }
    let heap = ctx.add_global_init(ValType::I32, ctx.heap_base);
    let realloc = ctx.define(realloc(heap));
    ctx.realloc = Some(realloc);
    ctx.heap = Some(heap);
    ctx.extra_exports
        .push(("cabi_realloc".to_string(), ExportKind::Func, realloc));
    ctx.extra_exports
        .push(("memory".to_string(), ExportKind::Memory, 0));

    for (f, import) in core {
        let wrapper = import_wrapper(ctx, f, import);
        let index = ctx.define(wrapper);
        let r = FnRef {
            index,
            params: f.params.iter().map(|(_, t)| t.ty()).collect(),
            ret: f.result.as_ref().map(WitTy::ty),
        };
        match &f.interface {
            Some(i) => {
                ctx.wit_externs
                    .entry(i.module.clone())
                    .or_default()
                    .insert(f.name.clone(), r);
            }
            None => {
                ctx.externs.insert(f.name.clone(), r);
            }
        }
    }
}

/// Exports the world's functions, implemented by Belsk2 functions of the
/// unit named after their interface, or of `exported` for functions of
/// the world itself.
pub fn exports(ctx: &mut Ctx, world: &World, units: &[Unit], exported: Option<usize>) {
    let at = Span::in_file(
        units
            .iter()
            .find_map(|u| u.program.stmts.first())
            .map_or(0, |s| s.span.file),
        1,
        1,
    );
    for f in &world.exports {
        if let Err(e) = export(ctx, f, units, exported) {
            ctx.errors.push(if e.span.is_some() { e } else { e.at(at) });
        }
    }
}

fn export(ctx: &mut Ctx, f: &WitFunc, units: &[Unit], exported: Option<usize>) -> R<()> {
    let what = format!("the WIT world exports '{}'", core_name(f));
    if let Some(why) = &f.unsupported {
        return Err(Error::compile(format!(
            "{what}, but {why}, which Belsk2 does not support yet"
        )));
    }
    if named(f) {
        return Err(Error::compile(format!(
            "{what}, which uses WIT records or variants; WASM builds do not support them yet"
        )));
    }
    let unit = match &f.interface {
        Some(i) => units
            .iter()
            .position(|u| u.name == i.module && !u.interface)
            .ok_or_else(|| {
                Error::compile(format!(
                    "{what}; add `{}` to src/{}.belsk2",
                    f.signature(),
                    i.module.replace('.', "/")
                ))
            })?,
        None => exported.ok_or_else(|| {
            Error::compile(format!(
                "{what}; add `{}` to src/main.belsk2 or src/lib.belsk2",
                f.signature()
            ))
        })?,
    };
    let module = units.get(unit).map(|u| u.name.clone()).unwrap_or_default();
    let decl = ctx.unit_fn(unit, &f.name).ok_or_else(|| {
        Error::compile(format!(
            "{what}; add `{}` to module '{module}'",
            f.signature()
        ))
    })?;
    let params: Vec<Ty> = f.params.iter().map(|(_, t)| t.ty()).collect();
    let ret = f.result.as_ref().map(WitTy::ty);
    let actual_params: Vec<Ty> = decl.params.iter().map(|p| p.resolved.canonical()).collect();
    let actual_ret = if decl.ret.is_none() && decl.ret_resolved.is_any() {
        None
    } else {
        Some(decl.ret_resolved.canonical())
    };
    if !decl.type_params.is_empty() || actual_params != params || actual_ret != ret {
        return Err(Error::compile(format!(
            "'{}' does not match the WIT world, which expects `{}`",
            f.name,
            f.signature()
        ))
        .at(decl.span));
    }
    let target = ctx.user_fn(unit, &decl, &[])?;

    let mut flat_params: Vec<ValType> = f.params.iter().flat_map(|(_, t)| flat(t)).collect();
    if flat_params.len() > MAX_FLAT_PARAMS {
        return Err(Error::compile(format!(
            "{what}, which has too many parameters for the canonical ABI"
        ))
        .at(decl.span));
    }
    let result_flat = f.result.as_ref().map(flat).unwrap_or_default();
    let results = if result_flat.len() > 1 {
        vec![ValType::I32]
    } else {
        result_flat
    };
    let mut a = Asm::new(std::mem::take(&mut flat_params), results);
    let mut local = 0;
    for (_, t) in &f.params {
        let n = flat(t).len() as u32;
        for i in 0..n {
            a.op(I::LocalGet(local + i));
        }
        local += n;
        lift(ctx, &mut a, t);
    }
    a.op(I::Call(target.index));
    let heap = ctx.heap.unwrap_or(0);
    let reset = [I::I32Const(ctx.heap_base), I::GlobalSet(heap)];
    match &f.result {
        Some(t) if in_memory(t) => {
            lower(ctx, &mut a, t);
            let (p, l) = (a.local(ValType::I32), a.local(ValType::I32));
            a.ops([
                I::LocalSet(l),
                I::LocalSet(p),
                I::I32Const(RET_AREA),
                I::LocalGet(p),
                I::I32Store(mem(0)),
                I::I32Const(RET_AREA),
                I::LocalGet(l),
                I::I32Store(mem(4)),
                I::I32Const(RET_AREA),
            ]);
            let mut post = Asm::new(vec![ValType::I32], Vec::new());
            post.ops(reset);
            let post = ctx.define(post);
            ctx.export(post, &format!("cabi_post_{}", core_name(f)));
        }
        _ => a.ops(reset),
    }
    let glue = ctx.define(a);
    ctx.export(glue, &core_name(f));
    Ok(())
}

/// A Belsk2-callable function that calls the host function `import`.
fn import_wrapper(ctx: &mut Ctx, f: &WitFunc, import: u32) -> Asm {
    let params: Vec<ValType> = f
        .params
        .iter()
        .filter_map(|(_, t)| ctx.types.val(&t.ty()))
        .collect();
    let results: Vec<ValType> = f
        .result
        .iter()
        .filter_map(|t| ctx.types.val(&t.ty()))
        .collect();
    let mut a = Asm::new(params, results);
    for (i, (_, t)) in f.params.iter().enumerate() {
        a.op(I::LocalGet(i as u32));
        lower(ctx, &mut a, t);
    }
    match &f.result {
        Some(t) if in_memory(t) => {
            let rp = a.local(ValType::I32);
            let realloc = ctx.realloc.unwrap_or(0);
            a.ops([
                I::I32Const(0),
                I::I32Const(0),
                I::I32Const(4),
                I::I32Const(8),
                I::Call(realloc),
                I::LocalTee(rp),
                I::Call(import),
                I::LocalGet(rp),
                I::I32Load(mem(0)),
                I::LocalGet(rp),
                I::I32Load(mem(4)),
            ]);
            lift(ctx, &mut a, t);
        }
        Some(WitTy::Bool) => a.ops([I::Call(import), I::I32Const(0), I::I32Ne]),
        _ => a.op(I::Call(import)),
    }
    a
}

/// Turns the flattened value(s) of `t` on the stack into a Belsk2 value.
fn lift(ctx: &mut Ctx, a: &mut Asm, t: &WitTy) {
    match t {
        WitTy::Bool => a.ops([I::I32Const(0), I::I32Ne]),
        WitTy::String => a.op(I::Call(ctx.helper(Helper::LiftStr))),
        WitTy::List(elem) => a.op(I::Call(ctx.helper(Helper::LiftList((**elem).clone())))),
        _ => {}
    }
}

/// Turns the Belsk2 value of `t` on the stack into its flattened values.
fn lower(ctx: &mut Ctx, a: &mut Asm, t: &WitTy) {
    match t {
        WitTy::String => a.op(I::Call(ctx.helper(Helper::LowerStr))),
        WitTy::List(elem) => a.op(I::Call(ctx.helper(Helper::LowerList((**elem).clone())))),
        _ => {}
    }
}

/// `cabi_realloc(old, old_size, align, new_size)`: a bump allocator that
/// grows the memory when needed.
fn realloc(heap: u32) -> Asm {
    use ValType::I32;
    let mut a = Asm::new(vec![I32, I32, I32, I32], vec![I32]);
    let (p, end) = (a.local(I32), a.local(I32));
    let mem_bytes = [I::MemorySize(0), I::I32Const(16), I::I32Shl];
    a.ops([
        I::GlobalGet(heap),
        I::LocalGet(2),
        I::I32Add,
        I::I32Const(1),
        I::I32Sub,
        I::I32Const(0),
        I::LocalGet(2),
        I::I32Sub,
        I::I32And,
        I::LocalTee(p),
        I::LocalGet(3),
        I::I32Add,
        I::LocalTee(end),
    ]);
    a.ops(mem_bytes.clone());
    a.ops([I::I32GtU, I::If(BlockType::Empty), I::LocalGet(end)]);
    a.ops(mem_bytes);
    a.ops([
        I::I32Sub,
        I::I32Const(0xFFFF),
        I::I32Add,
        I::I32Const(16),
        I::I32ShrU,
        I::MemoryGrow(0),
        I::I32Const(-1),
        I::I32Eq,
        I::If(BlockType::Empty),
        I::Unreachable,
        I::End,
        I::End,
        I::LocalGet(end),
        I::GlobalSet(heap),
        I::LocalGet(0),
        I::If(BlockType::Empty),
        I::LocalGet(p),
        I::LocalGet(0),
        I::LocalGet(1),
        I::LocalGet(3),
        I::LocalGet(1),
        I::LocalGet(3),
        I::I32LtU,
        I::Select,
        I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        },
        I::End,
        I::LocalGet(p),
    ]);
    a
}

/// Bodies of the canonical ABI helpers.
pub fn helper(ctx: &mut Ctx, h: &Helper) -> R<Asm> {
    use ValType::I32;
    let s = ctx.types.string();
    let sref = nullable(s);
    let realloc = ctx.realloc.unwrap_or(0);
    let inc = |a: &mut Asm, local: u32, by: i32| {
        a.ops([
            I::LocalGet(local),
            I::I32Const(by),
            I::I32Add,
            I::LocalSet(local),
        ]);
    };
    let mut a;
    match h {
        Helper::LowerStr => {
            a = Asm::new(vec![sref], vec![I32, I32]);
            let (n, i, c, bytes, ptr, p) = (
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
            );
            a.ops([I::LocalGet(0), I::ArrayLen, I::LocalSet(n)]);
            // Count the bytes.
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(0),
                I::LocalGet(i),
                I::ArrayGet(s),
                I::LocalSet(c),
                I::LocalGet(bytes),
                I::I32Const(1),
                I::I32Add,
                I::LocalGet(c),
                I::I32Const(0x80),
                I::I32GeU,
                I::I32Add,
                I::LocalGet(c),
                I::I32Const(0x800),
                I::I32GeU,
                I::I32Add,
                I::LocalGet(c),
                I::I32Const(0x10000),
                I::I32GeU,
                I::I32Add,
                I::LocalSet(bytes),
            ]);
            inc(&mut a, i, 1);
            a.ops([
                I::Br(0),
                I::End,
                I::End,
                I::I32Const(0),
                I::I32Const(0),
                I::I32Const(1),
                I::LocalGet(bytes),
                I::Call(realloc),
                I::LocalTee(ptr),
                I::LocalSet(p),
                I::I32Const(0),
                I::LocalSet(i),
            ]);
            // Encode.
            let byte = |a: &mut Asm, offset: u64, shift: i32, mask: i32, tag: i32| {
                a.ops([I::LocalGet(p), I::LocalGet(c)]);
                if shift > 0 {
                    a.ops([I::I32Const(shift), I::I32ShrU]);
                }
                a.ops([
                    I::I32Const(mask),
                    I::I32And,
                    I::I32Const(tag),
                    I::I32Or,
                    I::I32Store8(mem(offset)),
                ]);
            };
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(0),
                I::LocalGet(i),
                I::ArrayGet(s),
                I::LocalSet(c),
                I::LocalGet(c),
                I::I32Const(0x80),
                I::I32LtU,
                I::If(BlockType::Empty),
            ]);
            byte(&mut a, 0, 0, 0x7F, 0);
            inc(&mut a, p, 1);
            a.ops([
                I::Else,
                I::LocalGet(c),
                I::I32Const(0x800),
                I::I32LtU,
                I::If(BlockType::Empty),
            ]);
            byte(&mut a, 0, 6, 0x1F, 0xC0);
            byte(&mut a, 1, 0, 0x3F, 0x80);
            inc(&mut a, p, 2);
            a.ops([
                I::Else,
                I::LocalGet(c),
                I::I32Const(0x10000),
                I::I32LtU,
                I::If(BlockType::Empty),
            ]);
            byte(&mut a, 0, 12, 0x0F, 0xE0);
            byte(&mut a, 1, 6, 0x3F, 0x80);
            byte(&mut a, 2, 0, 0x3F, 0x80);
            inc(&mut a, p, 3);
            a.op(I::Else);
            byte(&mut a, 0, 18, 0x07, 0xF0);
            byte(&mut a, 1, 12, 0x3F, 0x80);
            byte(&mut a, 2, 6, 0x3F, 0x80);
            byte(&mut a, 3, 0, 0x3F, 0x80);
            inc(&mut a, p, 4);
            a.ops([I::End, I::End, I::End]);
            inc(&mut a, i, 1);
            a.ops([
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(ptr),
                I::LocalGet(bytes),
            ]);
        }
        Helper::LiftStr => {
            a = Asm::new(vec![I32, I32], vec![sref]);
            let (i, n, k, b, c, out) = (
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(sref),
            );
            let load = |a: &mut Asm, offset: u64| {
                a.ops([
                    I::LocalGet(0),
                    I::LocalGet(i),
                    I::I32Add,
                    I::I32Load8U(mem(offset)),
                ]);
            };
            // Count the characters: every byte that does not continue one.
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(1),
                I::I32GeU,
                I::BrIf(1),
            ]);
            load(&mut a, 0);
            a.ops([
                I::I32Const(0xC0),
                I::I32And,
                I::I32Const(0x80),
                I::I32Ne,
                I::LocalGet(n),
                I::I32Add,
                I::LocalSet(n),
            ]);
            inc(&mut a, i, 1);
            a.ops([
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(n),
                I::ArrayNewDefault(s),
                I::LocalSet(out),
                I::I32Const(0),
                I::LocalSet(i),
            ]);
            // Decode.
            let cont = |a: &mut Asm, offset: u64, shift: i32| {
                load(a, offset);
                a.ops([I::I32Const(0x3F), I::I32And]);
                if shift > 0 {
                    a.ops([I::I32Const(shift), I::I32Shl]);
                }
                a.op(I::I32Or);
            };
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(1),
                I::I32GeU,
                I::BrIf(1),
            ]);
            load(&mut a, 0);
            a.ops([
                I::LocalSet(b),
                I::LocalGet(b),
                I::I32Const(0x80),
                I::I32LtU,
                I::If(BlockType::Empty),
                I::LocalGet(b),
                I::LocalSet(c),
            ]);
            inc(&mut a, i, 1);
            a.ops([
                I::Else,
                I::LocalGet(b),
                I::I32Const(0xE0),
                I::I32LtU,
                I::If(BlockType::Empty),
                I::LocalGet(b),
                I::I32Const(0x1F),
                I::I32And,
                I::I32Const(6),
                I::I32Shl,
            ]);
            cont(&mut a, 1, 0);
            a.op(I::LocalSet(c));
            inc(&mut a, i, 2);
            a.ops([
                I::Else,
                I::LocalGet(b),
                I::I32Const(0xF0),
                I::I32LtU,
                I::If(BlockType::Empty),
                I::LocalGet(b),
                I::I32Const(0x0F),
                I::I32And,
                I::I32Const(12),
                I::I32Shl,
            ]);
            cont(&mut a, 1, 6);
            cont(&mut a, 2, 0);
            a.op(I::LocalSet(c));
            inc(&mut a, i, 3);
            a.ops([
                I::Else,
                I::LocalGet(b),
                I::I32Const(0x07),
                I::I32And,
                I::I32Const(18),
                I::I32Shl,
            ]);
            cont(&mut a, 1, 12);
            cont(&mut a, 2, 6);
            cont(&mut a, 3, 0);
            a.op(I::LocalSet(c));
            inc(&mut a, i, 4);
            a.ops([
                I::End,
                I::End,
                I::End,
                I::LocalGet(out),
                I::LocalGet(k),
                I::LocalGet(c),
                I::ArraySet(s),
            ]);
            inc(&mut a, k, 1);
            a.ops([I::Br(0), I::End, I::End, I::LocalGet(out)]);
        }
        Helper::LowerList(elem) | Helper::LiftList(elem) => {
            let ty = elem.ty();
            let at = ctx.types.array(&ty).ok_or_else(|| {
                Error::compile(format!(
                    "lists of {} are not supported yet",
                    elem.wit_name()
                ))
            })?;
            let (size, align) = size_align(elem);
            let lower_side = matches!(h, Helper::LowerList(_));
            a = if lower_side {
                Asm::new(vec![nullable(at.obj)], vec![I32, I32])
            } else {
                Asm::new(vec![I32, I32], vec![nullable(at.obj)])
            };
            let (n, ptr, i, addr) = (a.local(I32), a.local(I32), a.local(I32), a.local(I32));
            let raw = a.local(nullable(at.raw));
            if lower_side {
                a.ops([
                    I::LocalGet(0),
                    I::StructGet {
                        struct_type_index: at.obj,
                        field_index: 1,
                    },
                    I::LocalSet(n),
                    I::LocalGet(0),
                    I::StructGet {
                        struct_type_index: at.obj,
                        field_index: 0,
                    },
                    I::LocalSet(raw),
                    I::I32Const(0),
                    I::I32Const(0),
                    I::I32Const(align),
                    I::LocalGet(n),
                    I::I32Const(size),
                    I::I32Mul,
                    I::Call(realloc),
                    I::LocalSet(ptr),
                ]);
            } else {
                a.ops([
                    I::LocalGet(0),
                    I::LocalSet(ptr),
                    I::LocalGet(1),
                    I::LocalSet(n),
                    I::LocalGet(n),
                    I::ArrayNewDefault(at.raw),
                    I::LocalSet(raw),
                ]);
            }
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(ptr),
                I::LocalGet(i),
                I::I32Const(size),
                I::I32Mul,
                I::I32Add,
                I::LocalSet(addr),
            ]);
            let item = [I::LocalGet(raw), I::LocalGet(i), I::ArrayGet(at.raw)];
            if lower_side {
                if in_memory(elem) {
                    let (tp, tl) = (a.local(I32), a.local(I32));
                    a.ops(item);
                    lower(ctx, &mut a, elem);
                    a.ops([
                        I::LocalSet(tl),
                        I::LocalSet(tp),
                        I::LocalGet(addr),
                        I::LocalGet(tp),
                        I::I32Store(mem(0)),
                        I::LocalGet(addr),
                        I::LocalGet(tl),
                        I::I32Store(mem(4)),
                    ]);
                } else {
                    a.op(I::LocalGet(addr));
                    a.ops(item);
                    a.op(store(elem));
                }
            } else {
                a.ops([I::LocalGet(raw), I::LocalGet(i)]);
                if in_memory(elem) {
                    a.ops([
                        I::LocalGet(addr),
                        I::I32Load(mem(0)),
                        I::LocalGet(addr),
                        I::I32Load(mem(4)),
                    ]);
                    lift(ctx, &mut a, elem);
                } else {
                    a.ops([I::LocalGet(addr), load(elem)]);
                    lift(ctx, &mut a, elem);
                }
                a.op(I::ArraySet(at.raw));
            }
            inc(&mut a, i, 1);
            a.ops([I::Br(0), I::End, I::End]);
            if lower_side {
                a.ops([I::LocalGet(ptr), I::LocalGet(n)]);
            } else {
                a.ops([
                    I::LocalGet(raw),
                    I::RefAsNonNull,
                    I::LocalGet(n),
                    I::StructNew(at.obj),
                ]);
            }
        }
        _ => return Err(Error::compile("internal compiler error: not an ABI helper")),
    }
    Ok(a)
}

fn store(t: &WitTy) -> I<'static> {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::I8 | IntKind::U8) => I::I32Store8(mem(0)),
        WitTy::Int(IntKind::I16 | IntKind::U16) => I::I32Store16(mem(0)),
        WitTy::Int(IntKind::I64 | IntKind::U64) => I::I64Store(mem(0)),
        WitTy::Float(FloatKind::F32) => I::F32Store(mem(0)),
        WitTy::Float(FloatKind::F64) => I::F64Store(mem(0)),
        _ => I::I32Store(mem(0)),
    }
}

fn load(t: &WitTy) -> I<'static> {
    match t {
        WitTy::Bool | WitTy::Int(IntKind::U8) => I::I32Load8U(mem(0)),
        WitTy::Int(IntKind::I8) => I::I32Load8S(mem(0)),
        WitTy::Int(IntKind::U16) => I::I32Load16U(mem(0)),
        WitTy::Int(IntKind::I16) => I::I32Load16S(mem(0)),
        WitTy::Int(IntKind::I64 | IntKind::U64) => I::I64Load(mem(0)),
        WitTy::Float(FloatKind::F32) => I::F32Load(mem(0)),
        WitTy::Float(FloatKind::F64) => I::F64Load(mem(0)),
        _ => I::I32Load(mem(0)),
    }
}

/// Wraps a core module into a component of the world.
pub fn encode(
    mut core: Vec<u8>,
    resolve: &wit_parser::Resolve,
    world: wit_parser::WorldId,
) -> Result<Vec<u8>, Vec<Error>> {
    let fail = |e: &dyn std::fmt::Display| {
        vec![Error::compile(format!(
            "internal compiler error: cannot build the component: {e:#}"
        ))]
    };
    wit_component::embed_component_metadata(
        &mut core,
        resolve,
        world,
        wit_component::StringEncoding::UTF8,
        false,
    )
    .map_err(|e| fail(&e))?;
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&core)
        .map_err(|e| fail(&e))?
        .encode()
        .map_err(|e| fail(&e))
}
