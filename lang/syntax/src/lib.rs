//! Front end of the Belsk2 language: tokens, lexer, AST and parser.
//!
//! ```
//! let program = belsk2_syntax::parse("var x = 1 + 2; prinb(x);").unwrap();
//! assert_eq!(program.stmts.len(), 2);
//! ```

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;
pub mod types;

pub use error::{Error, ErrorKind, Result};
pub use lexer::tokenize;
pub use parser::parse;
pub use span::Span;
pub use types::{FloatKind, IntKind, NumTy, Sig, Ty, TypeExpr, TypeExprKind};
