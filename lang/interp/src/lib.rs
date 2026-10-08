//! Tree-walking interpreter for Belsk2.
//!
//! ```
//! use belsk2_interp::{Interpreter, Value};
//!
//! let mut interp = Interpreter::new();
//! let mut out = Vec::new();
//! interp
//!     .run_source_with_writer("fn sq(x: int): int { return x * x; }", &mut out)
//!     .unwrap();
//! let v = interp.call_function("sq", &[Value::from(7.0)], &mut out).unwrap();
//! assert_eq!(v, Value::from(49.0));
//! ```

mod builtins;
pub mod env;
mod interpreter;
mod value;

pub use belsk2_syntax::{Error, ErrorKind, Result, Span};
pub use builtins::BUILTINS;
pub use interpreter::{Input, Interpreter, DEFAULT_MAX_CALL_DEPTH};
pub use value::{format_number, Array, Function, Value};
