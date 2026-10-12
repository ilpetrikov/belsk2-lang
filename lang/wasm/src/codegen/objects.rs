//! User-defined types in WASM builds.
//!
//! Every class, struct and enum with data is a GC struct, and all of them
//! are defined in one recursion group (they may refer to each other):
//!
//! - `$object` is `{ tag: i32 }`, the supertype of all of them. An
//!   interface value is a reference to `$object`.
//! - A class or struct is `{ tag, fields... }` with the fields of its base
//!   classes first, a subtype of its base class's struct (or `$object`).
//! - An enum with data has a base struct `{ tag, case }` and a struct for
//!   each case, `{ tag, case, data... }`. An enum without data is an `i32`,
//!   the index of its case.
//!
//! The tag tells the type of a value at run time: `is`, `as` and calls of
//! virtual and interface methods (through a "dispatch" function per
//! method) look at it. Structs are values: a changed field gives a new
//! struct, and a method of a struct returns `this` after its result so the
//! caller can store it where the struct came from.

use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::ast::{Expr, ExprKind, FnDecl, MemberKind, StmtKind, TypeDecl};
use belsk2_syntax::{Error, FloatKind, IntKind, MemberInfo, Span, Ty, TypeKind, TypeRef, Virt};
use wasm_encoder::{BlockType, HeapType, Instruction as I, ValType};

use super::{ret_type, unsupported, Ctx, FnGen, Job, R};
use crate::helpers::{Asm, Helper};
use crate::types::{non_null, nullable, ArrayType, Def};
use crate::Unit;

/// A user-defined type in the module.
pub struct ClassInfo {
    pub unit: usize,
    pub decl: Rc<TypeDecl>,
    pub ty: TypeRef,
    /// Identifies the type of a value at run time.
    pub tag: i32,
    /// Its struct type (`$object` for an interface, the base struct for an
    /// enum with data); `None` for an enum without data.
    pub st: Option<u32>,
    /// The struct of each case of an enum with data.
    pub cases: Vec<u32>,
    /// Instance fields, those of base classes first. Field `i` is struct
    /// field `i + 1` (after the tag).
    pub fields: Vec<(String, Ty)>,
    /// Static fields: their globals.
    pub statics: HashMap<String, (u32, Ty)>,
}

impl ClassInfo {
    /// The struct field index and type of instance field `name`.
    pub fn field(&self, name: &str) -> Option<(u32, Ty)> {
        let i = self.fields.iter().position(|(n, _)| n == name)?;
        self.fields.get(i).map(|(_, t)| (i as u32 + 1, t.clone()))
    }

    fn st(&self) -> u32 {
        self.st.unwrap_or(0)
    }
}

/// What a member function is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Method,
    Static,
    Getter,
    Setter,
}

/// A function of a type, ready to be called: the receiver (unless
/// static), then `params`.
#[derive(Clone)]
pub struct MemberRef {
    pub index: u32,
    pub params: Vec<Ty>,
    pub ret: Option<Ty>,
    /// A method of a struct returns `this` too (after its result).
    pub returns_this: bool,
}

fn has_data(t: &TypeRef) -> bool {
    t.variants().iter().any(|v| !v.fields.is_empty())
}

fn mentions_user(t: &Ty) -> bool {
    match t {
        Ty::Named(_) => true,
        Ty::Array(e) => mentions_user(e),
        Ty::Map(k, v) | Ty::KeyValue(k, v) => mentions_user(k) || mentions_user(v),
        _ => false,
    }
}

/// Defines the struct types of every user-defined type, their static
/// fields and their tags.
pub fn layout(ctx: &mut Ctx, units: &[Unit]) {
    let mut decls: Vec<(usize, Rc<TypeDecl>, TypeRef)> = Vec::new();
    for (u, unit) in units.iter().enumerate() {
        for s in &unit.program.stmts {
            if let StmtKind::Type(d) = &s.kind {
                if d.members
                    .iter()
                    .any(|m| matches!(m.kind, MemberKind::Drop(_)))
                {
                    ctx.errors.push(
                        Error::compile(
                            "destructors cannot be used in WASM builds: the garbage collector of WebAssembly does not run them",
                        )
                        .at(s.span),
                    );
                }
                if let Some(info) = &d.info {
                    decls.push((u, Rc::clone(d), info.reference()));
                }
            }
        }
    }
    if decls.is_empty() {
        return;
    }
    // Types the fields use that are not part of the group come first.
    let mut field_tys: Vec<Ty> = Vec::new();
    for (_, _, t) in &decls {
        t.with_body(|b| {
            field_tys.extend(b.fields.iter().chain(&b.statics).map(|f| f.ty.clone()));
            for v in &b.variants {
                field_tys.extend(v.fields.iter().map(|(_, t)| t.clone()));
            }
        });
    }
    ctx.types.string();
    for ty in &field_tys {
        prepare(ctx, ty);
    }

    let mut next = ctx.types.next_index();
    let start = next;
    let object = next;
    next += 1;
    // Base classes before the classes derived from them.
    decls.sort_by_key(|(_, _, t)| t.chain().len());
    for (i, (unit, decl, t)) in decls.iter().enumerate() {
        let tag = i as i32 + 1;
        let (st, cases) = match t.kind {
            TypeKind::Interface => (Some(object), Vec::new()),
            TypeKind::Enum if !has_data(t) => (None, Vec::new()),
            TypeKind::Enum => {
                let base = next;
                next += 1;
                let cases = t
                    .variants()
                    .iter()
                    .map(|_| {
                        next += 1;
                        next - 1
                    })
                    .collect();
                (Some(base), cases)
            }
            _ => {
                next += 1;
                (Some(next - 1), Vec::new())
            }
        };
        let vt = match st {
            Some(st) => nullable(st),
            None => ValType::I32,
        };
        ctx.types.set_user(t.id(), vt);
        let fields = t.all_fields().into_iter().map(|f| (f.name, f.ty)).collect();
        ctx.classes.insert(
            t.id(),
            ClassInfo {
                unit: *unit,
                decl: Rc::clone(decl),
                ty: t.clone(),
                tag,
                st,
                cases,
                fields,
                statics: HashMap::new(),
            },
        );
    }
    // Arrays and dictionaries of user-defined types used by fields belong
    // to the group.
    let mut extra: Vec<(u32, Def)> = Vec::new();
    let first_span = decls.first().map(|d| d.1.span).unwrap_or_default();
    for ty in &field_tys {
        if let Err(e) = user_types(ctx, ty, &mut next, &mut extra) {
            ctx.errors.push(e.at(first_span));
        }
    }

    let mut defs: Vec<Option<Def>> = (start..next).map(|_| None).collect();
    let mut put = |index: u32, def: Def| {
        if let Some(slot) = defs.get_mut((index - start) as usize) {
            *slot = Some(def);
        }
    };
    put(
        object,
        Def::Struct {
            fields: vec![ValType::I32],
            sub: None,
            last: false,
        },
    );
    let mut errors = Vec::new();
    let ids: Vec<usize> = decls.iter().map(|(_, _, t)| t.id()).collect();
    for id in &ids {
        let Some(info) = ctx.classes.get(id) else {
            continue;
        };
        let (kind, st, cases, fields, span) = (
            info.ty.kind,
            info.st,
            info.cases.clone(),
            info.fields.clone(),
            info.decl.span,
        );
        let base_st = info
            .ty
            .base()
            .and_then(|b| ctx.classes.get(&b.id()))
            .and_then(|b| b.st);
        let variants = info.ty.variants();
        let mut val = |ty: &Ty| match ctx.types.val(ty) {
            Some(v) => v,
            None => {
                errors.push(unsupported(format!("a field of type {ty} is"), span));
                ValType::I32
            }
        };
        match (kind, st) {
            (TypeKind::Class | TypeKind::Struct, Some(st)) => {
                let mut fs = vec![ValType::I32];
                fs.extend(fields.iter().map(|(_, t)| val(t)));
                put(
                    st,
                    Def::Struct {
                        fields: fs,
                        sub: Some(base_st.unwrap_or(object)),
                        last: false,
                    },
                );
            }
            (TypeKind::Enum, Some(base)) => {
                put(
                    base,
                    Def::Struct {
                        fields: vec![ValType::I32, ValType::I32],
                        sub: Some(object),
                        last: false,
                    },
                );
                for (case, v) in cases.iter().zip(&variants) {
                    let mut fs = vec![ValType::I32, ValType::I32];
                    fs.extend(v.fields.iter().map(|(_, t)| val(t)));
                    put(
                        *case,
                        Def::Struct {
                            fields: fs,
                            sub: Some(base),
                            last: true,
                        },
                    );
                }
            }
            _ => {}
        }
    }
    for (index, def) in extra {
        put(index, def);
    }
    ctx.errors.extend(errors);
    ctx.types.add_rec(defs.into_iter().flatten().collect());
    ctx.object = object;

    // Static fields are globals.
    for id in &ids {
        let statics: Vec<(String, Ty)> = ctx
            .classes
            .get(id)
            .and_then(|c| {
                c.ty.with_body(|b| {
                    b.statics
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect()
                })
            })
            .unwrap_or_default();
        let span = ctx.classes.get(id).map(|c| c.decl.span).unwrap_or_default();
        for (name, ty) in statics {
            match ctx.types.val(&ty) {
                Some(vt) => {
                    let g = ctx.add_global(vt);
                    if let Some(c) = ctx.classes.get_mut(id) {
                        c.statics.insert(name, (g, ty));
                    }
                }
                None => ctx
                    .errors
                    .push(unsupported(format!("a static field of type {ty} is"), span)),
            }
        }
    }
}

/// Defines the arrays and dictionaries (of types outside the group) that
/// fields need.
fn prepare(ctx: &mut Ctx, ty: &Ty) {
    match ty {
        Ty::Array(e) => {
            prepare(ctx, e);
            if !mentions_user(e) {
                ctx.types.array(e);
            }
        }
        Ty::Map(k, v) => {
            prepare(ctx, &Ty::array((**k).clone()));
            prepare(ctx, &Ty::array((**v).clone()));
            if !mentions_user(ty) {
                ctx.types.map(k, v);
            }
        }
        _ => {}
    }
}

/// Reserves group indices for arrays and dictionaries of user-defined
/// types.
fn user_types(ctx: &mut Ctx, ty: &Ty, next: &mut u32, out: &mut Vec<(u32, Def)>) -> R<()> {
    match ty {
        Ty::Array(e) => {
            user_types(ctx, e, next, out)?;
            if !mentions_user(e) || ctx.types.has_array(e) {
                return Ok(());
            }
            let Some(elem) = ctx.types.val(e) else {
                return Ok(());
            };
            let (raw, obj) = (*next, *next + 1);
            *next += 2;
            ctx.types.set_array(e, ArrayType { raw, obj, elem });
            out.push((raw, Def::Array(elem)));
            out.push((
                obj,
                Def::Struct {
                    fields: vec![non_null(raw), ValType::I32],
                    sub: None,
                    last: true,
                },
            ));
        }
        Ty::Map(k, v) => {
            let (ka, va) = (Ty::array((**k).clone()), Ty::array((**v).clone()));
            user_types(ctx, &ka, next, out)?;
            user_types(ctx, &va, next, out)?;
            if !mentions_user(ty) || ctx.types.has_map(k, v) {
                return Ok(());
            }
            let (Some(keys), Some(values)) = (ctx.types.array(k), ctx.types.array(v)) else {
                return Ok(());
            };
            let st = *next;
            *next += 1;
            let fields = ctx.types.map_fields(&keys, &values);
            ctx.types
                .set_map(k, v, crate::types::MapType { st, keys, values });
            out.push((
                st,
                Def::Struct {
                    fields,
                    sub: None,
                    last: true,
                },
            ));
        }
        Ty::KeyValue(..) if mentions_user(ty) => {
            return Err(Error::compile(
                "fields of type KeyValuePair with user-defined types are not supported in WASM builds yet",
            ))
        }
        _ => {}
    }
    Ok(())
}

// ---- lookups ----------------------------------------------------------

/// The value type of `t`.
pub fn val_of(ctx: &mut Ctx, t: &TypeRef) -> ValType {
    ctx.types.val(&Ty::Named(t.clone())).unwrap_or(ValType::I32)
}

/// Every type with values (classes that can be created, structs, enums
/// with data) that is a `t`.
fn concrete(ctx: &Ctx, t: &TypeRef) -> Vec<usize> {
    let mut ids: Vec<(i32, usize)> = ctx
        .classes
        .iter()
        .filter(|(_, c)| {
            let has_values = match c.ty.kind {
                TypeKind::Class => !c.ty.is_abstract(),
                TypeKind::Struct => true,
                TypeKind::Enum => c.st.is_some(),
                TypeKind::Interface => false,
            };
            has_values && c.ty.is_subtype_of(t)
        })
        .map(|(id, c)| (c.tag, *id))
        .collect();
    ids.sort();
    ids.into_iter().map(|(_, id)| id).collect()
}

/// The method `name` an object of type `c` runs (its own or inherited,
/// not abstract) and the type that declares it.
pub fn find_method(ctx: &Ctx, c: &TypeRef, name: &str) -> Option<(usize, Rc<FnDecl>)> {
    c.chain().iter().find_map(|t| {
        let info = ctx.classes.get(&t.id())?;
        info.decl.members.iter().find_map(|m| match &m.kind {
            MemberKind::Method(d) if d.name == name && m.mods.virt != Virt::Abstract => {
                Some((t.id(), Rc::clone(d)))
            }
            _ => None,
        })
    })
}

/// The `get` or `set` of property `name` of `c` (its own or inherited).
pub fn find_accessor(ctx: &Ctx, c: &TypeRef, name: &str, set: bool) -> Option<(usize, Rc<FnDecl>)> {
    c.chain().iter().find_map(|t| {
        let info = ctx.classes.get(&t.id())?;
        info.decl.members.iter().find_map(|m| match &m.kind {
            MemberKind::Prop(p) if p.name == name => {
                let a = if set { p.set.as_ref() } else { p.get.as_ref() };
                a.map(|a| (t.id(), Rc::clone(&a.decl)))
            }
            _ => None,
        })
    })
}

/// Whether a call of method `name` on a value of type `t` must look at
/// the value's real type.
pub fn is_virtual(t: &TypeRef, name: &str) -> bool {
    match t.member(name) {
        Some((_, MemberInfo::Method(m))) => {
            t.kind == TypeKind::Interface || (m.virt != Virt::None && !m.is_static)
        }
        Some((_, MemberInfo::Prop(_) | MemberInfo::Field(_))) => t.kind == TypeKind::Interface,
        _ => false,
    }
}

/// Reserves a function for `key` and builds it with `build` (which may
/// use the function itself).
pub(super) fn obj_fn(ctx: &mut Ctx, key: String, build: impl FnOnce(&mut Ctx) -> R<Asm>) -> u32 {
    if let Some(f) = ctx.keys.get(&key) {
        return f.index;
    }
    let index = ctx.add_def(0);
    ctx.keys.insert(
        key,
        super::FnRef {
            index,
            params: Vec::new(),
            ret: None,
        },
    );
    match build(ctx) {
        Ok(asm) => {
            let ty = ctx.types.func(asm.params.clone(), asm.results.clone());
            if let Some(def) = ctx.def_mut(index) {
                def.ty = ty;
                def.body = Some(asm);
            }
        }
        Err(e) => ctx.errors.push(e),
    }
    index
}

fn member_key(kind: &str, tid: usize, name: &str, inst: &[Ty]) -> String {
    let names: Vec<String> = inst.iter().map(|t| format!("{t:?}")).collect();
    format!("{kind}:{tid}:{name}<{}>", names.join(","))
}

/// The function of a method, property accessor or static method `decl`
/// of type `tid`.
pub fn member_fn(
    ctx: &mut Ctx,
    tid: usize,
    decl: &Rc<FnDecl>,
    kind: Kind,
    inst: &[Ty],
) -> R<MemberRef> {
    let key = member_key(&format!("{kind:?}"), tid, &decl.name, inst);
    let info_ty = ctx
        .classes
        .get(&tid)
        .map(|c| c.ty.clone())
        .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
    let is_struct = info_ty.kind == TypeKind::Struct;
    let subst: HashMap<String, Ty> = decl
        .type_params
        .iter()
        .cloned()
        .zip(inst.iter().cloned())
        .collect();
    let sub = |t: &Ty| t.substitute(&|n| subst.get(n).cloned());
    let mut params = Vec::new();
    for p in &decl.params {
        if p.resolved.is_any() {
            return Err(Error::compile(format!(
                "parameter '{}' needs a type in WASM builds",
                p.name
            ))
            .at(p.span));
        }
        params.push(sub(&p.resolved));
    }
    let ret = match kind {
        Kind::Setter => None,
        Kind::Getter => Some(sub(&decl.ret_resolved)),
        _ => ret_type(decl)?.map(|t| sub(&t)),
    };
    let returns_this = is_struct && matches!(kind, Kind::Method | Kind::Setter);
    if let Some(f) = ctx.keys.get(&key) {
        return Ok(MemberRef {
            index: f.index,
            params,
            ret,
            returns_this,
        });
    }
    let this = val_of(ctx, &info_ty);
    let mut vals = Vec::new();
    if kind != Kind::Static {
        vals.push(this);
    }
    for (p, t) in decl.params.iter().zip(&params) {
        vals.push(ctx.types.val(t).ok_or_else(|| {
            unsupported(format!("parameter '{}' of type {t} is", p.name), p.span)
        })?);
    }
    let mut results: Vec<ValType> = Vec::new();
    if let Some(t) = &ret {
        results.push(
            ctx.types
                .val(t)
                .ok_or_else(|| unsupported(format!("returning {t} is"), decl.span))?,
        );
    }
    if returns_this {
        results.push(this);
    }
    let ty = ctx.types.func(vals, results);
    let index = ctx.add_def(ty);
    ctx.keys.insert(
        key,
        super::FnRef {
            index,
            params: params.clone(),
            ret: ret.clone(),
        },
    );
    ctx.pending.push((
        index,
        Job::Member {
            tid,
            decl: Rc::clone(decl),
            kind,
            subst,
            ret: ret.clone(),
            returns_this,
        },
    ));
    Ok(MemberRef {
        index,
        params,
        ret,
        returns_this,
    })
}

/// The constructor parameters of type `tid`.
fn ctor_params(ctx: &Ctx, tid: usize) -> Vec<Ty> {
    ctx.classes
        .get(&tid)
        .and_then(|c| {
            c.ty.with_body(|b| b.ctor.iter().map(|(_, t)| t.clone()).collect())
        })
        .unwrap_or_default()
}

/// `new` creates a value of type `tid` (`Point(1, 2)`); `init` runs the
/// constructor on `this` (also for `: base(...)`).
pub fn ctor_fn(ctx: &mut Ctx, tid: usize, new: bool) -> R<MemberRef> {
    let key = format!("{}:{tid}", if new { "New" } else { "Init" });
    let ty = ctx
        .classes
        .get(&tid)
        .map(|c| c.ty.clone())
        .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
    let params = ctor_params(ctx, tid);
    let returns_this = !new && ty.kind == TypeKind::Struct;
    let ret = new.then(|| Ty::Named(ty.clone()));
    if let Some(f) = ctx.keys.get(&key) {
        return Ok(MemberRef {
            index: f.index,
            params,
            ret,
            returns_this,
        });
    }
    let this = val_of(ctx, &ty);
    let mut vals = Vec::new();
    if !new {
        vals.push(this);
    }
    for t in &params {
        vals.push(ctx.types.val(t).ok_or_else(|| {
            unsupported(format!("a parameter of type {t} is"), ty_span(ctx, tid))
        })?);
    }
    let results = if new || returns_this {
        vec![this]
    } else {
        Vec::new()
    };
    let fty = ctx.types.func(vals, results);
    let index = ctx.add_def(fty);
    ctx.keys.insert(
        key,
        super::FnRef {
            index,
            params: params.clone(),
            ret: ret.clone(),
        },
    );
    ctx.pending.push((index, Job::Ctor { tid, new }));
    Ok(MemberRef {
        index,
        params,
        ret,
        returns_this,
    })
}

fn ty_span(ctx: &Ctx, tid: usize) -> Span {
    ctx.classes
        .get(&tid)
        .map(|c| c.decl.span)
        .unwrap_or_default()
}

/// The parameter and return types of member `name` of `t`, for calls
/// through a dispatch function.
fn member_sig(t: &TypeRef, name: &str, kind: Kind) -> R<(Vec<Ty>, Option<Ty>)> {
    let fail = || Error::compile(format!("internal compiler error: no member '{name}'"));
    let (_, m) = t.member(name).ok_or_else(fail)?;
    Ok(match (m, kind) {
        (MemberInfo::Method(m), _) => {
            if !m.sig.type_params.is_empty() {
                return Err(Error::compile(format!(
                    "generic virtual or interface methods ('{name}') are not supported in WASM builds yet"
                )));
            }
            let ret =
                (!m.sig.ret.is_any() && !matches!(m.sig.ret, Ty::Null)).then(|| m.sig.ret.clone());
            (m.sig.params.clone(), ret)
        }
        (MemberInfo::Prop(p), Kind::Setter) => (vec![p.ty], None),
        (MemberInfo::Prop(p), _) => (Vec::new(), Some(p.ty)),
        (MemberInfo::Field(f), Kind::Setter) => (vec![f.ty], None),
        (MemberInfo::Field(f), _) => (Vec::new(), Some(f.ty)),
        _ => return Err(fail()),
    })
}

/// A function that calls method (or property accessor) `name` of the
/// real type of a value of type `tid`.
pub fn dispatch_fn(ctx: &mut Ctx, tid: usize, name: &str, kind: Kind) -> R<MemberRef> {
    let t = ctx
        .classes
        .get(&tid)
        .map(|c| c.ty.clone())
        .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
    let (params, ret) = member_sig(&t, name, kind)?;
    let key = format!("Dispatch{kind:?}:{tid}:{name}");
    let p2 = params.clone();
    let r2 = ret.clone();
    let n2 = name.to_string();
    let index = obj_fn(ctx, key, move |ctx| {
        build_dispatch(ctx, &t, &n2, kind, &p2, r2.as_ref())
    });
    Ok(MemberRef {
        index,
        params,
        ret,
        returns_this: false,
    })
}

fn build_dispatch(
    ctx: &mut Ctx,
    t: &TypeRef,
    name: &str,
    kind: Kind,
    params: &[Ty],
    ret: Option<&Ty>,
) -> R<Asm> {
    let this = val_of(ctx, t);
    let mut vals = vec![this];
    for p in params {
        vals.push(ctx.types.val(p).ok_or_else(|| {
            Error::compile(format!(
                "a parameter of type {p} is not supported in WASM builds yet"
            ))
        })?);
    }
    let results: Vec<ValType> = ret.and_then(|r| ctx.types.val(r)).into_iter().collect();
    let mut a = Asm::new(vals, results);
    let tag = a.local(ValType::I32);
    let object = ctx.object;
    a.ops([
        I::LocalGet(0),
        I::StructGet {
            struct_type_index: object,
            field_index: 0,
        },
        I::LocalSet(tag),
    ]);
    for c in concrete(ctx, t) {
        let Some((ctag, cty, cst)) = ctx.classes.get(&c).map(|i| (i.tag, i.ty.clone(), i.st()))
        else {
            continue;
        };
        // How objects of `c` do it: a method, a property, or a field.
        let field = ctx.classes.get(&c).and_then(|i| i.field(name));
        let target: Option<(MemberRef, u32)> = match kind {
            Kind::Method => match find_method(ctx, &cty, name) {
                Some((owner, decl)) => {
                    let st = ctx.classes.get(&owner).map(|i| i.st()).unwrap_or(cst);
                    Some((member_fn(ctx, owner, &decl, Kind::Method, &[])?, st))
                }
                None => None,
            },
            Kind::Getter | Kind::Setter if field.is_none() => {
                match find_accessor(ctx, &cty, name, kind == Kind::Setter) {
                    Some((owner, decl)) => {
                        let st = ctx.classes.get(&owner).map(|i| i.st()).unwrap_or(cst);
                        Some((member_fn(ctx, owner, &decl, kind, &[])?, st))
                    }
                    None => None,
                }
            }
            _ => None,
        };
        a.ops([
            I::LocalGet(tag),
            I::I32Const(ctag),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        match (target, field) {
            (Some((f, st)), _) => {
                a.ops([I::LocalGet(0), I::RefCastNullable(HeapType::Concrete(st))]);
                for i in 0..params.len() {
                    a.op(I::LocalGet(i as u32 + 1));
                }
                a.op(I::Call(f.index));
                if f.returns_this {
                    a.op(I::Drop);
                }
            }
            (None, Some((index, _))) => {
                a.ops([I::LocalGet(0), I::RefCastNullable(HeapType::Concrete(cst))]);
                if kind == Kind::Setter {
                    a.ops([
                        I::LocalGet(1),
                        I::StructSet {
                            struct_type_index: cst,
                            field_index: index,
                        },
                    ]);
                } else {
                    a.op(I::StructGet {
                        struct_type_index: cst,
                        field_index: index,
                    });
                }
            }
            (None, None) => a.op(I::Unreachable),
        }
        a.ops([I::Return, I::End]);
    }
    a.op(I::Unreachable);
    Ok(a)
}

// ---- function bodies ----------------------------------------------------

/// Generates a method, static method or property accessor.
pub fn gen_member(
    ctx: &mut Ctx,
    tid: usize,
    decl: Rc<FnDecl>,
    kind: Kind,
    subst: HashMap<String, Ty>,
    ret: Option<Ty>,
    returns_this: bool,
) -> R<Asm> {
    let (unit, ty) = ctx
        .classes
        .get(&tid)
        .map(|c| (c.unit, c.ty.clone()))
        .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
    let sub = |t: &Ty| t.substitute(&|n| subst.get(n).cloned());
    let params: Vec<Ty> = decl.params.iter().map(|p| sub(&p.resolved)).collect();
    let this_vt = val_of(ctx, &ty);
    let has_this = kind != Kind::Static;
    let mut vals = Vec::new();
    if has_this {
        vals.push(this_vt);
    }
    vals.extend(params.iter().filter_map(|t| ctx.types.val(t)));
    let mut results: Vec<ValType> = ret.iter().filter_map(|t| ctx.types.val(t)).collect();
    if returns_this {
        results.push(this_vt);
    }
    let mut g = FnGen::new(ctx, unit, subst.clone(), ret.clone(), vals, results);
    g.class = Some(tid);
    g.this = has_this.then_some(0);
    g.returns_this = returns_this;
    let offset = u32::from(has_this);
    for (i, (p, t)) in decl.params.iter().zip(params).enumerate() {
        g.declare(&p.name, i as u32 + offset, t);
    }
    g.block(&decl.body)?;
    if ret.is_some() {
        g.emit(I::Unreachable);
    } else if returns_this {
        g.emit(I::LocalGet(0));
    }
    Ok(g.a)
}

/// Whether a constructor starts with `: base(...)`.
fn calls_base(decl: &FnDecl) -> bool {
    matches!(decl.body.stmts.first().map(|s| &s.kind),
        Some(StmtKind::Expr(Expr { kind: ExprKind::Call { callee, .. }, .. }))
            if matches!(callee.kind, ExprKind::Base))
}

/// Generates `new` (allocates and initializes) or `init` (the
/// constructor's code) of type `tid`.
pub fn gen_ctor(ctx: &mut Ctx, tid: usize, new: bool) -> R<Asm> {
    let (unit, ty, decl, st, tag, fields) = ctx
        .classes
        .get(&tid)
        .map(|c| {
            (
                c.unit,
                c.ty.clone(),
                Rc::clone(&c.decl),
                c.st(),
                c.tag,
                c.fields.clone(),
            )
        })
        .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
    let params = ctor_params(ctx, tid);
    let this_vt = val_of(ctx, &ty);
    let pvals: Vec<ValType> = params.iter().filter_map(|t| ctx.types.val(t)).collect();
    let is_struct = ty.kind == TypeKind::Struct;
    if new {
        let mut g = FnGen::new(ctx, unit, HashMap::new(), None, pvals, vec![this_vt]);
        g.class = Some(tid);
        // Field values (or defaults), base classes' first, each evaluated
        // where its type is declared.
        g.emit(I::I32Const(tag));
        for c in ty.chain().iter().rev() {
            let Some((cunit, cdecl)) = g
                .ctx
                .classes
                .get(&c.id())
                .map(|i| (i.unit, Rc::clone(&i.decl)))
            else {
                continue;
            };
            for m in &cdecl.members {
                let MemberKind::Field(f) = &m.kind else {
                    continue;
                };
                if m.mods.is_static {
                    continue;
                }
                match &f.value {
                    Some(e) => {
                        g.unit = cunit;
                        g.class = Some(c.id());
                        g.value(e, &f.resolved)?;
                    }
                    None => g.default_value(&f.resolved, m.span)?,
                }
            }
        }
        g.unit = unit;
        g.class = Some(tid);
        g.emit(I::StructNew(st));
        let this = g.a.local(this_vt);
        g.emit(I::LocalSet(this));
        let init = ctor_fn(g.ctx, tid, false)?;
        g.emit(I::LocalGet(this));
        for i in 0..params.len() {
            g.emit(I::LocalGet(i as u32));
        }
        g.emit(I::Call(init.index));
        if init.returns_this {
            g.emit(I::LocalSet(this));
        }
        g.emit(I::LocalGet(this));
        return Ok(g.a);
    }
    let mut vals = vec![this_vt];
    vals.extend(pvals);
    let results = if is_struct { vec![this_vt] } else { Vec::new() };
    let mut g = FnGen::new(ctx, unit, HashMap::new(), None, vals, results);
    g.class = Some(tid);
    g.this = Some(0);
    g.returns_this = is_struct;
    let explicit = decl.members.iter().find_map(|m| match &m.kind {
        MemberKind::Init(d) => Some(Rc::clone(d)),
        _ => None,
    });
    let base = ty.base().map(|b| b.id());
    // Without `: base(...)`, the base class's constructor runs first,
    // without arguments.
    if let Some(b) = base {
        if !explicit.as_deref().is_some_and(calls_base) {
            let f = ctor_fn(g.ctx, b, false)?;
            g.emit(I::LocalGet(0));
            g.emit(I::Call(f.index));
        }
    }
    match explicit {
        Some(d) => {
            for (i, p) in d.params.iter().enumerate() {
                g.declare(&p.name, i as u32 + 1, p.resolved.clone());
            }
            g.block(&d.body)?;
        }
        None => {
            // The arguments go to the fields without a value, in order.
            let own: Vec<String> = decl
                .members
                .iter()
                .filter_map(|m| match &m.kind {
                    MemberKind::Field(f) if !m.mods.is_static && f.value.is_none() => {
                        Some(f.name.clone())
                    }
                    _ => None,
                })
                .collect();
            for (k, name) in own.iter().enumerate() {
                let Some(index) = fields.iter().position(|(n, _)| n == name) else {
                    continue;
                };
                let index = index as u32 + 1;
                let arg = k as u32 + 1;
                if is_struct {
                    g.struct_with(tid, 0, index, arg);
                    g.emit(I::LocalSet(0));
                } else {
                    g.a.ops([
                        I::LocalGet(0),
                        I::LocalGet(arg),
                        I::StructSet {
                            struct_type_index: st,
                            field_index: index,
                        },
                    ]);
                }
            }
        }
    }
    if is_struct {
        g.emit(I::LocalGet(0));
    }
    Ok(g.a)
}

impl FnGen<'_> {
    /// Leaves a copy of the struct in local `base` (of type `tid`) with
    /// field `index` replaced by local `value` on the stack.
    pub(super) fn struct_with(&mut self, tid: usize, base: u32, index: u32, value: u32) {
        let Some((st, n)) = self
            .ctx
            .classes
            .get(&tid)
            .map(|c| (c.st(), c.fields.len() as u32))
        else {
            return;
        };
        for i in 0..=n {
            if i == index {
                self.emit(I::LocalGet(value));
            } else {
                self.a.ops([
                    I::LocalGet(base),
                    I::StructGet {
                        struct_type_index: st,
                        field_index: i,
                    },
                ]);
            }
        }
        self.emit(I::StructNew(st));
    }

    /// The value a field of type `ty` has before it is set.
    pub(super) fn default_value(&mut self, ty: &Ty, span: Span) -> R<()> {
        let ty = self.t(ty);
        match ty.canonical() {
            Ty::Named(t) => {
                let Some((kind, st, data)) = self
                    .ctx
                    .classes
                    .get(&t.id())
                    .map(|c| (c.ty.kind, c.st, !c.cases.is_empty()))
                else {
                    return Err(unsupported(format!("values of {t:?} are"), span));
                };
                match (kind, st) {
                    (TypeKind::Enum, None) => self.emit(I::I32Const(0)),
                    (TypeKind::Struct, _) => {
                        let f = default_fn(self.ctx, t.id());
                        self.emit(I::Call(f));
                    }
                    (TypeKind::Enum, Some(_)) if data => {
                        let f = default_fn(self.ctx, t.id());
                        self.emit(I::Call(f));
                    }
                    (_, Some(st)) => self.emit(I::RefNull(HeapType::Concrete(st))),
                    _ => self.emit(I::I32Const(0)),
                }
            }
            other => {
                let vt = self.vt(&other, span)?;
                self.emit(match vt {
                    ValType::I64 => I::I64Const(0),
                    ValType::F32 => I::F32Const(0.0.into()),
                    ValType::F64 => I::F64Const(0.0.into()),
                    ValType::Ref(r) => I::RefNull(r.heap_type),
                    _ => I::I32Const(0),
                });
            }
        }
        Ok(())
    }
}

/// A function creating the default value of a struct or an enum with
/// data (fields with their default values; the first case).
fn default_fn(ctx: &mut Ctx, tid: usize) -> u32 {
    obj_fn(ctx, format!("Default:{tid}"), move |ctx| {
        let (ty, st, tag, fields, cases) = ctx
            .classes
            .get(&tid)
            .map(|c| {
                (
                    c.ty.clone(),
                    c.st(),
                    c.tag,
                    c.fields.clone(),
                    c.cases.clone(),
                )
            })
            .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
        let vt = val_of(ctx, &ty);
        let mut a = Asm::new(Vec::new(), vec![vt]);
        a.op(I::I32Const(tag));
        let (tys, target) = match cases.first() {
            Some(&case) => {
                a.op(I::I32Const(0));
                let tys: Vec<Ty> = ty
                    .variants()
                    .first()
                    .map(|v| v.fields.iter().map(|(_, t)| t.clone()).collect())
                    .unwrap_or_default();
                (tys, case)
            }
            None => (fields.into_iter().map(|(_, t)| t).collect(), st),
        };
        for t in tys {
            default_into(ctx, &mut a, &t, tid)?;
        }
        a.op(I::StructNew(target));
        Ok(a)
    })
}

/// Pushes the default value of `t` (inside a default function of type
/// `own`, which it does not call again).
fn default_into(ctx: &mut Ctx, a: &mut Asm, t: &Ty, own: usize) -> R<()> {
    match t.canonical() {
        Ty::Named(n) => {
            let info = ctx
                .classes
                .get(&n.id())
                .map(|c| (c.ty.kind, c.st, !c.cases.is_empty()));
            match info {
                Some((TypeKind::Enum, None, _)) => a.op(I::I32Const(0)),
                Some((TypeKind::Struct, _, _)) | Some((TypeKind::Enum, _, true))
                    if n.id() != own =>
                {
                    a.op(I::Call(default_fn(ctx, n.id())));
                }
                Some((_, Some(st), _)) => a.op(I::RefNull(HeapType::Concrete(st))),
                _ => a.op(I::I32Const(0)),
            }
        }
        other => {
            let vt = ctx.types.val(&other).ok_or_else(|| {
                Error::compile(format!(
                    "a field of type {other} is not supported in WASM builds yet"
                ))
            })?;
            a.op(match vt {
                ValType::I64 => I::I64Const(0),
                ValType::F32 => I::F32Const(0.0.into()),
                ValType::F64 => I::F64Const(0.0.into()),
                ValType::Ref(r) => I::RefNull(r.heap_type),
                _ => I::I32Const(0),
            });
        }
    }
    Ok(())
}

// ---- type tests, equality, text ------------------------------------------

/// A function telling whether an object (`ref null $object`) is a `t`.
pub fn is_fn(ctx: &mut Ctx, t: &TypeRef) -> u32 {
    let t = t.clone();
    obj_fn(ctx, format!("Is:{}", t.id()), move |ctx| {
        let object = ctx.object;
        let mut a = Asm::new(vec![nullable(object)], vec![ValType::I32]);
        a.ops([
            I::LocalGet(0),
            I::RefIsNull,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
        let tag = a.local(ValType::I32);
        a.ops([
            I::LocalGet(0),
            I::StructGet {
                struct_type_index: object,
                field_index: 0,
            },
            I::LocalSet(tag),
        ]);
        for c in concrete(ctx, &t) {
            let ctag = ctx.classes.get(&c).map(|i| i.tag).unwrap_or(-1);
            a.ops([
                I::LocalGet(tag),
                I::I32Const(ctag),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::I32Const(1),
                I::Return,
                I::End,
            ]);
        }
        a.op(I::I32Const(0));
        Ok(a)
    })
}

/// Instructions comparing two values of type `ty` on the stack (`i32`
/// result), as `==` does.
pub fn eq_instrs(ctx: &mut Ctx, ty: &Ty) -> R<Vec<I<'static>>> {
    Ok(match ty.canonical() {
        Ty::String => vec![I::Call(ctx.helper(Helper::StrEq))],
        Ty::Array(inner) => vec![I::Call(ctx.helper(Helper::ArrEq((*inner).clone())))],
        Ty::Float(FloatKind::F32) => vec![I::F32Eq],
        Ty::Float(FloatKind::F64) => vec![I::F64Eq],
        Ty::Int(k) if k.bits() == 64 => vec![I::I64Eq],
        Ty::Map(k, v) => vec![I::Call(super::maps::eq_fn(ctx, &k, &v)?)],
        Ty::KeyValue(k, v) => vec![I::Call(super::maps::pair_eq_fn(ctx, &k, &v)?)],
        Ty::Named(t) => {
            let info = ctx
                .classes
                .get(&t.id())
                .map(|c| (c.ty.kind, c.st.is_some()));
            match info {
                Some((TypeKind::Enum, false)) => vec![I::I32Eq],
                Some((TypeKind::Struct, _)) | Some((TypeKind::Enum, true)) => {
                    vec![I::Call(eq_fn(ctx, t.id()))]
                }
                _ => vec![I::RefEq],
            }
        }
        _ => vec![I::I32Eq],
    })
}

/// Field-by-field equality of two structs, or of two enum values with
/// data.
fn eq_fn(ctx: &mut Ctx, tid: usize) -> u32 {
    obj_fn(ctx, format!("Eq:{tid}"), move |ctx| {
        let (ty, st, fields, cases) = ctx
            .classes
            .get(&tid)
            .map(|c| (c.ty.clone(), c.st(), c.fields.clone(), c.cases.clone()))
            .ok_or_else(|| Error::compile("internal compiler error: unknown type"))?;
        let vt = val_of(ctx, &ty);
        let mut a = Asm::new(vec![vt, vt], vec![ValType::I32]);
        let fail = [
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ];
        if cases.is_empty() {
            for (i, (_, fty)) in fields.iter().enumerate() {
                let get = I::StructGet {
                    struct_type_index: st,
                    field_index: i as u32 + 1,
                };
                a.ops([I::LocalGet(0), get.clone(), I::LocalGet(1), get]);
                let eq = eq_instrs(ctx, fty)?;
                a.ops(eq);
                a.ops(fail.clone());
            }
            a.op(I::I32Const(1));
            return Ok(a);
        }
        let case = I::StructGet {
            struct_type_index: st,
            field_index: 1,
        };
        a.ops([
            I::LocalGet(0),
            case.clone(),
            I::LocalGet(1),
            case.clone(),
            I::I32Ne,
        ]);
        a.ops([I::If(BlockType::Empty), I::I32Const(0), I::Return, I::End]);
        for (k, (cst, v)) in cases.iter().zip(ty.variants()).enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            a.ops([
                I::LocalGet(0),
                case.clone(),
                I::I32Const(k as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            for (i, (_, fty)) in v.fields.iter().enumerate() {
                let get = I::StructGet {
                    struct_type_index: *cst,
                    field_index: i as u32 + 2,
                };
                let cast = I::RefCastNullable(HeapType::Concrete(*cst));
                a.ops([
                    I::LocalGet(0),
                    cast.clone(),
                    get.clone(),
                    I::LocalGet(1),
                    cast,
                    get,
                ]);
                let eq = eq_instrs(ctx, fty)?;
                a.ops(eq);
                a.ops(fail.clone());
            }
            a.op(I::End);
        }
        a.op(I::I32Const(1));
        Ok(a)
    })
}

/// A function `(value, depth) -> string` formatting values of `ty` as
/// the interpreter prints them.
pub fn show_fn(ctx: &mut Ctx, ty: &Ty) -> R<u32> {
    let ty = ty.canonical();
    let key = format!("Show:{ty:?}");
    let t2 = ty.clone();
    Ok(obj_fn(ctx, key, move |ctx| build_show(ctx, &t2)))
}

fn lit(ctx: &mut Ctx, a: &mut Asm, s: &str) {
    let ops = ctx.string_lit(s);
    a.ops(ops);
}

/// Pushes local `v` (of type `ty`) as text; `depth` is the nesting so far.
fn show_value(ctx: &mut Ctx, a: &mut Asm, ty: &Ty, v: u32, depth: u32) -> R<()> {
    let s = ctx.types.string();
    match ty.canonical() {
        Ty::String => {
            a.op(I::LocalGet(v));
            a.op(I::Call(ctx.helper(Helper::StrOrNull)));
        }
        Ty::Int(k) => {
            a.op(I::LocalGet(v));
            if k.bits() != 64 {
                a.op(if k.signed() {
                    I::I64ExtendI32S
                } else {
                    I::I64ExtendI32U
                });
            }
            a.op(I::Call(ctx.helper(Helper::IntToStr {
                unsigned: k == IntKind::U64,
            })));
        }
        Ty::Bool => {
            a.ops([I::LocalGet(v), I::If(BlockType::Result(nullable(s)))]);
            lit(ctx, a, "true");
            a.op(I::Else);
            lit(ctx, a, "false");
            a.op(I::End);
        }
        Ty::Char => a.ops([
            I::LocalGet(v),
            I::ArrayNewFixed {
                array_type_index: s,
                array_size: 1,
            },
        ]),
        Ty::Float(k) => {
            a.op(I::LocalGet(v));
            if k == FloatKind::F32 {
                a.op(I::F64PromoteF32);
            }
            let f = super::format_fn(ctx)?;
            a.ops([I::I32Const(i32::from(k == FloatKind::F32)), I::Call(f)]);
        }
        other => {
            let f = show_fn(ctx, &other)?;
            a.ops([
                I::LocalGet(v),
                I::LocalGet(depth),
                I::I32Const(1),
                I::I32Add,
                I::Call(f),
            ]);
        }
    }
    Ok(())
}

fn build_show(ctx: &mut Ctx, ty: &Ty) -> R<Asm> {
    let s = ctx.types.string();
    let sref = nullable(s);
    let concat = ctx.helper(Helper::Concat);
    let vt = ctx.types.val(ty).ok_or_else(|| {
        Error::compile(format!("printing {ty} is not supported in WASM builds yet"))
    })?;
    let mut a = Asm::new(vec![vt, ValType::I32], vec![sref]);
    let out = a.local(sref);
    let ret_lit = |ctx: &mut Ctx, a: &mut Asm, text: &str| {
        lit(ctx, a, text);
        a.op(I::Return);
    };
    match ty {
        Ty::Array(elem) => {
            let at = ctx.types.array(elem).ok_or_else(|| {
                Error::compile(format!(
                    "arrays of {elem} are not supported in WASM builds yet"
                ))
            })?;
            a.ops([I::LocalGet(0), I::RefIsNull, I::If(BlockType::Empty)]);
            ret_lit(ctx, &mut a, "null");
            a.ops([
                I::End,
                I::LocalGet(1),
                I::I32Const(64),
                I::I32GeS,
                I::If(BlockType::Empty),
            ]);
            ret_lit(ctx, &mut a, "[...]");
            a.op(I::End);
            let (i, n, x) = (
                a.local(ValType::I32),
                a.local(ValType::I32),
                a.local(at.elem),
            );
            lit(ctx, &mut a, "[");
            a.ops([
                I::LocalSet(out),
                I::LocalGet(0),
                I::StructGet {
                    struct_type_index: at.obj,
                    field_index: 1,
                },
                I::LocalSet(n),
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(i),
                I::If(BlockType::Empty),
                I::LocalGet(out),
            ]);
            lit(ctx, &mut a, ", ");
            a.ops([
                I::Call(concat),
                I::LocalSet(out),
                I::End,
                I::LocalGet(0),
                I::StructGet {
                    struct_type_index: at.obj,
                    field_index: 0,
                },
                I::LocalGet(i),
                I::ArrayGet(at.raw),
                I::LocalSet(x),
                I::LocalGet(out),
            ]);
            show_value(ctx, &mut a, elem, x, 1)?;
            a.ops([
                I::Call(concat),
                I::LocalSet(out),
                I::LocalGet(i),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(i),
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(out),
            ]);
            lit(ctx, &mut a, "]");
            a.op(I::Call(concat));
        }
        Ty::Map(k, v) => {
            let m = ctx.types.map(k, v).ok_or_else(|| {
                Error::compile(format!("printing {ty} is not supported in WASM builds yet"))
            })?;
            a.ops([I::LocalGet(0), I::RefIsNull, I::If(BlockType::Empty)]);
            ret_lit(ctx, &mut a, "null");
            a.ops([
                I::End,
                I::LocalGet(1),
                I::I32Const(64),
                I::I32GeS,
                I::If(BlockType::Empty),
            ]);
            ret_lit(ctx, &mut a, "{...}");
            a.op(I::End);
            let (i, n, kv, vv) = (
                a.local(ValType::I32),
                a.local(ValType::I32),
                a.local(m.keys.elem),
                a.local(m.values.elem),
            );
            let get = |a: &mut Asm, f: u32, arr: ArrayType, into: u32| {
                a.ops([
                    I::LocalGet(0),
                    I::StructGet {
                        struct_type_index: m.st,
                        field_index: f,
                    },
                    I::StructGet {
                        struct_type_index: arr.obj,
                        field_index: 0,
                    },
                    I::LocalGet(i),
                    I::ArrayGet(arr.raw),
                    I::LocalSet(into),
                ]);
            };
            lit(ctx, &mut a, "{");
            a.ops([
                I::LocalSet(out),
                I::LocalGet(0),
                I::StructGet {
                    struct_type_index: m.st,
                    field_index: 0,
                },
                I::StructGet {
                    struct_type_index: m.keys.obj,
                    field_index: 1,
                },
                I::LocalSet(n),
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(n),
                I::I32GeU,
                I::BrIf(1),
                I::LocalGet(i),
                I::If(BlockType::Empty),
                I::LocalGet(out),
            ]);
            lit(ctx, &mut a, ", ");
            a.ops([I::Call(concat), I::LocalSet(out), I::End]);
            get(&mut a, 0, m.keys, kv);
            get(&mut a, 1, m.values, vv);
            a.op(I::LocalGet(out));
            show_value(ctx, &mut a, k, kv, 1)?;
            a.op(I::Call(concat));
            lit(ctx, &mut a, ": ");
            a.op(I::Call(concat));
            show_value(ctx, &mut a, v, vv, 1)?;
            a.ops([
                I::Call(concat),
                I::LocalSet(out),
                I::LocalGet(i),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(i),
                I::Br(0),
                I::End,
                I::End,
                I::LocalGet(out),
            ]);
            lit(ctx, &mut a, "}");
            a.op(I::Call(concat));
        }
        Ty::KeyValue(k, v) => {
            let st = ctx.types.pair(k, v).ok_or_else(|| {
                Error::compile(format!("printing {ty} is not supported in WASM builds yet"))
            })?;
            let (kv, vv) = (
                a.local(ctx.types.val(k).unwrap_or(ValType::I32)),
                a.local(ctx.types.val(v).unwrap_or(ValType::I32)),
            );
            a.ops([
                I::LocalGet(0),
                I::StructGet {
                    struct_type_index: st,
                    field_index: 0,
                },
                I::LocalSet(kv),
                I::LocalGet(0),
                I::StructGet {
                    struct_type_index: st,
                    field_index: 1,
                },
                I::LocalSet(vv),
            ]);
            lit(ctx, &mut a, "[");
            show_value(ctx, &mut a, k, kv, 1)?;
            a.op(I::Call(concat));
            lit(ctx, &mut a, ", ");
            a.op(I::Call(concat));
            show_value(ctx, &mut a, v, vv, 1)?;
            a.op(I::Call(concat));
            lit(ctx, &mut a, "]");
            a.op(I::Call(concat));
        }
        Ty::Named(t) => {
            let info = ctx
                .classes
                .get(&t.id())
                .map(|c| (c.ty.kind, c.st, c.cases.clone(), c.fields.clone()));
            let Some((kind, st, cases, fields)) = info else {
                return Err(Error::compile("internal compiler error: unknown type"));
            };
            let variants = t.variants();
            match (kind, st) {
                (TypeKind::Enum, None) => {
                    for (k, v) in variants.iter().enumerate() {
                        a.ops([
                            I::LocalGet(0),
                            I::I32Const(k as i32),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                        ]);
                        ret_lit(ctx, &mut a, &v.name);
                        a.op(I::End);
                    }
                    lit(ctx, &mut a, "?");
                }
                (TypeKind::Enum, Some(base)) => {
                    let case = I::StructGet {
                        struct_type_index: base,
                        field_index: 1,
                    };
                    for (k, (cst, v)) in cases.iter().zip(&variants).enumerate() {
                        a.ops([
                            I::LocalGet(0),
                            case.clone(),
                            I::I32Const(k as i32),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                        ]);
                        if v.fields.is_empty() {
                            ret_lit(ctx, &mut a, &v.name);
                        } else {
                            lit(ctx, &mut a, &format!("{}(", v.name));
                            a.op(I::LocalSet(out));
                            for (i, (_, fty)) in v.fields.iter().enumerate() {
                                let fv = {
                                    let vt = ctx.types.val(fty).unwrap_or(ValType::I32);
                                    a.local(vt)
                                };
                                a.ops([
                                    I::LocalGet(0),
                                    I::RefCastNullable(HeapType::Concrete(*cst)),
                                    I::StructGet {
                                        struct_type_index: *cst,
                                        field_index: i as u32 + 2,
                                    },
                                    I::LocalSet(fv),
                                    I::LocalGet(out),
                                ]);
                                if i > 0 {
                                    lit(ctx, &mut a, ", ");
                                    a.op(I::Call(concat));
                                }
                                show_value(ctx, &mut a, fty, fv, 1)?;
                                a.ops([I::Call(concat), I::LocalSet(out)]);
                            }
                            a.op(I::LocalGet(out));
                            lit(ctx, &mut a, ")");
                            a.ops([I::Call(concat), I::Return]);
                        }
                        a.op(I::End);
                    }
                    lit(ctx, &mut a, "?");
                }
                (TypeKind::Struct, Some(st)) => {
                    show_fields(ctx, &mut a, &t.name, st, &fields, 64, out)?;
                }
                (_, Some(_)) => {
                    // A class or interface: the fields of the object's real
                    // class.
                    a.ops([I::LocalGet(0), I::RefIsNull, I::If(BlockType::Empty)]);
                    ret_lit(ctx, &mut a, "null");
                    a.op(I::End);
                    let tag = a.local(ValType::I32);
                    let object = ctx.object;
                    a.ops([
                        I::LocalGet(0),
                        I::StructGet {
                            struct_type_index: object,
                            field_index: 0,
                        },
                        I::LocalSet(tag),
                    ]);
                    for c in concrete(ctx, t) {
                        let Some((ctag, cty, cst)) =
                            ctx.classes.get(&c).map(|i| (i.tag, i.ty.clone(), i.st()))
                        else {
                            continue;
                        };
                        let f = show_fn(ctx, &Ty::Named(cty.clone()))?;
                        let exact = if cty == *t || cty.kind != TypeKind::Class {
                            None
                        } else {
                            Some(f)
                        };
                        a.ops([
                            I::LocalGet(tag),
                            I::I32Const(ctag),
                            I::I32Eq,
                            I::If(BlockType::Empty),
                        ]);
                        match exact {
                            Some(f) => a.ops([
                                I::LocalGet(0),
                                I::RefCastNullable(HeapType::Concrete(cst)),
                                I::LocalGet(1),
                                I::Call(f),
                                I::Return,
                            ]),
                            None => {
                                let fields = ctx
                                    .classes
                                    .get(&c)
                                    .map(|i| i.fields.clone())
                                    .unwrap_or_default();
                                let limit = if cty.kind == TypeKind::Class { 8 } else { 64 };
                                let local = a.local(nullable(cst));
                                a.ops([
                                    I::LocalGet(0),
                                    I::RefCastNullable(HeapType::Concrete(cst)),
                                    I::LocalSet(local),
                                ]);
                                show_fields_of(
                                    ctx, &mut a, &cty.name, cst, &fields, limit, out, local,
                                )?;
                                a.op(I::Return);
                            }
                        }
                        a.op(I::End);
                    }
                    lit(ctx, &mut a, "?");
                }
                _ => lit(ctx, &mut a, "?"),
            }
        }
        other => {
            return Err(Error::compile(format!(
                "printing {other} is not supported in WASM builds yet"
            )))
        }
    }
    Ok(a)
}

/// `Name { a: 1, b: 2 }` for the struct in local 0.
fn show_fields(
    ctx: &mut Ctx,
    a: &mut Asm,
    name: &str,
    st: u32,
    fields: &[(String, Ty)],
    limit: i32,
    out: u32,
) -> R<()> {
    show_fields_of(ctx, a, name, st, fields, limit, out, 0)
}

#[allow(clippy::too_many_arguments)]
fn show_fields_of(
    ctx: &mut Ctx,
    a: &mut Asm,
    name: &str,
    st: u32,
    fields: &[(String, Ty)],
    limit: i32,
    out: u32,
    value: u32,
) -> R<()> {
    let concat = ctx.helper(Helper::Concat);
    a.ops([
        I::LocalGet(1),
        I::I32Const(limit),
        I::I32GeS,
        I::If(BlockType::Empty),
    ]);
    lit(ctx, a, &format!("{name} {{...}}"));
    a.ops([I::Return, I::End]);
    lit(ctx, a, &format!("{name} {{"));
    a.op(I::LocalSet(out));
    for (i, (fname, fty)) in fields.iter().enumerate() {
        let vt = ctx.types.val(fty).unwrap_or(ValType::I32);
        let fv = a.local(vt);
        a.ops([
            I::LocalGet(value),
            I::StructGet {
                struct_type_index: st,
                field_index: i as u32 + 1,
            },
            I::LocalSet(fv),
            I::LocalGet(out),
        ]);
        lit(
            ctx,
            a,
            &format!("{}{fname}: ", if i == 0 { " " } else { ", " }),
        );
        a.op(I::Call(concat));
        show_value(ctx, a, fty, fv, 1)?;
        a.ops([I::Call(concat), I::LocalSet(out)]);
    }
    a.op(I::LocalGet(out));
    lit(ctx, a, if fields.is_empty() { "}" } else { " }" });
    a.op(I::Call(concat));
    Ok(())
}

/// A function turning the case index of an enum (without data) into the
/// number it stands for (`int(Mode.High)`), as an `i64`.
pub fn enum_value_fn(ctx: &mut Ctx, t: &TypeRef) -> u32 {
    let t = t.clone();
    obj_fn(ctx, format!("EnumValue:{}", t.id()), move |_| {
        let mut a = Asm::new(vec![ValType::I32], vec![ValType::I64]);
        for (k, v) in t.variants().iter().enumerate() {
            a.ops([
                I::LocalGet(0),
                I::I32Const(k as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::I64Const(v.value as i64),
                I::Return,
                I::End,
            ]);
        }
        a.op(I::I64Const(0));
        Ok(a)
    })
}
