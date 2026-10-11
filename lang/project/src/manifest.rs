//! `belsk2.toml`.
//!
//! ```toml
//! [package]
//! name = "hello"
//! version = "0.1.0"
//! description = "Says hello"
//! authors = ["Ann <ann@example.com>"]
//! license = "MIT"
//!
//! [dependencies]
//! utils = { path = "../utils" }
//! json = { git = "https://github.com/someone/json.belsk2", tag = "v1.0.0" }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use belsk2_syntax::Error;
use serde::{Deserialize, Serialize};

/// The file name of a project manifest.
pub const MANIFEST: &str = "belsk2.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageInfo,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
    /// Set for a WebAssembly component: the WIT world it implements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildInfo>,
}

/// `[build]`: `target` is what `belsk2 build` produces by default
/// (`wasm32` or `wasi0.2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// `[component]`: `world` is a `.wit` file or a directory of them
/// (relative to the project root); `name` picks a world when the package
/// has several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInfo {
    pub world: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// Where a dependency comes from. Exactly one of `path` and `git` is set;
/// `branch`, `tag` and `rev` pick a commit of a git dependency.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// The name a package or dependency is imported under: `-` becomes `_`,
/// so `my-lib` is used as `using my_lib;`.
pub fn module_name(package: &str) -> String {
    package.replace('-', "_")
}

/// Package names: lowercase letters, digits, `-` and `_`, starting with a
/// letter.
pub fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !belsk2_syntax::parser::KEYWORDS.contains(&module_name(name).as_str())
}

fn manifest_error(path: &Path, msg: impl std::fmt::Display) -> Error {
    Error::project(format!("{}: {msg}", path.display()))
}

impl Manifest {
    /// A manifest for `belsk2 new`.
    pub fn new(name: &str) -> Self {
        Manifest {
            package: PackageInfo {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                description: None,
                authors: Vec::new(),
                license: None,
            },
            dependencies: BTreeMap::new(),
            component: None,
            build: None,
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self, Error> {
        let manifest: Manifest =
            toml::from_str(text).map_err(|e| manifest_error(path, e.message()))?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| manifest_error(path, e))?;
        Self::parse(&text, path)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    pub fn version(&self) -> Result<semver::Version, semver::Error> {
        semver::Version::parse(&self.package.version)
    }

    fn validate(&self, path: &Path) -> Result<(), Error> {
        let name = &self.package.name;
        if !valid_package_name(name) {
            return Err(manifest_error(
                path,
                format!(
                    "invalid package name \"{name}\": use lowercase letters, digits, '-' and '_', starting with a letter"
                ),
            ));
        }
        if let Err(e) = self.version() {
            return Err(manifest_error(
                path,
                format!(
                    "invalid version \"{}\": {e} (use semantic versioning, like 0.1.0)",
                    self.package.version
                ),
            ));
        }
        for (dep, spec) in &self.dependencies {
            if !valid_package_name(dep) {
                return Err(manifest_error(
                    path,
                    format!("invalid dependency name \"{dep}\""),
                ));
            }
            let refs = [&spec.branch, &spec.tag, &spec.rev]
                .iter()
                .filter(|r| r.is_some())
                .count();
            match (&spec.path, &spec.git) {
                (Some(_), Some(_)) => {
                    return Err(manifest_error(
                        path,
                        format!("dependency \"{dep}\" has both `path` and `git`; use one"),
                    ))
                }
                (None, None) => {
                    return Err(manifest_error(
                        path,
                        format!(
                            "dependency \"{dep}\" needs `path = \"...\"` or `git = \"...\"` (there is no package registry)"
                        ),
                    ))
                }
                (Some(_), None) if refs > 0 => {
                    return Err(manifest_error(
                        path,
                        format!("dependency \"{dep}\": `branch`, `tag` and `rev` only apply to git dependencies"),
                    ))
                }
                _ if refs > 1 => {
                    return Err(manifest_error(
                        path,
                        format!("dependency \"{dep}\": use only one of `branch`, `tag` and `rev`"),
                    ))
                }
                _ => {}
            }
        }
        Ok(())
    }
}
