use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::{Error, Result, Ty};

use crate::num::coerce;
use crate::value::Value;

/// Environments are shared: a block, loop body or closure sees (and can
/// modify) the variables of every enclosing scope.
pub type EnvRef = Rc<RefCell<Env>>;

struct Slot {
    value: Value,
    ty: Ty,
}

#[derive(Default)]
pub struct Env {
    vars: HashMap<String, Slot>,
    parent: Option<EnvRef>,
}

impl Env {
    pub fn root() -> EnvRef {
        Rc::new(RefCell::new(Env::default()))
    }

    pub fn child(parent: &EnvRef) -> EnvRef {
        Rc::new(RefCell::new(Env {
            vars: HashMap::new(),
            parent: Some(Rc::clone(parent)),
        }))
    }

    /// Declares a variable in this scope, replacing any earlier declaration
    /// with the same name. The value must already have type `ty`
    /// (see [`coerce`]).
    pub fn define(&mut self, name: &str, value: Value, ty: Ty) {
        self.vars.insert(name.to_string(), Slot { value, ty });
    }

    /// Static types of the variables in this scope, for the checker.
    pub fn static_types(&self) -> Vec<(String, Ty)> {
        self.vars
            .iter()
            .map(|(name, slot)| {
                let ty = match (&slot.value, &slot.ty) {
                    (Value::Function(f), _) => belsk2_typeck::fn_type(&f.decl),
                    (Value::Null, Ty::Any) => Ty::Any,
                    (v, Ty::Any) => v.ty(),
                    (_, declared) => declared.clone(),
                };
                (name.clone(), ty)
            })
            .collect()
    }
}

/// Looks a variable up through the scope chain.
pub fn lookup(env: &EnvRef, name: &str) -> Option<Value> {
    let mut cur = Rc::clone(env);
    loop {
        let next = {
            let e = cur.borrow();
            if let Some(slot) = e.vars.get(name) {
                return Some(slot.value.clone());
            }
            e.parent.clone()?
        };
        cur = next;
    }
}

/// Assigns to an existing variable, checking its declared type.
pub fn assign(env: &EnvRef, name: &str, value: Value) -> Result<()> {
    let mut cur = Rc::clone(env);
    loop {
        let next = {
            let mut e = cur.borrow_mut();
            if let Some(slot) = e.vars.get_mut(name) {
                slot.value = coerce(&slot.ty, value, &format!("'{name}'"))?;
                return Ok(());
            }
            e.parent.clone()
        };
        match next {
            Some(p) => cur = p,
            None => return Err(Error::runtime(format!("undefined variable '{name}'"))),
        }
    }
}
