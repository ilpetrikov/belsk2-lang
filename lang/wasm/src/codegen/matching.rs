//! `switch` and `match`.

use std::collections::HashMap;

use belsk2_syntax::ast::{
    ArmBody, Block, Expr, ExprKind, Match, Pattern, PatternKind, PatternRes, SwitchCase,
};
use belsk2_syntax::{Span, Ty, TypeKind};
use wasm_encoder::{BlockType, HeapType, Instruction as I, ValType};

use super::objects;
use super::{arith, unsupported, FnGen, R};

impl FnGen<'_> {
    /// `switch subject { case ...: ... default: ... }`: the first case
    /// with an equal value runs; `break` leaves the switch.
    pub(super) fn switch_stmt(
        &mut self,
        subject: &Expr,
        cases: &[SwitchCase],
        default: Option<&Block>,
        span: Span,
    ) -> R<()> {
        let ty = self.t(&subject.ty);
        let vt = self.vt(&ty, span)?;
        let s = self.a.local(vt);
        self.expr(subject)?;
        self.emit(I::LocalSet(s));
        self.emit(I::Block(BlockType::Empty));
        self.depth += 1;
        let end = self.depth;
        let cont = self.loops.last().map(|l| l.1).unwrap_or(u32::MAX);
        self.loops.push((end, cont));
        let result = self.switch_cases(s, &ty, cases, default, end);
        self.loops.pop();
        result?;
        self.emit(I::End);
        self.depth -= 1;
        Ok(())
    }

    fn switch_cases(
        &mut self,
        s: u32,
        ty: &Ty,
        cases: &[SwitchCase],
        default: Option<&Block>,
        end: u32,
    ) -> R<()> {
        for case in cases {
            self.emit(I::I32Const(0));
            for v in &case.values {
                self.emit(I::LocalGet(s));
                self.literal(v, ty)?;
                let eq = objects::eq_instrs(self.ctx, ty)?;
                self.a.ops(eq);
                self.emit(I::I32Or);
            }
            self.emit(I::If(BlockType::Empty));
            self.depth += 1;
            self.block(&case.body)?;
            self.emit(I::Br(self.depth - end));
            self.emit(I::End);
            self.depth -= 1;
        }
        if let Some(d) = default {
            self.block(d)?;
        }
        Ok(())
    }

    /// A constant compared with a value of type `ty`.
    fn literal(&mut self, e: &Expr, ty: &Ty) -> R<()> {
        let numeric = arith(&self.t(&e.ty)).is_some() && arith(ty).is_some();
        if numeric && !matches!(e.kind, ExprKind::Null) {
            self.convert_expr(e, ty, e.span)
        } else {
            self.value(e, ty)
        }
    }

    /// `match`: the first arm whose pattern fits (and whose condition
    /// holds) runs. `result` is the type of the value of a `match`
    /// expression.
    pub(super) fn match_code(&mut self, m: &Match, result: Option<&Ty>) -> R<()> {
        let ty = self.t(&m.subject.ty);
        let vt = self.vt(&ty, m.span)?;
        let s = self.a.local(vt);
        self.expr(&m.subject)?;
        self.emit(I::LocalSet(s));
        let block = match result {
            Some(r) => BlockType::Result(self.vt(r, m.span)?),
            None => BlockType::Empty,
        };
        self.emit(I::Block(block));
        self.depth += 1;
        let end = self.depth;
        for arm in &m.arms {
            self.emit(I::Block(BlockType::Empty));
            self.depth += 1;
            let next = self.depth;
            self.scopes.push(HashMap::new());
            let r = self.arm(arm, s, &ty, next, end, result);
            self.scopes.pop();
            r?;
            self.emit(I::End);
            self.depth -= 1;
        }
        // The checker made sure some arm fits.
        self.emit(I::Unreachable);
        self.emit(I::End);
        self.depth -= 1;
        Ok(())
    }

    fn arm(
        &mut self,
        arm: &belsk2_syntax::ast::Arm,
        s: u32,
        ty: &Ty,
        next: u32,
        end: u32,
        result: Option<&Ty>,
    ) -> R<()> {
        self.pattern(&arm.pattern, s, ty, next)?;
        if let Some(g) = &arm.guard {
            self.cond(g)?;
            self.emit(I::I32Eqz);
            self.emit(I::BrIf(self.depth - next));
        }
        match (&arm.body, result) {
            (ArmBody::Expr(e), Some(r)) => self.value(e, r)?,
            (ArmBody::Expr(e), None) => {
                if self.expr(e)?.is_some() {
                    self.emit(I::Drop);
                }
            }
            (ArmBody::Block(b), _) => self.block(b)?,
        }
        self.emit(I::Br(self.depth - end));
        Ok(())
    }

    /// Tests local `v` (of type `ty`) against `p`, branching to block
    /// `fail` if it does not fit, and binds the pattern's variables.
    fn pattern(&mut self, p: &Pattern, v: u32, ty: &Ty, fail: u32) -> R<()> {
        match (&p.kind, &p.resolved) {
            (PatternKind::Wildcard, _) => {}
            (PatternKind::Name(name), PatternRes::Bind(_)) => self.declare(name, v, ty.clone()),
            (PatternKind::Name(_), PatternRes::Variant(i))
            | (PatternKind::Variant { .. }, PatternRes::Variant(i)) => {
                let Ty::Named(t) = ty.canonical() else {
                    return Err(unsupported("this pattern is", p.span));
                };
                let info = self
                    .ctx
                    .classes
                    .get(&t.id())
                    .map(|c| (c.st, c.cases.get(*i).copied()));
                match info {
                    Some((None, _)) => {
                        self.a
                            .ops([I::LocalGet(v), I::I32Const(*i as i32), I::I32Ne]);
                    }
                    Some((Some(base), _)) => self.a.ops([
                        I::LocalGet(v),
                        I::StructGet {
                            struct_type_index: base,
                            field_index: 1,
                        },
                        I::I32Const(*i as i32),
                        I::I32Ne,
                    ]),
                    None => return Err(unsupported("this pattern is", p.span)),
                }
                self.emit(I::BrIf(self.depth - fail));
                if let (PatternKind::Variant { args, .. }, Some((_, Some(cst)))) = (&p.kind, info) {
                    let fields = t
                        .variants()
                        .get(*i)
                        .map(|c| c.fields.clone())
                        .unwrap_or_default();
                    for (k, (arg, (_, fty))) in args.iter().zip(&fields).enumerate() {
                        if matches!(arg.kind, PatternKind::Wildcard) {
                            continue;
                        }
                        let fvt = self.vt(fty, arg.span)?;
                        let f = self.a.local(fvt);
                        self.a.ops([
                            I::LocalGet(v),
                            I::RefCastNullable(HeapType::Concrete(cst)),
                            I::StructGet {
                                struct_type_index: cst,
                                field_index: k as u32 + 2,
                            },
                            I::LocalSet(f),
                        ]);
                        self.pattern(arg, f, fty, fail)?;
                    }
                }
            }
            (PatternKind::Literal(e), _) => {
                self.emit(I::LocalGet(v));
                self.literal(e, ty)?;
                let eq = objects::eq_instrs(self.ctx, ty)?;
                self.a.ops(eq);
                self.emit(I::I32Eqz);
                self.emit(I::BrIf(self.depth - fail));
            }
            (PatternKind::Type { bind, .. }, PatternRes::Type(t)) => {
                let t = self.t(t);
                self.type_test(v, ty, &t, bind.as_deref(), fail, p.span)?;
            }
            (PatternKind::Name(_), PatternRes::Type(t)) => {
                let t = self.t(t);
                self.type_test(v, ty, &t, None, fail, p.span)?;
            }
            (PatternKind::Or(alts), _) => {
                self.emit(I::Block(BlockType::Empty));
                self.depth += 1;
                let ok = self.depth;
                for alt in alts {
                    self.emit(I::Block(BlockType::Empty));
                    self.depth += 1;
                    let next = self.depth;
                    self.pattern(alt, v, ty, next)?;
                    self.emit(I::Br(self.depth - ok));
                    self.emit(I::End);
                    self.depth -= 1;
                }
                self.emit(I::Br(self.depth - fail));
                self.emit(I::End);
                self.depth -= 1;
            }
            _ => return Err(unsupported("this pattern is", p.span)),
        }
        Ok(())
    }

    /// `Dog d`: local `v` (of type `from`) must be a `t`.
    fn type_test(
        &mut self,
        v: u32,
        from: &Ty,
        t: &Ty,
        bind: Option<&str>,
        fail: u32,
        span: Span,
    ) -> R<()> {
        if from.is_any() {
            self.any_test(v, t, span)?;
            self.emit(I::I32Eqz);
            self.emit(I::BrIf(self.depth - fail));
            let vt = self.vt(t, span)?;
            let local = self.a.local(vt);
            self.emit(I::LocalGet(v));
            self.unbox(t, span)?;
            self.emit(I::LocalSet(local));
            if let Some(name) = bind {
                self.declare(name, local, t.clone());
            }
            return Ok(());
        }
        let bound = match t {
            Ty::Named(n)
                if n.kind != TypeKind::Enum
                    || self
                        .ctx
                        .classes
                        .get(&n.id())
                        .is_some_and(|c| c.st.is_some()) =>
            {
                let f = objects::is_fn(self.ctx, n);
                self.a.ops([I::LocalGet(v), I::Call(f), I::I32Eqz]);
                self.emit(I::BrIf(self.depth - fail));
                let vt = self.vt(t, span)?;
                let ValType::Ref(r) = vt else {
                    return Err(unsupported("this pattern is", span));
                };
                let local = self.a.local(vt);
                self.a.ops([
                    I::LocalGet(v),
                    I::RefCastNullable(r.heap_type),
                    I::LocalSet(local),
                ]);
                local
            }
            _ if from.canonical() == t.canonical() => v,
            _ => return Err(unsupported(format!("testing for {t} is"), span)),
        };
        if let Some(name) = bind {
            self.declare(name, bound, t.clone());
        }
        Ok(())
    }
}
