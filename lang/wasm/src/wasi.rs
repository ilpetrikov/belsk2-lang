//! The `wasi0.2` target: a component for WASI 0.2 hosts (`wasmtime run`,
//! ...). `prinb` and `input` use `wasi:cli/stdout` and `wasi:cli/stdin`,
//! and `main` is exported as `wasi:cli/run`: it gets the command-line
//! arguments from `wasi:cli/environment`, and an exit code other than 0
//! goes to `wasi:cli/exit`. The id bank (`idb`, `reab`) is a small Belsk2
//! module compiled into the program ([`BANK`]).

use std::rc::Rc;

use belsk2_project::World;
use belsk2_syntax::ast::FnDecl;
use belsk2_syntax::{Error, Ty};
use wasm_encoder::{BlockType, Instruction as I, MemArg, ValType};
use wit_parser::{CloneMaps, Resolve, WorldId};

use crate::codegen::{Ctx, R};
use crate::helpers::{Asm, Helper};
use crate::types::nullable;

/// The parts of WASI 0.2 that Belsk2 uses. Hosts provide the full
/// interfaces; importing a subset of them is allowed.
const WASI_WIT: &str = r#"
package belsk2:wasi;

world imports {
    import wasi:cli/stdout@0.2.0;
    import wasi:cli/stdin@0.2.0;
    import wasi:cli/environment@0.2.0;
    import wasi:cli/exit@0.2.0;
}

world command {
    include imports;
    export wasi:cli/run@0.2.0;
}
package wasi:io@0.2.0 {
    interface error {
        resource error;
    }

    interface streams {
        use error.{error};

        variant stream-error {
            last-operation-failed(error),
            closed,
        }

        resource input-stream {
            blocking-read: func(len: u64) -> result<list<u8>, stream-error>;
        }

        resource output-stream {
            blocking-write-and-flush: func(contents: list<u8>) -> result<_, stream-error>;
        }
    }
}

package wasi:cli@0.2.0 {
    interface stdout {
        use wasi:io/streams@0.2.0.{output-stream};
        get-stdout: func() -> output-stream;
    }

    interface stdin {
        use wasi:io/streams@0.2.0.{input-stream};
        get-stdin: func() -> input-stream;
    }

    interface environment {
        get-arguments: func() -> list<string>;
    }

    interface exit {
        exit: func(status: result);
        exit-with-code: func(status-code: u8);
    }

    interface run {
        run: func() -> result;
    }
}

"#;

const STREAMS: &str = "wasi:io/streams@0.2.0";
/// Most bytes `blocking-write-and-flush` accepts at once.
const MAX_WRITE: i32 = 4096;
/// Where read input waits to be used, in linear memory (after the
/// return area), and how big that buffer is.
const INPUT: i32 = 16;
const INPUT_SIZE: i32 = 4096;
/// Where the allocator starts for this target: after the input buffer.
pub const HEAP_BASE: i32 = INPUT + INPUT_SIZE;

/// The id bank, compiled into every wasi0.2 program: `idb 5 = x` stores
/// `str(x)` under 5, `reab(5)` stores the line it reads, and `prinb(5)`
/// prints what is stored under 5 (if anything).
pub const BANK: &str = r#"
var bankKeys: long[] = [];
var bankValues: string[] = [];

fn bankSet(k: long, v: string) {
    var i = 0;
    while i < len(bankKeys) {
        if bankKeys[i] == k {
            bankValues[i] = v;
            return;
        }
        i += 1;
    }
    push(bankKeys, k);
    push(bankValues, v);
}

fn bankGet(k: long): string {
    var i = 0;
    while i < len(bankKeys) {
        if bankKeys[i] == k {
            return bankValues[i];
        }
        i += 1;
    }
    return null;
}
"#;

/// Function indices of the WASI imports and the globals caching the
/// stream handles.
#[derive(Clone, Copy)]
pub struct Wasi {
    get_stdout: u32,
    get_stdin: u32,
    write: u32,
    read: u32,
    get_arguments: u32,
    exit_with_code: u32,
    stdout: u32,
    stdin: u32,
    /// Read but unused input: `INPUT[pos..len]`; `eof` once stdin ended.
    in_pos: u32,
    in_len: u32,
    in_eof: u32,
}

fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

fn mem4(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

/// Adds the WASI imports. Must run before any function is defined.
pub fn imports(ctx: &mut Ctx) {
    use ValType::{I32, I64};
    let handle = ctx.types.func(Vec::new(), vec![I32]);
    let get_stdout = ctx.import("wasi:cli/stdout@0.2.0", "get-stdout", handle);
    let get_stdin = ctx.import("wasi:cli/stdin@0.2.0", "get-stdin", handle);
    let ty = ctx.types.func(vec![I32, I32, I32, I32], Vec::new());
    let write = ctx.import(
        STREAMS,
        "[method]output-stream.blocking-write-and-flush",
        ty,
    );
    let ty = ctx.types.func(vec![I32, I64, I32], Vec::new());
    let read = ctx.import(STREAMS, "[method]input-stream.blocking-read", ty);
    let ty = ctx.types.func(vec![I32], Vec::new());
    let get_arguments = ctx.import("wasi:cli/environment@0.2.0", "get-arguments", ty);
    let exit_with_code = ctx.import("wasi:cli/exit@0.2.0", "exit-with-code", ty);
    let stdout = ctx.add_global(I32);
    let stdin = ctx.add_global(I32);
    let (in_pos, in_len, in_eof) = (
        ctx.add_global(I32),
        ctx.add_global(I32),
        ctx.add_global(I32),
    );
    ctx.heap_base = HEAP_BASE;
    ctx.wasi = Some(Wasi {
        get_stdout,
        get_stdin,
        write,
        read,
        get_arguments,
        exit_with_code,
        stdout,
        stdin,
        in_pos,
        in_len,
        in_eof,
    });
}

/// Exports `main` as `wasi:cli/run`. `main(args)` gets the command-line
/// arguments (without the program name); an exit code other than 0
/// returned by `main` ends the program with that code.
pub fn run_export(ctx: &mut Ctx, main: &Rc<FnDecl>, unit: usize) -> R<()> {
    let Some(w) = ctx.wasi else {
        return Err(Error::compile("internal compiler error: no WASI imports"));
    };
    let f = ctx.user_fn(unit, main, &[])?;
    let mut a = Asm::new(Vec::new(), vec![ValType::I32]);
    if !main.params.is_empty() {
        // get-arguments writes (address, count) of a list of strings.
        let realloc = ctx.realloc.unwrap_or(0);
        let (rp, ptr, n) = (
            a.local(ValType::I32),
            a.local(ValType::I32),
            a.local(ValType::I32),
        );
        a.ops([
            I::I32Const(0),
            I::I32Const(0),
            I::I32Const(4),
            I::I32Const(8),
            I::Call(realloc),
            I::LocalTee(rp),
            I::Call(w.get_arguments),
            I::LocalGet(rp),
            I::I32Load(mem4(0)),
            I::LocalSet(ptr),
            I::LocalGet(rp),
            I::I32Load(mem4(4)),
            I::LocalSet(n),
            // Skip the program name (a string is 8 bytes: address, length).
            I::LocalGet(n),
            I::If(BlockType::Empty),
            I::LocalGet(ptr),
            I::I32Const(8),
            I::I32Add,
            I::LocalSet(ptr),
            I::LocalGet(n),
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(n),
            I::End,
            I::LocalGet(ptr),
            I::LocalGet(n),
        ]);
        let lift = ctx.helper(Helper::LiftList(belsk2_project::WitTy::String));
        a.op(I::Call(lift));
    }
    a.op(I::Call(f.index));
    let code = a.local(ValType::I32);
    match &f.ret {
        Some(Ty::Int(k)) if k.bits() == 64 => a.op(I::I32WrapI64),
        Some(Ty::Int(_)) => {}
        _ => a.op(I::I32Const(0)),
    }
    a.ops([
        I::LocalTee(code),
        I::If(BlockType::Empty),
        I::LocalGet(code),
        I::I32Const(0xFF),
        I::I32And,
        I::Call(w.exit_with_code),
        I::End,
        I::I32Const(0),
    ]);
    let index = ctx.define(a);
    ctx.export(index, "wasi:cli/run@0.2.0#run");
    Ok(())
}

/// The resolve and world to embed: the WASI world, merged into the
/// project's own world if it has one.
pub fn world(user: Option<&World>, command: bool) -> Result<(Resolve, WorldId), Vec<Error>> {
    let fail = |e: &dyn std::fmt::Display| {
        vec![Error::compile(format!(
            "internal compiler error: the WASI world: {e:#}"
        ))]
    };
    let mut resolve = user.map(|w| w.resolve.clone()).unwrap_or_default();
    let pkg = resolve
        .push_str("belsk2-wasi.wit", WASI_WIT)
        .map_err(|e| fail(&e))?;
    let name = if command { "command" } else { "imports" };
    let wasi = resolve
        .select_world(&[pkg], Some(name))
        .map_err(|e| fail(&e))?;
    match user {
        Some(w) => {
            resolve
                .merge_worlds(wasi, w.id, &mut CloneMaps::default())
                .map_err(|e| fail(&e))?;
            Ok((resolve, w.id))
        }
        None => Ok((resolve, wasi)),
    }
}

/// The cached handle of a stream, fetched on first use.
fn handle(a: &mut Asm, global: u32, get: u32) {
    a.ops([
        I::GlobalGet(global),
        I::I32Eqz,
        I::If(BlockType::Empty),
        I::Call(get),
        I::GlobalSet(global),
        I::End,
        I::GlobalGet(global),
    ]);
}

/// Bodies of `Helper::WasiWrite` (writes a string to stdout) and
/// `Helper::WasiReadLine` (reads a line from stdin, `""` at the end).
/// Both give back the memory they used.
pub fn helper(ctx: &mut Ctx, h: &Helper) -> R<Asm> {
    use ValType::I32;
    let Some(w) = ctx.wasi else {
        return Err(Error::compile(
            "input and output need the wasi0.2 target (`belsk2 build --target wasi0.2`)",
        ));
    };
    let sref = nullable(ctx.types.string());
    let realloc = ctx.realloc.unwrap_or(0);
    let heap = ctx.heap.unwrap_or(0);
    let alloc = |a: &mut Asm, align: i32, size: i32, into: u32| {
        a.ops([
            I::I32Const(0),
            I::I32Const(0),
            I::I32Const(align),
            I::I32Const(size),
            I::Call(realloc),
            I::LocalSet(into),
        ]);
    };
    let mut a;
    match h {
        Helper::WasiWrite => {
            a = Asm::new(vec![sref], Vec::new());
            let (saved, h, ptr, len, off, n, rp) = (
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
            );
            a.ops([I::GlobalGet(heap), I::LocalSet(saved)]);
            handle(&mut a, w.stdout, w.get_stdout);
            a.op(I::LocalSet(h));
            let lower = ctx.helper(Helper::LowerStr);
            a.ops([
                I::LocalGet(0),
                I::Call(lower),
                I::LocalSet(len),
                I::LocalSet(ptr),
            ]);
            alloc(&mut a, 4, 12, rp);
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(off),
                I::LocalGet(len),
                I::I32GeU,
                I::BrIf(1),
                // n = min(len - off, MAX_WRITE)
                I::LocalGet(len),
                I::LocalGet(off),
                I::I32Sub,
                I::I32Const(MAX_WRITE),
                I::LocalGet(len),
                I::LocalGet(off),
                I::I32Sub,
                I::I32Const(MAX_WRITE),
                I::I32LtU,
                I::Select,
                I::LocalSet(n),
                I::LocalGet(h),
                I::LocalGet(ptr),
                I::LocalGet(off),
                I::I32Add,
                I::LocalGet(n),
                I::LocalGet(rp),
                I::Call(w.write),
                I::LocalGet(rp),
                I::I32Load8U(mem(0)),
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
                I::LocalGet(off),
                I::LocalGet(n),
                I::I32Add,
                I::LocalSet(off),
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(saved),
                I::GlobalSet(heap),
            ]);
        }
        Helper::WasiReadLine => {
            a = Asm::new(Vec::new(), vec![sref]);
            let (saved, h, buf, cap, len, rp, b, out, n) = (
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(I32),
                a.local(sref),
                a.local(I32),
            );
            a.ops([I::GlobalGet(heap), I::LocalSet(saved)]);
            handle(&mut a, w.stdin, w.get_stdin);
            a.op(I::LocalSet(h));
            alloc(&mut a, 4, 12, rp);
            a.ops([I::I32Const(64), I::LocalSet(cap)]);
            alloc(&mut a, 1, 64, buf);
            a.ops([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                // Out of buffered input: read the next block (up to
                // INPUT_SIZE bytes) into the buffer.
                I::GlobalGet(w.in_pos),
                I::GlobalGet(w.in_len),
                I::I32GeU,
                I::If(BlockType::Empty),
                I::GlobalGet(w.in_eof),
                I::BrIf(2),
                I::LocalGet(h),
                I::I64Const(i64::from(INPUT_SIZE)),
                I::LocalGet(rp),
                I::Call(w.read),
                // An error (or the end of input) ends the input.
                I::LocalGet(rp),
                I::I32Load8U(mem(0)),
                I::LocalGet(rp),
                I::I32Load(mem(8)),
                I::LocalTee(n),
                I::I32Eqz,
                I::I32Or,
                I::If(BlockType::Empty),
                I::I32Const(1),
                I::GlobalSet(w.in_eof),
                I::Br(3),
                I::End,
                I::I32Const(INPUT),
                I::LocalGet(rp),
                I::I32Load(mem(4)),
                I::LocalGet(n),
                I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                },
                I::I32Const(0),
                I::GlobalSet(w.in_pos),
                I::LocalGet(n),
                I::GlobalSet(w.in_len),
                I::End,
                // The next byte; a newline ends the line.
                I::I32Const(INPUT),
                I::GlobalGet(w.in_pos),
                I::I32Add,
                I::I32Load8U(mem(0)),
                I::LocalSet(b),
                I::GlobalGet(w.in_pos),
                I::I32Const(1),
                I::I32Add,
                I::GlobalSet(w.in_pos),
                I::LocalGet(b),
                I::I32Const(10),
                I::I32Eq,
                I::BrIf(1),
                I::LocalGet(len),
                I::LocalGet(cap),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::LocalGet(buf),
                I::LocalGet(cap),
                I::I32Const(1),
                I::LocalGet(cap),
                I::I32Const(2),
                I::I32Mul,
                I::Call(realloc),
                I::LocalSet(buf),
                I::LocalGet(cap),
                I::I32Const(2),
                I::I32Mul,
                I::LocalSet(cap),
                I::End,
                I::LocalGet(buf),
                I::LocalGet(len),
                I::I32Add,
                I::LocalGet(b),
                I::I32Store8(mem(0)),
                I::LocalGet(len),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(len),
                I::Br(0),
                I::End,
                I::End,
                // Drop a trailing '\r' (Windows line endings).
                I::LocalGet(len),
                I::If(BlockType::Empty),
                I::LocalGet(buf),
                I::LocalGet(len),
                I::I32Add,
                I::I32Const(1),
                I::I32Sub,
                I::I32Load8U(mem(0)),
                I::I32Const(13),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::LocalGet(len),
                I::I32Const(1),
                I::I32Sub,
                I::LocalSet(len),
                I::End,
                I::End,
            ]);
            let lift = ctx.helper(Helper::LiftStr);
            a.ops([
                I::LocalGet(buf),
                I::LocalGet(len),
                I::Call(lift),
                I::LocalSet(out),
                I::LocalGet(saved),
                I::GlobalSet(heap),
                I::LocalGet(out),
            ]);
        }
        _ => return Err(Error::compile("internal compiler error: not a WASI helper")),
    }
    Ok(a)
}
