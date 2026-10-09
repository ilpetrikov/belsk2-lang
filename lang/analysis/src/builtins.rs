//! Signatures and descriptions of the built-in functions, for hover and
//! completion.

/// `(name, signature, description)`.
const DOCS: &[(&str, &str, &str)] = &[
    (
        "prinb",
        "fn prinb(value?)",
        "Prints a value followed by a new line.",
    ),
    (
        "reab",
        "fn reab(slot: int): string",
        "Reads the value stored in idb slot `slot`.",
    ),
    (
        "input",
        "fn input(prompt?: string): string",
        "Reads a line from standard input.",
    ),
    (
        "len",
        "fn len(value: string | T[] | {K: V}): int",
        "Length of a string, array or dictionary.",
    ),
    (
        "str",
        "fn str(value): string",
        "Converts a value to its text form.",
    ),
    (
        "num",
        "fn num(value): double",
        "Older name for `double(value)`.",
    ),
    (
        "bool",
        "fn bool(value): bool",
        "Converts a value to `bool`.",
    ),
    (
        "push",
        "fn push(array: T[], item: T): T[]",
        "Appends `item` to `array`.",
    ),
    (
        "pop",
        "fn pop(array: T[]): T",
        "Removes and returns the last item of `array`.",
    ),
    (
        "substr",
        "fn substr(s: string, start: int, length: int): string",
        "Part of a string.",
    ),
    (
        "type",
        "fn type(value): string",
        "Name of the run-time type of a value.",
    ),
    (
        "has",
        "fn has(dict: {K: V}, key: K): bool",
        "Whether `dict` contains `key`.",
    ),
    (
        "remove",
        "fn remove(dict: {K: V}, key: K): bool",
        "Removes `key`; returns whether it was there.",
    ),
    (
        "keys",
        "fn keys(dict: {K: V}): K[]",
        "The keys of a dictionary, in insertion order.",
    ),
    (
        "values",
        "fn values(dict: {K: V}): V[]",
        "The values of a dictionary, in insertion order.",
    ),
    (
        "char",
        "fn char(value): char",
        "Converts a number or one-character string to `char`.",
    ),
];

/// Signature and description of a built-in function.
pub fn doc(name: &str) -> Option<(String, String)> {
    if let Some((_, sig, text)) = DOCS.iter().find(|(n, ..)| *n == name) {
        return Some(((*sig).to_string(), (*text).to_string()));
    }
    if NUMERIC.contains(&name) {
        return Some((
            format!("fn {name}(value): {name}"),
            format!("Converts a number, char, bool or string to `{name}`."),
        ));
    }
    None
}

const NUMERIC: &[&str] = &[
    "sbyte", "byte", "short", "ushort", "int", "uint", "long", "ulong", "float", "double",
];

/// Type names offered by completion.
pub const TYPE_NAMES: &[&str] = &[
    "any", "bool", "char", "string", "sbyte", "byte", "short", "ushort", "int", "uint", "long",
    "ulong", "float", "double",
];
