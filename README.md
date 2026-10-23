# php-in-rust

A rewrite of the PHP interpreter ([php-src](https://github.com/php/php-src)) in idiomatic Rust.

The goal is a drop-in `php` CLI that runs PHP 8.5 code with the same observable behaviour as php-src:
the same output, warnings, exceptions and error messages. It's measured against php-src's own `.phpt`
conformance suite.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the analysis of php-src, the Rust design and the module mapping,
and [PORTING.md](PORTING.md) for the conventions used when porting code.

## Status

10,681 of the 13,111 php-src conformance tests that run here pass (81%); 1,275 more are skipped
(they need SAPIs, extensions or platforms outside the scope below). Every passing test is listed in
`tests/phpt/PASSING` and checked by `cargo test`.

| Area | Passing | Failing | Skipped |
|---|---:|---:|---:|
| Zend engine (`Zend/tests`) | 4,333 | 1,185 | 188 |
| Language (`tests/{lang,classes,func,basic,output}`) | 617 | 79 | 130 |
| ext/standard | 3,050 | 352 | 641 |
| ext/spl | 689 | 113 | 12 |
| ext/date | 670 | 56 | 18 |
| ext/mbstring | 321 | 38 | 91 |
| ext/pcre | 162 | 7 | 9 |
| ext/bcmath | 165 | 5 | 1 |
| ext/json | 104 | 2 | 2 |
| ext/filter | 101 | 5 | 21 |
| ext/sqlite3 | 94 | 9 | 4 |
| ext/hash | 78 | 7 | 1 |
| ext/pdo, ext/pdo_sqlite | 79 | 14 | 135 |
| ext/random | 68 | 1 | 1 |
| ext/tokenizer | 53 | 1 | 0 |
| ext/calendar | 49 | 0 | 6 |
| ext/ctype | 48 | 0 | 1 |
| ext/reflection | 0 | 561 | 14 |

Known gaps:

- **Reflection:** `ext/reflection` isn't implemented yet.
- **Cycle collection:** there's no cycle collector, so reference cycles are only freed at shutdown.
- **Memory use:** compiling very large files takes far more memory than php-src. The AST is about 30× the source size.
- **Not ported:**
  - SAPIs other than CLI (fpm, apache2handler, cgi);
  - opcache and the JIT;
  - extensions that bind external C libraries (curl, gd, intl, openssl, …).
- **`memory_limit` approximation:** it's enforced against this runtime's own heap, and no script may use more than 4 GB even with `memory_limit=-1`.

`tests/phpt/KNOWN_FAILURES` lists the tests that only passed before `memory_limit` was enforced.

## Layout

| Crate | Purpose |
|---|---|
| `crates/php-syntax` | Lexer, parser, AST, name resolution, AST printer (Zend scanner/parser) |
| `crates/php-runtime` | Values, arrays, objects, classes, interpreter, errors, output (Zend engine + `main/`) |
| `crates/php-ext` | Extensions: standard, spl, json, pcre, ctype, date, random, mbstring, hash, bcmath, calendar, filter, pdo, pdo_sqlite, sqlite3, tokenizer |
| `crates/php-regex` | PCRE2-compatible regular expression engine (pure Rust) |
| `crates/php-timelib` | Port of timelib with PHP's bundled timezone database |
| `crates/php-hash` | The hash algorithms of ext/hash |
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
cargo build --release -p php-cli -p php-phpt
```

```bash
cargo test --workspace
```

The `php-cli` regression test runs every test in `tests/phpt/PASSING` against the release binaries. To run the
conformance suite directly:

```bash
./target/release/php-run-tests -p target/release/php -j4 tests/phpt
```

```bash
./target/release/php-run-tests -p target/release/php --passing-list tests/phpt/PASSING
```

## License

The Rust code in this repository is licensed under the PHP License 3.01, matching the project it reimplements.
The test files under `tests/phpt` come from php-src.
