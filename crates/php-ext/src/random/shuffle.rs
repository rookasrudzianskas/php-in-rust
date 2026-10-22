//! The shuffling and sampling primitives built on the random engines:
//! `php_array_data_shuffle` and `php_array_pick_keys` (`ext/standard/array.c`),
//! `php_binary_string_shuffle` (`ext/standard/string.c`), and the functions
//! `shuffle()`, `array_rand()` and `str_shuffle()` that use them with the
//! global Mt19937.

use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::algo::RANGE_ATTEMPTS;
use super::engine::{BROKEN_ENGINE_ERROR, Engine};

/// `php_array_data_shuffle` on a list of values: Fisher-Yates from the end,
/// `range(0, n_left)` per step.
pub fn shuffle_values(vm: &mut Vm, engine: &Engine, values: &mut [Value]) -> PResult<()> {
    let mut n_left = values.len();
    if n_left < 2 {
        return Ok(());
    }
    loop {
        n_left -= 1;
        if n_left == 0 {
            return Ok(());
        }
        let rnd_idx = engine.range(vm, 0, n_left as i64)? as usize;
        if rnd_idx != n_left {
            values.swap(n_left, rnd_idx);
        }
    }
}

/// `php_array_data_shuffle`: the values of `array` in random order, renumbered
/// from 0. Returns `None` for an empty array (php-src leaves it untouched).
pub fn shuffled_array(vm: &mut Vm, engine: &Engine, array: &PhpArray) -> PResult<Option<PhpArray>> {
    if array.is_empty() {
        return Ok(None);
    }
    let mut values: Vec<Value> = array.values().cloned().collect();
    shuffle_values(vm, engine, &mut values)?;
    Ok(Some(PhpArray::from_values(values)))
}

/// `php_binary_string_shuffle`.
pub fn shuffle_bytes(vm: &mut Vm, engine: &Engine, bytes: &mut [u8]) -> PResult<()> {
    let mut n_left = bytes.len();
    if n_left <= 1 {
        return Ok(());
    }
    loop {
        n_left -= 1;
        if n_left == 0 {
            return Ok(());
        }
        let rnd_idx = engine.range(vm, 0, n_left as i64)? as usize;
        if rnd_idx != n_left {
            bytes.swap(n_left, rnd_idx);
        }
    }
}

/// `php_array_pick_keys` (non-silent): a single key for `num_req == 1`,
/// otherwise a list of `num_req` keys in array order.
///
/// Arrays are treated as compact (no deleted slots), so `array_rand($a)`
/// consumes the same random numbers as php-src does for an array without holes.
pub fn pick_keys(vm: &mut Vm, engine: &Engine, input: &PhpArray, num_req: i64) -> PResult<Value> {
    let num_avail = input.len();
    if num_avail == 0 {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    if num_req == 1 {
        let randval = engine.range(vm, 0, num_avail as i64 - 1)? as usize;
        let key = input
            .keys()
            .nth(randval)
            .expect("random index within the array");
        return Ok(key.into_value());
    }
    if num_req <= 0 || num_req as u64 > num_avail as u64 {
        return Err(vm.argument_value_error(
            2,
            "must be between 1 and the number of elements in argument #1 ($array)",
        ));
    }
    let mut num_req = num_req as usize;
    let mut negative_bitset = false;
    if num_req > (num_avail >> 1) {
        negative_bitset = true;
        num_req = num_avail - num_req;
    }
    let mut bitset = vec![false; num_avail];
    let mut remaining = num_req;
    let mut failures = 0u32;
    while remaining > 0 {
        let randval = engine.range(vm, 0, num_avail as i64 - 1)? as usize;
        if bitset[randval] {
            failures += 1;
            if failures > RANGE_ATTEMPTS {
                return Err(vm.throw_error(
                    BROKEN_ENGINE_ERROR,
                    format!(
                        "Failed to generate an acceptable random number in {RANGE_ATTEMPTS} attempts"
                    ),
                ));
            }
        } else {
            bitset[randval] = true;
            remaining -= 1;
            failures = 0;
        }
    }
    let keys: Vec<Value> = input
        .keys()
        .enumerate()
        .filter(|(i, _)| bitset[*i] ^ negative_bitset)
        .map(|(_, k)| k.into_value())
        .collect();
    Ok(Value::array(PhpArray::from_values(keys)))
}

// ---------------------------------------------------------------------------
// shuffle(), array_rand(), str_shuffle()
// ---------------------------------------------------------------------------

fn shuffle(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let r = args.by_ref(0);
    let Value::Array(array) = r.get() else {
        return Ok(Value::Bool(true));
    };
    if let Some(shuffled) = shuffled_array(vm, &Engine::Global, &array)? {
        r.set(Value::array(shuffled));
    }
    Ok(Value::Bool(true))
}

fn array_rand(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    pick_keys(vm, &Engine::Global, &input, args.int(1))
}

fn str_shuffle(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut bytes = args.str(0).to_vec();
    if bytes.len() > 1 {
        shuffle_bytes(vm, &Engine::Global, &mut bytes)?;
    }
    Ok(Value::String(PhpStr::from_vec(bytes)))
}

pub static SHUFFLE: NativeFunction = NativeFunction {
    sig: "shuffle(array &$array): true",
    handler: shuffle,
};
pub static ARRAY_RAND: NativeFunction = NativeFunction {
    sig: "array_rand(array $array, int $num = 1): int|string|array",
    handler: array_rand,
};
pub static STR_SHUFFLE: NativeFunction = NativeFunction {
    sig: "str_shuffle(string $string): string",
    handler: str_shuffle,
};

/// Registers `shuffle`, `array_rand` and `str_shuffle` (they live in
/// `ext/standard` in php-src but are driven by the random engines).
pub fn register(r: &mut Registry<'_>) {
    r.function(&SHUFFLE);
    r.function(&ARRAY_RAND);
    r.function(&STR_SHUFFLE);
}
