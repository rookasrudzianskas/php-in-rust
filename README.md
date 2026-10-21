# php-in-rust

A rewrite of the PHP interpreter ([php-src](https://github.com/php/php-src)) in idiomatic Rust.

The goal is a drop-in `php` CLI that runs PHP 8.5 code with the same observable behaviour as php-src:
the same output, warnings, exceptions and error messages. It's measured against php-src's own `.phpt`
conformance suite.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the analysis of php-src, the Rust design and the module mapping.

## Layout

| Crate | Purpose |
|---|---|
| `crates/php-syntax` | Lexer, parser, AST, name resolution, AST printer (Zend scanner/parser) |
| `crates/php-runtime` | Values, arrays, objects, classes, interpreter, errors, output (Zend engine + `main/`) |
| `crates/php-ext` | Extensions: standard, spl, json, pcre, ctype, date, random, ... |
| `crates/php-cli` | The `php` command-line binary (`sapi/cli`) |
| `crates/php-phpt` | `php-run-tests`, a port of `run-tests.php` |
| `tests/phpt` | Conformance tests taken from php-src (PHP License 3.01, see `tests/phpt/LICENSE-PHP`) |

## Building

```bash
cargo build --release
```

```bash
./target/release/php -r 'echo "Hello from Rust\n";'
```

## Testing

```bash
cargo test --workspace
```

```bash
./target/release/php-run-tests -p target/release/php tests/phpt/Zend/tests
```

`tests/phpt/PASSING` lists every conformance test that passes. Regression mode fails if any of them break:

```bash
./target/release/php-run-tests -p target/release/php --passing-list tests/phpt/PASSING
```

## License

The Rust code in this repository is licensed under the PHP License 3.01, matching the project it reimplements.
The test files under `tests/phpt` come from php-src.
