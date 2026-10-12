//! The module's type section and how Belsk2 types map to WebAssembly.
//!
//! - `bool`, `char` and integers up to 32 bits are `i32` (kept within the
//!   range of their type), `long` and `ulong` are `i64`, `float` is `f32`,
//!   `double` and `bel` are `f64`.
//! - `string` is a nullable reference to a GC array of code points
//!   (`(array (mut i32))`), so indexing and `len` count characters as the
//!   interpreter does.
//! - `T[]` is a nullable reference to a struct `{ data, len }` whose `data`
//!   is a GC array with room to grow (`push`).
//! - User-defined types are GC structs in one recursion group (they may
//!   refer to each other); see [`crate::objects`]. An enum without data is
//!   an `i32` (the index of its case).

use std::collections::HashMap;

use belsk2_syntax::{FloatKind, Ty};
use wasm_encoder::{
    AbstractHeapType, ArrayType as WasmArray, CompositeInnerType, CompositeType, FieldType,
    FuncType, HeapType, RefType, StorageType, StructType, SubType, TypeSection, ValType,
};

/// A type definition.
#[derive(Clone)]
pub enum Def {
    Func(Vec<ValType>, Vec<ValType>),
    Array(ValType),
    /// A struct; `sub` is its supertype, and it may have subtypes unless
    /// `last`.
    Struct {
        fields: Vec<ValType>,
        sub: Option<u32>,
        last: bool,
    },
}

enum Entry {
    One(Def),
    /// Types that may refer to each other.
    Rec(Vec<Def>),
}

/// Every type of the module, in the order they are defined. A type only
/// refers to types defined before it, or to types of its own recursion
/// group.
#[derive(Default)]
pub struct Types {
    entries: Vec<Entry>,
    count: u32,
    funcs: HashMap<(Vec<ValType>, Vec<ValType>), u32>,
    string: Option<u32>,
    arrays: HashMap<String, ArrayType>,
    maps: HashMap<String, MapType>,
    pairs: HashMap<String, u32>,
    /// Every array, dictionary and pair type with its struct (for printing
    /// an `any`).
    containers: Vec<(Ty, u32)>,
    boxes: Option<Boxes>,
    /// The value type of each user-defined type, by type id.
    user: HashMap<usize, ValType>,
}

/// The two types behind a Belsk2 array.
#[derive(Debug, Clone, Copy)]
pub struct ArrayType {
    /// `(array (mut elem))`: the storage.
    pub raw: u32,
    /// `(struct (field data (mut (ref raw))) (field len (mut i32)))`.
    pub obj: u32,
    pub elem: ValType,
}

/// The struct behind `Dictionary<K, V>`: `{ keys, values, table }`.
/// Keys and values are arrays in insertion order; `table` is a hash index
/// (positions + 1, 0 for free slots), built once there are enough entries
/// and dropped when an entry is removed.
#[derive(Debug, Clone, Copy)]
pub struct MapType {
    pub st: u32,
    pub keys: ArrayType,
    pub values: ArrayType,
}

/// The structs boxing a value stored in an `any`: `{ kind, value }`, all
/// subtypes of `base` (`{ kind }`).
#[derive(Debug, Clone, Copy)]
pub struct Boxes {
    pub base: u32,
    pub i32: u32,
    pub i64: u32,
    pub f32: u32,
    pub f64: u32,
}

/// The type of `any` values: `(ref null eq)`.
pub fn any_type() -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Abstract {
            shared: false,
            ty: AbstractHeapType::Eq,
        },
    })
}

pub fn nullable(index: u32) -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Concrete(index),
    })
}

pub fn non_null(index: u32) -> ValType {
    ValType::Ref(RefType {
        nullable: false,
        heap_type: HeapType::Concrete(index),
    })
}

/// The key of `elem[]` in the array cache (user-defined types differ by
/// id, not just by name).
fn array_key(elem: &Ty) -> String {
    format!("{elem:?}")
}

fn map_key(key: &Ty, value: &Ty) -> String {
    format!("{key:?}|{value:?}")
}

impl Types {
    fn add(&mut self, def: Def) -> u32 {
        self.entries.push(Entry::One(def));
        self.count += 1;
        self.count - 1
    }

    /// The index the next type will get.
    pub fn next_index(&self) -> u32 {
        self.count
    }

    /// Adds a recursion group; its types get consecutive indices from
    /// [`Types::next_index`].
    pub fn add_rec(&mut self, defs: Vec<Def>) {
        self.count += defs.len() as u32;
        self.entries.push(Entry::Rec(defs));
    }

    /// Records the value type of a user-defined type.
    pub fn set_user(&mut self, id: usize, t: ValType) {
        self.user.insert(id, t);
    }

    /// Records the types of `elem[]` defined in a recursion group.
    pub fn set_array(&mut self, elem: &Ty, t: ArrayType) {
        self.arrays.insert(array_key(elem), t);
        self.containers.push((Ty::array(elem.clone()), t.obj));
    }

    /// Every array, dictionary and pair type so far, with its struct.
    pub fn containers(&self) -> Vec<(Ty, u32)> {
        self.containers.clone()
    }

    /// The box types of `any` values.
    pub fn boxes(&mut self) -> Boxes {
        if let Some(b) = self.boxes {
            return b;
        }
        let base = self.add(Def::Struct {
            fields: vec![ValType::I32],
            sub: None,
            last: false,
        });
        let mut one = |t: ValType| {
            self.add(Def::Struct {
                fields: vec![ValType::I32, t],
                sub: Some(base),
                last: true,
            })
        };
        let b = Boxes {
            base,
            i32: one(ValType::I32),
            i64: one(ValType::I64),
            f32: one(ValType::F32),
            f64: one(ValType::F64),
        };
        self.boxes = Some(b);
        b
    }

    pub fn has_array(&self, elem: &Ty) -> bool {
        self.arrays.contains_key(&array_key(elem))
    }

    pub fn has_map(&self, key: &Ty, value: &Ty) -> bool {
        self.maps.contains_key(&map_key(key, value))
    }

    /// Records the struct of `Dictionary<key, value>` defined in a
    /// recursion group.
    pub fn set_map(&mut self, key: &Ty, value: &Ty, t: MapType) {
        self.maps.insert(map_key(key, value), t);
        self.containers
            .push((Ty::map(key.clone(), value.clone()), t.st));
    }

    /// The fields of the struct of a dictionary.
    pub fn map_fields(&mut self, keys: &ArrayType, values: &ArrayType) -> Vec<ValType> {
        let table = self.string();
        vec![nullable(keys.obj), nullable(values.obj), nullable(table)]
    }

    /// The types of `Dictionary<key, value>`.
    pub fn map(&mut self, key: &Ty, value: &Ty) -> Option<MapType> {
        if let Some(t) = self.maps.get(&map_key(key, value)) {
            return Some(*t);
        }
        if key.is_any() || key.has_params() || value.has_params() {
            return None;
        }
        let keys = self.array(key)?;
        let values = self.array(value)?;
        let fields = self.map_fields(&keys, &values);
        let st = self.add(Def::Struct {
            fields,
            sub: None,
            last: true,
        });
        let t = MapType { st, keys, values };
        self.maps.insert(map_key(key, value), t);
        self.containers
            .push((Ty::map(key.clone(), value.clone()), st));
        Some(t)
    }

    /// The struct of `KeyValuePair<key, value>`: `{ key, value }`.
    pub fn pair(&mut self, key: &Ty, value: &Ty) -> Option<u32> {
        if let Some(t) = self.pairs.get(&map_key(key, value)) {
            return Some(*t);
        }
        let fields = vec![self.val(key)?, self.val(value)?];
        let st = self.add(Def::Struct {
            fields,
            sub: None,
            last: true,
        });
        self.pairs.insert(map_key(key, value), st);
        self.containers
            .push((Ty::key_value(key.clone(), value.clone()), st));
        Some(st)
    }

    pub fn func(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let key = (params, results);
        if let Some(&i) = self.funcs.get(&key) {
            return i;
        }
        let i = self.add(Def::Func(key.0.clone(), key.1.clone()));
        self.funcs.insert(key, i);
        i
    }

    /// The array type of strings.
    pub fn string(&mut self) -> u32 {
        match self.string {
            Some(i) => i,
            None => {
                let i = self.add(Def::Array(ValType::I32));
                self.string = Some(i);
                i
            }
        }
    }

    /// The types of `elem[]`, if `elem` can be stored yet.
    pub fn array(&mut self, elem: &Ty) -> Option<ArrayType> {
        let key = array_key(elem);
        if let Some(t) = self.arrays.get(&key) {
            return Some(*t);
        }
        let elem_ty = elem.clone();
        let elem = self.val(elem)?;
        let raw = self.add(Def::Array(elem));
        let obj = self.add(Def::Struct {
            fields: vec![non_null(raw), ValType::I32],
            sub: None,
            last: true,
        });
        let t = ArrayType { raw, obj, elem };
        self.arrays.insert(key, t);
        self.containers.push((Ty::array(elem_ty), obj));
        Some(t)
    }

    /// The WebAssembly type of values of `ty`, if it has one yet.
    pub fn val(&mut self, ty: &Ty) -> Option<ValType> {
        Some(match ty {
            Ty::Bool | Ty::Char => ValType::I32,
            Ty::Int(k) if k.bits() == 64 => ValType::I64,
            Ty::Int(_) => ValType::I32,
            Ty::Float(FloatKind::F32) => ValType::F32,
            Ty::Float(FloatKind::F64) | Ty::Bel => ValType::F64,
            Ty::String | Ty::Ster => nullable(self.string()),
            Ty::Array(elem) if !elem.has_params() => nullable(self.array(elem)?.obj),
            Ty::Any => any_type(),
            Ty::Named(t) => *self.user.get(&t.id())?,
            Ty::Map(k, v) => nullable(self.map(k, v)?.st),
            Ty::KeyValue(k, v) => nullable(self.pair(k, v)?),
            _ => return None,
        })
    }

    pub fn section(&self) -> TypeSection {
        let mut section = TypeSection::new();
        for entry in &self.entries {
            match entry {
                Entry::One(def) => section.ty().subtype(&sub_type(def)),
                Entry::Rec(defs) => section.ty().rec(defs.iter().map(sub_type)),
            }
        }
        section
    }
}

fn sub_type(def: &Def) -> SubType {
    let field = |t: &ValType| FieldType {
        element_type: StorageType::Val(*t),
        mutable: true,
    };
    let (inner, sub, last) = match def {
        Def::Func(params, results) => (
            CompositeInnerType::Func(FuncType::new(
                params.iter().copied(),
                results.iter().copied(),
            )),
            None,
            true,
        ),
        Def::Array(elem) => (
            CompositeInnerType::Array(WasmArray(field(elem))),
            None,
            true,
        ),
        Def::Struct { fields, sub, last } => (
            CompositeInnerType::Struct(StructType {
                fields: fields.iter().map(field).collect(),
            }),
            *sub,
            *last,
        ),
    };
    SubType {
        is_final: last,
        supertype_idxs: sub.into_iter().collect(),
        composite_type: CompositeType {
            inner,
            shared: false,
            descriptor: None,
            describes: None,
        },
    }
}
