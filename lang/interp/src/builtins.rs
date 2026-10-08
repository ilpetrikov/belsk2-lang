//! Built-in functions available in every program.

use std::io::Write;

use belsk2_syntax::{Error, Result};

use crate::interpreter::Interpreter;
use crate::value::{is_integral, Value};

fn arity(name: &str, args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            format!("{min}")
        } else {
            format!("{min} to {max}")
        };
        return Err(Error::runtime(format!(
            "{name}() expects {expected} argument{}, got {}",
            if min == max && max == 1 { "" } else { "s" },
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

fn parse_number(name: &str, s: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .map_err(|_| Error::runtime(format!("{name}(): cannot convert \"{s}\" to a number")))
}

fn slot_id(name: &str, v: &Value) -> Result<i64> {
    match v {
        Value::Number(n) if is_integral(*n) => Ok(*n as i64),
        other => Err(wrong_type(name, "a whole number (idb slot)", other)),
    }
}

pub fn call(
    interp: &mut Interpreter,
    name: &str,
    args: Vec<Value>,
    out: &mut dyn Write,
) -> Result<Value> {
    let mut it = args.iter();
    let first = it.next();
    let second = it.next();
    let third = it.next();

    match name {
        // prinb(x) prints x. A whole number naming an idb slot prints the
        // value stored in that slot instead.
        "prinb" => {
            arity(name, &args, 0, 1)?;
            match first {
                None => writeln!(out)?,
                Some(v) => {
                    let banked = match v {
                        Value::Number(n) if is_integral(*n) => interp.id_bank.get(&(*n as i64)),
                        _ => None,
                    };
                    writeln!(out, "{}", banked.unwrap_or(v))?;
                }
            }
            Ok(Value::Null)
        }
        // reab(slot) reads a line from input into an idb slot and returns it.
        "reab" => {
            arity(name, &args, 1, 1)?;
            let id = slot_id(name, first.unwrap_or(&Value::Null))?;
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
            match first {
                Some(Value::String(s)) => Ok(Value::from(s.chars().count() as i64)),
                Some(Value::Array(a)) => Ok(Value::from(a.borrow()?.len() as i64)),
                Some(v) => Err(wrong_type(name, "a string or array", v)),
                None => Ok(Value::Null),
            }
        }
        "str" => {
            arity(name, &args, 1, 1)?;
            Ok(Value::String(
                first.map(|v| v.to_string()).unwrap_or_default(),
            ))
        }
        "num" | "float" => {
            arity(name, &args, 1, 1)?;
            match first {
                Some(Value::Number(n)) => Ok(Value::Number(*n)),
                Some(Value::String(s)) => Ok(Value::Number(parse_number(name, s)?)),
                Some(Value::Bool(b)) => Ok(Value::Number(if *b { 1.0 } else { 0.0 })),
                Some(v) => Err(wrong_type(name, "a number, string or bool", v)),
                None => Ok(Value::Null),
            }
        }
        "int" => {
            arity(name, &args, 1, 1)?;
            match first {
                Some(Value::Number(n)) => Ok(Value::Number(n.trunc())),
                Some(Value::String(s)) => Ok(Value::Number(parse_number(name, s)?.trunc())),
                Some(Value::Bool(b)) => Ok(Value::Number(if *b { 1.0 } else { 0.0 })),
                Some(v) => Err(wrong_type(name, "a number, string or bool", v)),
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
            match (first, second) {
                (Some(Value::Array(a)), Some(v)) => {
                    a.borrow_mut()?.push(v.clone());
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
            let (Some(Value::String(s)), Some(start), Some(length)) = (first, second, third) else {
                return Err(wrong_type(name, "a string", first.unwrap_or(&Value::Null)));
            };
            let (Value::Number(start), Value::Number(length)) = (start, length) else {
                return Err(Error::type_error(
                    "substr() expects numbers for start and length",
                ));
            };
            let start = start.max(0.0) as usize;
            let length = length.max(0.0) as usize;
            Ok(Value::String(s.chars().skip(start).take(length).collect()))
        }
        "type" => {
            arity(name, &args, 1, 1)?;
            Ok(Value::string(first.map(Value::type_name).unwrap_or("null")))
        }
        _ => Err(Error::runtime(format!("unknown built-in '{name}'"))),
    }
}
