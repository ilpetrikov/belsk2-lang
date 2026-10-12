//! Memory management for class objects.
//!
//! Values are reference counted: an object is freed as soon as nothing
//! refers to it. Objects that refer to each other in a cycle keep each
//! other alive, so a cycle collector finds groups of objects that only
//! the group itself refers to and frees them. It is conservative: an
//! object referred to from anywhere it does not look into (a variable,
//! a closure, a shared array) counts as alive.
//!
//! Destructors (`~Player()`) of freed objects do not run inside `Drop`,
//! where the interpreter is not available: the object's fields go to a
//! queue, and the interpreter runs the destructor at the next statement.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::value::{Class, Object, Value};

/// A cycle collection runs after this many objects were created.
const COLLECT_EVERY: usize = 10_000;

thread_local! {
    static OBJECTS: RefCell<Vec<Weak<Object>>> = const { RefCell::new(Vec::new()) };
    static CREATED: Cell<usize> = const { Cell::new(0) };
    static FINALIZE: RefCell<Vec<(Rc<Class>, Vec<Value>)>> = const { RefCell::new(Vec::new()) };
}

/// Tracks a new object. Returns whether a collection is due.
pub(crate) fn register(o: &Rc<Object>) -> bool {
    let _ = OBJECTS.try_with(|objects| {
        if let Ok(mut objects) = objects.try_borrow_mut() {
            objects.push(Rc::downgrade(o));
        }
    });
    CREATED
        .try_with(|n| {
            let count = n.get().wrapping_add(1);
            n.set(count);
            count % COLLECT_EVERY == 0
        })
        .unwrap_or(false)
}

/// Queues the destructor of a freed object.
pub(crate) fn enqueue(class: Rc<Class>, fields: Vec<Value>) {
    let _ = FINALIZE.try_with(|q| {
        if let Ok(mut q) = q.try_borrow_mut() {
            q.push((class, fields));
        }
    });
}

pub(crate) fn has_pending() -> bool {
    FINALIZE
        .try_with(|q| q.try_borrow().is_ok_and(|q| !q.is_empty()))
        .unwrap_or(false)
}

/// The objects whose destructors are due.
pub(crate) fn take_pending() -> Vec<(Rc<Class>, Vec<Value>)> {
    FINALIZE
        .try_with(|q| {
            q.try_borrow_mut()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Every object still alive (and forgets the freed ones).
fn live_objects() -> Vec<Rc<Object>> {
    OBJECTS
        .try_with(|objects| {
            let Ok(mut objects) = objects.try_borrow_mut() else {
                return Vec::new();
            };
            objects.retain(|w| w.strong_count() > 0);
            objects.iter().filter_map(Weak::upgrade).collect()
        })
        .unwrap_or_default()
}

/// The values directly inside `v` that may lead to objects.
fn children(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(a) => {
            if let Ok(items) = a.borrow() {
                out.extend(items.iter().cloned());
            }
        }
        Value::Map(m) => {
            if let Ok(pairs) = m.pairs() {
                for (k, v) in pairs {
                    out.push(k);
                    out.push(v);
                }
            }
        }
        Value::Struct(s) => out.extend(s.fields.iter().cloned()),
        Value::Enum(_, _, data) => out.extend(data.iter().cloned()),
        Value::Object(o) => {
            if let Ok(fields) = o.fields.try_borrow() {
                out.extend(fields.iter().cloned());
            }
        }
        _ => {}
    }
}

/// Whether nothing but `v`'s holder refers to the container `v`, so that
/// what it holds can be counted as held by that holder.
fn unique(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.is_unique(),
        Value::Map(m) => m.is_unique(),
        Value::Struct(s) => Rc::strong_count(s) == 1,
        Value::Enum(_, _, data) => !data.is_empty() && Rc::strong_count(data) == 1,
        _ => false,
    }
}

/// Finds the objects that only other unreachable objects refer to.
pub(crate) fn find_garbage() -> Vec<Rc<Object>> {
    let objects = live_objects();
    let index: HashMap<*const Object, usize> = objects
        .iter()
        .enumerate()
        .map(|(i, o)| (Rc::as_ptr(o), i))
        .collect();
    // `objects` itself holds one reference to each.
    let strong: Vec<usize> = objects
        .iter()
        .map(|o| Rc::strong_count(o).saturating_sub(1))
        .collect();

    // References between tracked objects, through fields and through
    // containers that only the referring object holds.
    let mut internal = vec![0usize; objects.len()];
    let mut busy = vec![false; objects.len()];
    for (i, o) in objects.iter().enumerate() {
        let Ok(fields) = o.fields.try_borrow() else {
            if let Some(b) = busy.get_mut(i) {
                *b = true;
            }
            continue;
        };
        // Containers are checked for being unique before they are copied
        // into the work list (copying adds a reference).
        let mut work: Vec<Value> = Vec::new();
        let visit = |v: &Value, work: &mut Vec<Value>, internal: &mut Vec<usize>| match v {
            Value::Object(p) => {
                if let Some(slot) = index.get(&Rc::as_ptr(p)).and_then(|&j| internal.get_mut(j)) {
                    *slot += 1;
                }
            }
            other if unique(other) => work.push(other.clone()),
            _ => {}
        };
        for v in fields.iter() {
            visit(v, &mut work, &mut internal);
        }
        while let Some(c) = work.pop() {
            let mut inner = Vec::new();
            match &c {
                Value::Array(a) => {
                    if let Ok(items) = a.borrow() {
                        for v in items.iter() {
                            visit(v, &mut inner, &mut internal);
                        }
                    }
                }
                Value::Map(m) => {
                    if let Ok(pairs) = m.pairs() {
                        for (k, v) in pairs {
                            visit(&k, &mut inner, &mut internal);
                            visit(&v, &mut inner, &mut internal);
                        }
                    }
                }
                Value::Struct(s) => {
                    for v in &s.fields {
                        visit(v, &mut inner, &mut internal);
                    }
                }
                Value::Enum(_, _, data) => {
                    for v in data.iter() {
                        visit(v, &mut inner, &mut internal);
                    }
                }
                _ => {}
            }
            work.extend(inner);
        }
    }

    // Objects referred to from outside are alive, and so is everything
    // they lead to.
    let mut alive = vec![false; objects.len()];
    let mut work: Vec<Value> = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        let external = strong.get(i).copied().unwrap_or(0) > internal.get(i).copied().unwrap_or(0);
        if external || busy.get(i).copied().unwrap_or(true) {
            if let Some(a) = alive.get_mut(i) {
                *a = true;
            }
            work.push(Value::Object(Rc::clone(o)));
        }
    }
    let mut seen_containers = 0usize;
    while let Some(v) = work.pop() {
        let mut next = Vec::new();
        children(&v, &mut next);
        for c in next {
            if let Value::Object(p) = &c {
                match index.get(&Rc::as_ptr(p)).and_then(|&j| alive.get_mut(j)) {
                    Some(a) if *a => continue,
                    Some(a) => *a = true,
                    None => {}
                }
                work.push(c);
            } else if matches!(
                c,
                Value::Array(_) | Value::Map(_) | Value::Struct(_) | Value::Enum(..)
            ) {
                // Shared containers may be reached many times; the limit
                // keeps cyclic arrays from looping forever.
                seen_containers += 1;
                if seen_containers < 10_000_000 {
                    work.push(c);
                }
            }
        }
    }
    objects
        .into_iter()
        .zip(alive)
        .filter(|(_, a)| !a)
        .map(|(o, _)| o)
        .collect()
}
