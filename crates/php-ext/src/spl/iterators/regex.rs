//! `RegexIterator` and `RecursiveRegexIterator`: filter (and transform) the
//! elements of the inner iterator with a PCRE pattern.
//!
//! The pattern is compiled at construction with `php-regex` (compile
//! warnings become an `InvalidArgumentException`, like php-src's
//! `EH_THROW`). `MATCH` mode matches directly (`pcre2_match()`, which does
//! not touch `preg_last_error()`); `GET_MATCH`, `ALL_MATCHES`, `SPLIT` and
//! `REPLACE` reuse ext/pcre through `preg_match()`, `preg_match_all()`,
//! `preg_split()` and `preg_replace()` (php-src calls the same
//! `php_pcre_*_impl()` functions), after validating the flags the way
//! `php_pcre_match_impl()` reports them for `RegexIterator::accept()`.

use php_regex::{MatchOptions, Regex, compile_php_pattern};
use php_runtime::class::{ClassConstant, PropertyInfo, member_flags};
use php_runtime::errors::ErrorHandlingMode;
use php_runtime::types::TypeDecl;
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpRef, PhpStr, Registry, Value, Vm};

use super::dual::{self, DitType, checked, with};
use super::inner::object_init_with_constructor;
use crate::spl::engine;

pub const REGIT_USE_KEY: i64 = 0x0000_0001;
pub const REGIT_INVERTED: i64 = 0x0000_0002;

pub const REGIT_MODE_MATCH: i64 = 0;
pub const REGIT_MODE_GET_MATCH: i64 = 1;
pub const REGIT_MODE_ALL_MATCHES: i64 = 2;
pub const REGIT_MODE_SPLIT: i64 = 3;
pub const REGIT_MODE_REPLACE: i64 = 4;
pub const REGIT_MODE_MAX: i64 = 5;

const MODE_ERROR: &str = "must be RegexIterator::MATCH, RegexIterator::GET_MATCH, \
     RegexIterator::ALL_MATCHES, RegexIterator::SPLIT, or RegexIterator::REPLACE";

/// `u.regex` of `spl_dual_it_object`.
pub struct RegexState {
    pub flags: i64,
    pub preg_flags: i64,
    /// The pattern as given (`getRegex()`).
    pub regex: PhpStr,
    pub mode: i64,
    /// The compiled pattern (`pce`).
    pub compiled: Regex,
}

/// Argument parsing and pattern compilation of `spl_dual_it_construct` for
/// the regex iterators.
pub fn construct_state(vm: &mut Vm, args: &Args) -> PResult<RegexState> {
    let regex = args.str(1).clone();
    let mode = args.int(2);
    let flags = args.int(3);
    let preg_flags = args.int(4);
    if !(0..REGIT_MODE_MAX).contains(&mode) {
        return Err(vm.argument_value_error(3, MODE_ERROR));
    }
    // pcre_get_compiled_regex_cache() might emit E_WARNINGs that we want
    // to promote to exceptions.
    let compiled = match compile_php_pattern(&regex) {
        Ok(p) => p.regex,
        Err(e) => {
            let saved = std::mem::replace(
                &mut vm.errors.mode,
                ErrorHandlingMode::Throw(PhpStr::from("InvalidArgumentException")),
            );
            let r = vm.warning(e.message_bytes());
            vm.errors.mode = saved;
            r?;
            return Err(vm.throw_error(
                "InvalidArgumentException",
                String::from_utf8_lossy(&e.message_bytes()).into_owned(),
            ));
        }
    };
    Ok(RegexState {
        flags,
        preg_flags,
        regex,
        mode,
        compiled,
    })
}

/// An ini integer, or `default` when the directive is unknown.
fn ini_limit(vm: &Vm, name: &str, default: u32) -> u32 {
    match vm.ini_get(name) {
        Some(_) => vm.ini_int(name) as u32,
        None => default,
    }
}

fn regex_field<R>(obj: &ObjRef, f: impl FnOnce(&mut RegexState) -> R) -> R {
    with(obj, |s| {
        f(s.regex.as_mut().expect("constructed regex iterator"))
    })
}

/// `PHP_METHOD(RegexIterator, accept)`.
fn regex_accept(vm: &mut Vm, this: &ObjRef) -> PResult<Value> {
    let (data, key) = with(this, |s| (s.data.clone(), s.key.clone()));
    let Some(data) = data else {
        return Ok(Value::Bool(false));
    };
    let (flags, preg_flags, mode, pattern, compiled) = regex_field(this, |r| {
        (
            r.flags,
            r.preg_flags,
            r.mode,
            r.regex.clone(),
            r.compiled.clone(),
        )
    });
    let subject = if flags & REGIT_USE_KEY != 0 {
        vm.to_php_string(&key.unwrap_or(Value::Undef))?
    } else {
        let data = data.unref();
        if matches!(data, Value::Array(_)) {
            return Ok(Value::Bool(false));
        }
        vm.to_php_string(&data)?
    };

    let mut result = match mode {
        REGIT_MODE_ALL_MATCHES | REGIT_MODE_GET_MATCH => {
            let global = mode == REGIT_MODE_ALL_MATCHES;
            let matches = PhpRef::new(Value::empty_array());
            with(this, |s| s.data = Some(Value::empty_array()));
            // php_pcre_match_impl()'s flag check, reported for accept().
            if preg_flags != 0 && preg_flags & 0xff != 0 {
                let order = preg_flags & 0xff;
                if (global && !(1..=2).contains(&order)) || (!global && order != 0) {
                    return Err(vm.argument_value_error(4, "must be a PREG_* constant"));
                }
            }
            let func = if global {
                "preg_match_all"
            } else {
                "preg_match"
            };
            let count = vm.call_native(
                func,
                vec![
                    Value::String(pattern),
                    Value::String(subject),
                    Value::Ref(matches.clone()),
                    Value::Int(preg_flags),
                    Value::Int(0),
                ],
            )?;
            with(this, |s| s.data = Some(matches.get()));
            matches!(count, Value::Int(n) if n > 0)
        }
        REGIT_MODE_SPLIT => {
            with(this, |s| s.data = None);
            let parts = vm.call_native(
                "preg_split",
                vec![
                    Value::String(pattern),
                    Value::String(subject),
                    Value::Int(-1),
                    Value::Int(preg_flags),
                ],
            )?;
            let count = parts.as_array().map_or(0, |a| a.len());
            with(this, |s| s.data = Some(parts));
            count > 1
        }
        REGIT_MODE_REPLACE => {
            let replacement = this
                .get_prop(b"replacement")
                .map_or(Value::Null, |v| v.unref());
            let replacement = vm.to_php_string(&replacement)?;
            let count = PhpRef::new(Value::Int(0));
            let ret = vm.call_native(
                "preg_replace",
                vec![
                    Value::String(pattern),
                    Value::String(replacement),
                    Value::String(subject),
                    Value::Int(-1),
                    Value::Ref(count.clone()),
                ],
            )?;
            let Value::String(replaced) = ret else {
                return Ok(Value::Bool(false));
            };
            if flags & REGIT_USE_KEY != 0 {
                with(this, |s| s.key = Some(Value::String(replaced)));
            } else {
                with(this, |s| s.data = Some(Value::String(replaced)));
            }
            matches!(count.get(), Value::Int(n) if n > 0)
        }
        _ => {
            // REGIT_MODE_MATCH: pcre2_match() with the ini limits.
            let opts = MatchOptions {
                match_limit: ini_limit(vm, "pcre.backtrack_limit", 1_000_000),
                depth_limit: ini_limit(vm, "pcre.recursion_limit", 100_000),
                ..MatchOptions::default()
            };
            matches!(compiled.match_at(&subject, 0, opts), Ok(Some(_)))
        }
    };
    if flags & REGIT_INVERTED != 0 {
        result = !result;
    }
    Ok(Value::Bool(result))
}

fn ri_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dual::construct(vm, args, "RegexIterator", "Iterator", DitType::Regex)?;
    Ok(Value::Null)
}

/// Matches `(string)current()` (or the key) against the pattern.
fn ri_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    regex_accept(vm, &this)
}

fn ri_get_regex(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::String(regex_field(&this, |r| r.regex.clone())))
}

fn ri_get_mode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Int(regex_field(&this, |r| r.mode)))
}

fn ri_set_mode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    if !(0..REGIT_MODE_MAX).contains(&mode) {
        return Err(vm.argument_value_error(1, MODE_ERROR));
    }
    let this = checked(vm, args)?;
    regex_field(&this, |r| r.mode = mode);
    Ok(Value::Null)
}

fn ri_get_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Int(regex_field(&this, |r| r.flags)))
}

fn ri_set_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = args.int(0);
    let this = checked(vm, args)?;
    regex_field(&this, |r| r.flags = flags);
    Ok(Value::Null)
}

fn ri_get_preg_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Int(regex_field(&this, |r| r.preg_flags)))
}

fn ri_set_preg_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let preg_flags = args.int(0);
    let this = checked(vm, args)?;
    regex_field(&this, |r| r.preg_flags = preg_flags);
    Ok(Value::Null)
}

static RI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator, string $pattern, int $mode = RegexIterator::MATCH, int $flags = 0, int $pregFlags = 0) {}",
    handler: ri_construct,
};
static RI_ACCEPT: NativeFunction = NativeFunction {
    sig: "public function accept(): bool {}",
    handler: ri_accept,
};
static RI_GET_MODE: NativeFunction = NativeFunction {
    sig: "public function getMode(): int {}",
    handler: ri_get_mode,
};
static RI_SET_MODE: NativeFunction = NativeFunction {
    sig: "public function setMode(int $mode): void {}",
    handler: ri_set_mode,
};
static RI_GET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function getFlags(): int {}",
    handler: ri_get_flags,
};
static RI_SET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function setFlags(int $flags): void {}",
    handler: ri_set_flags,
};
static RI_GET_REGEX: NativeFunction = NativeFunction {
    sig: "public function getRegex(): string {}",
    handler: ri_get_regex,
};
static RI_GET_PREG_FLAGS: NativeFunction = NativeFunction {
    sig: "public function getPregFlags(): int {}",
    handler: ri_get_preg_flags,
};
static RI_SET_PREG_FLAGS: NativeFunction = NativeFunction {
    sig: "public function setPregFlags(int $pregFlags): void {}",
    handler: ri_set_preg_flags,
};

// ---------------------------------------------------------------------------
// RecursiveRegexIterator
// ---------------------------------------------------------------------------

fn rri_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dual::construct(
        vm,
        args,
        "RecursiveRegexIterator",
        "RecursiveIterator",
        DitType::RecursiveRegex,
    )?;
    Ok(Value::Null)
}

/// Arrays (children) are accepted when not empty; other elements go
/// through `RegexIterator::accept()` (not a possible override).
fn rri_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let data = with(&this, |s| s.data.as_ref().map(Value::unref));
    match data {
        None => Ok(Value::Bool(false)),
        Some(Value::Array(a)) => Ok(Value::Bool(!a.is_empty())),
        Some(_) => regex_accept(vm, &this),
    }
}

fn rri_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let inner = dual::inner_obj(&this).expect("constructed regex iterator");
    let children = vm
        .call_method(&inner, b"getChildren", Vec::new())?
        .into_unref();
    let (regex, mode, flags, preg_flags) =
        regex_field(&this, |r| (r.regex.clone(), r.mode, r.flags, r.preg_flags));
    let ce = this.class().clone();
    let obj = object_init_with_constructor(
        vm,
        &ce,
        vec![
            children,
            Value::String(regex),
            Value::Int(mode),
            Value::Int(flags),
            Value::Int(preg_flags),
        ],
    )?;
    Ok(Value::Object(obj))
}

static RRI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(RecursiveIterator $iterator, string $pattern, int $mode = RecursiveRegexIterator::MATCH, int $flags = 0, int $pregFlags = 0) {}",
    handler: rri_construct,
};
static RRI_ACCEPT: NativeFunction = NativeFunction {
    sig: "public function accept(): bool {}",
    handler: rri_accept,
};
static RRI_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: dual::rfi_has_children,
};
static RRI_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): RecursiveRegexIterator {}",
    handler: rri_get_children,
};

pub fn register(r: &mut Registry) {
    let parent = engine::class(r, "FilterIterator");
    let mut ce = php_runtime::class::ClassEntry::new_subclass("RegexIterator", 0, &parent);
    for (name, value) in [
        ("USE_KEY", REGIT_USE_KEY),
        ("INVERT_MATCH", REGIT_INVERTED),
        ("MATCH", REGIT_MODE_MATCH),
        ("GET_MATCH", REGIT_MODE_GET_MATCH),
        ("ALL_MATCHES", REGIT_MODE_ALL_MATCHES),
        ("SPLIT", REGIT_MODE_SPLIT),
        ("REPLACE", REGIT_MODE_REPLACE),
    ] {
        ce.add_constant(ClassConstant::new(name, Value::Int(value), "RegexIterator"));
    }
    let mut prop = PropertyInfo::new("replacement", "RegexIterator", member_flags::PUBLIC);
    prop.ty = Some(TypeDecl::parse("?string").expect("valid type"));
    prop.default = Value::Null;
    ce.declare_property(prop);
    engine::add_methods(
        &mut ce,
        &[
            &RI_CONSTRUCT,
            &RI_ACCEPT,
            &RI_GET_MODE,
            &RI_SET_MODE,
            &RI_GET_FLAGS,
            &RI_SET_FLAGS,
            &RI_GET_REGEX,
            &RI_GET_PREG_FLAGS,
            &RI_SET_PREG_FLAGS,
        ],
    );
    r.class(ce);

    dual::subclass(
        r,
        "RecursiveRegexIterator",
        0,
        "RegexIterator",
        &[super::RECURSIVE_ITERATOR],
        &[
            &RRI_CONSTRUCT,
            &RRI_ACCEPT,
            &RRI_HAS_CHILDREN,
            &RRI_GET_CHILDREN,
        ],
    );
}
