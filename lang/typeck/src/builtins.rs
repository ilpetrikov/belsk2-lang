//! Signatures of the built-in functions.

use belsk2_syntax::ast::Expr;
use belsk2_syntax::{Error, Result, Ty};

use crate::checker::{const_of, Const};

/// Names of all built-in functions. Every numeric type name and `char`
/// doubles as an explicit conversion: `int(2.9)`, `byte(300)`, `char(65)`.
pub const BUILTINS: &[&str] = &[
    "prinb", "reab", "input", "len", "str", "num", "bool", "push", "pop", "substr", "type",
    "sbyte", "byte", "short", "ushort", "int", "uint", "long", "ulong", "float", "double", "char",
    "has", "remove", "keys", "values",
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

/// Checks argument `i` with `ok`; `what` describes the expected type.
fn expect(name: &str, args: &[Expr], i: usize, what: &str, ok: impl Fn(&Ty) -> bool) -> Result<()> {
    let Some(arg) = args.get(i) else {
        return Ok(());
    };
    if arg.ty.is_any() || ok(&arg.ty) {
        return Ok(());
    }
    Err(Error::type_error(format!("{name}() expects {what}, got {}", arg.ty)).at(arg.span))
}

fn is_integer(t: &Ty) -> bool {
    matches!(t, Ty::Int(_))
}

fn is_string(t: &Ty) -> bool {
    matches!(t, Ty::String | Ty::Ster)
}

/// Checks a call to the built-in `name` and returns its result type.
pub fn check(name: &str, args: &[Expr]) -> Result<Ty> {
    let n = args.len();
    if let Some(target) = Ty::from_name(name).filter(|t| t.num().is_some() || *t == Ty::Char) {
        return conversion(name, args, target);
    }
    match name {
        "prinb" => {
            arity(name, n, 0, 1)?;
            Ok(Ty::Null)
        }
        "reab" => {
            arity(name, n, 1, 1)?;
            expect(name, args, 0, "an integer (idb slot)", is_integer)?;
            Ok(Ty::String)
        }
        "input" => {
            arity(name, n, 0, 1)?;
            Ok(Ty::String)
        }
        "len" => {
            arity(name, n, 1, 1)?;
            expect(name, args, 0, "a string, array or dictionary", |t| {
                is_string(t) || matches!(t, Ty::Array(_) | Ty::Map(..))
            })?;
            Ok(Ty::INT)
        }
        "str" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::String)
        }
        // `num(x)` is the older name for `double(x)`.
        "num" => conversion(name, args, Ty::DOUBLE),
        "bool" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::Bool)
        }
        "push" => {
            arity(name, n, 2, 2)?;
            expect(name, args, 0, "an array", |t| matches!(t, Ty::Array(_)))?;
            Ok(args.first().map(|a| a.ty.clone()).unwrap_or(Ty::Any))
        }
        "pop" => {
            arity(name, n, 1, 1)?;
            expect(name, args, 0, "an array", |t| matches!(t, Ty::Array(_)))?;
            Ok(match args.first().map(|a| &a.ty) {
                Some(Ty::Array(elem)) => (**elem).clone(),
                _ => Ty::Any,
            })
        }
        "substr" => {
            arity(name, n, 3, 3)?;
            expect(name, args, 0, "a string", is_string)?;
            expect(name, args, 1, "an integer for start", is_integer)?;
            expect(name, args, 2, "an integer for length", is_integer)?;
            Ok(Ty::String)
        }
        "type" => {
            arity(name, n, 1, 1)?;
            Ok(Ty::String)
        }
        // has(dict, key) / remove(dict, key): whether the key is (was) there.
        "has" | "remove" => {
            arity(name, n, 2, 2)?;
            expect(name, args, 0, "a dictionary", |t| matches!(t, Ty::Map(..)))?;
            Ok(Ty::Bool)
        }
        "keys" | "values" => {
            arity(name, n, 1, 1)?;
            expect(name, args, 0, "a dictionary", |t| matches!(t, Ty::Map(..)))?;
            Ok(match (name, args.first().map(|a| &a.ty)) {
                ("keys", Some(Ty::Map(k, _))) => Ty::Array(k.clone()),
                (_, Some(Ty::Map(_, v))) => Ty::Array(v.clone()),
                _ => Ty::array(Ty::Any),
            })
        }
        _ => Err(Error::type_error(format!("unknown built-in '{name}'"))),
    }
}

/// `int(x)`, `double(x)`, `char(x)`, ...: explicit conversions from numbers,
/// chars, bools and strings (which are parsed).
fn conversion(name: &str, args: &[Expr], target: Ty) -> Result<Ty> {
    arity(name, args.len(), 1, 1)?;
    expect(name, args, 0, "a number, char, bool or string", |t| {
        t.num().is_some() || matches!(t, Ty::Char | Ty::Bool) || is_string(t)
    })?;
    let Some(arg) = args.first() else {
        return Ok(target);
    };
    if let Some(Const::Str(s)) = const_of(arg) {
        let valid = if target == Ty::Char {
            s.chars().count() == 1
        } else {
            s.trim().parse::<f64>().is_ok()
        };
        if !valid {
            let expected = if target == Ty::Char {
                "a single character"
            } else {
                "a number"
            };
            return Err(
                Error::type_error(format!("{name}(): \"{s}\" is not {expected}")).at(arg.span),
            );
        }
    }
    Ok(target)
}
