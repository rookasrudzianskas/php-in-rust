//! ext/standard/math.c

mod base;
mod number_format;
pub mod round;

#[cfg(test)]
mod tests;

use php_runtime::value::numeric::double_fits_long;
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use crate::random::check_allocation;
use crate::random::enums::{case_name, register_pure_enum};
use round::{
    PHP_ROUND_AWAY_FROM_ZERO, PHP_ROUND_CEILING, PHP_ROUND_FLOOR, PHP_ROUND_HALF_DOWN,
    PHP_ROUND_HALF_EVEN, PHP_ROUND_HALF_ODD, PHP_ROUND_HALF_UP, PHP_ROUND_TOWARD_ZERO,
    php_math_round,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One-argument float functions (`Z_PARAM_DOUBLE` -> `RETURN_DOUBLE`).
macro_rules! float_fn {
    ($handler:ident, $item:ident, $sig:literal, |$x:ident| $body:expr) => {
        fn $handler(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let $x = args.float(0);
            Ok(Value::Float($body))
        }
        static $item: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

/// One-argument float predicates (`RETURN_BOOL`).
macro_rules! float_pred {
    ($handler:ident, $item:ident, $sig:literal, |$x:ident| $body:expr) => {
        fn $handler(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let $x = args.float(0);
            Ok(Value::Bool($body))
        }
        static $item: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

/// Two-argument float functions.
macro_rules! float_fn2 {
    ($handler:ident, $item:ident, $sig:literal, |$x:ident, $y:ident| $body:expr) => {
        fn $handler(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let ($x, $y) = (args.float(0), args.float(1));
            Ok(Value::Float($body))
        }
        static $item: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

// ---------------------------------------------------------------------------
// abs, ceil, floor, round, clamp
// ---------------------------------------------------------------------------

fn abs(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match args.get(0) {
        Value::Int(i) if *i == i64::MIN => Value::Float(-(i64::MIN as f64)),
        Value::Int(i) => Value::Int(i.abs()),
        Value::Float(d) => Value::Float(d.abs()),
        other => unreachable!("abs() argument is int|float after ZPP, got {other:?}"),
    })
}
static ABS: NativeFunction = NativeFunction {
    sig: "abs(int|float $num): int|float",
    handler: abs,
};

fn ceil(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Float(args.float(0).ceil()))
}
static CEIL: NativeFunction = NativeFunction {
    sig: "ceil(int|float $num): float",
    handler: ceil,
};

fn floor(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Float(args.float(0).floor()))
}
static FLOOR: NativeFunction = NativeFunction {
    sig: "floor(int|float $num): float",
    handler: floor,
};

/// `php_math_round_mode_from_enum`.
fn round_mode_from_case(name: &str) -> i64 {
    (match name {
        "HalfAwayFromZero" => PHP_ROUND_HALF_UP,
        "HalfTowardsZero" => PHP_ROUND_HALF_DOWN,
        "HalfEven" => PHP_ROUND_HALF_EVEN,
        "HalfOdd" => PHP_ROUND_HALF_ODD,
        "TowardsZero" => PHP_ROUND_TOWARD_ZERO,
        "AwayFromZero" => PHP_ROUND_AWAY_FROM_ZERO,
        "NegativeInfinity" => PHP_ROUND_FLOOR,
        _ => PHP_ROUND_CEILING, // PositiveInfinity
    }) as i64
}

fn round(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut places: i32 = 0;
    if args.len() >= 2 {
        let precision = args.int(1);
        places = precision.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }
    let mode = match args.get(2) {
        Value::Object(o) => round_mode_from_case(&case_name(o).unwrap_or_default()),
        v => v.as_int().unwrap_or(PHP_ROUND_HALF_UP as i64),
    };
    if !(PHP_ROUND_HALF_UP as i64..=PHP_ROUND_AWAY_FROM_ZERO as i64).contains(&mode) {
        return Err(vm.argument_value_error(3, "must be a valid rounding mode (RoundingMode::*)"));
    }
    Ok(match args.get(0) {
        // A long that doesn't need to be rounded.
        Value::Int(i) if places >= 0 => Value::Float(*i as f64),
        v => Value::Float(php_math_round(
            v.as_float()
                .unwrap_or_else(|| v.as_int().unwrap_or(0) as f64),
            places,
            mode as i32,
        )),
    })
}
static ROUND: NativeFunction = NativeFunction {
    sig: "round(int|float $num, int $precision = 0, int|RoundingMode $mode = RoundingMode::HalfAwayFromZero): float",
    handler: round,
};

/// `php_math_clamp`.
fn clamp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let value = args.get(0).unref();
    let min = args.get(1).unref();
    let max = args.get(2).unref();
    if matches!(min, Value::Float(d) if d.is_nan()) {
        return Err(vm.argument_value_error(2, "must not be NAN"));
    }
    if matches!(max, Value::Float(d) if d.is_nan()) {
        return Err(vm.argument_value_error(3, "must not be NAN"));
    }
    if vm.compare(&max, &min)? == -1 {
        return Err(
            vm.argument_value_error(2, "must be smaller than or equal to argument #3 ($max)")
        );
    }
    if vm.compare(&max, &value)? == -1 {
        return Ok(max);
    }
    if vm.compare(&value, &min)? == -1 {
        return Ok(min);
    }
    Ok(value)
}
static CLAMP: NativeFunction = NativeFunction {
    sig: "clamp(mixed $value, mixed $min, mixed $max): mixed",
    handler: clamp,
};

// ---------------------------------------------------------------------------
// Trigonometric, hyperbolic, exponential and logarithmic functions
// ---------------------------------------------------------------------------

float_fn!(sin, SIN, "sin(float $num): float", |x| x.sin());
float_fn!(cos, COS, "cos(float $num): float", |x| x.cos());
float_fn!(tan, TAN, "tan(float $num): float", |x| x.tan());
float_fn!(asin, ASIN, "asin(float $num): float", |x| x.asin());
float_fn!(acos, ACOS, "acos(float $num): float", |x| x.acos());
float_fn!(atan, ATAN, "atan(float $num): float", |x| x.atan());
float_fn!(atanh, ATANH, "atanh(float $num): float", |x| x.atanh());
float_fn2!(atan2, ATAN2, "atan2(float $y, float $x): float", |y, x| y
    .atan2(x));
float_fn!(sinh, SINH, "sinh(float $num): float", |x| x.sinh());
float_fn!(cosh, COSH, "cosh(float $num): float", |x| x.cosh());
float_fn!(tanh, TANH, "tanh(float $num): float", |x| x.tanh());
float_fn!(asinh, ASINH, "asinh(float $num): float", |x| x.asinh());
float_fn!(acosh, ACOSH, "acosh(float $num): float", |x| x.acosh());
float_fn!(expm1, EXPM1, "expm1(float $num): float", |x| x.exp_m1());
float_fn!(log1p, LOG1P, "log1p(float $num): float", |x| x.ln_1p());
float_fn!(exp, EXP, "exp(float $num): float", |x| x.exp());
float_fn!(log10, LOG10, "log10(float $num): float", |x| x.log10());
float_fn!(sqrt, SQRT, "sqrt(float $num): float", |x| x.sqrt());
float_fn2!(hypot, HYPOT, "hypot(float $x, float $y): float", |x, y| x
    .hypot(y));
float_fn!(deg2rad, DEG2RAD, "deg2rad(float $num): float", |x| (x
    / 180.0)
    * std::f64::consts::PI);
float_fn!(rad2deg, RAD2DEG, "rad2deg(float $num): float", |x| (x
    / std::f64::consts::PI)
    * 180.0);
float_fn2!(
    fmod,
    FMOD,
    "fmod(float $num1, float $num2): float",
    |x, y| x % y
);
float_fn2!(
    fdiv,
    FDIV,
    "fdiv(float $num1, float $num2): float",
    |x, y| x / y
);
float_fn2!(
    fpow,
    FPOW,
    "fpow(float $num, float $exponent): float",
    |x, y| x.powf(y)
);

float_pred!(is_finite, IS_FINITE, "is_finite(float $num): bool", |x| x
    .is_finite());
float_pred!(is_nan, IS_NAN, "is_nan(float $num): bool", |x| x.is_nan());
float_pred!(
    is_infinite,
    IS_INFINITE,
    "is_infinite(float $num): bool",
    |x| x.is_infinite()
);

fn pi(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Float(std::f64::consts::PI))
}
static PI: NativeFunction = NativeFunction {
    sig: "pi(): float",
    handler: pi,
};

fn log(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let num = args.float(0);
    if args.len() == 1 {
        return Ok(Value::Float(num.ln()));
    }
    let base = args.float(1);
    if base == 2.0 {
        return Ok(Value::Float(num.log2()));
    }
    if base == 10.0 {
        return Ok(Value::Float(num.log10()));
    }
    if base == 1.0 {
        return Ok(Value::Float(f64::NAN));
    }
    if base <= 0.0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    Ok(Value::Float(num.ln() / base.ln()))
}
static LOG: NativeFunction = NativeFunction {
    sig: "log(float $num, float $base = M_E): float",
    handler: log,
};

fn pow(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (base, exponent) = (args.get(0).clone(), args.get(1).clone());
    vm.pow(&base, &exponent)
}
static POW: NativeFunction = NativeFunction {
    sig: "pow(mixed $num, mixed $exponent): int|float|object",
    handler: pow,
};

fn intdiv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(vm.intdiv(args.int(0), args.int(1))?))
}
static INTDIV: NativeFunction = NativeFunction {
    sig: "intdiv(int $num1, int $num2): int",
    handler: intdiv,
};

// ---------------------------------------------------------------------------
// Base conversion
// ---------------------------------------------------------------------------

fn bindec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    base::basetozval(vm, &s, 2)
}
static BINDEC: NativeFunction = NativeFunction {
    sig: "bindec(string $binary_string): int|float",
    handler: bindec,
};

fn hexdec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    base::basetozval(vm, &s, 16)
}
static HEXDEC: NativeFunction = NativeFunction {
    sig: "hexdec(string $hex_string): int|float",
    handler: hexdec,
};

fn octdec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    base::basetozval(vm, &s, 8)
}
static OCTDEC: NativeFunction = NativeFunction {
    sig: "octdec(string $octal_string): int|float",
    handler: octdec,
};

fn decbin(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(base::bytes_value(base::longtobase_pwr2(args.int(0), 1)))
}
static DECBIN: NativeFunction = NativeFunction {
    sig: "decbin(int $num): string",
    handler: decbin,
};

fn decoct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(base::bytes_value(base::longtobase_pwr2(args.int(0), 3)))
}
static DECOCT: NativeFunction = NativeFunction {
    sig: "decoct(int $num): string",
    handler: decoct,
};

fn dechex(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(base::bytes_value(base::longtobase_pwr2(args.int(0), 4)))
}
static DECHEX: NativeFunction = NativeFunction {
    sig: "dechex(int $num): string",
    handler: dechex,
};

fn base_convert(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let number = args.str(0).clone();
    let (frombase, tobase) = (args.int(1), args.int(2));
    if !(2..=36).contains(&frombase) {
        return Err(vm.argument_value_error(2, "must be between 2 and 36 (inclusive)"));
    }
    if !(2..=36).contains(&tobase) {
        return Err(vm.argument_value_error(3, "must be between 2 and 36 (inclusive)"));
    }
    let temp = base::basetozval(vm, &number, frombase as u32)?;
    match base::zvaltobase(&temp, tobase as u32) {
        Ok(digits) => Ok(base::bytes_value(digits)),
        Err(()) => Err(vm.throw_value_error(format!(
            "An infinite value cannot be converted to base {tobase}"
        ))),
    }
}
static BASE_CONVERT: NativeFunction = NativeFunction {
    sig: "base_convert(string $num, int $from_base, int $to_base): string",
    handler: base_convert,
};

// ---------------------------------------------------------------------------
// number_format
// ---------------------------------------------------------------------------

fn number_format(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dec = args.int(1);
    let dec_point = args
        .str_opt(2)
        .map_or_else(|| b".".to_vec(), |s| s.to_vec());
    let thousand_sep = args
        .str_opt(3)
        .map_or_else(|| b",".to_vec(), |s| s.to_vec());

    if dec > i32::MAX as i64 || dec < i32::MIN as i64 {
        return Err(
            vm.argument_value_error(2, &format!("must be between {} and {}", i32::MIN, i32::MAX))
        );
    }
    if dec > 0 {
        // The result gets `dec` decimals; fail like the allocator would.
        check_allocation(vm, dec as usize + dec_point.len() + 400)?;
    }
    let bytes = match args.get(0) {
        Value::Int(n) => number_format::number_format_long(*n, dec, &dec_point, &thousand_sep),
        Value::Float(d) => {
            let d = *d;
            // Doubles of >= 2^52 cannot have fractional digits any more, and
            // casting to long does not lose precision on rounding.
            if (d >= 4503599627370496.0 || d <= -4503599627370496.0) && double_fits_long(d) {
                number_format::number_format_long(d as i64, dec, &dec_point, &thousand_sep)
            } else {
                number_format::number_format_ex(d, dec as i32, &dec_point, &thousand_sep)
            }
        }
        other => unreachable!("number_format() argument is int|float after ZPP, got {other:?}"),
    };
    Ok(base::bytes_value(bytes))
}
// The C implementation parses `Z_PARAM_NUMBER`, so integers keep their exact
// value (the stub declares `float $num`, which would convert them first).
static NUMBER_FORMAT: NativeFunction = NativeFunction {
    sig: "number_format(int|float $num, int $decimals = 0, ?string $decimal_separator = \".\", ?string $thousands_separator = \",\"): string",
    handler: number_format,
};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

// The `M_*` literals keep the digits of php-src's stub.
#[allow(clippy::excessive_precision)]
pub fn register(r: &mut Registry<'_>) {
    use std::f64::consts as c;

    for (name, value) in [
        ("M_E", c::E),
        ("M_LOG2E", c::LOG2_E),
        ("M_LOG10E", c::LOG10_E),
        ("M_LN2", c::LN_2),
        ("M_LN10", c::LN_10),
        ("M_PI", c::PI),
        ("M_PI_2", c::FRAC_PI_2),
        ("M_PI_4", c::FRAC_PI_4),
        ("M_1_PI", c::FRAC_1_PI),
        ("M_2_PI", c::FRAC_2_PI),
        ("M_SQRTPI", 1.77245385090551602729),
        ("M_2_SQRTPI", c::FRAC_2_SQRT_PI),
        ("M_LNPI", 1.14472988584940017414),
        ("M_EULER", 0.57721566490153286061),
        ("M_SQRT2", c::SQRT_2),
        ("M_SQRT1_2", c::FRAC_1_SQRT_2),
        ("M_SQRT3", 1.73205080756887729352),
        ("INF", f64::INFINITY),
        ("NAN", f64::NAN),
    ] {
        r.constant(name, value);
    }
    r.constant("PHP_ROUND_HALF_UP", PHP_ROUND_HALF_UP as i64);
    r.constant("PHP_ROUND_HALF_DOWN", PHP_ROUND_HALF_DOWN as i64);
    r.constant("PHP_ROUND_HALF_EVEN", PHP_ROUND_HALF_EVEN as i64);
    r.constant("PHP_ROUND_HALF_ODD", PHP_ROUND_HALF_ODD as i64);

    register_pure_enum(
        r,
        "RoundingMode",
        &[
            "HalfAwayFromZero",
            "HalfTowardsZero",
            "HalfEven",
            "HalfOdd",
            "TowardsZero",
            "AwayFromZero",
            "NegativeInfinity",
            "PositiveInfinity",
        ],
    );

    r.functions(&[
        &ABS,
        &CEIL,
        &FLOOR,
        &ROUND,
        &CLAMP,
        &SIN,
        &COS,
        &TAN,
        &ASIN,
        &ACOS,
        &ATAN,
        &ATANH,
        &ATAN2,
        &SINH,
        &COSH,
        &TANH,
        &ASINH,
        &ACOSH,
        &EXPM1,
        &LOG1P,
        &PI,
        &IS_FINITE,
        &IS_NAN,
        &INTDIV,
        &IS_INFINITE,
        &POW,
        &EXP,
        &LOG,
        &LOG10,
        &SQRT,
        &HYPOT,
        &DEG2RAD,
        &RAD2DEG,
        &BINDEC,
        &HEXDEC,
        &OCTDEC,
        &DECBIN,
        &DECOCT,
        &DECHEX,
        &BASE_CONVERT,
        &NUMBER_FORMAT,
        &FMOD,
        &FDIV,
        &FPOW,
    ]);
}
