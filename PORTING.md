# Porting guide

Conventions for porting php-src code into this repository.

## Source of truth

* Behaviour, messages and edge cases come from php-src (C source, `*.stub.php`, `.phpt` tests), never from
  memory or documentation. When the C code and the docs disagree, the C code wins.
* Output must be byte-identical: warnings, notices, deprecations, exception messages, `var_dump` layout,
  float formatting.

## Built-in functions

* One Rust module per php-src source file (`ext/standard/string.c` → `crates/php-ext/src/standard/string.rs`).
  Split large modules into submodules inside that module's own directory.
* Copy each function's signature **verbatim** from the `*.stub.php` file into `NativeFunction::sig`
  (types, parameter names, defaults, by-ref markers). The runtime derives argument parsing, coercion and
  `TypeError`/`ArgumentCountError`/`ValueError` messages for wrong argument types from it.
* Handlers receive already-coerced arguments (`args.str(0)`, `args.int(1)`, …). Validate values the way the C
  code does after `zend_parse_parameters`: `zend_argument_value_error(1, "must be ...")` becomes
  `vm.argument_value_error(1, "must be ...")`, and `php_error_docref(NULL, E_WARNING, ...)` becomes
  `vm.warning(...)`.
* Register constants with their php-src values and in the same module as php-src.
* PHP strings are byte strings: operate on `&[u8]`, never assume UTF-8.

## Tests

* Unit-test every function with expectations taken from the php-src `.phpt` files of that function
  (`crates/php-ext/src/testing.rs` provides `TestVm`).
* Once the interpreter runs a test, add it to `tests/phpt/PASSING`.

## Style

* Idiomatic Rust, `rustfmt`, `cargo clippy` clean, no `unsafe` unless unavoidable and documented.
* Comment only non-obvious PHP behaviour, citing the php-src function it mirrors.

## Commits

* One logical change per commit, message in imperative mood (`ext/standard: port string.c`).
* No AI attribution or co-author trailers.
