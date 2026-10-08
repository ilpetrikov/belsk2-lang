//! Built-in functions available in every program. Their static signatures
//! live in `belsk2-typeck`; these checks only matter for values of type
//! `any`.

use std::io::Write;

use belsk2_syntax::{Error, Result, Ty};

use crate::collections::Array;
use crate::interpreter::Interpreter;
use crate::num::convert;
use crate::value::Value;

fn arity(name: &str, args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            format!("{min}")
        } else {
            format!("{min} to {max}")
        };
        let plural = if min == max && max == 1 { "" } else { "s" };
        return Err(Error::runtime(format!(
            "{name}() expects {expected} argument{plural}, got {}",
            args.len()
        )));
    }
    Ok(())
}

fn wrong_type(name: &str, expected: &str, got: &Value) -> Error {
    Error::type_error(format!(
        "{name}() expects {expected}, got {}",
        got.type_name()
    ))
}

/// An integer argument as `i64` (saturating for huge `ulong` values).
fn int_arg(name: &str, what: &str, v: &Value) -> Result<i64> {
    match v {
        Value::Int(i, _) => Ok(i64::try_from(*i).unwrap_or(i64::MAX)),
        other => Err(wrong_type(name, what, other)),
    }
}

/// A whole number naming an idb slot. Whole `double`s are accepted for
/// compatibility with older programs.
fn slot_of(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i, _) => i64::try_from(*i).ok(),
        Value::Float(f, _) if f.fract() == 0.0 && f.abs() < 9e15 => Some(*f as i64),
        _ => None,
    }
}

pub fn call(
    interp: &mut Interpreter,
    name: &str,
    args: Vec<Value>,
    out: &mut dyn Write,
) -> Result<Value> {
    let first = args.first();

    if let Some(target) = Ty::from_name(name).filter(|t| t.num().is_some() || *t == Ty::Char) {
        arity(name, &args, 1, 1)?;
        return match first {
            Some(v) => convert(v.clone(), &target).map_err(|e| Error {
                message: format!("{name}(): {}", e.message),
                ..e
            }),
            None => Ok(Value::Null),
        };
    }

    match name {
        // prinb(x) prints x. A whole number naming an idb slot prints the
        // value stored in that slot instead.
        "prinb" => {
            arity(name, &args, 0, 1)?;
            match first {
                None => writeln!(out)?,
                Some(v) => {
                    let banked = slot_of(v).and_then(|id| interp.id_bank.get(&id));
                    writeln!(out, "{}", banked.unwrap_or(v))?;
                }
            }
            Ok(Value::Null)
        }
        // reab(slot) reads a line from input into an idb slot and returns it.
        "reab" => {
            arity(name, &args, 1, 1)?;
            let id = first.and_then(slot_of).ok_or_else(|| {
                wrong_type(name, "an integer (idb slot)", first.unwrap_or(&Value::Null))
            })?;
            out.flush()?;
            let line = interp.input.read_line()?.unwrap_or_default();
            let v = Value::String(line);
            interp.id_bank.insert(id, v.clone());
            Ok(v)
        }
        "input" => {
            arity(name, &args, 0, 1)?;
            if let Some(prompt) = first {
                write!(out, "{prompt}")?;
            }
            out.flush()?;
            Ok(Value::String(interp.input.read_line()?.unwrap_or_default()))
        }
        "len" => {
            arity(name, &args, 1, 1)?;
            let n = match first {
                Some(Value::String(s)) => s.chars().count(),
                Some(Value::Array(a)) => a.borrow()?.len(),
                Some(Value::Map(m)) => m.len(),
                Some(v) => return Err(wrong_type(name, "a string, array or dictionary", v)),
                None => 0,
            };
            Ok(Value::int(i32::try_from(n).unwrap_or(i32::MAX)))
        }
        "str" => {
            arity(name, &args, 1, 1)?;
            Ok(Value::String(
                first.map(|v| v.to_string()).unwrap_or_default(),
            ))
        }
        "num" => {
            arity(name, &args, 1, 1)?;
            match first {
                Some(v) => convert(v.clone(), &Ty::DOUBLE),
                None => Ok(Value::Null),
            }
        }
        "bool" => {
            arity(name, &args, 1, 1)?;
            Ok(Value::Bool(first.is_some_and(Value::is_truthy)))
        }
        // push(arr, x) appends to arr in place and returns arr, so both
        // `push(a, x)` and `a = push(a, x)` work.
        "push" => {
            arity(name, &args, 2, 2)?;
            match (first, args.get(1)) {
                (Some(Value::Array(a)), Some(v)) => {
                    a.push(v.clone())?;
                    Ok(Value::Array(a.clone()))
                }
                (Some(v), _) => Err(wrong_type(name, "an array", v)),
                _ => Ok(Value::Null),
            }
        }
        // pop(arr) removes and returns the last element (null if empty).
        "pop" => {
            arity(name, &args, 1, 1)?;
            match first {
                Some(Value::Array(a)) => Ok(a.borrow_mut()?.pop().unwrap_or(Value::Null)),
                Some(v) => Err(wrong_type(name, "an array", v)),
                None => Ok(Value::Null),
            }
        }
        // substr(s, start, length), counted in characters. Out-of-range
        // parts are clipped.
        "substr" => {
            arity(name, &args, 3, 3)?;
            let Some(Value::String(s)) = first else {
                return Err(wrong_type(name, "a string", first.unwrap_or(&Value::Null)));
            };
            let null = Value::Null;
            let start = int_arg(name, "an integer for start", args.get(1).unwrap_or(&null))?;
            let length = int_arg(name, "an integer for length", args.get(2).unwrap_or(&null))?;
            let start = usize::try_from(start.max(0)).unwrap_or(usize::MAX);
            let length = usize::try_from(length.max(0)).unwrap_or(usize::MAX);
            Ok(Value::String(s.chars().skip(start).take(length).collect()))
        }
        "has" | "remove" | "keys" | "values" => {
            arity(
                name,
                &args,
                if matches!(name, "has" | "remove") {
                    2
                } else {
                    1
                },
                2,
            )?;
            let Some(Value::Map(m)) = first else {
                return Err(wrong_type(
                    name,
                    "a dictionary",
                    first.unwrap_or(&Value::Null),
                ));
            };
            let key = args.get(1).unwrap_or(&Value::Null);
            Ok(match name {
                "has" => Value::Bool(m.contains(key)?),
                "remove" => Value::Bool(m.remove(key)?),
                "keys" => Value::Array(Array::typed(m.key_ty().clone(), m.keys()?)),
                _ => Value::Array(Array::typed(m.value_ty().clone(), m.values()?)),
            })
        }
        "type" => {
            arity(name, &args, 1, 1)?;
            Ok(Value::String(
                first.map(Value::type_name).unwrap_or_else(|| "null".into()),
            ))
        }
        _ => Err(Error::runtime(format!("unknown built-in '{name}'"))),
    }
}
