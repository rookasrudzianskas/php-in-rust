//! # php-runtime — the Zend engine layer of php-in-rust
//!
//! This crate holds PHP's value model, object model, conversions and
//! operators, error reporting, output buffering, ini directives, constants,
//! the native-function contract and the [`Vm`] context. The tree-walking
//! interpreter lives on top of it (in this crate, later) and plugs in through
//! [`vm::Host`].
//!
//! ## Values ([`value`])
//! * [`Value`] — the 16-byte `zval`: `Undef | Null | Bool | Int | Float |
//!   String(PhpStr) | Array(PhpArrayRef) | Object(ObjRef) | Resource(ResRef) |
//!   Ref(PhpRef)`. `deref()`/`unref()` look through references;
//!   `type_name()`/`value_name()` give PHP's spellings for messages.
//! * [`PhpStr`] — binary string, `Rc`-shared, COW `push_bytes` (amortised O(1)
//!   when unique), cached hash.
//! * [`PhpArray`] — ordered hash map with exact `zend_hash.c` semantics: key
//!   normalisation ([`ArrayKey::from_bytes`], [`value::array::cast_key`]),
//!   `next_free`, packed fast path, stable [`Pos`]itions for iterators,
//!   internal pointer (`current`, `move_forward`, `move_backward`, `reset`,
//!   `end`), `pop/shift/unshift`,
//!   stable `sort_by`. [`PhpArrayRef`] = `Rc<PhpArray>`; mutate with
//!   `Rc::make_mut` (its `Clone` is `zend_array_dup`).
//! * [`PhpRef`] — `zend_reference` (`Rc<RefCell<Value>>`, never nested).
//!
//! ## Conversions and operators
//! `Vm::{to_bool, to_int, to_float, to_php_string, to_array_key, to_number,
//! cast_to_array, cast_to_object}` and `Vm::{add, sub, mul, div, modulo, pow,
//! intdiv, concat, concat_assign, shift_left, shift_right, bitwise_or/and/xor/not,
//! bool_xor, bool_not, negate, compare, is_equal, is_smaller,
//! is_smaller_or_equal, spaceship, increment, decrement}` emit PHP 8.5's
//! warnings/deprecations and throw its TypeErrors/ArithmeticErrors.
//! [`value::ops::is_identical`] is `===`. Numeric strings: [`value::numeric`];
//! float formatting (`precision` / `serialize_precision`): [`value::float_fmt`].
//!
//! ## Objects and classes
//! [`ObjRef`] (handle ids reused like the object store; destructor queue —
//! `Vm::run_pending_destructors`), [`ClassEntry`] / [`ClassRef`] with
//! properties, constants, methods, magic-method cache and [`class::ObjectHandlers`]
//! hooks for internal classes; [`Function`] (`FunctionKind::User(Rc<dyn Any>)`
//! or `FunctionKind::Native(&'static NativeFunction)`).
//!
//! ## The VM
//! [`Vm`] owns output ([`output`]: `echo`, `ob_*`), errors ([`errors`]:
//! `zend_error`, `php_error_docref`, `warning`, `notice`, `deprecated`,
//! `fatal_error`, `@` via `begin_silence`/`end_silence`), ini (`ini_get`,
//! `ini_set`), constants (`constant`, `define_constant`), the function and
//! class tables (`find_function`, `find_class`, `lookup_class`), frames
//! (`backtrace`) and exceptions (`throw_error`, `throw_type_error`,
//! `throw_value_error`, `argument_value_error`, `create_exception`,
//! `report_uncaught_exception`). Control flow uses [`Flow`] and
//! [`PResult<T>`] (`Result<T, Flow>`).
//!
//! ## Writing built-in functions ([`native`])
//! ```ignore
//! fn strlen(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
//!     Ok(Value::Int(args.str(0).len() as i64))
//! }
//! pub static STRLEN: NativeFunction =
//!     NativeFunction { sig: "strlen(string $string): int", handler: strlen };
//! pub fn register(r: &mut Registry) {
//!     r.function(&STRLEN);
//!     r.constant("STR_PAD_RIGHT", 1i64);
//! }
//! ```
//! The signature (php-src stub syntax) drives `zend_parse_parameters`
//! semantics: arity and named-argument errors, defaults (literals, constants,
//! `A | B`, `UNKNOWN`), weak/strict coercion, the `null` deprecation,
//! class/array/callable checks and by-reference parameters. Call natives from
//! Rust with `Vm::call_native(name, args)`; invoke PHP callables with
//! `Vm::call(&callable, args)`.

// Let-chains (Rust 1.88) are not available at the workspace MSRV (1.85).
#![allow(clippy::collapsible_if)]

pub mod builtins;
pub mod class;
pub mod compile;
pub mod constants;
pub mod enums;
pub mod errors;
pub mod ini;
pub mod interp;
pub mod native;
pub mod object;
pub mod output;
pub mod types;
pub mod util;
pub mod value;
pub mod vm;

pub use class::{ClassEntry, ClassRef, Function, FunctionKind, Method};
pub use native::{Args, CallArgs, NativeFunction, NativeHandler, Registry};
pub use object::ObjRef;
pub use value::{ArrayKey, PhpArray, PhpArrayRef, PhpRef, PhpStr, Pos, ResRef, Value};
pub use vm::{Flow, Host, PResult, Vm};

#[cfg(test)]
mod tests;
