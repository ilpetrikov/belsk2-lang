//! Static checker for Belsk2.
//!
//! Runs after parsing and before execution. It infers a type for every
//! expression, records it in the AST, inserts implicit conversions
//! (`ExprKind::Convert`) and reports mistakes that would otherwise only show
//! up at run time: wrong argument counts, type mismatches, undefined names
//! and so on. Values of type `any` are checked at run time instead.
//!
//! ```
//! let mut program = belsk2_syntax::parse(r#"var x = 1 - "a";"#).unwrap();
//! let errors = belsk2_typeck::check(&mut program);
//! assert_eq!(errors.len(), 1);
//! ```

mod builtins;
mod checker;

use belsk2_syntax::ast::{FnDecl, Program};
use belsk2_syntax::{Error, Ty};

pub use builtins::{is_builtin, BUILTINS};
pub use checker::Checker;

/// Checks a program, filling in types. Returns every error found (empty
/// if the program is valid).
pub fn check(program: &mut Program) -> Vec<Error> {
    Checker::new().check_program(program)
}

/// The static type of a checked function declaration.
pub fn fn_type(decl: &FnDecl) -> Ty {
    Ty::Fn(Some(std::rc::Rc::new(checker::sig_of(decl))))
}
