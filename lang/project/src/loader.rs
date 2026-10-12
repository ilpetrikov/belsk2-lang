use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use belsk2_syntax::ast::{Program, StmtKind};
use belsk2_syntax::{Error, FileId, SourceMap, Span};

use crate::git::{self, default_home, normalize_url, GitCache, GitRef};
use crate::lockfile::{LockedPackage, Lockfile};
use crate::manifest::{module_name, Dependency, Manifest, MANIFEST};
use crate::wit::{WitInterface, World};

pub type ModuleId = usize;

#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    /// Directory holding the package's modules (normally `root/src`).
    pub src: PathBuf,
    /// Dependencies by the name they are imported under, as package indices.
    pub deps: BTreeMap<String, usize>,
}

#[derive(Debug)]
pub struct Module {
    pub package: usize,
    /// Dotted name, e.g. `utils`, `net.http`, `json` (a dependency's root).
    pub name: String,
    pub path: PathBuf,
    /// The module's file in [`Loaded::sources`].
    pub file: FileId,
    pub program: Program,
    /// The modules this one imports, in the order of its `using` statements,
    /// with the span of each `using`.
    pub uses: Vec<(ModuleId, Span)>,
    /// Set for an interface imported by the WIT world (`using graphics;`):
    /// the module has no source; its functions come from the host.
    pub interface: Option<WitInterface>,
}

/// A loaded project: packages, parsed modules and their sources.
#[derive(Debug)]
pub struct Loaded {
    pub packages: Vec<Package>,
    pub modules: Vec<Module>,
    pub sources: SourceMap,
    /// The module a program starts from (`main.belsk2`, or the file given
    /// to [`load_file`]).
    pub entry: Option<ModuleId>,
    /// Every module, each after the modules it uses.
    pub order: Vec<ModuleId>,
    /// The WIT world of a component project (`[component]`).
    pub world: Option<Rc<World>>,
    /// The module holding the types of the WIT world as Belsk2
    /// declarations; every module of the project uses it.
    pub wit_types: Option<ModuleId>,
}

impl Loaded {
    pub fn root_package(&self) -> Option<&Package> {
        self.packages.first()
    }

    /// `order`, restricted to the modules the entry module needs.
    pub fn run_order(&self) -> Vec<ModuleId> {
        let Some(entry) = self.entry else {
            return Vec::new();
        };
        let mut needed = vec![false; self.modules.len()];
        let mut stack = vec![entry];
        while let Some(m) = stack.pop() {
            if let Some(flag) = needed.get_mut(m) {
                if !*flag {
                    *flag = true;
                    if let Some(module) = self.modules.get(m) {
                        stack.extend(module.uses.iter().map(|(u, _)| *u));
                    }
                }
            }
        }
        self.order
            .iter()
            .copied()
            .filter(|m| needed.get(*m).copied().unwrap_or(false))
            .collect()
    }
}

/// How dependencies are fetched.
#[derive(Debug, Clone)]
pub struct Options {
    /// Where downloaded dependencies are cached (see [`default_home`]).
    pub home: PathBuf,
    /// Never use the network; only what is already downloaded.
    pub offline: bool,
    /// Fail instead of changing `belsk2.lock`.
    pub locked: bool,
    /// Ignore `belsk2.lock` and move git dependencies to the newest commit
    /// of their branch or tag.
    pub update: bool,
    /// File contents to use instead of what is on disk (unsaved editor
    /// buffers), keyed by path.
    pub overlays: HashMap<PathBuf, String>,
    /// Whether `belsk2.lock` may be written. Editors turn this off.
    pub write_lock: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            home: default_home(),
            offline: false,
            locked: false,
            update: false,
            overlays: HashMap::new(),
            write_lock: true,
        }
    }
}

/// What to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The entry point and what it uses.
    Run,
    /// Every module of the project (and what they use), for checking.
    All,
}

struct Loader {
    git: GitCache,
    update: bool,
    old_lock: Lockfile,
    new_lock: Vec<LockedPackage>,
    /// Dependencies that could not be loaded (package, import name), so
    /// their `using`s do not cause a second error.
    failed_deps: Vec<(usize, String)>,
    /// Package name -> (git URL, commit) it was loaded from.
    git_sources: HashMap<String, (String, String)>,
    packages: Vec<Package>,
    package_by_root: HashMap<PathBuf, usize>,
    modules: Vec<Module>,
    module_by_path: HashMap<PathBuf, ModuleId>,
    world: Option<Rc<World>>,
    sources: SourceMap,
    errors: Vec<Error>,
    /// Canonical path -> unsaved contents.
    overlays: HashMap<PathBuf, String>,
    write_lock: bool,
}

/// A path as shown in messages: relative to the current directory when
/// it is inside it, with `/` separators.
pub fn display_path(path: &Path) -> String {
    let full = canonical(path);
    let shown = std::env::current_dir()
        .ok()
        .map(|cwd| canonical(&cwd))
        .and_then(|cwd| full.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or(full);
    let s = shown.display().to_string().replace('\\', "/");
    // Windows canonical paths start with `\\?\`.
    s.strip_prefix("//?/").map(str::to_string).unwrap_or(s)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The first characters of a commit hash, for messages.
fn short(commit: &str) -> &str {
    commit.get(..8).unwrap_or(commit)
}

/// Path of the file for module `parts` inside `src`.
fn module_file(src: &Path, parts: &[String]) -> PathBuf {
    let mut path = src.to_path_buf();
    for p in parts {
        path.push(p);
    }
    path.set_extension("belsk2");
    path
}

/// Loads the project whose `belsk2.toml` is in `root`, fetching git
/// dependencies as needed and keeping `belsk2.lock` up to date.
pub fn load(root: &Path, mode: Mode) -> Result<Loaded, (Vec<Error>, SourceMap)> {
    load_with(root, mode, &Options::default())
}

pub fn load_with(
    root: &Path,
    mode: Mode,
    options: &Options,
) -> Result<Loaded, (Vec<Error>, SourceMap)> {
    let (loader, entry) = start_project(root, mode, options);
    loader.finish(entry)
}

/// Like [`load_with`], but keeps going after errors: returns whatever
/// could be loaded together with every error. Used by editors.
pub fn load_lenient(root: &Path, mode: Mode, options: &Options) -> (Loaded, Vec<Error>) {
    let (loader, entry) = start_project(root, mode, options);
    loader.finish_lenient(entry)
}

fn start_project(root: &Path, mode: Mode, options: &Options) -> (Loader, Option<ModuleId>) {
    let mut loader = Loader::new(options);
    match Lockfile::load(root) {
        Ok(lock) => loader.old_lock = lock.resolved(root),
        Err(e) => loader.errors.push(e),
    }
    let pkg = loader.load_package(root);
    if loader.errors.is_empty() {
        let new_lock = Lockfile {
            version: 1,
            packages: std::mem::take(&mut loader.new_lock),
        };
        if !new_lock.same_as(&loader.old_lock) {
            if options.locked {
                loader.errors.push(Error::project(format!(
                    "{} needs to be updated, but --locked was given",
                    root.join(crate::lockfile::LOCKFILE).display()
                )));
            } else if !loader.write_lock {
            } else if let Err(e) = new_lock.portable(root).save(root) {
                loader.errors.push(e);
            }
        }
    }
    let Some(pkg) = pkg else {
        return (loader, None);
    };
    let src = loader
        .packages
        .get(pkg)
        .map(|p| p.src.clone())
        .unwrap_or_default();
    let main = src.join("main.belsk2");
    let lib = src.join("lib.belsk2");

    let mut entry = None;
    if main.is_file() {
        entry = loader.load_module(pkg, "main".to_string(), &main);
    }
    match mode {
        Mode::Run if entry.is_none() => {
            let msg = if lib.is_file() {
                format!(
                    "this is a library (no {}); libraries are used by other projects, not run",
                    main.display()
                )
            } else {
                format!("{} not found", main.display())
            };
            loader.errors.push(Error::project(msg));
        }
        Mode::Run | Mode::All => {
            let mut files = Vec::new();
            collect_files(&src, &mut files);
            files.sort();
            for file in files {
                // A run needs what the entry uses, and every file with a
                // part of a `partial` type (they join into one type).
                if mode == Mode::Run && !has_partial(&loader, &file) {
                    continue;
                }
                let name = file
                    .strip_prefix(&src)
                    .unwrap_or(&file)
                    .with_extension("")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(".");
                loader.load_module(pkg, name, &file);
            }
            if mode == Mode::Run {
                return (loader, entry);
            }
            if loader.modules.is_empty() {
                loader.errors.push(Error::project(format!(
                    "no .belsk2 files in {}",
                    src.display()
                )));
            }
        }
    }
    (loader, entry)
}

/// Whether a file (or its unsaved editor contents) may declare part of a
/// `partial` type.
fn has_partial(loader: &Loader, file: &Path) -> bool {
    let text = match loader.overlays.get(&canonical(file)) {
        Some(t) => t.clone(),
        None => std::fs::read_to_string(file).unwrap_or_default(),
    };
    text.contains("partial")
}

/// Loads a single file as a program. Modules it uses are looked up next to
/// it (`using utils;` reads `utils.belsk2` from the same directory).
pub fn load_file(file: &Path) -> Result<Loaded, (Vec<Error>, SourceMap)> {
    let (loader, entry) = start_file(file, &Options::default());
    loader.finish(entry)
}

/// Like [`load_file`], but keeps going after errors (see [`load_lenient`]).
pub fn load_file_lenient(file: &Path, options: &Options) -> (Loaded, Vec<Error>) {
    let (loader, entry) = start_file(file, options);
    loader.finish_lenient(entry)
}

fn start_file(file: &Path, options: &Options) -> (Loader, Option<ModuleId>) {
    let mut loader = Loader::new(options);
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string());
    loader.packages.push(Package {
        name: name.clone(),
        version: "0.0.0".to_string(),
        root: dir.clone(),
        src: dir,
        deps: BTreeMap::new(),
    });
    let entry = loader.load_module(0, name, file);
    (loader, entry)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "belsk2") {
            out.push(path);
        }
    }
}

impl Loader {
    fn new(options: &Options) -> Self {
        Loader {
            git: GitCache::new(&options.home, options.offline),
            update: options.update,
            old_lock: Lockfile::default(),
            new_lock: Vec::new(),
            git_sources: HashMap::new(),
            failed_deps: Vec::new(),
            packages: Vec::new(),
            package_by_root: HashMap::new(),
            modules: Vec::new(),
            module_by_path: HashMap::new(),
            world: None,
            sources: SourceMap::new(),
            errors: Vec::new(),
            overlays: options
                .overlays
                .iter()
                .map(|(path, text)| (canonical(path), text.clone()))
                .collect(),
            write_lock: options.write_lock,
        }
    }

    fn finish_lenient(mut self, entry: Option<ModuleId>) -> (Loaded, Vec<Error>) {
        let wit_types = self.add_wit_types();
        let order = self.order();
        let errors = std::mem::take(&mut self.errors);
        let loaded = Loaded {
            packages: self.packages,
            modules: self.modules,
            sources: self.sources,
            entry,
            order,
            world: self.world,
            wit_types,
        };
        (loaded, errors)
    }

    fn finish(mut self, entry: Option<ModuleId>) -> Result<Loaded, (Vec<Error>, SourceMap)> {
        let wit_types = self.add_wit_types();
        let order = self.order();
        if !self.errors.is_empty() {
            return Err((self.errors, self.sources));
        }
        Ok(Loaded {
            packages: self.packages,
            modules: self.modules,
            sources: self.sources,
            entry,
            order,
            world: self.world,
            wit_types,
        })
    }

    /// Adds the types of the WIT world as a module that every module of
    /// the project uses.
    fn add_wit_types(&mut self) -> Option<ModuleId> {
        let world = self.world.clone()?;
        let source = world.types_source()?;
        let file = self.sources.add(
            format!("<wit types of {}>", display_path(&world.path)),
            source.as_str(),
        );
        let program = match belsk2_syntax::parse_file(&source, file) {
            Ok(p) => p,
            Err(e) => {
                self.errors.push(e);
                return None;
            }
        };
        let id = self.modules.len();
        for m in self.modules.iter_mut().filter(|m| m.package == 0) {
            m.uses.push((id, Span::in_file(m.file, 1, 1)));
        }
        self.modules.push(Module {
            package: 0,
            name: "wit".to_string(),
            path: world.path.clone(),
            file,
            program,
            uses: Vec::new(),
            interface: None,
        });
        Some(id)
    }

    /// Reads a package (and, recursively, its dependencies).
    fn load_package(&mut self, root: &Path) -> Option<usize> {
        let key = canonical(root);
        if let Some(&idx) = self.package_by_root.get(&key) {
            return Some(idx);
        }
        let manifest = match Manifest::load(&root.join(MANIFEST)) {
            Ok(m) => m,
            Err(e) => {
                self.errors.push(e);
                return None;
            }
        };
        let idx = self.packages.len();
        self.package_by_root.insert(key, idx);
        if let (0, Some(component)) = (idx, &manifest.component) {
            let path = root.join(&component.world);
            match crate::wit::load_world(&path, component.name.as_deref()) {
                Ok(world) => self.world = Some(Rc::new(world)),
                Err(e) => self.errors.push(e),
            }
        }
        self.packages.push(Package {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
            root: root.to_path_buf(),
            src: root.join("src"),
            deps: BTreeMap::new(),
        });
        for (name, spec) in &manifest.dependencies {
            let Some((dep_root, locked)) = self.locate_dependency(root, name, spec) else {
                self.failed_deps.push((idx, module_name(name)));
                continue;
            };
            if let Some(dep) = self.load_package(&dep_root) {
                if let (Some(mut entry), Some(p)) = (locked, self.packages.get(dep)) {
                    entry.version = p.version.clone();
                    if !self.new_lock.contains(&entry) {
                        self.new_lock.push(entry);
                    }
                }
                let actual = self
                    .packages
                    .get(dep)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                if actual != *name {
                    self.errors.push(Error::project(format!(
                        "{}: dependency \"{name}\" points to package \"{actual}\"",
                        root.join(MANIFEST).display()
                    )));
                }
                if let Some(p) = self.packages.get_mut(idx) {
                    p.deps.insert(module_name(name), dep);
                }
            }
        }
        Some(idx)
    }

    /// The directory a dependency lives in, and for git dependencies the
    /// entry for `belsk2.lock`.
    fn locate_dependency(
        &mut self,
        root: &Path,
        name: &str,
        spec: &Dependency,
    ) -> Option<(PathBuf, Option<LockedPackage>)> {
        if let Some(path) = &spec.path {
            let dir = root.join(path);
            if !dir.join(MANIFEST).is_file() {
                self.errors.push(Error::project(format!(
                    "{}: dependency \"{name}\": no {MANIFEST} in {}",
                    root.join(MANIFEST).display(),
                    dir.display()
                )));
                return None;
            }
            return Some((dir, None));
        }
        let manifest = root.join(MANIFEST);
        let url = normalize_url(spec.git.as_deref().unwrap_or_default(), root);
        let reference = GitRef::of(spec);
        let reference_key = reference.to_lock();
        let locked = if self.update {
            None
        } else {
            self.old_lock
                .find(&url, &reference_key)
                .map(|p| p.commit.clone())
        };
        let commit = match locked {
            Some(c) => c,
            None => match self.git.resolve(&url, &reference) {
                Ok(c) => c,
                Err(msg) => {
                    self.errors.push(git::error(&manifest, name, msg));
                    return None;
                }
            },
        };
        if let Some((other_url, other_commit)) = self.git_sources.get(name) {
            if !git::same_source(other_url, &url) || *other_commit != commit {
                self.errors.push(Error::project(format!(
                    "package \"{name}\" is used from two different sources: {other_url} at {} and {url} at {}",
                    short(other_commit),
                    short(&commit)
                )));
                return None;
            }
        }
        self.git_sources
            .insert(name.to_string(), (url.clone(), commit.clone()));
        let dir = match self.git.checkout(&url, &commit) {
            Ok(d) => d,
            Err(msg) => {
                self.errors.push(git::error(&manifest, name, msg));
                return None;
            }
        };
        if !dir.join(MANIFEST).is_file() {
            self.errors.push(git::error(
                &manifest,
                name,
                format!("{url} has no {MANIFEST} at its root"),
            ));
            return None;
        }
        let entry = LockedPackage {
            name: name.to_string(),
            version: String::new(),
            git: url,
            reference: reference_key,
            commit,
        };
        Some((dir, Some(entry)))
    }

    /// Reads and parses a module, then the modules it uses.
    fn load_module(&mut self, package: usize, name: String, path: &Path) -> Option<ModuleId> {
        let key = canonical(path);
        if let Some(&id) = self.module_by_path.get(&key) {
            return Some(id);
        }
        let read = match self.overlays.get(&key) {
            Some(text) => Ok(text.clone()),
            None => std::fs::read_to_string(path),
        };
        let text = match read {
            Ok(t) => t,
            Err(e) => {
                self.errors
                    .push(Error::io(format!("cannot read {}: {e}", path.display())));
                return None;
            }
        };
        let file = self.sources.add(display_path(path), text.as_str());
        let program = match belsk2_syntax::parse_file(&text, file) {
            Ok(p) => p,
            Err(e) => {
                self.errors.push(e);
                Program { stmts: Vec::new() }
            }
        };
        let id = self.modules.len();
        self.module_by_path.insert(key, id);
        let usings: Vec<(Vec<String>, Span)> = program
            .stmts
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::Using(path) => Some((path.clone(), s.span)),
                _ => None,
            })
            .collect();
        self.modules.push(Module {
            package,
            name,
            path: path.to_path_buf(),
            file,
            program,
            uses: Vec::new(),
            interface: None,
        });

        let mut uses = Vec::new();
        for (parts, span) in usings {
            if let Some(target) = self.resolve_using(package, &parts, span) {
                if uses.iter().any(|(u, _)| *u == target) {
                    self.errors.push(
                        Error::project(format!("module '{}' is already used", parts.join(".")))
                            .at(span),
                    );
                    continue;
                }
                uses.push((target, span));
            }
        }
        if let Some(m) = self.modules.get_mut(id) {
            m.uses = uses;
        }
        Some(id)
    }

    /// Finds the module named by `using parts;` inside `package`.
    fn resolve_using(&mut self, package: usize, parts: &[String], span: Span) -> Option<ModuleId> {
        let pkg = self.packages.get(package)?;
        let (first, rest) = parts.split_first()?;
        if self
            .failed_deps
            .iter()
            .any(|(p, n)| *p == package && n == first)
        {
            return None;
        }
        let dotted = parts.join(".");
        let local = module_file(&pkg.src, parts);
        if let Some(id) = self.interface_module(package, &dotted, &local, span) {
            return id;
        }
        let pkg = self.packages.get(package)?;

        let (target_pkg, file, name) = if let Some(&dep) = pkg.deps.get(first) {
            if local.is_file() {
                self.errors.push(
                    Error::project(format!(
                        "'{dotted}' is ambiguous: it names both the dependency '{first}' and {}",
                        local.display()
                    ))
                    .at(span),
                );
                return None;
            }
            let dep_pkg = self.packages.get(dep)?;
            let file = if rest.is_empty() {
                dep_pkg.src.join("lib.belsk2")
            } else {
                module_file(&dep_pkg.src, rest)
            };
            (dep, file, dotted.clone())
        } else {
            // Modules inside a dependency are named after it: `json.parser`.
            let name = if package != 0 {
                format!("{}.{dotted}", module_name(&pkg.name))
            } else {
                dotted.clone()
            };
            (package, local, name)
        };

        if parts.len() == 1 && first == "main" && target_pkg == package {
            self.errors.push(
                Error::project("the entry module 'main' cannot be used by other modules").at(span),
            );
            return None;
        }
        if !file.is_file() {
            let msg = if target_pkg != package && rest.is_empty() {
                format!(
                    "dependency '{first}' is not a library: {} not found",
                    file.display()
                )
            } else {
                format!(
                    "module '{dotted}' not found: {} does not exist",
                    file.display()
                )
            };
            self.errors.push(Error::project(msg).at(span));
            return None;
        }
        self.load_module(target_pkg, name, &file)
    }

    /// `using graphics;` for an interface the WIT world imports: a module
    /// without source whose functions the host provides. `None` if `name`
    /// is not such an interface.
    fn interface_module(
        &mut self,
        package: usize,
        name: &str,
        local: &Path,
        span: Span,
    ) -> Option<Option<ModuleId>> {
        if package != 0 {
            return None;
        }
        let world = self.world.clone()?;
        let iface = world
            .import_interfaces
            .iter()
            .find(|i| i.module == name)?
            .clone();
        if local.is_file() {
            self.errors.push(
                Error::project(format!(
                    "'{name}' is ambiguous: it names both the WIT interface '{}' and {}",
                    iface.key,
                    local.display()
                ))
                .at(span),
            );
            return Some(None);
        }
        let key = world.path.join(format!("#{name}"));
        if let Some(&id) = self.module_by_path.get(&key) {
            return Some(Some(id));
        }
        let file = self
            .sources
            .add(format!("<wit {}>", iface.key), String::new().as_str());
        let id = self.modules.len();
        self.module_by_path.insert(key, id);
        self.modules.push(Module {
            package,
            name: name.to_string(),
            path: world.path.clone(),
            file,
            program: Program { stmts: Vec::new() },
            uses: Vec::new(),
            interface: Some(iface),
        });
        Some(Some(id))
    }

    /// Orders modules so that each comes after the modules it uses, and
    /// reports `using` cycles.
    fn order(&mut self) -> Vec<ModuleId> {
        #[derive(Clone, Copy, PartialEq)]
        enum State {
            New,
            Active,
            Done,
        }
        let n = self.modules.len();
        let mut state = vec![State::New; n];
        let mut order = Vec::with_capacity(n);
        for start in 0..n {
            if state.get(start) != Some(&State::New) {
                continue;
            }
            // Iterative DFS: (module, index of the next `use` to visit).
            let mut stack: Vec<(ModuleId, usize)> = vec![(start, 0)];
            if let Some(s) = state.get_mut(start) {
                *s = State::Active;
            }
            while let Some(&mut (m, ref mut next)) = stack.last_mut() {
                let uses = self
                    .modules
                    .get(m)
                    .map(|x| x.uses.as_slice())
                    .unwrap_or(&[]);
                if let Some(&(u, span)) = uses.get(*next) {
                    *next += 1;
                    match state.get(u).copied() {
                        Some(State::New) => {
                            if let Some(s) = state.get_mut(u) {
                                *s = State::Active;
                            }
                            stack.push((u, 0));
                        }
                        Some(State::Active) => {
                            let from = stack.iter().position(|(x, _)| *x == u).unwrap_or(0);
                            let mut cycle: Vec<String> = stack
                                .iter()
                                .skip(from)
                                .filter_map(|(x, _)| self.modules.get(*x).map(|m| m.name.clone()))
                                .collect();
                            cycle.extend(self.modules.get(u).map(|m| m.name.clone()));
                            self.errors.push(
                                Error::project(format!(
                                    "modules use each other in a cycle: {}",
                                    cycle.join(" -> ")
                                ))
                                .at(span),
                            );
                        }
                        _ => {}
                    }
                } else {
                    if let Some(s) = state.get_mut(m) {
                        *s = State::Done;
                    }
                    order.push(m);
                    stack.pop();
                }
            }
        }
        order
    }
}
