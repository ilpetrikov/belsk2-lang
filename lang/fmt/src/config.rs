//! `belsk2fmt.toml`: formatter settings.

use std::path::{Path, PathBuf};

use belsk2_syntax::Error;
use serde::Deserialize;

/// Names of the config file, in the order they are looked for.
pub const CONFIG_FILES: &[&str] = &["belsk2fmt.toml", ".belsk2fmt.toml"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndentStyle {
    Spaces,
    Tabs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BraceStyle {
    /// `if x {` (K&R, the default)
    SameLine,
    /// `{` on its own line (Allman, as in C#)
    NextLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    /// Keep what the file uses (`\r\n` if it has any, else `\n`).
    Auto,
    Lf,
    Crlf,
}

/// Formatter settings. Every field has a default, so a config file only
/// lists what it changes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub indent_style: IndentStyle,
    /// Spaces per indentation level (also the tab width).
    pub indent_width: usize,
    pub brace_style: BraceStyle,
    /// Most empty lines kept in a row.
    pub max_blank_lines: usize,
    /// Spaces around binary operators (`a + b`). When false, the spacing
    /// in the source is kept.
    pub space_around_operators: bool,
    pub line_ending: LineEnding,
    /// End the file with a newline.
    pub trailing_newline: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            indent_style: IndentStyle::Spaces,
            indent_width: 4,
            brace_style: BraceStyle::SameLine,
            max_blank_lines: 1,
            space_around_operators: true,
            line_ending: LineEnding::Auto,
            trailing_newline: true,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, Error> {
        let config: Config = toml::from_str(text)
            .map_err(|e| Error::project(format!("invalid formatter config: {e}")))?;
        if config.indent_width == 0 || config.indent_width > 16 {
            return Err(Error::project("indent_width must be between 1 and 16"));
        }
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Config, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("cannot read {}: {e}", path.display())))?;
        Config::parse(&text)
            .map_err(|e| Error::project(format!("{}: {}", path.display(), e.message)))
    }

    /// The config file that applies to files in `dir`: the nearest one in
    /// `dir` or a parent directory.
    pub fn find(dir: &Path) -> Option<PathBuf> {
        let mut dir = Some(dir);
        while let Some(d) = dir {
            for name in CONFIG_FILES {
                let path = d.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
            dir = d.parent();
        }
        None
    }

    /// The settings for `file`: from the nearest config file, or the
    /// defaults.
    pub fn for_file(file: &Path) -> Result<Config, Error> {
        match file.parent().and_then(Config::find) {
            Some(path) => Config::load(&path),
            None => Ok(Config::default()),
        }
    }

    pub(crate) fn indent(&self, level: usize) -> String {
        match self.indent_style {
            IndentStyle::Spaces => " ".repeat(level * self.indent_width),
            IndentStyle::Tabs => "\t".repeat(level),
        }
    }
}
