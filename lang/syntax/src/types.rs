//! The type model shared by the checker, the interpreter and (later) the
//! compiler.

use std::cell::OnceCell;
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::span::Span;

/// A fixed-width integer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntKind {
    /// `sbyte`
    I8,
    /// `byte`
    U8,
    /// `short`
    I16,
    /// `ushort`
    U16,
    /// `int`
    I32,
    /// `uint`
    U32,
    /// `long`
    I64,
    /// `ulong`
    U64,
}

impl IntKind {
    pub const ALL: [IntKind; 8] = [
        IntKind::I8,
        IntKind::U8,
        IntKind::I16,
        IntKind::U16,
        IntKind::I32,
        IntKind::U32,
        IntKind::I64,
        IntKind::U64,
    ];

    pub fn name(self) -> &'static str {
        match self {
            IntKind::I8 => "sbyte",
            IntKind::U8 => "byte",
            IntKind::I16 => "short",
            IntKind::U16 => "ushort",
            IntKind::I32 => "int",
            IntKind::U32 => "uint",
            IntKind::I64 => "long",
            IntKind::U64 => "ulong",
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64
        )
    }

    pub fn bits(self) -> u32 {
        match self {
            IntKind::I8 | IntKind::U8 => 8,
            IntKind::I16 | IntKind::U16 => 16,
            IntKind::I32 | IntKind::U32 => 32,
            IntKind::I64 | IntKind::U64 => 64,
        }
    }

    pub fn min(self) -> i128 {
        match self {
            IntKind::I8 => i8::MIN.into(),
            IntKind::I16 => i16::MIN.into(),
            IntKind::I32 => i32::MIN.into(),
            IntKind::I64 => i64::MIN.into(),
            _ => 0,
        }
    }

    pub fn max(self) -> i128 {
        match self {
            IntKind::I8 => i8::MAX.into(),
            IntKind::U8 => u8::MAX.into(),
            IntKind::I16 => i16::MAX.into(),
            IntKind::U16 => u16::MAX.into(),
            IntKind::I32 => i32::MAX.into(),
            IntKind::U32 => u32::MAX.into(),
            IntKind::I64 => i64::MAX.into(),
            IntKind::U64 => u64::MAX.into(),
        }
    }

    pub fn fits(self, v: i128) -> bool {
        v >= self.min() && v <= self.max()
    }

    /// Reduces `v` to this width, wrapping around like C# unchecked
    /// arithmetic.
    pub fn wrap(self, v: i128) -> i128 {
        match self {
            IntKind::I8 => (v as i8).into(),
            IntKind::U8 => (v as u8).into(),
            IntKind::I16 => (v as i16).into(),
            IntKind::U16 => (v as u16).into(),
            IntKind::I32 => (v as i32).into(),
            IntKind::U32 => (v as u32).into(),
            IntKind::I64 => (v as i64).into(),
            IntKind::U64 => (v as u64).into(),
        }
    }
}

/// A floating-point type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FloatKind {
    /// `float`
    F32,
    /// `double`
    F64,
}

impl FloatKind {
    pub fn name(self) -> &'static str {
        match self {
            FloatKind::F32 => "float",
            FloatKind::F64 => "double",
        }
    }

    /// Rounds `v` to this precision.
    pub fn round(self, v: f64) -> f64 {
        match self {
            FloatKind::F32 => (v as f32).into(),
            FloatKind::F64 => v,
        }
    }
}

/// A numeric type: the part of [`Ty`] that arithmetic works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumTy {
    Int(IntKind),
    Float(FloatKind),
}

impl NumTy {
    pub fn ty(self) -> Ty {
        match self {
            NumTy::Int(k) => Ty::Int(k),
            NumTy::Float(k) => Ty::Float(k),
        }
    }

    /// The C# implicit (widening) numeric conversions.
    pub fn widens_to(self, to: NumTy) -> bool {
        use FloatKind::*;
        use IntKind::*;
        use NumTy::{Float as F, Int as I};
        if self == to {
            return true;
        }
        match (self, to) {
            (_, F(F64)) => true,
            (F(_), _) => false,
            (_, F(F32)) => true,
            (I(I8), I(I16 | I32 | I64)) => true,
            (I(U8), I(I16 | U16 | I32 | U32 | I64 | U64)) => true,
            (I(I16), I(I32 | I64)) => true,
            (I(U16), I(I32 | U32 | I64 | U64)) => true,
            (I(I32), I(I64)) => true,
            (I(U32), I(I64 | U64)) => true,
            _ => false,
        }
    }

    /// The type both operands of a binary operator are converted to
    /// (C# binary numeric promotion). `None` if there is none, as for
    /// `ulong` combined with a signed type.
    pub fn promote(a: NumTy, b: NumTy) -> Option<NumTy> {
        use IntKind::*;
        let (a, b) = match (a, b) {
            (NumTy::Float(x), NumTy::Float(y)) => return Some(NumTy::Float(x.max(y))),
            (NumTy::Float(x), _) | (_, NumTy::Float(x)) => return Some(NumTy::Float(x)),
            (NumTy::Int(a), NumTy::Int(b)) => (a, b),
        };
        let has = |k: IntKind| a == k || b == k;
        let other_signed = |k: IntKind| (a == k && b.signed()) || (b == k && a.signed());
        Some(NumTy::Int(if has(U64) {
            if other_signed(U64) {
                return None;
            }
            U64
        } else if has(I64) {
            I64
        } else if has(U32) {
            if other_signed(U32) {
                I64
            } else {
                U32
            }
        } else {
            I32
        }))
    }

    /// The result type of unary `-`.
    pub fn negated(self) -> Option<NumTy> {
        match self {
            NumTy::Float(_) => Some(self),
            NumTy::Int(IntKind::U64) => None,
            NumTy::Int(IntKind::U32 | IntKind::I64) => Some(NumTy::Int(IntKind::I64)),
            NumTy::Int(_) => Some(NumTy::Int(IntKind::I32)),
        }
    }

    /// Small integer types are widened to `int` before arithmetic.
    pub fn int_promoted(k: IntKind) -> IntKind {
        match k {
            IntKind::I8 | IntKind::U8 | IntKind::I16 | IntKind::U16 => IntKind::I32,
            k => k,
        }
    }
}

/// The static type of a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Unknown until run time; compatible with everything and checked when
    /// the program runs.
    Any,
    /// The type of the `null` literal.
    Null,
    Bool,
    Int(IntKind),
    Float(FloatKind),
    Char,
    String,
    /// A `double` that may not exceed 1000.
    Bel,
    /// A `string` that may not be `null`.
    Ster,
    /// An array with elements of the given type.
    Array(Rc<Ty>),
    /// `Dictionary<K, V>`: a hash map that keeps insertion order.
    Map(Rc<Ty>, Rc<Ty>),
    /// A type parameter of a generic function (`T` in `fn f<T>(x: T)`).
    Param(Rc<str>),
    /// A function; the signature is known for named declarations.
    Fn(Option<Rc<Sig>>),
    /// A value of a user-defined type (`struct`, `enum`, `class`,
    /// `interface`).
    Named(TypeRef),
    /// A user-defined type used as a value: `Point` in `Point(1, 2)` or
    /// `Color` in `Color.Red`.
    Type(TypeRef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub name: String,
    /// Type parameters of a generic function, in declaration order.
    pub type_params: Vec<Rc<str>>,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

impl Ty {
    pub const INT: Ty = Ty::Int(IntKind::I32);
    pub const DOUBLE: Ty = Ty::Float(FloatKind::F64);

    pub fn array(elem: Ty) -> Ty {
        Ty::Array(Rc::new(elem))
    }

    pub fn map(key: Ty, value: Ty) -> Ty {
        Ty::Map(Rc::new(key), Rc::new(value))
    }

    /// Whether values of this type can be dictionary keys.
    pub fn is_key(&self) -> bool {
        matches!(
            self.canonical(),
            Ty::Any | Ty::Bool | Ty::Int(_) | Ty::Char | Ty::String | Ty::Param(_)
        )
    }

    /// Whether this type mentions a type parameter.
    pub fn has_params(&self) -> bool {
        match self {
            Ty::Param(_) => true,
            Ty::Array(elem) => elem.has_params(),
            Ty::Map(k, v) => k.has_params() || v.has_params(),
            _ => false,
        }
    }

    /// Replaces type parameters using `subst`.
    pub fn substitute(&self, subst: &dyn Fn(&str) -> Option<Ty>) -> Ty {
        match self {
            Ty::Param(name) => subst(name).unwrap_or_else(|| self.clone()),
            Ty::Array(elem) => Ty::array(elem.substitute(subst)),
            Ty::Map(k, v) => Ty::map(k.substitute(subst), v.substitute(subst)),
            t => t.clone(),
        }
    }

    /// The type named by a built-in type keyword.
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "any" | "object" => Ty::Any,
            "bool" => Ty::Bool,
            "sbyte" => Ty::Int(IntKind::I8),
            "byte" => Ty::Int(IntKind::U8),
            "short" => Ty::Int(IntKind::I16),
            "ushort" => Ty::Int(IntKind::U16),
            "int" => Ty::Int(IntKind::I32),
            "uint" => Ty::Int(IntKind::U32),
            "long" => Ty::Int(IntKind::I64),
            "ulong" => Ty::Int(IntKind::U64),
            "float" => Ty::Float(FloatKind::F32),
            "double" => Ty::Float(FloatKind::F64),
            "char" => Ty::Char,
            "string" => Ty::String,
            "bel" => Ty::Bel,
            "ster" => Ty::Ster,
            "array" => Ty::array(Ty::Any),
            "fn" => Ty::Fn(None),
            _ => return None,
        })
    }

    /// The numeric type used for arithmetic, if this is a number.
    pub fn num(&self) -> Option<NumTy> {
        match self {
            Ty::Int(k) => Some(NumTy::Int(*k)),
            Ty::Float(k) => Some(NumTy::Float(*k)),
            Ty::Bel => Some(NumTy::Float(FloatKind::F64)),
            _ => None,
        }
    }

    /// `bel` behaves as `double` and `ster` as `string`.
    pub fn canonical(&self) -> Ty {
        match self {
            Ty::Bel => Ty::DOUBLE,
            Ty::Ster => Ty::String,
            t => t.clone(),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Ty::Any)
    }

    /// Whether `null` is a valid value of this type. Numbers, `bool` and
    /// `char` are value types and are never null, as in C#.
    pub fn nullable(&self) -> bool {
        match self {
            Ty::Any
            | Ty::Null
            | Ty::String
            | Ty::Array(_)
            | Ty::Map(..)
            | Ty::Fn(_)
            | Ty::Param(_) => true,
            Ty::Named(t) => t.kind.is_reference(),
            _ => false,
        }
    }

    /// The user-defined type of a value, if it has one.
    pub fn named(&self) -> Option<&TypeRef> {
        match self {
            Ty::Named(t) => Some(t),
            _ => None,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Any => f.write_str("any"),
            Ty::Null => f.write_str("null"),
            Ty::Bool => f.write_str("bool"),
            Ty::Int(k) => f.write_str(k.name()),
            Ty::Float(k) => f.write_str(k.name()),
            Ty::Char => f.write_str("char"),
            Ty::String => f.write_str("string"),
            Ty::Bel => f.write_str("bel"),
            Ty::Ster => f.write_str("ster"),
            Ty::Array(elem) => write!(f, "{elem}[]"),
            Ty::Map(k, v) => write!(f, "Dictionary<{k}, {v}>"),
            Ty::Param(name) => f.write_str(name),
            Ty::Fn(_) => f.write_str("fn"),
            Ty::Named(t) => f.write_str(&t.name),
            Ty::Type(t) => write!(f, "type {}", t.name),
        }
    }
}

/// A type as written in source code, e.g. `int`, `string[]`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    /// `int`, `List<int>`
    Named { name: String, args: Vec<TypeExpr> },
    /// `T[]`
    Array(Box<TypeExpr>),
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeExprKind::Named { name, args } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    f.write_str(">")?;
                }
                Ok(())
            }
            TypeExprKind::Array(elem) => write!(f, "{elem}[]"),
        }
    }
}

/// The kind of a user-defined type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Struct,
    Enum,
    Class,
    Interface,
}

impl TypeKind {
    pub fn keyword(self) -> &'static str {
        match self {
            TypeKind::Struct => "struct",
            TypeKind::Enum => "enum",
            TypeKind::Class => "class",
            TypeKind::Interface => "interface",
        }
    }

    /// Classes and interfaces are reference types: copies share the
    /// object, and the value may be `null`. Structs and enums are values.
    pub fn is_reference(self) -> bool {
        matches!(self, TypeKind::Class | TypeKind::Interface)
    }
}

/// Who may use a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Access {
    #[default]
    Public,
    /// The type and the types derived from it.
    Protected,
    /// The type only (for top-level declarations: the module only).
    Private,
}

impl Access {
    pub fn keyword(self) -> &'static str {
        match self {
            Access::Public => "public",
            Access::Protected => "protected",
            Access::Private => "private",
        }
    }
}

/// How a method takes part in inheritance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Virt {
    #[default]
    None,
    Virtual,
    Override,
    Abstract,
}

/// A field of a user-defined type (an auto-property is a field too).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub access: Access,
    /// Who may assign the field; `None` for a read-only field (assigned
    /// only by its initializer and `init`).
    pub set: Option<Access>,
    pub is_static: bool,
    /// Whether the declaration gives a value (`x: int = 1`).
    pub has_value: bool,
    pub span: Span,
}

/// A computed property (`Area: double { get { ... } }`).
#[derive(Debug, Clone, PartialEq)]
pub struct PropInfo {
    pub name: String,
    pub ty: Ty,
    pub access: Access,
    pub get: Option<Access>,
    pub set: Option<Access>,
    pub is_static: bool,
    pub virt: Virt,
    pub span: Span,
}

/// A method.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo {
    pub sig: Rc<Sig>,
    pub access: Access,
    pub is_static: bool,
    pub virt: Virt,
    pub span: Span,
}

/// A case of an enum: `Red` or `Circle(radius: double)`.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    pub span: Span,
}

/// The members of a user-defined type, known once the checker has
/// resolved its declaration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeBody {
    pub base: Option<TypeRef>,
    pub interfaces: Vec<TypeRef>,
    /// Instance fields declared by this type (not inherited ones).
    pub fields: Vec<FieldInfo>,
    pub statics: Vec<FieldInfo>,
    pub props: Vec<PropInfo>,
    pub methods: Vec<MethodInfo>,
    pub variants: Vec<VariantInfo>,
    /// The parameters of the constructor (`init`, or one parameter per
    /// field without a value).
    pub ctor: Vec<(String, Ty)>,
    /// Whether the constructor is declared with `init`.
    pub explicit_ctor: bool,
    pub ctor_access: Access,
}

/// A user-defined type. Its body is set once, when the checker resolves
/// the declaration.
#[derive(Debug)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub is_abstract: bool,
    pub span: Span,
    id: usize,
    body: OnceCell<TypeBody>,
}

impl PartialEq for TypeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

static NEXT_TYPE_ID: AtomicUsize = AtomicUsize::new(1);

impl TypeInfo {
    pub fn new(name: &str, kind: TypeKind, is_abstract: bool, span: Span) -> Rc<TypeInfo> {
        Rc::new(TypeInfo {
            name: name.to_string(),
            kind,
            is_abstract,
            span,
            id: NEXT_TYPE_ID.fetch_add(1, Ordering::Relaxed),
            body: OnceCell::new(),
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    /// Sets the members; only the first call has an effect.
    pub fn set_body(&self, body: TypeBody) {
        let _ = self.body.set(body);
    }

    pub fn body(&self) -> Option<&TypeBody> {
        self.body.get()
    }

    pub fn reference(self: &Rc<Self>) -> TypeRef {
        TypeRef {
            name: Rc::from(self.name.as_str()),
            kind: self.kind,
            id: self.id,
            info: Rc::downgrade(self),
        }
    }
}

/// A reference to a user-defined type, as used inside [`Ty`]. It does not
/// keep the type alive (types may refer to themselves); the declaration in
/// the syntax tree owns the [`TypeInfo`].
#[derive(Clone)]
pub struct TypeRef {
    pub name: Rc<str>,
    pub kind: TypeKind,
    id: usize,
    info: Weak<TypeInfo>,
}

impl PartialEq for TypeRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeRef {}

impl std::hash::Hash for TypeRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl fmt::Debug for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.name, self.id)
    }
}

/// A member of a user-defined type.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberInfo {
    Field(FieldInfo),
    Prop(PropInfo),
    Method(MethodInfo),
    Variant(VariantInfo),
}

/// Inheritance chains longer than this are treated as cycles.
const MAX_DEPTH: usize = 64;

impl TypeRef {
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn info(&self) -> Option<Rc<TypeInfo>> {
        self.info.upgrade()
    }

    /// Runs `f` on the members, if they are known.
    pub fn with_body<R>(&self, f: impl FnOnce(&TypeBody) -> R) -> Option<R> {
        let info = self.info.upgrade()?;
        info.body().map(f)
    }

    pub fn is_abstract(&self) -> bool {
        self.info().is_some_and(|i| i.is_abstract)
    }

    pub fn base(&self) -> Option<TypeRef> {
        self.with_body(|b| b.base.clone()).flatten()
    }

    /// The base class and the implemented (or extended) interfaces.
    pub fn parents(&self) -> Vec<TypeRef> {
        self.with_body(|b| {
            let mut v: Vec<TypeRef> = b.base.iter().cloned().collect();
            v.extend(b.interfaces.iter().cloned());
            v
        })
        .unwrap_or_default()
    }

    /// This type and its base classes, nearest first.
    pub fn chain(&self) -> Vec<TypeRef> {
        let mut out = vec![self.clone()];
        let mut cur = self.base();
        while let Some(t) = cur {
            if out.len() >= MAX_DEPTH || out.contains(&t) {
                break;
            }
            cur = t.base();
            out.push(t);
        }
        out
    }

    /// Whether a value of this type can be used as `other`: the same
    /// type, a base class, or an implemented interface.
    pub fn is_subtype_of(&self, other: &TypeRef) -> bool {
        let mut seen = Vec::new();
        self.subtype(other, &mut seen)
    }

    fn subtype(&self, other: &TypeRef, seen: &mut Vec<usize>) -> bool {
        if self == other {
            return true;
        }
        if seen.contains(&self.id) || seen.len() >= MAX_DEPTH {
            return false;
        }
        seen.push(self.id);
        self.parents().iter().any(|p| p.subtype(other, seen))
    }

    /// Every instance field, inherited ones first.
    pub fn all_fields(&self) -> Vec<FieldInfo> {
        let mut out = Vec::new();
        for t in self.chain().iter().rev() {
            if let Some(fields) = t.with_body(|b| b.fields.clone()) {
                out.extend(fields);
            }
        }
        out
    }

    /// Finds a member by name in this type, its base classes and the
    /// interfaces it implements. Returns the declaring type too.
    pub fn member(&self, name: &str) -> Option<(TypeRef, MemberInfo)> {
        self.members()
            .into_iter()
            .find(|(_, n, _)| n == name)
            .map(|(t, _, m)| (t, m))
    }

    /// Every member (declaring type, name, member), nearest first; a
    /// member hides those with the same name further up.
    pub fn members(&self) -> Vec<(TypeRef, String, MemberInfo)> {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        self.collect_members(&mut out, &mut seen);
        out
    }

    fn collect_members(&self, out: &mut Vec<(TypeRef, String, MemberInfo)>, seen: &mut Vec<usize>) {
        if seen.contains(&self.id) || seen.len() >= MAX_DEPTH {
            return;
        }
        seen.push(self.id);
        let own = self
            .with_body(|b| {
                let mut v = Vec::new();
                for f in b.fields.iter().chain(&b.statics) {
                    v.push((f.name.clone(), MemberInfo::Field(f.clone())));
                }
                for p in &b.props {
                    v.push((p.name.clone(), MemberInfo::Prop(p.clone())));
                }
                for m in &b.methods {
                    v.push((m.sig.name.clone(), MemberInfo::Method(m.clone())));
                }
                for c in &b.variants {
                    v.push((c.name.clone(), MemberInfo::Variant(c.clone())));
                }
                v
            })
            .unwrap_or_default();
        for (name, m) in own {
            if !out.iter().any(|(_, n, _)| *n == name) {
                out.push((self.clone(), name, m));
            }
        }
        for p in self.parents() {
            p.collect_members(out, seen);
        }
    }

    pub fn variants(&self) -> Vec<VariantInfo> {
        self.with_body(|b| b.variants.clone()).unwrap_or_default()
    }
}

impl MemberInfo {
    pub fn is_static(&self) -> bool {
        match self {
            MemberInfo::Field(f) => f.is_static,
            MemberInfo::Prop(p) => p.is_static,
            MemberInfo::Method(m) => m.is_static,
            MemberInfo::Variant(_) => true,
        }
    }

    pub fn access(&self) -> Access {
        match self {
            MemberInfo::Field(f) => f.access,
            MemberInfo::Prop(p) => p.access,
            MemberInfo::Method(m) => m.access,
            MemberInfo::Variant(_) => Access::Public,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            MemberInfo::Field(f) => f.span,
            MemberInfo::Prop(p) => p.span,
            MemberInfo::Method(m) => m.span,
            MemberInfo::Variant(v) => v.span,
        }
    }
}
