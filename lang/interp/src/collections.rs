//! Arrays and dictionaries. Both are reference types: copies of a value
//! share the same contents, so `push(a, x)`, `a[0] = x` or `m["k"] = v` is
//! visible through every alias. Each knows its element types, so values
//! coming from `any` are checked when they are stored.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use belsk2_syntax::{Error, Result, Ty};
use indexmap::IndexMap;

use crate::num::coerce;
use crate::value::Value;

struct ArrayInner {
    elem: Ty,
    items: RefCell<Vec<Value>>,
}

#[derive(Clone)]
pub struct Array(Rc<ArrayInner>);

impl Array {
    /// An array of `any`.
    pub fn new(items: Vec<Value>) -> Self {
        Array::typed(Ty::Any, items)
    }

    /// An array whose elements have type `elem`. The items must already
    /// have that type.
    pub fn typed(elem: Ty, items: Vec<Value>) -> Self {
        Array(Rc::new(ArrayInner {
            elem,
            items: RefCell::new(items),
        }))
    }

    pub fn elem_ty(&self) -> &Ty {
        &self.0.elem
    }

    /// Reads the elements. Fails only if the array is being modified at the
    /// same moment, which the interpreter never does.
    pub fn borrow(&self) -> Result<Ref<'_, Vec<Value>>> {
        self.0
            .items
            .try_borrow()
            .map_err(|_| Error::runtime("array is being modified"))
    }

    pub fn borrow_mut(&self) -> Result<RefMut<'_, Vec<Value>>> {
        self.0
            .items
            .try_borrow_mut()
            .map_err(|_| Error::runtime("array is being modified"))
    }

    /// Converts `v` to the element type (an error if it does not fit).
    pub fn check(&self, v: Value) -> Result<Value> {
        coerce(
            &self.0.elem,
            v,
            &format!("an element of {}", Ty::Array(Rc::new(self.0.elem.clone()))),
        )
    }

    pub fn push(&self, v: Value) -> Result<()> {
        let v = self.check(v)?;
        self.borrow_mut()?.push(v);
        Ok(())
    }

    /// A copy of the elements (the elements themselves are shared).
    pub fn snapshot(&self) -> Result<Vec<Value>> {
        Ok(self.borrow()?.clone())
    }

    pub fn len(&self) -> usize {
        self.0.items.try_borrow().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ptr_eq(&self, other: &Array) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A dictionary key. Integers of every type hash alike, so a key does not
/// depend on whether it was written as `1` or `1L`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Bool(bool),
    Int(i128),
    Char(char),
    Str(String),
}

impl Key {
    pub fn of(v: &Value) -> Result<Key> {
        Ok(match v {
            Value::Bool(b) => Key::Bool(*b),
            Value::Int(i, _) => Key::Int(*i),
            Value::Char(c) => Key::Char(*c),
            Value::String(s) => Key::Str(s.clone()),
            other => {
                return Err(Error::type_error(format!(
                    "{} cannot be a dictionary key",
                    other.type_name()
                )))
            }
        })
    }
}

struct MapInner {
    key: Ty,
    value: Ty,
    /// The original key value is kept next to the value, so iteration
    /// returns keys with their exact types.
    entries: RefCell<IndexMap<Key, (Value, Value)>>,
}

/// `Dictionary<K, V>`: keeps insertion order.
#[derive(Clone)]
pub struct Map(Rc<MapInner>);

impl Map {
    pub fn new(key: Ty, value: Ty) -> Self {
        Map(Rc::new(MapInner {
            key,
            value,
            entries: RefCell::new(IndexMap::new()),
        }))
    }

    pub fn key_ty(&self) -> &Ty {
        &self.0.key
    }

    pub fn value_ty(&self) -> &Ty {
        &self.0.value
    }

    fn entries(&self) -> Result<Ref<'_, IndexMap<Key, (Value, Value)>>> {
        self.0
            .entries
            .try_borrow()
            .map_err(|_| Error::runtime("dictionary is being modified"))
    }

    fn entries_mut(&self) -> Result<RefMut<'_, IndexMap<Key, (Value, Value)>>> {
        self.0
            .entries
            .try_borrow_mut()
            .map_err(|_| Error::runtime("dictionary is being modified"))
    }

    fn ty(&self) -> Ty {
        Ty::map(self.0.key.clone(), self.0.value.clone())
    }

    fn check_key(&self, k: Value) -> Result<Value> {
        coerce(&self.0.key, k, &format!("a key of {}", self.ty()))
    }

    pub fn get(&self, k: &Value) -> Result<Value> {
        let key = Key::of(&self.check_key(k.clone())?)?;
        match self.entries()?.get(&key) {
            Some((_, v)) => Ok(v.clone()),
            None => Err(Error::runtime(format!("key not found: {k}"))),
        }
    }

    pub fn insert(&self, k: Value, v: Value) -> Result<()> {
        let k = self.check_key(k)?;
        let v = coerce(&self.0.value, v, &format!("a value of {}", self.ty()))?;
        let key = Key::of(&k)?;
        let mut entries = self.entries_mut()?;
        match entries.get_mut(&key) {
            // Keep the original key and its position.
            Some(slot) => slot.1 = v,
            None => {
                entries.insert(key, (k, v));
            }
        }
        Ok(())
    }

    pub fn contains(&self, k: &Value) -> Result<bool> {
        let key = Key::of(&self.check_key(k.clone())?)?;
        Ok(self.entries()?.contains_key(&key))
    }

    pub fn remove(&self, k: &Value) -> Result<bool> {
        let key = Key::of(&self.check_key(k.clone())?)?;
        Ok(self.entries_mut()?.shift_remove(&key).is_some())
    }

    pub fn keys(&self) -> Result<Vec<Value>> {
        Ok(self.entries()?.values().map(|(k, _)| k.clone()).collect())
    }

    pub fn values(&self) -> Result<Vec<Value>> {
        Ok(self.entries()?.values().map(|(_, v)| v.clone()).collect())
    }

    /// Key/value pairs in insertion order.
    pub fn pairs(&self) -> Result<Vec<(Value, Value)>> {
        Ok(self.entries()?.values().cloned().collect())
    }

    pub fn len(&self) -> usize {
        self.0.entries.try_borrow().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ptr_eq(&self, other: &Map) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Moves the contents of `v` onto `work` if `v` is the last reference to
/// an array or dictionary, so they are dropped by the loop in
/// [`drop_iteratively`] instead of recursively.
fn unlink(v: Value, work: &mut Vec<Value>) {
    match v {
        Value::Array(a) if Rc::strong_count(&a.0) == 1 => {
            if let Ok(mut items) = a.0.items.try_borrow_mut() {
                work.append(&mut items);
            }
        }
        Value::Map(m) if Rc::strong_count(&m.0) == 1 => {
            if let Ok(mut entries) = m.0.entries.try_borrow_mut() {
                for (_, (k, v)) in entries.drain(..) {
                    work.push(k);
                    work.push(v);
                }
            }
        }
        _ => {}
    }
}

/// Dropping deeply nested collections (`a = [a]` in a loop) must not
/// recurse, otherwise it could overflow the stack.
fn drop_iteratively(mut work: Vec<Value>) {
    while let Some(v) = work.pop() {
        unlink(v, &mut work);
    }
}

impl Drop for Array {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) != 1 {
            return;
        }
        if let Ok(mut items) = self.0.items.try_borrow_mut() {
            let work = std::mem::take(&mut *items);
            drop(items);
            drop_iteratively(work);
        }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) != 1 {
            return;
        }
        if let Ok(mut entries) = self.0.entries.try_borrow_mut() {
            let work = entries.drain(..).flat_map(|(_, (k, v))| [k, v]).collect();
            drop(entries);
            drop_iteratively(work);
        }
    }
}
