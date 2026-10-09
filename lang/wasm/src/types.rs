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

use std::collections::HashMap;

use belsk2_syntax::{FloatKind, Ty};
use wasm_encoder::{FieldType, HeapType, RefType, StorageType, TypeSection, ValType};

enum Def {
    Func(Vec<ValType>, Vec<ValType>),
    Array(ValType),
    Struct(Vec<ValType>),
}

/// Every type of the module, in the order they are defined. A type only
/// refers to types defined before it.
#[derive(Default)]
pub struct Types {
    defs: Vec<Def>,
    funcs: HashMap<(Vec<ValType>, Vec<ValType>), u32>,
    string: Option<u32>,
    arrays: HashMap<String, ArrayType>,
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

pub fn nullable(index: u32) -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Concrete(index),
    })
}

fn non_null(index: u32) -> ValType {
    ValType::Ref(RefType {
        nullable: false,
        heap_type: HeapType::Concrete(index),
    })
}

impl Types {
    fn add(&mut self, def: Def) -> u32 {
        self.defs.push(def);
        self.defs.len() as u32 - 1
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
        let key = elem.to_string();
        if let Some(t) = self.arrays.get(&key) {
            return Some(*t);
        }
        let elem = self.val(elem)?;
        let raw = self.add(Def::Array(elem));
        let obj = self.add(Def::Struct(vec![non_null(raw), ValType::I32]));
        let t = ArrayType { raw, obj, elem };
        self.arrays.insert(key, t);
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
            Ty::Array(elem) if !elem.is_any() && !elem.has_params() => {
                nullable(self.array(elem)?.obj)
            }
            _ => return None,
        })
    }

    pub fn section(&self) -> TypeSection {
        let mut section = TypeSection::new();
        for def in &self.defs {
            match def {
                Def::Func(params, results) => {
                    section
                        .ty()
                        .function(params.iter().copied(), results.iter().copied());
                }
                Def::Array(elem) => section.ty().array(&StorageType::Val(*elem), true),
                Def::Struct(fields) => section.ty().struct_(fields.iter().map(|t| FieldType {
                    element_type: StorageType::Val(*t),
                    mutable: true,
                })),
            }
        }
        section
    }
}
