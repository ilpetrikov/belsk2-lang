//! Signatures of the built-in functions.

use belsk2_syntax::ast::{Expr, ExprKind};
use belsk2_syntax::{Error, Result};

use crate::ty::Ty;

/// Names of all built-in functions.
pub const BUILTINS: &[&str] = &[
    "prinb", "reab", "input", "len", "str", "num", "int", "float", "bool", "push", "pop", "substr",
    "type",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

fn arity(name: &str, n: usize, min: usize, max: usize) -> Result<()> {
    if n < min || n > max {
        let expected = if min == max {
            format!("{min}")
        } else {
            format!("{min} to {max}")
        };
        let plural = if min == max && max == 1 { "" } else { "s" };
        return Err(Error::type_error(format!(
            "{name}() expects {expected} argument{plural}, got {n}"
        )));
    }
    Ok(())
}

fn expect(
    name: &str,
    i: usize,
    args: &[Expr],
    tys: &[Ty],
    allowed: &[Ty],
    what: &str,
) -> Result<()> {
    let (Some(arg), Some(ty)) = (args.get(i), tys.get(i)) else {
        return Ok(());
    };
    if ty.one_of(allowed) {
        return Ok(());
    }
    Err(Error::type_error(format!("{name}() expects {what}, got {ty}")).at(arg.span))
}

/// Checks a call to the built-in `name` and returns its result type.
pub fn check(name: &str, args: &[Expr], tys: &[Ty]) -> Result<Ty> {
    let n = args.len();
    match name {
        "prinb" => {
            arity(name, n, 0, 1)?;
            Ok(Ty::Null)
        }
        "reab" => {
            arity(name, n, 1, 1)?;
            expect(name, 0, args, tys, &[Ty::Number], "a number (idb slot)")?;
            Ok(Ty::String)
        }
        "input" => {
            arity(name, n, 0, 1)?;
            Ok(Ty::String)
        }
        "len" => {
            arity(name, n, 1, 1)?;
            expect(
                name,
                0,
                args,
                tys,
                &[Ty::String, Ty::Array],
                "a string or array",
            )?;
            Ok(Ty::Number)
        }
        "str" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::String)
        }
        "num" | "float" | "int" => {
            arity(name, n, 1, 1)?;
            expect(
                name,
                0,
                args,
                tys,
                &[Ty::Number, Ty::String, Ty::Bool],
                "a number, string or bool",
            )?;
            if let Some(Expr {
                kind: ExprKind::String(s),
                span,
                ..
            }) = args.first()
            {
                if s.trim().parse::<f64>().is_err() {
                    return Err(
                        Error::type_error(format!("{name}(): \"{s}\" is not a number")).at(*span),
                    );
                }
            }
            Ok(Ty::Number)
        }
        "bool" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::Bool)
        }
        "push" => {
            arity(name, n, 2, 2)?;
            expect(name, 0, args, tys, &[Ty::Array], "an array")?;
            Ok(Ty::Array)
        }
        "pop" => {
            arity(name, n, 1, 1)?;
            expect(name, 0, args, tys, &[Ty::Array], "an array")?;
            Ok(Ty::Any)
        }
        "substr" => {
            arity(name, n, 3, 3)?;
            expect(name, 0, args, tys, &[Ty::String], "a string")?;
            expect(name, 1, args, tys, &[Ty::Number], "a number for start")?;
            expect(name, 2, args, tys, &[Ty::Number], "a number for length")?;
            Ok(Ty::String)
        }
        "type" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::String)
        }
        _ => Err(Error::type_error(format!("unknown built-in '{name}'"))),
    }
}
