//! The `PhpToken` class (`PHP_METHOD(PhpToken, ...)` in `tokenizer.c`).

use php_runtime::class::{ClassEntry, Function, PropertyInfo, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{Args, ClassRef, NativeFunction, ObjRef, PResult, PhpStr, Registry, Value, Vm};
use php_syntax::TokenKind;

use super::data::get_token_type_name;

/// Declared slots of the four properties (`OBJ_PROP_NUM(obj, n)`).
pub const SLOT_ID: usize = 0;
pub const SLOT_TEXT: usize = 1;
pub const SLOT_LINE: usize = 2;
pub const SLOT_POS: usize = 3;

/// Stores the four standard properties of a freshly allocated token object.
pub fn init_token(obj: &ObjRef, id: i64, text: PhpStr, line: i64, pos: i64) {
    let mut props = obj.obj().props.borrow_mut();
    props[SLOT_ID] = Value::Int(id);
    props[SLOT_TEXT] = Value::String(text);
    props[SLOT_LINE] = Value::Int(line);
    props[SLOT_POS] = Value::Int(pos);
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("PhpToken method called without $this")
}

fn uninitialized(vm: &mut Vm, prop: &str) -> php_runtime::Flow {
    vm.throw_error(
        "Error",
        format!("Typed property PhpToken::${prop} must not be accessed before initialization"),
    )
}

/// `php_token_get_id()`.
fn token_id(vm: &mut Vm, obj: &ObjRef) -> PResult<i64> {
    let v = obj.obj().props.borrow().get(SLOT_ID).map(Value::unref);
    match v {
        Some(Value::Int(n)) => Ok(n),
        _ => Err(uninitialized(vm, "id")),
    }
}

/// `php_token_get_text()`.
fn token_text(vm: &mut Vm, obj: &ObjRef) -> PResult<PhpStr> {
    let v = obj.obj().props.borrow().get(SLOT_TEXT).map(Value::unref);
    match v {
        Some(Value::String(s)) => Ok(s),
        _ => Err(uninitialized(vm, "text")),
    }
}

/// `PhpToken::tokenize()`.
fn tokenize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let source = args.str(0).clone();
    let flags = args.int(1);
    let class: ClassRef = match args.scope() {
        Some(c) => c.clone(),
        None => vm
            .find_class(b"PhpToken")
            .expect("PhpToken is registered by the tokenizer module"),
    };

    // Check construction preconditions in advance, so these are not repeated for each token.
    if class.is_abstract() {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot instantiate abstract class {}", class.name),
        ));
    }
    // TODO(runtime): `zend_update_class_constants(token_class)` (evaluating the
    // constant expressions of default property values) needs a runtime hook.

    super::tokenize_common(vm, &source, flags, Some(&class))
}

/// `PhpToken::__construct()`.
fn construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    init_token(
        &obj,
        args.int(0),
        args.str(1).clone(),
        args.int(2),
        args.int(3),
    );
    Ok(Value::Null)
}

/// `PhpToken::is()`.
fn is(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let kind = args.get(0).unref();
    match &kind {
        Value::Int(want) => Ok(Value::Bool(token_id(vm, &obj)? == *want)),
        Value::String(want) => Ok(Value::Bool(
            token_text(vm, &obj)?.as_bytes() == want.as_bytes(),
        )),
        Value::Array(entries) => {
            let mut id: Option<i64> = None;
            let mut text: Option<PhpStr> = None;
            for entry in entries.values() {
                match entry.unref() {
                    Value::Int(want) => {
                        let have = match id {
                            Some(have) => have,
                            None => *id.insert(token_id(vm, &obj)?),
                        };
                        if have == want {
                            return Ok(Value::Bool(true));
                        }
                    }
                    Value::String(want) => {
                        let have = match &text {
                            Some(have) => have.clone(),
                            None => text.insert(token_text(vm, &obj)?).clone(),
                        };
                        if have.as_bytes() == want.as_bytes() {
                            return Ok(Value::Bool(true));
                        }
                    }
                    other => {
                        return Err(vm.argument_type_error(
                            1,
                            &format!(
                                "must only have elements of type string|int, {} given",
                                other.value_name()
                            ),
                        ));
                    }
                }
            }
            Ok(Value::Bool(false))
        }
        other => Err(vm.argument_type_error(
            1,
            &format!(
                "must be of type string|int|array, {} given",
                other.value_name()
            ),
        )),
    }
}

/// `PhpToken::isIgnorable()`.
fn is_ignorable(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let id = token_id(vm, &this(args))?;
    Ok(Value::Bool(
        [
            TokenKind::Whitespace,
            TokenKind::Comment,
            TokenKind::DocComment,
            TokenKind::OpenTag,
        ]
        .iter()
        .any(|k| i64::from(k.id()) == id),
    ))
}

/// `PhpToken::getTokenName()`.
fn get_token_name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let id = token_id(vm, &this(args))?;
    if id < 256 {
        // RETURN_CHAR(): the id truncated to a byte.
        return Ok(Value::bytes(&[id as u8]));
    }
    // `get_token_type_name(int)` receives the id converted to `int`.
    Ok(match get_token_type_name(id as i32) {
        Some(name) => Value::str(name),
        None => Value::Null,
    })
}

/// `PhpToken::__toString()`.
fn to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(token_text(vm, &this(args))?))
}

static TOKENIZE: NativeFunction = NativeFunction {
    sig: "public static function tokenize(string $code, int $flags = 0): array",
    handler: tokenize,
};
static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "final public function __construct(int $id, string $text, int $line = -1, int $pos = -1)",
    handler: construct,
};
static IS: NativeFunction = NativeFunction {
    sig: "public function is($kind): bool",
    handler: is,
};
static IS_IGNORABLE: NativeFunction = NativeFunction {
    sig: "public function isIgnorable(): bool",
    handler: is_ignorable,
};
static GET_TOKEN_NAME: NativeFunction = NativeFunction {
    sig: "public function getTokenName(): ?string",
    handler: get_token_name,
};
static TO_STRING: NativeFunction = NativeFunction {
    sig: "public function __toString(): string",
    handler: to_string,
};

/// `register_class_PhpToken(zend_ce_stringable)`.
pub fn register(r: &mut Registry) {
    let stringable = r
        .find_class("Stringable")
        .expect("Stringable is registered by the engine");
    let mut ce = ClassEntry::new("PhpToken", 0);
    ce.interfaces.push(stringable);

    // Typed properties without defaults: uninitialised until the constructor runs.
    for (name, ty) in [
        ("id", "int"),
        ("text", "string"),
        ("line", "int"),
        ("pos", "int"),
    ] {
        let mut info = PropertyInfo::new(name, "PhpToken", member_flags::PUBLIC);
        info.ty = Some(TypeDecl::parse(ty).expect("valid type"));
        info.default = Value::Undef;
        ce.declare_property(info);
    }

    use member_flags::{FINAL, PUBLIC, STATIC};
    for (nf, flags) in [
        (&TOKENIZE, PUBLIC | STATIC),
        (&CONSTRUCT, PUBLIC | FINAL),
        (&IS, PUBLIC),
        (&IS_IGNORABLE, PUBLIC),
        (&GET_TOKEN_NAME, PUBLIC),
        (&TO_STRING, PUBLIC),
    ] {
        ce.add_method(Function::native_method(nf, "PhpToken", flags));
    }
    r.class(ce);
}
