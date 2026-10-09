//! Recording what a program does, for editors: how often each line ran,
//! the last value written on each line and the contents of the id bank.
//! The IDE shows this next to the code ("live values").

use std::collections::BTreeMap;

use belsk2_syntax::ast::{Expr, ExprKind};
use belsk2_syntax::{FileId, Span};

use crate::value::Value;

/// Longest value text kept, in characters.
const MAX_VALUE: usize = 80;
/// Most names remembered per line.
const MAX_NAMES: usize = 4;

/// What happened on one line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineTrace {
    /// How many times a statement starting on this line ran.
    pub hits: u64,
    /// Last value of each name written on this line, in first-write order.
    pub values: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    pub lines: BTreeMap<(FileId, usize), LineTrace>,
    /// The id bank: slot -> value.
    pub idb: BTreeMap<i64, String>,
}

/// A short text form of a value.
pub fn short(value: &Value) -> String {
    let text = match value {
        Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_VALUE {
        let mut cut: String = text.chars().take(MAX_VALUE - 1).collect();
        cut.push('…');
        cut
    } else {
        text
    }
}

/// How an assignment target is shown: `x`, `items[i]`, `grid[…]`.
pub fn describe(target: &Expr) -> Option<String> {
    match &target.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Index { object, index } => {
            let base = describe(object)?;
            let idx = match &index.kind {
                ExprKind::Ident(n) => n.clone(),
                ExprKind::Int(v, _) => v.to_string(),
                ExprKind::String(s) => format!("{s:?}"),
                ExprKind::Convert { expr, .. } => return describe_index(&base, expr),
                _ => "…".to_string(),
            };
            Some(format!("{base}[{idx}]"))
        }
        _ => None,
    }
}

fn describe_index(base: &str, index: &Expr) -> Option<String> {
    let idx = match &index.kind {
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Int(v, _) => v.to_string(),
        _ => "…".to_string(),
    };
    Some(format!("{base}[{idx}]"))
}

impl Trace {
    pub fn hit(&mut self, span: Span) {
        let line = self.lines.entry((span.file, span.line)).or_default();
        line.hits = line.hits.saturating_add(1);
    }

    pub fn value(&mut self, span: Span, name: String, value: &Value) {
        let line = self.lines.entry((span.file, span.line)).or_default();
        let text = short(value);
        if let Some(slot) = line.values.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = text;
        } else if line.values.len() < MAX_NAMES {
            line.values.push((name, text));
        }
    }

    pub fn idb(&mut self, id: i64, value: &Value) {
        self.idb.insert(id, short(value));
    }
}
