//! Members of user-defined types in function bodies: fields, properties,
//! method calls, constructors, `is` and `as`.

use belsk2_syntax::ast::{Expr, ExprKind};
use belsk2_syntax::{Error, Span, Ty, TypeKind, TypeRef};
use wasm_encoder::{BlockType, Instruction as I, ValType};

use super::objects::{self, Kind, MemberRef};
use super::{unsupported, FnGen, Place, R};

impl FnGen<'_> {
    pub(super) fn this_get(&mut self, span: Span) -> R<()> {
        match self.this {
            Some(i) => {
                self.emit(I::LocalGet(i));
                Ok(())
            }
            None => Err(Error::compile("'this' outside of a method").at(span)),
        }
    }

    /// The base class of the type whose method is running.
    fn current_base(&self, span: Span) -> R<TypeRef> {
        self.class
            .and_then(|c| self.ctx.classes.get(&c))
            .and_then(|c| c.ty.base())
            .ok_or_else(|| Error::compile("'base' needs a class with a base class").at(span))
    }

    /// The struct type of a user-defined type.
    fn st_of(&self, t: &TypeRef) -> u32 {
        self.ctx
            .classes
            .get(&t.id())
            .and_then(|c| c.st)
            .unwrap_or(0)
    }

    /// `object.name` as a value.
    pub(super) fn member_get(&mut self, object: &Expr, name: &str, span: Span) -> R<()> {
        match self.t(&object.ty) {
            Ty::KeyValue(k, v) => {
                let st = self
                    .ctx
                    .types
                    .pair(&k, &v)
                    .ok_or_else(|| unsupported(format!("KeyValuePair<{k}, {v}> is"), span))?;
                self.expr(object)?;
                self.emit(I::StructGet {
                    struct_type_index: st,
                    field_index: u32::from(name == "Value"),
                });
                Ok(())
            }
            Ty::Type(t) => {
                let info = self
                    .ctx
                    .classes
                    .get(&t.id())
                    .map(|c| (c.tag, c.cases.clone(), c.statics.get(name).cloned()));
                let Some((tag, cases, global)) = info else {
                    return Err(unsupported(format!("'{name}' of {} is", t.name), span));
                };
                if let Some(k) = t.variants().iter().position(|v| v.name == name) {
                    match cases.get(k) {
                        Some(&cst) => {
                            self.a
                                .ops([I::I32Const(tag), I::I32Const(k as i32), I::StructNew(cst)])
                        }
                        None => self.emit(I::I32Const(k as i32)),
                    }
                    return Ok(());
                }
                if let Some((g, _)) = global {
                    self.emit(I::GlobalGet(g));
                    return Ok(());
                }
                let Some((owner, decl)) = objects::find_accessor(self.ctx, &t, name, false) else {
                    return Err(unsupported(format!("'{name}' of {} is", t.name), span));
                };
                let f = objects::member_fn(self.ctx, owner, &decl, Kind::Static, &[])?;
                self.emit(I::Call(f.index));
                Ok(())
            }
            Ty::Named(t) => {
                let is_base = matches!(object.kind, ExprKind::Base);
                let field = if t.kind == TypeKind::Interface {
                    None
                } else {
                    self.ctx.classes.get(&t.id()).and_then(|c| c.field(name))
                };
                if let Some((index, _)) = field {
                    self.expr(object)?;
                    let st = self.st_of(&t);
                    self.emit(I::StructGet {
                        struct_type_index: st,
                        field_index: index,
                    });
                    return Ok(());
                }
                let f = self.accessor(&t, name, false, is_base, span)?;
                self.expr(object)?;
                self.emit(I::Call(f.index));
                Ok(())
            }
            other => Err(unsupported(format!("members of {other} are"), span)),
        }
    }

    /// The function reading (or setting) property `name` of a value of
    /// type `t`.
    fn accessor(
        &mut self,
        t: &TypeRef,
        name: &str,
        set: bool,
        is_base: bool,
        span: Span,
    ) -> R<MemberRef> {
        let kind = if set { Kind::Setter } else { Kind::Getter };
        if !is_base && objects::is_virtual(t, name) {
            return objects::dispatch_fn(self.ctx, t.id(), name, kind).map_err(|e| e.or_at(span));
        }
        let start = if is_base {
            self.current_base(span)?
        } else {
            t.clone()
        };
        match objects::find_accessor(self.ctx, &start, name, set) {
            Some((owner, decl)) => objects::member_fn(self.ctx, owner, &decl, kind, &[]),
            None => Err(unsupported(format!("'{name}' of {} is", t.name), span)),
        }
    }

    /// `object.name` as an assignment target.
    pub(super) fn member_place(&mut self, object: &Expr, name: &str, span: Span) -> R<(Place, Ty)> {
        match self.t(&object.ty) {
            Ty::Type(t) => {
                let global = self
                    .ctx
                    .classes
                    .get(&t.id())
                    .and_then(|c| c.statics.get(name).cloned());
                if let Some((g, ty)) = global {
                    return Ok((Place::Global(g), ty));
                }
                let get = objects::find_accessor(self.ctx, &t, name, false);
                let set = objects::find_accessor(self.ctx, &t, name, true);
                let (Some((go, gd)), Some((so, sd))) = (get, set) else {
                    return Err(unsupported(
                        format!("assigning '{name}' of {} is", t.name),
                        span,
                    ));
                };
                let get = objects::member_fn(self.ctx, go, &gd, Kind::Static, &[])?;
                let set = objects::member_fn(self.ctx, so, &sd, Kind::Static, &[])?;
                let ty = get.ret.clone().unwrap_or(Ty::Any);
                Ok((
                    Place::Prop {
                        get,
                        set,
                        recv: None,
                        inner: None,
                    },
                    ty,
                ))
            }
            Ty::Named(t) => {
                let is_struct = t.kind == TypeKind::Struct;
                let field = if t.kind == TypeKind::Interface {
                    None
                } else {
                    self.ctx.classes.get(&t.id()).and_then(|c| c.field(name))
                };
                // A struct is changed where it is stored.
                let inner = if is_struct {
                    if !object.is_place() {
                        return Err(Error::compile(
                            "cannot change a field of a temporary struct value",
                        )
                        .at(span));
                    }
                    Some(Box::new(self.place(object)?.0))
                } else {
                    None
                };
                let st = self.st_of(&t);
                if let Some((index, ty)) = field {
                    return Ok(match inner {
                        Some(inner) => (
                            Place::StructField {
                                inner,
                                tid: t.id(),
                                index,
                                ty: ty.clone(),
                            },
                            ty,
                        ),
                        None => {
                            let vt = self.vt(&Ty::Named(t.clone()), span)?;
                            let obj = self.a.local(vt);
                            self.expr(object)?;
                            self.emit(I::LocalSet(obj));
                            (Place::Field { obj, st, index }, ty)
                        }
                    });
                }
                let is_base = matches!(object.kind, ExprKind::Base);
                let get = self.accessor(&t, name, false, is_base, span)?;
                let set = self.accessor(&t, name, true, is_base, span)?;
                let ty = get.ret.clone().unwrap_or(Ty::Any);
                let vt = self.vt(&Ty::Named(t.clone()), span)?;
                let recv = self.a.local(vt);
                match &inner {
                    Some(p) => self.place_get(p),
                    None => {
                        self.expr(object)?;
                    }
                }
                self.emit(I::LocalSet(recv));
                Ok((
                    Place::Prop {
                        get,
                        set,
                        recv: Some(recv),
                        inner,
                    },
                    ty,
                ))
            }
            other => Err(unsupported(
                format!("assigning members of {other} is"),
                span,
            )),
        }
    }

    /// `object.name(args)`: a method, a static method, or a case of an
    /// enum with data.
    pub(super) fn method_call(
        &mut self,
        object: &Expr,
        name: &str,
        args: &[Expr],
        inst: &[Ty],
        span: Span,
    ) -> R<Option<ValType>> {
        let inst: Vec<Ty> = inst.iter().map(|t| self.t(t)).collect();
        match self.t(&object.ty) {
            Ty::Type(t) => {
                if let Some(k) = t.variants().iter().position(|v| v.name == name) {
                    let info = self
                        .ctx
                        .classes
                        .get(&t.id())
                        .map(|c| (c.tag, c.cases.get(k).copied()));
                    let Some((tag, Some(cst))) = info else {
                        return Err(unsupported(format!("case {name} is"), span));
                    };
                    let fields = t
                        .variants()
                        .get(k)
                        .map(|v| v.fields.clone())
                        .unwrap_or_default();
                    self.emit(I::I32Const(tag));
                    self.emit(I::I32Const(k as i32));
                    for (arg, (_, fty)) in args.iter().zip(&fields) {
                        self.value(arg, fty)?;
                    }
                    self.emit(I::StructNew(cst));
                    return self.vt(&Ty::Named(t), span).map(Some);
                }
                let Some((owner, decl)) = objects::find_method(self.ctx, &t, name) else {
                    return Err(unsupported(format!("'{name}' of {} is", t.name), span));
                };
                let f = objects::member_fn(self.ctx, owner, &decl, Kind::Static, &inst)?;
                self.call_member(&f, None, args)
            }
            Ty::Named(t) => {
                let is_base = matches!(object.kind, ExprKind::Base);
                let f = if !is_base && objects::is_virtual(&t, name) {
                    objects::dispatch_fn(self.ctx, t.id(), name, Kind::Method)
                        .map_err(|e| e.or_at(span))?
                } else {
                    let start = if is_base {
                        self.current_base(span)?
                    } else {
                        t.clone()
                    };
                    let Some((owner, decl)) = objects::find_method(self.ctx, &start, name) else {
                        return Err(unsupported(format!("'{name}' of {} is", t.name), span));
                    };
                    objects::member_fn(self.ctx, owner, &decl, Kind::Method, &inst)?
                };
                // A method of a struct gives back `this`, which goes where
                // the struct came from.
                let vt = self.vt(&Ty::Named(t.clone()), span)?;
                let place = if f.returns_this && object.is_place() {
                    let (p, _) = self.place(object)?;
                    self.place_get(&p);
                    Some((p, vt))
                } else {
                    self.expr(object)?;
                    None
                };
                self.call_member(&f, place, args)
            }
            other => Err(unsupported(format!("methods of {other} are"), span)),
        }
    }

    /// Calls `f` (the receiver, if any, is on the stack). `receiver` is
    /// where to store the `this` a struct method returns (and its type).
    fn call_member(
        &mut self,
        f: &MemberRef,
        receiver: Option<(Place, ValType)>,
        args: &[Expr],
    ) -> R<Option<ValType>> {
        for (arg, param) in args.iter().zip(&f.params) {
            self.value(arg, param)?;
        }
        self.emit(I::Call(f.index));
        let ret = match &f.ret {
            Some(t) => self.ctx.types.val(t),
            None => None,
        };
        if f.returns_this {
            match receiver {
                Some((place, vt)) => {
                    let tmp = self.a.local(vt);
                    self.emit(I::LocalSet(tmp));
                    self.place_begin(&place);
                    self.emit(I::LocalGet(tmp));
                    self.place_set(&place);
                }
                None => self.emit(I::Drop),
            }
        }
        Ok(ret)
    }

    /// `Point(args)`: creates a value.
    pub(super) fn ctor_call(
        &mut self,
        t: &TypeRef,
        args: &[Expr],
        span: Span,
    ) -> R<Option<ValType>> {
        let f = objects::ctor_fn(self.ctx, t.id(), true).map_err(|e| e.or_at(span))?;
        for (arg, param) in args.iter().zip(&f.params) {
            self.value(arg, param)?;
        }
        self.emit(I::Call(f.index));
        self.vt(&Ty::Named(t.clone()), span).map(Some)
    }

    /// `: base(args)` of a constructor.
    pub(super) fn base_ctor_call(&mut self, args: &[Expr], span: Span) -> R<Option<ValType>> {
        let base = self.current_base(span)?;
        let f = objects::ctor_fn(self.ctx, base.id(), false)?;
        self.this_get(span)?;
        for (arg, param) in args.iter().zip(&f.params) {
            self.value(arg, param)?;
        }
        self.emit(I::Call(f.index));
        Ok(None)
    }

    /// `x is T`
    pub(super) fn is_expr(&mut self, e: &Expr, target: &Ty, span: Span) -> R<()> {
        let target = self.t(target);
        let from = self.t(&e.ty);
        match (&from, &target) {
            (Ty::Named(_), Ty::Named(b))
                if self
                    .ctx
                    .types
                    .val(&from)
                    .is_some_and(|v| matches!(v, ValType::Ref(_))) =>
            {
                self.expr(e)?;
                let f = objects::is_fn(self.ctx, b);
                self.emit(I::Call(f));
            }
            _ if from.canonical() == target.canonical() => {
                // A value type is always what its type says.
                if self.expr(e)?.is_some() {
                    self.emit(I::Drop);
                }
                self.emit(I::I32Const(1));
            }
            _ => return Err(unsupported(format!("'is' on {from} is"), span)),
        }
        Ok(())
    }

    /// `x as T`: `x` if it is a `T`, else `null`.
    pub(super) fn as_expr(&mut self, e: &Expr, target: &Ty, span: Span) -> R<()> {
        let target = self.t(target);
        let Ty::Named(t) = &target else {
            return Err(unsupported(format!("'as' with {target} is"), span));
        };
        let from_vt = self.vt(&e.ty, span)?;
        let to_vt = self.vt(&target, span)?;
        let ValType::Ref(to) = to_vt else {
            return Err(unsupported(format!("'as' with {target} is"), span));
        };
        let v = self.a.local(from_vt);
        self.expr(e)?;
        self.emit(I::LocalTee(v));
        let f = objects::is_fn(self.ctx, t);
        self.a.ops([
            I::Call(f),
            I::If(BlockType::Result(to_vt)),
            I::LocalGet(v),
            I::RefCastNullable(to.heap_type),
            I::Else,
            I::RefNull(to.heap_type),
            I::End,
        ]);
        Ok(())
    }
}
