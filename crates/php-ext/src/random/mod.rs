//! ext/random — pseudo-random generators, the CSPRNG helpers, the
//! `Random\*` classes and the global random functions (`lcg_value`,
//! `mt_srand`, `mt_rand`, `rand`, `srand`, `random_int`, `random_bytes`, ...).
//!
//! `shuffle()`, `array_rand()` and `str_shuffle()` belong to ext/standard in
//! php-src but are driven by the engines; they are implemented in
//! [`shuffle`] and registered with the rest of this module.

pub mod algo;
pub mod engine;
pub mod enums;
pub mod gammasection;
pub mod props;
pub mod randomizer;
pub mod shuffle;

#[cfg(test)]
mod tests;

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use algo::{MT_RAND_MT19937, MT_RAND_PHP, PHP_MT_RAND_MAX, random_bytes, random_int};
use engine::{Engine, globals, throw_random_exception};

/// The `memory_limit` in bytes (`None` when unlimited).
fn memory_limit(vm: &Vm) -> Option<usize> {
    let raw = vm.ini_get("memory_limit")?;
    let s = raw.to_string_lossy();
    let s = s.trim();
    let (digits, mult) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1usize << 10),
        'm' | 'M' => (&s[..s.len() - 1], 1usize << 20),
        'g' | 'G' => (&s[..s.len() - 1], 1usize << 30),
        _ => (s, 1),
    };
    let n: i64 = digits.trim().parse().ok()?;
    if n < 0 {
        None
    } else {
        Some((n as usize).saturating_mul(mult))
    }
}

/// Fails like the Zend memory manager when a single allocation of `size`
/// bytes exceeds `memory_limit`.
pub(crate) fn check_allocation(vm: &mut Vm, size: usize) -> PResult<()> {
    if let Some(limit) = memory_limit(vm)
        && size > limit
    {
        return Err(vm.fatal_error(format!(
            "Allowed memory size of {limit} bytes exhausted (tried to allocate {size} bytes)"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

fn lcg_value(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // The `#[\Deprecated]` attribute of php-src's stub.
    vm.engine_deprecated(
        "Function lcg_value() is deprecated since 8.4, use \\Random\\Randomizer::getFloat() instead",
    )?;
    Ok(Value::Float(globals(vm).combined_lcg()))
}

fn mt_srand(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(1);
    if mode == MT_RAND_PHP as i64 {
        globals(vm).mt19937.mode = MT_RAND_PHP;
        vm.engine_deprecated("The MT_RAND_PHP variant of Mt19937 is deprecated")?;
    } else {
        globals(vm).mt19937.mode = MT_RAND_MT19937;
    }
    let g = globals(vm);
    match args.int_opt(0) {
        Some(seed) => g.mt19937.seed32(seed as u32),
        None => g.seed_mt_default(),
    }
    g.mt19937_seeded = true;
    Ok(Value::Null)
}

/// `php_mt_rand_common`: `rand()` semantics (the legacy scaling for MT_RAND_PHP).
pub fn mt_rand_common(vm: &mut Vm, min: i64, max: i64) -> PResult<i64> {
    let mode = globals(vm).default_mt().mode;
    if mode == MT_RAND_MT19937 {
        return Engine::Global.php_random_range(vm, min, max);
    }
    let r = globals(vm).default_mt().generate().result >> 1;
    // Inlined RAND_RANGE_BADSCALING without undefined behaviour for
    // (max - min) > ZEND_LONG_MAX.
    let offset =
        ((max as f64 - min as f64 + 1.0) * (r as f64 / (PHP_MT_RAND_MAX as f64 + 1.0))) as u64;
    Ok(offset.wrapping_add(min as u64) as i64)
}

fn mt_rand_no_args(vm: &mut Vm) -> Value {
    // genrand_int31 in mt19937ar.c performs a right shift
    Value::Int((globals(vm).default_mt().generate().result as u32 >> 1) as i64)
}

fn wrong_count(vm: &mut Vm, name: &str, given: usize) -> php_runtime::Flow {
    vm.argument_count_error(format!(
        "{name}() expects exactly 2 arguments, {given} given"
    ))
}

fn mt_rand(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.len() {
        0 => Ok(mt_rand_no_args(vm)),
        2 => {
            let (min, max) = (args.int(0), args.int(1));
            if max < min {
                return Err(vm.argument_value_error(
                    2,
                    "must be greater than or equal to argument #1 ($min)",
                ));
            }
            Ok(Value::Int(mt_rand_common(vm, min, max)?))
        }
        n => Err(wrong_count(vm, "mt_rand", n)),
    }
}

fn rand(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.len() {
        0 => Ok(mt_rand_no_args(vm)),
        2 => {
            let (min, max) = (args.int(0), args.int(1));
            let v = if max < min {
                mt_rand_common(vm, max, min)?
            } else {
                mt_rand_common(vm, min, max)?
            };
            Ok(Value::Int(v))
        }
        n => Err(wrong_count(vm, "rand", n)),
    }
}

fn mt_getrandmax(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // It could be 2^32, but 2^31 maintains compatibility with the previous php_rand.
    Ok(Value::Int(PHP_MT_RAND_MAX))
}

fn random_bytes_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let size = args.int(0);
    if size < 1 {
        return Err(vm.argument_value_error(1, "must be greater than 0"));
    }
    let size = size as usize;
    check_allocation(vm, size)?;
    let mut bytes = vec![0u8; size];
    match random_bytes(&mut bytes) {
        Ok(()) => Ok(Value::from(bytes)),
        Err(e) => Err(throw_random_exception(vm, &e)),
    }
}

fn random_int_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (min, max) = (args.int(0), args.int(1));
    if min > max {
        return Err(vm.argument_value_error(1, "must be less than or equal to argument #2 ($max)"));
    }
    match random_int(min, max) {
        Ok(v) => Ok(Value::Int(v)),
        Err(e) => Err(throw_random_exception(vm, &e)),
    }
}

pub static LCG_VALUE: NativeFunction = NativeFunction {
    sig: "lcg_value(): float",
    handler: lcg_value,
};
pub static MT_SRAND: NativeFunction = NativeFunction {
    sig: "mt_srand(?int $seed = null, int $mode = MT_RAND_MT19937): void",
    handler: mt_srand,
};
pub static SRAND: NativeFunction = NativeFunction {
    sig: "srand(?int $seed = null, int $mode = MT_RAND_MT19937): void",
    handler: mt_srand,
};
pub static RAND: NativeFunction = NativeFunction {
    sig: "rand(int $min = UNKNOWN, int $max = UNKNOWN): int",
    handler: rand,
};
pub static MT_RAND: NativeFunction = NativeFunction {
    sig: "mt_rand(int $min = UNKNOWN, int $max = UNKNOWN): int",
    handler: mt_rand,
};
pub static MT_GETRANDMAX: NativeFunction = NativeFunction {
    sig: "mt_getrandmax(): int",
    handler: mt_getrandmax,
};
pub static GETRANDMAX: NativeFunction = NativeFunction {
    sig: "getrandmax(): int",
    handler: mt_getrandmax,
};
pub static RANDOM_BYTES: NativeFunction = NativeFunction {
    sig: "random_bytes(int $length): string",
    handler: random_bytes_fn,
};
pub static RANDOM_INT: NativeFunction = NativeFunction {
    sig: "random_int(int $min, int $max): int",
    handler: random_int_fn,
};

pub fn register(r: &mut Registry<'_>) {
    r.constant("MT_RAND_MT19937", MT_RAND_MT19937 as i64);
    r.deprecated_constant(
        "MT_RAND_PHP",
        MT_RAND_PHP as i64,
        "since 8.3, as it uses a biased non-standard variant of Mt19937",
    );
    r.functions(&[
        &LCG_VALUE,
        &MT_SRAND,
        &SRAND,
        &RAND,
        &MT_RAND,
        &MT_GETRANDMAX,
        &GETRANDMAX,
        &RANDOM_BYTES,
        &RANDOM_INT,
    ]);
    engine::register(r);
    randomizer::register(r);
    shuffle::register(r);
}
