//! ext/bcmath — arbitrary precision decimal arithmetic.
//!
//! * [`num`] ports libbcmath (`libbcmath/src/*.c`), [`digits`] its
//!   multiplication and division kernels.
//! * This module holds the procedural API (`bcadd()` & co., `bcmath.scale`)
//!   from `bcmath.c`; [`number`] holds the final class `BcMath\Number`.

pub mod digits;
pub mod num;
pub mod number;

#[cfg(test)]
mod tests;

use php_runtime::ini::{IniDef, atol_quantity, modifiable};
use php_runtime::{Args, Flow, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use num::{AllocError, BcNum, RaiseError, RaiseModError, RoundMode};

/// `INT_MAX`: the largest scale.
pub const INT_MAX: i64 = i32::MAX as i64;

/// `OnUpdateScale`: the value must be between 0 and `INT_MAX`.
fn validate_scale(v: &[u8]) -> bool {
    (0..=INT_MAX).contains(&atol_quantity(v))
}

/// `BCG(bc_precision)`.
pub(crate) fn default_scale(vm: &Vm) -> usize {
    vm.ini_int("bcmath.scale").clamp(0, INT_MAX) as usize
}

/// The `memory_limit` in bytes (`None` when unlimited).
pub(crate) fn memory_limit(vm: &Vm) -> Option<usize> {
    vm.ini_get("memory_limit")?;
    let limit = vm.ini_int("memory_limit");
    (limit >= 0).then_some(limit as usize)
}

/// The fatal error the Zend memory manager raises for an allocation of
/// `bytes` bytes beyond `memory_limit`.
pub(crate) fn memory_exhausted(vm: &mut Vm, bytes: usize) -> Flow {
    match memory_limit(vm) {
        Some(limit) => vm.fatal_error(format!(
            "Allowed memory size of {limit} bytes exhausted (tried to allocate {bytes} bytes)"
        )),
        None => vm.fatal_error(format!(
            "Out of memory (allocated 0) (tried to allocate {bytes} bytes)"
        )),
    }
}

/// Fails like `emalloc(bytes)` when `bytes` exceeds `memory_limit`.
pub(crate) fn check_alloc(vm: &mut Vm, bytes: usize) -> PResult<()> {
    match memory_limit(vm) {
        Some(limit) if bytes > limit => Err(memory_exhausted(vm, bytes)),
        _ => Ok(()),
    }
}

/// A result string of about `digits + scale` bytes (+ the `zend_string` header).
pub(crate) fn check_result_size(vm: &mut Vm, digits: usize, scale: usize) -> PResult<()> {
    check_alloc(vm, digits.saturating_add(scale).saturating_add(24 + 8))
}

/// `bcmath_check_scale`: the scale argument `arg_num` (1-based) or the default.
fn scale_arg(vm: &mut Vm, args: &Args, idx: usize, arg_num: usize) -> PResult<usize> {
    match args.int_opt(idx) {
        None => Ok(default_scale(vm)),
        Some(s) if (0..=INT_MAX).contains(&s) => Ok(s as usize),
        Some(_) => Err(vm.argument_value_error(arg_num, "must be between 0 and 2147483647")),
    }
}

/// `php_str2num` or "is not well-formed" for argument `arg_num`.
fn num_arg(vm: &mut Vm, s: &PhpStr, arg_num: usize) -> PResult<BcNum> {
    match BcNum::parse(s.as_bytes(), 0, true) {
        Some((n, _)) => Ok(n),
        None => Err(vm.argument_value_error(arg_num, "is not well-formed")),
    }
}

/// The fatal error for an allocation the memory limit rejects.
pub(crate) fn alloc_error_for(vm: &mut Vm, e: AllocError) -> Flow {
    memory_exhausted(vm, e.0)
}

/// `bc_pow_err`: `arg_num` is 0 for an operator.
pub(crate) fn raise_error(vm: &mut Vm, e: RaiseError, arg_num: usize) -> Flow {
    match e {
        RaiseError::DivideByZero => vm.throw_division_by_zero("Negative power of zero"),
        RaiseError::LenOverflow | RaiseError::ScaleOverflow | RaiseError::FullLenOverflow => {
            let msg = "exponent is too large, the number of digits overflowed";
            if arg_num == 0 {
                vm.throw_value_error(msg)
            } else {
                vm.argument_value_error(arg_num, msg)
            }
        }
        RaiseError::AllocOverflow { count, size } => vm.fatal_error(format!(
            "Possible integer overflow in memory allocation ({count} * {size} + 0)"
        )),
        RaiseError::Alloc(bytes) => memory_exhausted(vm, bytes),
    }
}

fn division_by_zero(vm: &mut Vm) -> Flow {
    vm.throw_division_by_zero("Division by zero")
}

fn modulo_by_zero(vm: &mut Vm) -> Flow {
    vm.throw_division_by_zero("Modulo by zero")
}

fn string(bytes: Vec<u8>) -> Value {
    Value::from(bytes)
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Parses both operands of a binary function and checks the result size.
fn binary_operands(vm: &mut Vm, args: &Args) -> PResult<(BcNum, BcNum, usize)> {
    let scale = scale_arg(vm, args, 2, 3)?;
    let first = num_arg(vm, args.str(0), 1)?;
    let second = num_arg(vm, args.str(1), 2)?;
    check_result_size(vm, first.len.max(second.len) + 1, scale)?;
    Ok((first, second, scale))
}

fn bcadd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    let result = num::add(&first, &second, scale);
    Ok(string(result.to_string_scale(scale)))
}

fn bcsub(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    let result = num::sub(&first, &second, scale);
    Ok(string(result.to_string_scale(scale)))
}

fn bcmul(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    let result = num::multiply(&first, &second, scale);
    Ok(string(result.to_string_scale(scale)))
}

fn bcdiv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    match num::divide(&first, &second, scale) {
        Some(result) => Ok(string(result.to_string_scale(scale))),
        None => Err(division_by_zero(vm)),
    }
}

fn bcmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    match num::modulo(&first, &second, scale) {
        Some(result) => Ok(string(result.to_string_scale(scale))),
        None => Err(modulo_by_zero(vm)),
    }
}

fn bcdivmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (first, second, scale) = binary_operands(vm, args)?;
    match num::divmod(&first, &second, scale) {
        Some((quot, rem)) => Ok(Value::array(PhpArray::from_values(vec![
            string(quot.to_string_scale(0)),
            string(rem.to_string_scale(scale)),
        ]))),
        None => Err(division_by_zero(vm)),
    }
}

fn bcpowmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scale = scale_arg(vm, args, 3, 4)?;
    let base = num_arg(vm, args.str(0), 1)?;
    let expo = num_arg(vm, args.str(1), 2)?;
    let modulus = num_arg(vm, args.str(2), 3)?;
    check_result_size(vm, modulus.len + base.len, scale)?;
    match num::raisemod(&base, &expo, &modulus, scale) {
        Ok(result) => Ok(string(result.to_string_scale(scale))),
        Err(RaiseModError::BaseHasFractional) => {
            Err(vm.argument_value_error(1, "cannot have a fractional part"))
        }
        Err(RaiseModError::ExpoHasFractional) => {
            Err(vm.argument_value_error(2, "cannot have a fractional part"))
        }
        Err(RaiseModError::ExpoIsNegative) => {
            Err(vm.argument_value_error(2, "must be greater than or equal to 0"))
        }
        Err(RaiseModError::ModHasFractional) => {
            Err(vm.argument_value_error(3, "cannot have a fractional part"))
        }
        Err(RaiseModError::ModIsZero) => Err(modulo_by_zero(vm)),
    }
}

fn bcpow(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scale = scale_arg(vm, args, 2, 3)?;
    let base = num_arg(vm, args.str(0), 1)?;
    let exponent = num_arg(vm, args.str(1), 2)?;

    // Check the exponent for scale digits and convert to a long.
    if exponent.scale != 0 {
        return Err(vm.argument_value_error(2, "cannot have a fractional part"));
    }
    let exp = exponent.to_long();
    if exp == 0 && (exponent.len > 1 || exponent.digits[0] != 0) {
        return Err(vm.argument_value_error(2, "is too large"));
    }
    check_result_size(vm, base.len, scale)?;
    let limit = memory_limit(vm);
    match num::raise(&base, exp, scale, limit) {
        Ok(result) => Ok(string(result.to_string_scale(scale))),
        Err(e) => Err(raise_error(vm, e, 2)),
    }
}

fn bcsqrt(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scale = scale_arg(vm, args, 1, 2)?;
    let num = num_arg(vm, args.str(0), 1)?;
    check_result_size(vm, num.len, scale)?;
    match num::sqrt(&num, scale) {
        Some(result) => Ok(string(result.to_string_scale(scale))),
        None => Err(vm.argument_value_error(1, "must be greater than or equal to 0")),
    }
}

fn bccomp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scale = scale_arg(vm, args, 2, 3)?;
    let parse = |vm: &mut Vm, s: &PhpStr, arg: usize| match BcNum::parse(s.as_bytes(), scale, false)
    {
        Some((n, _)) => Ok(n),
        None => Err(vm.argument_value_error(arg, "is not well-formed")),
    };
    let first = parse(vm, args.str(0), 1)?;
    let second = parse(vm, args.str(1), 2)?;
    Ok(Value::Int(i64::from(num::compare(&first, &second, scale))))
}

fn bcscale(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let old_scale = default_scale(vm) as i64;
    if let Some(new_scale) = args.int_opt(0) {
        if !(0..=INT_MAX).contains(&new_scale) {
            return Err(vm.argument_value_error(1, "must be between 0 and 2147483647"));
        }
        vm.ini_set("bcmath.scale", new_scale.to_string());
    }
    Ok(Value::Int(old_scale))
}

fn floor_or_ceil(vm: &mut Vm, args: &Args, is_floor: bool) -> PResult<Value> {
    let num = num_arg(vm, args.str(0), 1)?;
    let result = num::floor_or_ceil(&num, is_floor);
    Ok(string(result.to_string_scale(0)))
}

fn bcfloor(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    floor_or_ceil(vm, args, true)
}

fn bcceil(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    floor_or_ceil(vm, args, false)
}

/// `bcmath_check_precision`: only the upper bound is checked.
pub(crate) fn check_precision(vm: &mut Vm, precision: i64, arg_num: usize) -> PResult<()> {
    if precision > INT_MAX {
        return Err(vm.argument_value_error(
            arg_num,
            &format!("must be between {} and {INT_MAX}", i64::MIN),
        ));
    }
    Ok(())
}

/// The rounding mode of a `RoundingMode` case object.
pub(crate) fn rounding_mode(vm: &mut Vm, mode: &Value, arg_num: usize) -> PResult<RoundMode> {
    let name = mode
        .as_object()
        .and_then(crate::random::enums::case_name)
        .unwrap_or_default();
    match RoundMode::from_case_name(&name) {
        Some(m) => Ok(m),
        None => Err(vm.argument_value_error(arg_num, "is an unsupported rounding mode")),
    }
}

fn bcround(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let precision = args.int(1);
    check_precision(vm, precision, 2)?;
    let mode = rounding_mode(vm, args.get(2), 3)?;
    let num = num_arg(vm, args.str(0), 1)?;
    let limit = memory_limit(vm);
    match num::round(&num, precision, mode, limit) {
        Ok((result, scale)) => Ok(string(result.to_string_scale(scale))),
        Err(e) => Err(alloc_error_for(vm, e)),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub static BCADD: NativeFunction = NativeFunction {
    sig: "bcadd(string $num1, string $num2, ?int $scale = null): string",
    handler: bcadd,
};
pub static BCSUB: NativeFunction = NativeFunction {
    sig: "bcsub(string $num1, string $num2, ?int $scale = null): string",
    handler: bcsub,
};
pub static BCMUL: NativeFunction = NativeFunction {
    sig: "bcmul(string $num1, string $num2, ?int $scale = null): string",
    handler: bcmul,
};
pub static BCDIV: NativeFunction = NativeFunction {
    sig: "bcdiv(string $num1, string $num2, ?int $scale = null): string",
    handler: bcdiv,
};
pub static BCMOD: NativeFunction = NativeFunction {
    sig: "bcmod(string $num1, string $num2, ?int $scale = null): string",
    handler: bcmod,
};
pub static BCDIVMOD: NativeFunction = NativeFunction {
    sig: "bcdivmod(string $num1, string $num2, ?int $scale = null): array",
    handler: bcdivmod,
};
pub static BCPOWMOD: NativeFunction = NativeFunction {
    sig: "bcpowmod(string $num, string $exponent, string $modulus, ?int $scale = null): string",
    handler: bcpowmod,
};
pub static BCPOW: NativeFunction = NativeFunction {
    sig: "bcpow(string $num, string $exponent, ?int $scale = null): string",
    handler: bcpow,
};
pub static BCSQRT: NativeFunction = NativeFunction {
    sig: "bcsqrt(string $num, ?int $scale = null): string",
    handler: bcsqrt,
};
pub static BCCOMP: NativeFunction = NativeFunction {
    sig: "bccomp(string $num1, string $num2, ?int $scale = null): int",
    handler: bccomp,
};
pub static BCSCALE: NativeFunction = NativeFunction {
    sig: "bcscale(?int $scale = null): int",
    handler: bcscale,
};
pub static BCFLOOR: NativeFunction = NativeFunction {
    sig: "bcfloor(string $num): string",
    handler: bcfloor,
};
pub static BCCEIL: NativeFunction = NativeFunction {
    sig: "bcceil(string $num): string",
    handler: bcceil,
};
pub static BCROUND: NativeFunction = NativeFunction {
    sig: "bcround(string $num, int $precision = 0, RoundingMode $mode = RoundingMode::HalfAwayFromZero): string",
    handler: bcround,
};

pub fn register(r: &mut Registry<'_>) {
    r.ini(IniDef::new("bcmath.scale", Some("0"), modifiable::ALL).validated(validate_scale));
    r.functions(&[
        &BCADD, &BCSUB, &BCMUL, &BCDIV, &BCMOD, &BCDIVMOD, &BCPOWMOD, &BCPOW, &BCSQRT, &BCCOMP,
        &BCSCALE, &BCFLOOR, &BCCEIL, &BCROUND,
    ]);
    number::register(r);
}
