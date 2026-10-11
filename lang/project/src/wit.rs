//! The WIT world of a component project (`[component]` in `belsk2.toml`).
//!
//! The world says which functions the host provides (imports) and which
//! the program must provide (exports). Names are converted from WIT's
//! kebab-case to Belsk2's snake_case: `draw-sprite` is `draw_sprite`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use belsk2_syntax::{Error, FloatKind, IntKind, Sig, Ty};
use wit_parser::{
    Function, FunctionKind, Resolve, Type, TypeDefKind, WorldId, WorldItem, WorldKey,
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
        }
    }

    fn from_wit(resolve: &Resolve, ty: &Type) -> Result<WitTy, String> {
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
                let def = resolve
                    .types
                    .get(*id)
                    .ok_or_else(|| "an unknown type".to_string())?;
                match &def.kind {
                    TypeDefKind::List(elem) => {
                        WitTy::List(Box::new(WitTy::from_wit(resolve, elem)?))
                    }
                    TypeDefKind::Type(t) => WitTy::from_wit(resolve, t)?,
                    other => {
                        let kind = match other {
                            TypeDefKind::Record(_) => "record",
                            TypeDefKind::Resource | TypeDefKind::Handle(_) => "resource",
                            TypeDefKind::Flags(_) => "flags",
                            TypeDefKind::Tuple(_) => "tuple",
                            TypeDefKind::Variant(_) => "variant",
                            TypeDefKind::Enum(_) => "enum",
                            TypeDefKind::Option(_) => "option",
                            TypeDefKind::Result(_) => "result",
                            TypeDefKind::Map(..) => "map",
                            TypeDefKind::FixedLengthList(..) => "fixed-length list",
                            TypeDefKind::Future(_) => "future",
                            TypeDefKind::Stream(_) => "stream",
                            _ => "this type",
                        };
                        let name = def.name.as_deref().unwrap_or(kind);
                        return Err(format!("{kind} '{name}'"));
                    }
                }
            }
        })
    }
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
            .map(|(n, t)| format!("{}: {}", snake(n), t.ty()))
            .collect();
        let ret = self
            .result
            .as_ref()
            .map(|t| format!(": {}", t.ty()))
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
}

impl World {
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

fn func(resolve: &Resolve, f: &Function, interface: Option<WitInterface>) -> WitFunc {
    let mut unsupported = None;
    if !matches!(f.kind, FunctionKind::Freestanding) {
        unsupported = Some("it is a resource method or async function".to_string());
    }
    let mut params = Vec::new();
    for p in &f.params {
        match WitTy::from_wit(resolve, &p.ty) {
            Ok(t) => params.push((p.name.clone(), t)),
            Err(e) => unsupported = unsupported.or(Some(format!("it uses {e}"))),
        }
    }
    let result = match f.result.as_ref().map(|t| WitTy::from_wit(resolve, t)) {
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
    items: impl IntoIterator<Item = (&'a WorldKey, &'a WorldItem)>,
    funcs: &mut Vec<WitFunc>,
    interfaces: &mut Vec<WitInterface>,
) {
    for (key, item) in items {
        match item {
            WorldItem::Function(f) => funcs.push(func(resolve, f, None)),
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
                    funcs.push(func(resolve, f, Some(info.clone())));
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
    items(
        &resolve,
        &world.imports,
        &mut imports,
        &mut import_interfaces,
    );
    items(
        &resolve,
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
        resolve,
    })
}
