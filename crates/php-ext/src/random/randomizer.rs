//! `Random\Randomizer` and `Random\IntervalBoundary` (`ext/random/randomizer.c`).

use php_runtime::class::{ClassEntry, Function, PropertyInfo, class_flags, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, ArrayKey, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry,
    Value, Vm,
};

use super::algo::{PHP_MT_RAND_MAX, RANGE_ATTEMPTS};
use super::check_allocation;
use super::engine::{
    BROKEN_ENGINE_ERROR, Engine, RANDOM_EXCEPTION, SECURE_CLASS, clone_forbidden, find_class,
    or_invalid_serialization,
};
use super::enums::{case_name, register_pure_enum};
use super::gammasection;
use super::props::properties_load;
use super::shuffle::{pick_keys, shuffle_bytes, shuffled_array};

pub const RANDOMIZER_CLASS: &str = "Random\\Randomizer";
pub const INTERVAL_BOUNDARY: &str = "Random\\IntervalBoundary";

/// Native state of a `Random\Randomizer` (`php_random_randomizer`).
pub struct RandomizerData {
    pub engine: Engine,
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("Randomizer method called without $this")
}

/// `randomizer_common_init`: internal classes always carry engine state;
/// anything else is a userland engine driven through `generate()`.
fn common_init(engine_object: &ObjRef) -> Engine {
    if engine_object.class().is_internal() {
        Engine::Native(engine_object.clone())
    } else {
        Engine::User(engine_object.clone())
    }
}

fn engine_of(vm: &mut Vm, this: &ObjRef) -> PResult<Engine> {
    match this.native::<RandomizerData>() {
        Some(d) => Ok(d.engine.clone()),
        None => Err(vm.throw_error(
            "Error",
            format!("Object of type {RANDOMIZER_CLASS} is not initialized"),
        )),
    }
}

fn create_randomizer(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    Ok(ObjRef::new(ce.clone()))
}

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let engine_object = match args.object_opt(0) {
        Some(o) => o.clone(),
        None => {
            let ce = find_class(vm, SECURE_CLASS);
            vm.instantiate(&ce)?
        }
    };
    // zend_update_property() on the readonly property.
    if this.get_prop(b"engine").is_some() {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot modify readonly property {RANDOMIZER_CLASS}::$engine"),
        ));
    }
    this.set_prop(b"engine", Value::Object(engine_object.clone()));
    this.set_native(RandomizerData {
        engine: common_init(&engine_object),
    });
    Ok(Value::Null)
}

fn next_int(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let r = engine.generate(vm)?;
    if r.size > std::mem::size_of::<i64>() {
        return Err(vm.throw_error(RANDOM_EXCEPTION, "Generated value exceeds size of int"));
    }
    Ok(Value::Int((r.result >> 1) as i64))
}

fn next_float(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let result = engine.gen_u64(vm)?;
    // A double has 53 bits of precision: use the upper 53 bits of the result.
    let step_size = 1.0 / (1u64 << 53) as f64;
    Ok(Value::Float(step_size * (result >> 11) as f64))
}

fn get_float(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let (min, max) = (args.float(0), args.float(1));
    let boundary = case_name(args.object(2)).unwrap_or_default();
    if !min.is_finite() {
        return Err(vm.argument_value_error(1, "must be finite"));
    }
    if !max.is_finite() {
        return Err(vm.argument_value_error(2, "must be finite"));
    }
    let r = match boundary.as_str() {
        "ClosedClosed" => {
            if max < min {
                return Err(vm.argument_value_error(
                    2,
                    "must be greater than or equal to argument #1 ($min)",
                ));
            }
            gammasection::closed_closed(vm, &engine, min, max)?
        }
        kind => {
            if max <= min {
                return Err(vm.argument_value_error(2, "must be greater than argument #1 ($min)"));
            }
            match kind {
                "OpenClosed" => gammasection::open_closed(vm, &engine, min, max)?,
                "OpenOpen" => {
                    let r = gammasection::open_open(vm, &engine, min, max)?;
                    if r.is_nan() {
                        return Err(vm.throw_value_error(
                            "The given interval is empty, there are no floats between argument #1 ($min) and argument #2 ($max)",
                        ));
                    }
                    r
                }
                _ => gammasection::closed_open(vm, &engine, min, max)?,
            }
        }
    };
    Ok(Value::Float(r))
}

fn get_int(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let (min, max) = (args.int(0), args.int(1));
    if max < min {
        return Err(
            vm.argument_value_error(2, "must be greater than or equal to argument #1 ($min)")
        );
    }
    let result = if engine.is_mt_php_mode(vm) {
        let r = engine.generate(vm)?.result >> 1;
        // Inlined RAND_RANGE_BADSCALING without undefined behaviour for
        // (max - min) > ZEND_LONG_MAX.
        let offset =
            ((max as f64 - min as f64 + 1.0) * (r as f64 / (PHP_MT_RAND_MAX as f64 + 1.0))) as u64;
        offset.wrapping_add(min as u64) as i64
    } else {
        engine.range(vm, min, max)?
    };
    Ok(Value::Int(result))
}

fn get_bytes(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let user_length = args.int(0);
    if user_length < 1 {
        return Err(vm.argument_value_error(1, "must be greater than 0"));
    }
    let length = user_length as usize;
    check_allocation(vm, length)?;
    let mut out: Vec<u8> = Vec::with_capacity(length);
    let mut total = 0usize;
    // Fast path while the engine produces full 64-bit values; a short result
    // continues in the generic loop below (`goto non_64`).
    let mut pending = None;
    while total + 8 <= length {
        let r = engine.generate(vm)?;
        if r.size != 8 {
            pending = Some(r);
            break;
        }
        out.extend_from_slice(&r.result.to_le_bytes());
        total += 8;
    }
    while total < length {
        let r = match pending.take() {
            Some(r) => r,
            None => engine.generate(vm)?,
        };
        let mut result = r.result;
        for _ in 0..r.size {
            out.push((result & 0xff) as u8);
            result >>= 8;
            total += 1;
            if total >= length {
                break;
            }
        }
    }
    Ok(Value::from(out))
}

fn get_bytes_from_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let source = args.str(0).clone();
    let user_length = args.int(1);
    if source.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    if user_length < 1 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    let max_offset = source.len() - 1;
    let length = user_length as usize;
    check_allocation(vm, length)?;
    let mut out: Vec<u8> = Vec::with_capacity(length);
    if max_offset > 0xff {
        while out.len() < length {
            let offset = engine.range(vm, 0, max_offset as i64)? as usize;
            out.push(source[offset]);
        }
    } else {
        let mut mask = max_offset as u64;
        // Copy the top-most bit into all lower bits, then expand the lowest
        // byte into all bytes.
        mask |= mask >> 1;
        mask |= mask >> 2;
        mask |= mask >> 4;
        mask = mask.wrapping_mul(0x0101_0101_0101_0101);

        let mut failures = 0u32;
        while out.len() < length {
            let result = engine.generate(vm)?;
            let mut offsets = result.result & mask;
            for _ in 0..result.size {
                let offset = (offsets & 0xff) as usize;
                offsets >>= 8;
                if offset > max_offset {
                    failures += 1;
                    if failures > RANGE_ATTEMPTS {
                        return Err(vm.throw_error(
                            BROKEN_ENGINE_ERROR,
                            format!(
                                "Failed to generate an acceptable random number in {RANGE_ATTEMPTS} attempts"
                            ),
                        ));
                    }
                    continue;
                }
                failures = 0;
                out.push(source[offset]);
                if out.len() >= length {
                    break;
                }
            }
        }
    }
    Ok(Value::from(out))
}

fn shuffle_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let array = args.array(0).clone();
    match shuffled_array(vm, &engine, &array)? {
        Some(shuffled) => Ok(Value::array(shuffled)),
        None => Ok(Value::Array(array)),
    }
}

fn shuffle_bytes_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let mut bytes = args.str(0).to_vec();
    if bytes.len() < 2 {
        return Ok(Value::String(args.str(0).clone()));
    }
    shuffle_bytes(vm, &engine, &mut bytes)?;
    Ok(Value::String(PhpStr::from_vec(bytes)))
}

fn pick_array_keys(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let engine = engine_of(vm, &this(args))?;
    let input = args.array(0).clone();
    let picked = pick_keys(vm, &engine, &input, args.int(1))?;
    // Keep compatibility, but the result is always an array.
    Ok(match picked {
        Value::Array(a) => Value::Array(a),
        key => Value::array(PhpArray::from_values(vec![key])),
    })
}

fn serialize(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mut props = PhpArray::new();
    for (name, value) in this.properties() {
        props.insert(ArrayKey::Str(name), value);
    }
    Ok(Value::array(PhpArray::from_values(vec![Value::array(
        props,
    )])))
}

fn invalid_data(vm: &mut Vm) -> Flow {
    vm.throw_error(
        "Exception",
        format!("Invalid serialization data for {RANDOMIZER_CLASS} object"),
    )
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let data = args.array(0).clone();
    if data.len() != 1 {
        return Err(invalid_data(vm));
    }
    let Some(Value::Array(members)) = data.get_int(0).map(Value::unref) else {
        return Err(invalid_data(vm));
    };
    let loaded = properties_load(vm, &this, &members);
    or_invalid_serialization(vm, RANDOMIZER_CLASS, loaded)?;
    match this.get_prop(b"engine") {
        Some(Value::Object(engine_object)) => {
            this.set_native(RandomizerData {
                engine: common_init(&engine_object),
            });
            Ok(Value::Null)
        }
        _ => Err(invalid_data(vm)),
    }
}

static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(?\\Random\\Engine $engine = null)",
    handler: construct,
};
static NEXT_INT: NativeFunction = NativeFunction {
    sig: "public function nextInt(): int",
    handler: next_int,
};
static NEXT_FLOAT: NativeFunction = NativeFunction {
    sig: "public function nextFloat(): float",
    handler: next_float,
};
static GET_FLOAT: NativeFunction = NativeFunction {
    sig: "public function getFloat(float $min, float $max, \\Random\\IntervalBoundary $boundary = \\Random\\IntervalBoundary::ClosedOpen): float",
    handler: get_float,
};
static GET_INT: NativeFunction = NativeFunction {
    sig: "public function getInt(int $min, int $max): int",
    handler: get_int,
};
static GET_BYTES: NativeFunction = NativeFunction {
    sig: "public function getBytes(int $length): string",
    handler: get_bytes,
};
static GET_BYTES_FROM_STRING: NativeFunction = NativeFunction {
    sig: "public function getBytesFromString(string $string, int $length): string",
    handler: get_bytes_from_string,
};
static SHUFFLE_ARRAY: NativeFunction = NativeFunction {
    sig: "public function shuffleArray(array $array): array",
    handler: shuffle_array,
};
static SHUFFLE_BYTES: NativeFunction = NativeFunction {
    sig: "public function shuffleBytes(string $bytes): string",
    handler: shuffle_bytes_method,
};
static PICK_ARRAY_KEYS: NativeFunction = NativeFunction {
    sig: "public function pickArrayKeys(array $array, int $num): array",
    handler: pick_array_keys,
};
static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array",
    handler: serialize,
};
static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void",
    handler: unserialize,
};

/// Registers `Random\IntervalBoundary` and `Random\Randomizer`.
pub fn register(r: &mut Registry<'_>) {
    register_pure_enum(
        r,
        INTERVAL_BOUNDARY,
        &["ClosedOpen", "ClosedClosed", "OpenClosed", "OpenOpen"],
    );

    let mut ce = ClassEntry::new(RANDOMIZER_CLASS, class_flags::FINAL);
    let mut engine = PropertyInfo::new(
        "engine",
        RANDOMIZER_CLASS,
        member_flags::PUBLIC | member_flags::READONLY,
    );
    engine.ty = Some(TypeDecl::parse("\\Random\\Engine").expect("valid type"));
    engine.default = Value::Undef;
    ce.declare_property(engine);
    for nf in [
        &CONSTRUCT,
        &NEXT_INT,
        &NEXT_FLOAT,
        &GET_FLOAT,
        &GET_INT,
        &GET_BYTES,
        &GET_BYTES_FROM_STRING,
        &SHUFFLE_ARRAY,
        &SHUFFLE_BYTES,
        &PICK_ARRAY_KEYS,
        &SERIALIZE,
        &UNSERIALIZE,
    ] {
        ce.add_method(Function::native_method(
            nf,
            RANDOMIZER_CLASS,
            member_flags::PUBLIC,
        ));
    }
    ce.handlers.create_object = Some(create_randomizer);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}
