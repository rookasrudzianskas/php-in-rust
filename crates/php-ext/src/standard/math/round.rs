//! `_php_math_round` and its helpers (`ext/standard/math.c`).

/// `PHP_ROUND_*` (`php_math_round_mode.h`).
pub const PHP_ROUND_HALF_UP: i32 = 0x01;
pub const PHP_ROUND_HALF_DOWN: i32 = 0x02;
pub const PHP_ROUND_HALF_EVEN: i32 = 0x03;
pub const PHP_ROUND_HALF_ODD: i32 = 0x04;
pub const PHP_ROUND_CEILING: i32 = 0x05;
pub const PHP_ROUND_FLOOR: i32 = 0x06;
pub const PHP_ROUND_TOWARD_ZERO: i32 = 0x07;
pub const PHP_ROUND_AWAY_FROM_ZERO: i32 = 0x08;

/// `php_intpow10`: `pow(10.0, power)` with a lookup table for the exact powers.
fn intpow10(power: i32) -> f64 {
    const POWERS: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    if !(0..=22).contains(&power) {
        return 10f64.powf(power as f64);
    }
    POWERS[power as usize]
}

fn basic_edge_case(integral: f64, exponent: f64, places: i32) -> f64 {
    if places > 0 {
        ((integral + 0.5f64.copysign(integral)) / exponent).abs()
    } else {
        ((integral + 0.5f64.copysign(integral)) * exponent).abs()
    }
}

fn zero_edge_case(integral: f64, exponent: f64, places: i32) -> f64 {
    if places > 0 {
        (integral / exponent).abs()
    } else {
        (integral * exponent).abs()
    }
}

/// `php_round_helper`: rounds the scaled value to an integer in `mode`.
fn round_helper(integral: f64, value: f64, exponent: f64, places: i32, mode: i32) -> f64 {
    let value_abs = value.abs();
    match mode {
        PHP_ROUND_HALF_UP => {
            let edge_case = basic_edge_case(integral, exponent, places);
            if value_abs >= edge_case {
                // Increase the magnitude of the integral part (towards
                // infinity for positive, away from it for negative values).
                return integral + 1.0f64.copysign(integral);
            }
            integral
        }
        PHP_ROUND_HALF_DOWN => {
            let edge_case = basic_edge_case(integral, exponent, places);
            if value_abs > edge_case {
                return integral + 1.0f64.copysign(integral);
            }
            integral
        }
        PHP_ROUND_CEILING => {
            let edge_case = zero_edge_case(integral, exponent, places);
            if value > 0.0 && value_abs > edge_case {
                return integral + 1.0;
            }
            integral
        }
        PHP_ROUND_FLOOR => {
            let edge_case = zero_edge_case(integral, exponent, places);
            if value < 0.0 && value_abs > edge_case {
                return integral - 1.0;
            }
            integral
        }
        PHP_ROUND_TOWARD_ZERO => integral,
        PHP_ROUND_AWAY_FROM_ZERO => {
            let edge_case = zero_edge_case(integral, exponent, places);
            if value_abs > edge_case {
                return integral + 1.0f64.copysign(integral);
            }
            integral
        }
        PHP_ROUND_HALF_EVEN => {
            let edge_case = basic_edge_case(integral, exponent, places);
            if value_abs > edge_case {
                return integral + 1.0f64.copysign(integral);
            } else if value_abs == edge_case {
                let even = integral % 2.0 == 0.0;
                // If the integral part is not even we can make it even by
                // adding one in the direction of the existing sign.
                if !even {
                    return integral + 1.0f64.copysign(integral);
                }
            }
            integral
        }
        PHP_ROUND_HALF_ODD => {
            let edge_case = basic_edge_case(integral, exponent, places);
            if value_abs > edge_case {
                return integral + 1.0f64.copysign(integral);
            } else if value_abs == edge_case {
                let even = integral % 2.0 == 0.0;
                if even {
                    return integral + 1.0f64.copysign(integral);
                }
            }
            integral
        }
        _ => unreachable!("round mode validated by the caller"),
    }
}

/// `_php_math_round`: rounds `value` to `places` decimal places in `mode`.
pub fn php_math_round(value: f64, places: i32, mode: i32) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    if places == 0 && value == value.trunc() {
        return value;
    }
    let places = places.max(i32::MIN + 1);
    let exponent = intpow10(places.abs());

    // The integer part extracted by floor()/ceil() may be one too small
    // because of floating point error (0.285 * 1e10 => 2849999999.9999995);
    // if adding one reproduces `value` exactly, use that instead.
    let scaled = if places > 0 {
        value * exponent
    } else {
        value / exponent
    };
    let (mut tmp_value, tmp_value2) = if value >= 0.0 {
        let t = scaled.floor();
        (t, t + 1.0)
    } else {
        let t = scaled.ceil();
        (t, t - 1.0)
    };
    let back = if places > 0 {
        tmp_value2 / exponent
    } else {
        tmp_value2 * exponent
    };
    if back == value {
        tmp_value = tmp_value2;
    }

    // This value is beyond our precision, so rounding it is pointless.
    if tmp_value.abs() >= 1e16 {
        return value;
    }

    tmp_value = round_helper(tmp_value, value, exponent, places, mode);

    if places.abs() < 23 {
        if places > 0 {
            tmp_value / exponent
        } else {
            tmp_value * exponent
        }
    } else {
        // Simple division would give wrong results here: round-trip through a
        // string so strtod() returns the nearest representable value.
        let buf = format!("{tmp_value:.6}e{}", -(places as i64));
        match buf.parse::<f64>() {
            Ok(v) if v.is_finite() => v,
            _ => value,
        }
    }
}
