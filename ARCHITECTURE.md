# php-in-rust — Architecture

A rewrite of the PHP language runtime ([php-src](https://github.com/php/php-src),
tracking PHP 8.5 / 8.7-dev semantics) in idiomatic Rust.

## 1. What php-src is (analysis)

php-src is ~1.5M lines of C organised in four layers:

| Layer | php-src | Size | Role |
|---|---|---|---|
| Language front-end | `Zend/zend_language_scanner.l` (re2c), `Zend/zend_language_parser.y` (bison), `Zend/zend_ast.c` | ~8k LOC | Source → tokens → AST |
| Compiler | `Zend/zend_compile.c`, `zend_inheritance.c`, `Zend/Optimizer/` | ~40k LOC | AST → opcodes (`zend_op_array`), class linking |
| Executor / VM | `zend_execute.c`, `zend_vm_def.h` (generated `zend_vm_execute.h`), `zend_operators.c`, `zend_hash.c`, `zend_object_handlers.c`, `zend_generators.c`, `zend_fibers.c`, `zend_closures.c`, `zend_exceptions.c`, `zend_builtin_functions.c`, `zend_enum.c`, `zend_weakrefs.c`, `zend_attributes.c` | ~120k LOC | zval/HashTable/object model, opcode handlers, built-in classes |
| Runtime services | `main/` (request lifecycle, `php_error_cb`, output buffering, streams, `snprintf`, ini), `sapi/cli` | ~60k LOC | Errors, output, streams, CLI entry point |
| Extensions | `ext/*` (~70: standard, spl, json, pcre, date, ctype, mbstring, random, reflection, hash, pdo, …) | ~1.2M LOC (incl. bundled libs) | The function/class library |
| Tests | `*.phpt` files + `run-tests.php` | ~20k tests | Black-box conformance suite |

Key engine data structures:

* **zval** – a tagged 16-byte value: `UNDEF, NULL, FALSE, TRUE, LONG, DOUBLE, STRING, ARRAY, OBJECT, RESOURCE, REFERENCE`.
  Strings, arrays, objects, resources and references are refcounted heap values.
* **zend_string** – immutable-by-convention *byte* string with cached hash. Not UTF-8.
* **HashTable** – ordered hash map, keys are `zend_long` or `zend_string`; *packed* fast path for
  list-like arrays; tombstones (`IS_UNDEF`) keep iteration order; `nNextFreeElement`; internal pointer;
  value semantics with copy-on-write (refcount + separation).
* **zend_reference** – a refcounted box that makes two slots alias (`$a = &$b`). References survive array copies.
* **zend_object** – handle (`spl_object_id`), class entry, declared-property slot table, optional dynamic
  property HashTable, object handlers (vtable for read_property, read_dimension, compare, cast, …).
* **zend_class_entry** – name, parent, interfaces, constants, property info, static members, function table,
  plus internal-class hooks (`create_object`, `get_iterator`, …).
* **zend_function** – user op_array or internal C handler + `arginfo` (generated from `*.stub.php`).

Execution: compile each file to op_arrays; the VM executes opcodes on a VM stack of call frames.
Generators own a detached frame; fibers own a separate C stack. Errors go through `zend_error` →
`php_error_cb` (display / log / user error handler). Exceptions unwind via the frame's live-range /
try-catch tables. Destructors run when refcount hits 0 and at request shutdown; a cycle collector
reclaims cycles.

## 2. Rust design

### 2.1 Crates

```
crates/
  php-syntax/    Lexer, tokens (token_get_all-compatible names), AST, parser, parse errors.  No deps.
  php-runtime/   The engine: values, strings, arrays, objects, classes, interpreter, errors,
                 output buffering, ini, constants, include, Zend built-in functions & classes
                 (Exception hierarchy, Closure, Generator, Fiber, WeakMap, enums, ...).
  php-ext/       Extensions as modules: standard (string, array, math, var, file, ...), spl, json,
                 pcre, ctype, date, random, mbstring, hash, reflection, tokenizer, ...
  php-cli/       `php` binary – sapi/cli equivalent (-r, -f, -l, -d, -n, -v, -m, -i, -a, --ini ...).
  php-phpt/      `php-run-tests` – run-tests.php equivalent for .phpt files.
```

Dependency graph: `php-syntax ← php-runtime ← php-ext ← php-cli`. `php-phpt` is standalone.

### 2.2 Execution model

A **tree-walking interpreter over a lowered AST** (not a bytecode VM). The parser produces a
plain AST; `php-runtime` lowers each function body once into an executable tree with resolved
compiled-variable slots, pre-interned names and constant-folded literals, then interprets it.

* Control flow: every evaluator returns `Result<T, Flow>`; `Flow` carries `Exception(ObjRef)`,
  `Break(n)`, `Continue(n)`, `Return(Value)`, `Exit(code)`, `Fatal`. `?` does the unwinding.
* Generators and Fibers run on separate stackful coroutines (`corosensei`), so a generator body
  is interpreted by the same evaluator and `yield` is a coroutine suspend. Destroying a suspended
  generator resumes it with a "destroy" signal so `finally` blocks run.
* The main script runs on a thread with a large stack (deep recursion in PHP must not overflow).

### 2.3 Values (`php-runtime/src/value/`)

```rust
pub enum Value {            // == zval
    Undef,                  // uninitialised slot (unset var, uninit typed prop); never user-visible
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(PhpStr),         // byte string, cheap clone (Rc), COW on write
    Array(PhpArrayRef),     // Rc<PhpArray>, COW via Rc::make_mut  == zend_array + separation
    Object(ObjRef),         // Rc handle; identity semantics
    Resource(ResRef),
    Ref(PhpRef),            // Rc<RefCell<Value>> == zend_reference; never nests another Ref
}
```

* **PhpStr** is a byte string (`Rc<Vec<u8>>`-like) – PHP strings are binary, never assume UTF-8.
* **PhpArray** reproduces HashTable semantics exactly: insertion order, int/string keys with PHP key
  normalisation (`"8"` → `8`, but `"08"`, `"-0"`, `" 8"` stay strings; floats truncate; bools → 0/1;
  null → `""`), `next_free` element counter, tombstones so iteration positions are stable during
  `foreach` by reference, internal pointer (`current/next/reset/end/key/prev/each`), packed fast path.
* **References**: a variable/element/property slot holding `Value::Ref(r)` aliases every other slot
  holding the same `r`. Reading derefs transparently. A ref whose `Rc::strong_count == 1` behaves as a
  plain value when the containing array is copied (PHP's "refcount-1 reference" rule).
* **Objects**: `ObjRef(Rc<PhpObject>)` with `id` (reused like PHP handles), `class: ClassRef`,
  declared-property slots + optional dynamic properties (ordered), and `native: RefCell<Option<Box<dyn Any>>>`
  for internal-class state (ArrayObject storage, DateTime, Closure, Generator...).
  `__destruct` runs when the last handle drops: `Drop for ObjRef` pushes the object onto a pending-destructor
  queue that the interpreter drains at statement boundaries; remaining objects are destructed at shutdown
  in PHP order. (No cycle collector: `gc_collect_cycles()` returns 0.)

Conversions and operators (`zend_operators.c`) live in `value/convert.rs` + `value/ops.rs`:
numeric-string parsing (leading/trailing whitespace, "leading-numeric" vs "non-numeric" with the exact
PHP 8 warnings/TypeErrors), int overflow → float, `%`/`intdiv`/`<<` errors, string increment
(`"a"++ → "b"`, `"Az"++ → "Ba"`), PHP 8 comparison semantics (`0 == "a"` is false), `<=>` on arrays,
float → string (`precision`=14 for echo, `serialize_precision`=-1 → shortest round-trip, `1.0E+25` style).

### 2.4 Classes & functions

* `ClassEntry` (`Rc`): name, flags (abstract/final/interface/trait/enum/readonly), parent, interfaces,
  constants (lazily evaluated), property infos (visibility, static, readonly, type, default, hooks,
  asymmetric visibility), methods (`lowercase name → Rc<Method>`), static props, enum cases, attributes,
  internal hooks (`create_object`, `get_iterator`, `read_dimension`, `count`, `compare`, `cast`,
  `clone`, `get_debug_info`, `get_properties_for`…).
* `Function` is either `User(Rc<UserFunction>)` (lowered AST) or `Native(&'static NativeFunction)`.
* Names are case-insensitive for functions/classes/methods/constants-in-namespaces, case-sensitive for variables,
  properties and (most) constants.

### 2.5 Native (built-in) function contract

Every built-in function is declared with its **exact signature copied from php-src `*.stub.php`**
and a Rust handler:

```rust
pub type NativeHandler = fn(&mut Vm, &mut Args) -> PResult<Value>;

pub struct NativeFunction {
    pub sig: &'static str,          // e.g. "str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT): string"
    pub handler: NativeHandler,
}
```

The runtime parses `sig` (lazily, once) into arginfo: parameter names, types (incl. nullable/union),
by-reference flags, variadics and default values (literals, constants, `|` of constants, `[]`, `UNKNOWN` = no default).
Before calling the handler the runtime performs **zend_parse_parameters semantics**: arity checks
(`ArgumentCountError: strlen() expects exactly 1 argument, 0 given`), named arguments, weak-mode coercion
or strict_types checks, `TypeError: str_pad(): Argument #2 ($length) must be of type int, string given`,
`Deprecated: ... Passing null to parameter #1 ($string) of type string is deprecated`, and fills defaults.
Handlers therefore receive values that already match the declared types:

```rust
args.str(0) -> &PhpStr      args.int(1) -> i64      args.float(2) -> f64     args.bool(3) -> bool
args.array(0) -> &PhpArrayRef   args.get(i) -> &Value   args.is_passed(i) -> bool
args.by_ref(i) -> PhpRef    (for `&$param` – write through it)    args.rest(i) -> &[Value] (variadic)
```

Handlers report problems through the `Vm`:

```rust
vm.warning("message")          // "Warning: fname(): message in FILE on line N"  (php_error_docref style)
vm.notice(..) / vm.deprecated(..)
return Err(vm.throw_value_error("str_repeat(): Argument #2 ($times) must be greater than or equal to 0"));
return Err(vm.throw_type_error(..)); vm.throw(class_name, msg)
vm.call(&callable, vec![args]) -> PResult<Value>      // invoke a PHP callable (usort, array_map, ...)
vm.echo(bytes)                                         // through the output-buffer stack
vm.ini_get("precision"), vm.constant("PHP_EOL"), vm.current_function_name()
```

Modules register with `pub fn register(r: &mut Registry)` adding functions, constants, classes and ini entries.

### 2.6 Errors and output

* Error levels and display format mirror `php_error_cb` with CLI defaults
  (`display_errors=1`, `html_errors=0`): `"\nWarning: Undefined variable $x in /path/t.php on line 3\n"`.
  Uncaught exceptions: `"PHP Fatal error:  Uncaught Exception: msg in file:line\nStack trace:\n#0 {main}\n  thrown in file on line N"`.
* `set_error_handler`, `set_exception_handler`, `error_reporting`, `@` silence, `E_*` constants.
* Output goes through an `ob_*` stack (`ob_start` with callbacks, `ob_get_clean`, …) then to stdout.

### 2.7 Testing

1. Unit tests in every crate (`cargo test`).
2. The php-src conformance suite: `php-run-tests` executes `.phpt` files (same section and
   `--EXPECTF--` semantics as `run-tests.php`) against our `php` binary.
3. `tests/phpt/` holds the php-src tests we have adopted. `tests/phpt/PASSING` lists every test that
   passes; CI (and `cargo test -p php-cli`) fails if any listed test regresses. The list only grows.

## 3. Mapping: php-src → php-in-rust

| php-src | php-in-rust |
|---|---|
| `Zend/zend_language_scanner.l` | `php-syntax/src/lexer.rs` |
| `Zend/zend_language_parser.y`, `zend_ast.[ch]` | `php-syntax/src/parser/`, `ast.rs` |
| `Zend/zend_compile.c` | `php-runtime/src/compile/` (AST lowering, slot resolution, class declaration) |
| `Zend/zend_execute.c`, `zend_vm_def.h` | `php-runtime/src/interp/` |
| `Zend/zend_types.h`, `zend_hash.c`, `zend_string.c` | `php-runtime/src/value/` |
| `Zend/zend_operators.c`, `zend_strtod.c`, `main/snprintf.c` | `php-runtime/src/value/{convert,ops,float_fmt}.rs` |
| `Zend/zend_objects*.c`, `zend_object_handlers.c`, `zend_inheritance.c`, `zend_interfaces.c` | `php-runtime/src/object/`, `class/` |
| `Zend/zend_exceptions.c`, `zend_closures.c`, `zend_generators.c`, `zend_fibers.c`, `zend_enum.c`, `zend_weakrefs.c`, `zend_attributes.c` | `php-runtime/src/builtins/` |
| `Zend/zend_builtin_functions.c` | `php-runtime/src/builtins/functions.rs` |
| `main/main.c` (errors), `main/output.c` | `php-runtime/src/{errors,output}.rs` |
| `main/streams/` | `php-ext/src/standard/streams/` |
| `ext/<name>/` | `php-ext/src/<name>/` |
| `sapi/cli/` | `php-cli/` |
| `run-tests.php` | `php-phpt/` |

## 4. Scope

In scope: the PHP language (everything the Zend engine compiles and executes) and the extensions a
default CLI build exposes without external C libraries: Core, standard, spl, json, pcre, ctype, date,
random, hash, mbstring, reflection, tokenizer, filter, calendar, bcmath, pdo + pdo_sqlite / sqlite3
(database layer). Out of scope: SAPIs other than CLI (fpm, apache2handler, cgi, litespeed, phpdbg),
opcache/JIT, FFI, and extensions binding external services or C libraries (curl, gd, intl, openssl, ldap,
mysqli/pgsql drivers, imap, soap, snmp, …).
