//! The WIT world of a component project (`[component]` in `belsk2.toml`).
//!
//! The world says which functions the host provides (imports) and which
//! the program must provide (exports). Names are converted from WIT's
//! kebab-case to Belsk2's snake_case: `draw-sprite` is `draw_sprite`.
//!
//! The types functions use become Belsk2 declarations in a module of
//! their own (see [`World::types_source`]), visible in every module of the
//! project: a `record` is a `struct`, an `enum` an `enum`, and a
//! `variant`, `option<T>` or `result<T, E>` an enum with data
//! (`OptionString { None, Some(value: string) }`).

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use belsk2_syntax::ast::{Program, StmtKind};
use belsk2_syntax::{Error, FloatKind, IntKind, Sig, Ty};
use wit_parser::{
    Function, FunctionKind, Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldId, WorldItem,
    WorldKey,
};

/// A WIT type that Belsk2 supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitTy {
    Bool,
    Int(IntKind),
    Float(FloatKind),
    Char,
    String,
    List(Box<WitTy>),
    /// A record, enum, variant, option or result: a type declared in
    /// Belsk2.
    Named(Rc<WitType>),
}

/// A WIT type that is a Belsk2 declaration.
#[derive(Debug)]
pub struct WitType {
    /// The Belsk2 name: `Point`, `OptionString`.
    pub name: String,
    /// As written in WIT: `point`, `option<string>`.
    pub wit: String,
    pub kind: WitTypeKind,
    pub docs: Option<String>,
    /// The Belsk2 type, once the declarations are checked.
    ty: OnceCell<Ty>,
}

impl PartialEq for WitType {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for WitType {}

#[derive(Debug)]
pub enum WitTypeKind {
    /// A `struct`: fields in order, with their Belsk2 names.
    Record(Vec<(String, WitTy)>),
    /// An `enum` without data: its cases.
    Enum(Vec<String>),
    /// An enum with data (`variant`, `option`, `result`): its cases in
    /// order (the case's index is its discriminant), each with the field
    /// holding its payload, if it has one.
    Variant(Vec<(String, Option<(String, WitTy)>)>),
}

impl WitType {
    /// The Belsk2 type, once the declarations are checked.
    pub fn ty(&self) -> Option<Ty> {
        self.ty.get().cloned()
    }

    /// The Belsk2 declaration of the type.
    fn source(&self) -> String {
        let mut out = String::new();
        doc_lines(self.docs.as_ref(), &mut out);
        match &self.kind {
            WitTypeKind::Record(fields) => {
                out.push_str(&format!("struct {} {{\n", self.name));
                for (name, t) in fields {
                    out.push_str(&format!("    {name}: {};\n", t.belsk2()));
                }
                out.push_str("}\n");
            }
            WitTypeKind::Enum(cases) => {
                out.push_str(&format!("enum {} {{\n    {}\n}}\n", self.name, cases.join(",\n    ")));
            }
            WitTypeKind::Variant(cases) => {
                let cases: Vec<String> = cases
                    .iter()
                    .map(|(name, payload)| match payload {
                        Some((field, t)) => format!("{name}({field}: {})", t.belsk2()),
                        None => name.clone(),
                    })
                    .collect();
                out.push_str(&format!("enum {} {{\n    {}\n}}\n", self.name, cases.join(",\n    ")));
            }
        }
        out
    }

    /// A Belsk2 expression for the default value: zeros, empty strings and
    /// lists, the first case.
    fn default_expr(&self) -> String {
        match &self.kind {
            WitTypeKind::Record(fields) => {
                let args: Vec<String> = fields.iter().map(|(_, t)| t.default_expr()).collect();
                format!("{}({})", self.name, args.join(", "))
            }
            WitTypeKind::Enum(cases) => {
                format!("{}.{}", self.name, cases.first().map_or("", String::as_str))
            }
            WitTypeKind::Variant(cases) => match cases.first() {
                Some((case, Some((_, t)))) => format!("{}.{case}({})", self.name, t.default_expr()),
                Some((case, None)) => format!("{}.{case}", self.name),
                None => self.name.clone(),
            },
        }
    }
}

impl WitTy {
    /// The Belsk2 type of values of this WIT type.
    pub fn ty(&self) -> Ty {
        match self {
            WitTy::Bool => Ty::Bool,
            WitTy::Int(k) => Ty::Int(*k),
            WitTy::Float(k) => Ty::Float(*k),
            WitTy::Char => Ty::Char,
            WitTy::String => Ty::String,
            WitTy::List(elem) => Ty::array(elem.ty()),
            WitTy::Named(t) => t.ty.get().cloned().unwrap_or(Ty::Any),
        }
    }

    /// The Belsk2 type as written in code (known before the declarations
    /// are checked).
    pub fn belsk2(&self) -> String {
        match self {
            WitTy::List(elem) => format!("{}[]", elem.belsk2()),
            WitTy::Named(t) => t.name.clone(),
            other => other.ty().to_string(),
        }
    }

    /// A Belsk2 expression for the default value of the type.
    pub fn default_expr(&self) -> String {
        match self {
            WitTy::Bool => "false".into(),
            WitTy::Int(_) => "0".into(),
            WitTy::Float(_) => "0.0".into(),
            WitTy::Char => "char(32)".into(),
            WitTy::String => "\"\"".into(),
            WitTy::List(_) => "[]".into(),
            WitTy::Named(t) => t.default_expr(),
        }
    }

    /// The type in a generated name: `Int`, `StringList`, `Point`.
    fn title(&self) -> String {
        match self {
            WitTy::List(elem) => format!("{}List", elem.title()),
            WitTy::Named(t) => t.name.clone(),
            other => pascal(&other.ty().to_string()),
        }
    }

    /// The type as written in WIT.
    pub fn wit_name(&self) -> String {
        match self {
            WitTy::Bool => "bool".into(),
            WitTy::Int(k) => match k {
                IntKind::I8 => "s8",
                IntKind::U8 => "u8",
                IntKind::I16 => "s16",
                IntKind::U16 => "u16",
                IntKind::I32 => "s32",
                IntKind::U32 => "u32",
                IntKind::I64 => "s64",
                IntKind::U64 => "u64",
            }
            .into(),
            WitTy::Float(FloatKind::F32) => "f32".into(),
            WitTy::Float(FloatKind::F64) => "f64".into(),
            WitTy::Char => "char".into(),
            WitTy::String => "string".into(),
            WitTy::List(elem) => format!("list<{}>", elem.wit_name()),
            WitTy::Named(t) => t.wit.clone(),
        }
    }
}

/// The types of a world's functions, collected while they are read.
#[derive(Default)]
struct Types {
    by_id: HashMap<TypeId, Rc<WitType>>,
    by_name: HashMap<String, Rc<WitType>>,
    list: Vec<Rc<WitType>>,
}

impl Types {
    /// Registers a new type under `name`, or under a longer name if that
    /// is taken (by a type of another interface).
    fn add(&mut self, id: TypeId, name: String, prefix: &str, wit: String, kind: WitTypeKind, docs: Option<String>) -> WitTy {
        let mut unique = name.clone();
        if self.by_name.contains_key(&unique) {
            unique = format!("{prefix}{name}");
        }
        let mut n = 2;
        while self.by_name.contains_key(&unique) {
            unique = format!("{prefix}{name}{n}");
            n += 1;
        }
        let t = Rc::new(WitType {
            name: unique.clone(),
            wit,
            kind,
            docs,
            ty: OnceCell::new(),
        });
        self.by_id.insert(id, Rc::clone(&t));
        self.by_name.insert(unique, Rc::clone(&t));
        self.list.push(Rc::clone(&t));
        WitTy::Named(t)
    }

    fn resolve_ty(&mut self, resolve: &Resolve, ty: &Type) -> Result<WitTy, String> {
        Ok(match ty {
            Type::Bool => WitTy::Bool,
            Type::U8 => WitTy::Int(IntKind::U8),
            Type::S8 => WitTy::Int(IntKind::I8),
            Type::U16 => WitTy::Int(IntKind::U16),
            Type::S16 => WitTy::Int(IntKind::I16),
            Type::U32 => WitTy::Int(IntKind::U32),
            Type::S32 => WitTy::Int(IntKind::I32),
            Type::U64 => WitTy::Int(IntKind::U64),
            Type::S64 => WitTy::Int(IntKind::I64),
            Type::F32 => WitTy::Float(FloatKind::F32),
            Type::F64 => WitTy::Float(FloatKind::F64),
            Type::Char => WitTy::Char,
            Type::String => WitTy::String,
            Type::ErrorContext => return Err("error-context".into()),
            Type::Id(id) => {
                if let Some(t) = self.by_id.get(id) {
                    return Ok(WitTy::Named(Rc::clone(t)));
                }
                let def = resolve
                    .types
                    .get(*id)
                    .ok_or_else(|| "an unknown type".to_string())?;
                let docs = docs(&def.docs);
                let prefix = match def.owner {
                    TypeOwner::Interface(i) => resolve
                        .interfaces
                        .get(i)
                        .and_then(|i| i.name.as_deref())
                        .map(pascal)
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let named = |kind: WitTypeKind, this: &mut Self| -> Result<WitTy, String> {
                    let wit = def.name.clone().unwrap_or_default();
                    Ok(this.add(*id, pascal(&wit), &prefix, wit, kind, docs.clone()))
                };
                match &def.kind {
                    TypeDefKind::List(elem) => {
                        WitTy::List(Box::new(self.resolve_ty(resolve, elem)?))
                    }
                    TypeDefKind::Type(t) => self.resolve_ty(resolve, t)?,
                    TypeDefKind::Record(r) => {
                        let mut fields = Vec::new();
                        for f in &r.fields {
                            fields.push((snake(&f.name), self.resolve_ty(resolve, &f.ty)?));
                        }
                        named(WitTypeKind::Record(fields), self)?
                    }
                    TypeDefKind::Enum(e) => named(
                        WitTypeKind::Enum(e.cases.iter().map(|c| pascal(&c.name)).collect()),
                        self,
                    )?,
                    TypeDefKind::Variant(v) => {
                        let mut cases = Vec::new();
                        for c in &v.cases {
                            let payload = match &c.ty {
                                Some(t) => Some(("value".to_string(), self.resolve_ty(resolve, t)?)),
                                None => None,
                            };
                            cases.push((pascal(&c.name), payload));
                        }
                        named(WitTypeKind::Variant(cases), self)?
                    }
                    TypeDefKind::Option(t) => {
                        let inner = self.resolve_ty(resolve, t)?;
                        let name = format!("Option{}", inner.title());
                        let wit = format!("option<{}>", inner.wit_name());
                        let kind = WitTypeKind::Variant(vec![
                            ("None".to_string(), None),
                            ("Some".to_string(), Some(("value".to_string(), inner))),
                        ]);
                        self.anonymous(*id, name, wit, kind)
                    }
                    TypeDefKind::Result(r) => {
                        let ok = r.ok.as_ref().map(|t| self.resolve_ty(resolve, t)).transpose()?;
                        let err = r.err.as_ref().map(|t| self.resolve_ty(resolve, t)).transpose()?;
                        let name = format!(
                            "Result{}{}",
                            ok.as_ref().map(WitTy::title).unwrap_or_default(),
                            err.as_ref().map(|e| format!("Or{}", e.title())).unwrap_or_default()
                        );
                        let part = |t: &Option<WitTy>| t.as_ref().map_or("_".to_string(), WitTy::wit_name);
                        let wit = match (&ok, &err) {
                            (None, None) => "result".to_string(),
                            (_, None) => format!("result<{}>", part(&ok)),
                            _ => format!("result<{}, {}>", part(&ok), part(&err)),
                        };
                        let kind = WitTypeKind::Variant(vec![
                            ("Ok".to_string(), ok.map(|t| ("value".to_string(), t))),
                            ("Err".to_string(), err.map(|t| ("error".to_string(), t))),
                        ]);
                        self.anonymous(*id, name, wit, kind)
                    }
                    other => {
                        let kind = match other {
                            TypeDefKind::Resource | TypeDefKind::Handle(_) => "resource",
                            TypeDefKind::Flags(_) => "flags",
                            TypeDefKind::Tuple(_) => "tuple",
                            TypeDefKind::Map(..) => "map",
                            TypeDefKind::FixedLengthList(..) => "fixed-length list",
                            TypeDefKind::Future(_) => "future",
                            TypeDefKind::Stream(_) => "stream",
                            _ => "this type",
                        };
                        return Err(match &def.name {
                            Some(name) => format!("{kind} '{name}'"),
                            None => format!("a {kind}"),
                        });
                    }
                }
            }
        })
    }

    /// An `option` or `result`: one declaration for every use of the same
    /// type.
    fn anonymous(&mut self, id: TypeId, name: String, wit: String, kind: WitTypeKind) -> WitTy {
        if let Some(t) = self.by_name.get(&name).filter(|t| t.wit == wit) {
            let t = Rc::clone(t);
            self.by_id.insert(id, Rc::clone(&t));
            return WitTy::Named(t);
        }
        self.add(id, name, "Wit", wit, kind, None)
    }
}

/// `point` becomes `Point`, `error-code` becomes `ErrorCode`.
pub fn pascal(name: &str) -> String {
    name.split(['-', '_', ' '])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect()
}

/// An interface imported or exported by the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitInterface {
    /// The name its functions are used under in Belsk2: `using graphics;`.
    pub module: String,
    /// Its full WIT name, e.g. `my:engine/graphics@1.0.0` (also the name
    /// of the core module the imports come from).
    pub key: String,
    pub docs: Option<String>,
}

/// A function of the world.
#[derive(Debug, Clone)]
pub struct WitFunc {
    /// The name in WIT: `draw-sprite`.
    pub wit_name: String,
    /// The name in Belsk2: `draw_sprite`.
    pub name: String,
    /// The interface it belongs to; `None` for functions of the world
    /// itself.
    pub interface: Option<WitInterface>,
    pub params: Vec<(String, WitTy)>,
    pub result: Option<WitTy>,
    /// Why it cannot be used yet (a WIT type Belsk2 has no equivalent
    /// for).
    pub unsupported: Option<String>,
    pub docs: Option<String>,
}

impl WitFunc {
    /// The Belsk2 type of the function, for the checker. Functions with
    /// unsupported types are left untyped; using them is a compile error.
    pub fn ty(&self) -> Ty {
        if self.unsupported.is_some() {
            return Ty::Fn(None);
        }
        Ty::Fn(Some(Rc::new(Sig {
            name: self.name.clone(),
            type_params: Vec::new(),
            params: self.params.iter().map(|(_, t)| t.ty()).collect(),
            ret: self.result.as_ref().map_or(Ty::Null, WitTy::ty),
        })))
    }

    /// The signature as Belsk2 code: `fn draw_sprite(name: string): bool`.
    pub fn signature(&self) -> String {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|(n, t)| format!("{}: {}", snake(n), t.belsk2()))
            .collect();
        let ret = self
            .result
            .as_ref()
            .map(|t| format!(": {}", t.belsk2()))
            .unwrap_or_default();
        format!("fn {}({}){ret}", self.name, params.join(", "))
    }
}

/// A loaded WIT world.
#[derive(Debug)]
pub struct World {
    pub resolve: Resolve,
    pub id: WorldId,
    /// The `.wit` file or directory named in `belsk2.toml`.
    pub path: PathBuf,
    pub imports: Vec<WitFunc>,
    pub exports: Vec<WitFunc>,
    /// Interfaces the world imports and exports.
    pub import_interfaces: Vec<WitInterface>,
    pub export_interfaces: Vec<WitInterface>,
    /// The records, enums, variants, options and results the functions
    /// use.
    pub types: Vec<Rc<WitType>>,
}

impl World {
    /// The types as Belsk2 declarations; `None` if there are none.
    pub fn types_source(&self) -> Option<String> {
        if self.types.is_empty() {
            return None;
        }
        let decls: Vec<String> = self.types.iter().map(|t| t.source()).collect();
        Some(format!(
            "// The types of the WIT world, as Belsk2 declarations.\n\n{}",
            decls.join("\n")
        ))
    }

    /// Connects the types to their declarations, once those are checked.
    pub fn bind_types(&self, program: &Program) {
        for stmt in &program.stmts {
            if let StmtKind::Type(d) = &stmt.kind {
                if let (Some(info), Some(t)) = (&d.info, self.types.iter().find(|t| t.name == d.name)) {
                    let _ = t.ty.set(Ty::Named(info.reference()));
                }
            }
        }
    }

    /// Functions imported directly by the world (not in an interface):
    /// visible in every module of the project.
    pub fn root_imports(&self) -> impl Iterator<Item = &WitFunc> {
        self.imports.iter().filter(|f| f.interface.is_none())
    }

    /// The functions of an imported interface, by its Belsk2 name.
    pub fn interface_imports<'a>(&'a self, module: &'a str) -> impl Iterator<Item = &'a WitFunc> {
        self.imports
            .iter()
            .filter(move |f| f.interface.as_ref().is_some_and(|i| i.module == module))
    }
}

/// `draw-sprite` becomes `draw_sprite`.
pub fn snake(name: &str) -> String {
    name.replace('-', "_")
}

fn docs(d: &wit_parser::Docs) -> Option<String> {
    d.contents.clone()
}

fn func(resolve: &Resolve, types: &mut Types, f: &Function, interface: Option<WitInterface>) -> WitFunc {
    let mut unsupported = None;
    if !matches!(f.kind, FunctionKind::Freestanding) {
        unsupported = Some("it is a resource method or async function".to_string());
    }
    let mut params = Vec::new();
    for p in &f.params {
        match types.resolve_ty(resolve, &p.ty) {
            Ok(t) => params.push((p.name.clone(), t)),
            Err(e) => unsupported = unsupported.or(Some(format!("it uses {e}"))),
        }
    }
    let result = match f.result.as_ref().map(|t| types.resolve_ty(resolve, t)) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            unsupported = unsupported.or(Some(format!("it uses {e}")));
            None
        }
        None => None,
    };
    WitFunc {
        wit_name: f.name.clone(),
        name: snake(&f.name),
        interface,
        params,
        result,
        unsupported,
        docs: docs(&f.docs),
    }
}

fn items<'a>(
    resolve: &Resolve,
    types: &mut Types,
    items: impl IntoIterator<Item = (&'a WorldKey, &'a WorldItem)>,
    funcs: &mut Vec<WitFunc>,
    interfaces: &mut Vec<WitInterface>,
) {
    for (key, item) in items {
        match item {
            WorldItem::Function(f) => funcs.push(func(resolve, types, f, None)),
            WorldItem::Interface { id, .. } => {
                let Some(iface) = resolve.interfaces.get(*id) else {
                    continue;
                };
                let short = match key {
                    WorldKey::Name(n) => n.clone(),
                    WorldKey::Interface(_) => iface.name.clone().unwrap_or_default(),
                };
                let info = WitInterface {
                    module: snake(&short),
                    key: resolve.name_world_key(key),
                    docs: docs(&iface.docs),
                };
                for f in iface.functions.values() {
                    funcs.push(func(resolve, types, f, Some(info.clone())));
                }
                interfaces.push(info);
            }
            WorldItem::Type { .. } => {}
        }
    }
}

/// Loads the world `world_name` (or the only world) from `path`, a `.wit`
/// file or a directory. Packages in `deps/` next to it are loaded too.
pub fn load_world(path: &Path, world_name: Option<&str>) -> Result<World, Error> {
    let fail =
        |e: &dyn std::fmt::Display| Error::project(format!("{}: {e:#}", crate::display_path(path)));
    let mut resolve = Resolve::default();
    let pkg = if path.is_dir() {
        resolve.push_dir(path).map_err(|e| fail(&e))?.0
    } else {
        // A file inside a WIT directory: load the whole directory so its
        // `deps/` are found.
        match path.parent().filter(|d| d.join("deps").is_dir()) {
            Some(dir) => resolve.push_dir(dir).map_err(|e| fail(&e))?.0,
            None => resolve.push_file(path).map_err(|e| fail(&e))?,
        }
    };
    let id = resolve
        .select_world(&[pkg], world_name)
        .map_err(|e| fail(&e))?;
    let world = resolve
        .worlds
        .get(id)
        .ok_or_else(|| Error::project("the WIT world could not be found"))?;
    let (mut imports, mut exports) = (Vec::new(), Vec::new());
    let (mut import_interfaces, mut export_interfaces) = (Vec::new(), Vec::new());
    let mut types = Types::default();
    items(
        &resolve,
        &mut types,
        &world.imports,
        &mut imports,
        &mut import_interfaces,
    );
    items(
        &resolve,
        &mut types,
        &world.exports,
        &mut exports,
        &mut export_interfaces,
    );
    Ok(World {
        id,
        path: path.to_path_buf(),
        imports,
        exports,
        import_interfaces,
        export_interfaces,
        types: types.list,
        resolve,
    })
}

fn doc_lines(docs: Option<&String>, out: &mut String) {
    for line in docs.into_iter().flat_map(|d| d.lines()) {
        let line = line.trim_end();
        if line.is_empty() {
            out.push_str("///\n");
        } else {
            out.push_str(&format!("/// {line}\n"));
        }
    }
}

/// The source file (relative to the project root) where an export is
/// implemented: `src/<interface>.belsk2`, or `src/<root_module>` for
/// functions of the world itself.
pub fn export_file(f: &WitFunc, root_module: &str) -> PathBuf {
    let mut path = PathBuf::from("src");
    match &f.interface {
        Some(i) => {
            for part in i.module.split('.') {
                path.push(part);
            }
            path.set_extension("belsk2");
        }
        None => path.push(root_module),
    }
    path
}

/// A function stub for an export, returning a default value.
pub fn stub(f: &WitFunc) -> String {
    let mut out = String::new();
    doc_lines(f.docs.as_ref(), &mut out);
    out.push_str(&format!("{} {{\n", f.signature()));
    out.push_str("    // TODO: implement\n");
    if let Some(t) = &f.result {
        out.push_str(&format!("    return {};\n", t.default_expr()));
    }
    out.push_str("}\n");
    out
}

impl World {
    /// The world as Belsk2 declarations, for reading (`belsk2 bindgen`).
    pub fn describe(&self) -> String {
        let name = self
            .resolve
            .worlds
            .get(self.id)
            .map(|w| match w.package {
                Some(pkg) => self.resolve.id_of_name(pkg, &w.name),
                None => w.name.clone(),
            })
            .unwrap_or_default();
        let mut out = format!(
            "// The WIT world {name} ({}), as Belsk2 sees it.\n",
            crate::display_path(&self.path)
        );
        let sig = |f: &WitFunc, out: &mut String| {
            doc_lines(f.docs.as_ref(), out);
            match &f.unsupported {
                Some(why) => {
                    out.push_str(&format!("// `{}` cannot be used yet: {why}\n", f.wit_name))
                }
                None => out.push_str(&format!("{};\n", f.signature())),
            }
        };
        if let Some(types) = self.types_source() {
            out.push_str("\n// Types, visible in every module:\n\n");
            out.push_str(types.lines().skip(2).collect::<Vec<_>>().join("\n").as_str());
            out.push('\n');
        }
        let roots: Vec<&WitFunc> = self.root_imports().collect();
        if !roots.is_empty() {
            out.push_str("\n// Imports, callable from every module:\n\n");
            for f in roots {
                sig(f, &mut out);
            }
        }
        for iface in &self.import_interfaces {
            out.push_str(&format!("\n// Imported interface {}:\n", iface.key));
            doc_lines(iface.docs.as_ref(), &mut out);
            out.push_str(&format!("using {};\n\n", iface.module));
            for f in self.interface_imports(&iface.module) {
                sig(f, &mut out);
            }
        }
        if !self.exports.is_empty() {
            out.push_str("\n// Exports: implement these (`belsk2 bindgen --stubs` adds them).\n");
            let mut last = None;
            for f in &self.exports {
                let file = export_file(f, "main.belsk2 or src/lib.belsk2");
                if last.as_ref() != Some(&file) {
                    let shown = file.to_string_lossy().replace('\\', "/");
                    out.push_str(&format!("\n// in {shown}\n"));
                    last = Some(file);
                }
                sig(f, &mut out);
            }
        }
        out
    }
}
