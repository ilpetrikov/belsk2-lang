//! Static checker for Belsk2.
//!
//! Runs after parsing and before execution. It infers a type for every
//! expression and reports mistakes that would otherwise only show up at run
//! time: wrong argument counts, type mismatches, undefined names, and so on.
//! Values of type `any` are checked at run time instead.
//!
//! ```
//! let program = belsk2_syntax::parse(r#"var x = 1 - "a";"#).unwrap();
//! let errors = belsk2_typeck::check(&program);
//! assert_eq!(errors.len(), 1);
//! ```

mod builtins;
mod checker;
mod ty;

use belsk2_syntax::ast::{FnDecl, Program};
use belsk2_syntax::Error;

pub use builtins::{is_builtin, BUILTINS};
pub use checker::Checker;
pub use ty::{Sig, Ty};

/// Checks a program and returns every error found (empty if it is valid).
pub fn check(program: &Program) -> Vec<Error> {
    Checker::new().check_program(program)
}

/// The static type of a named function.
pub fn fn_type(decl: &FnDecl) -> Ty {
    Ty::Fn(Some(std::rc::Rc::new(checker::sig_of(decl))))
}
