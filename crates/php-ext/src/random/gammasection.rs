//! The γ-section algorithm for drawing floats from an interval
//! (`ext/random/gammasection.c`, after Goualard, "Drawing Random
//! Floating-Point Numbers from an Interval", ACM TOMACS 32:3, 2022).
//!
//! Each function returns NaN when the interval is empty, like php-src.

use php_runtime::{PResult, Vm};

use super::engine::Engine;

/// C's `nextafter(x, y)`.
fn nextafter(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x == y {
        return y;
    }
    if x == 0.0 {
        return f64::from_bits(1).copysign(y);
    }
    let bits = x.to_bits();
    let away_from_zero = (y > x) == (x > 0.0);
    f64::from_bits(if away_from_zero { bits + 1 } else { bits - 1 })
}

fn gamma_low(x: f64) -> f64 {
    x - nextafter(x, -f64::MAX)
}

fn gamma_high(x: f64) -> f64 {
    nextafter(x, f64::MAX) - x
}

fn gamma_max(x: f64, y: f64) -> f64 {
    if x.abs() > y.abs() {
        gamma_high(x)
    } else {
        gamma_low(y)
    }
}

fn splitint64(v: u64) -> (f64, f64) {
    ((v >> 2) as f64, (v & 3) as f64)
}

fn ceilint(a: f64, b: f64, g: f64) -> u64 {
    let s = b / g - a / g;
    let e = if a.abs() <= b.abs() {
        -a / g - (s - b / g)
    } else {
        b / g - (s + a / g)
    };
    let si = s.ceil();
    if s != si {
        si as u64
    } else {
        (si as u64).wrapping_add((e > 0.0) as u64)
    }
}

/// `php_random_gammasection_closed_open`: `[min, max)`.
pub fn closed_open(vm: &mut Vm, engine: &Engine, min: f64, max: f64) -> PResult<f64> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 1 {
        return Ok(f64::NAN);
    }
    let k = 1 + engine.range64(vm, hi - 1)?; // [1, hi]
    Ok(if min.abs() <= max.abs() {
        if k == hi {
            min
        } else {
            let (k_hi, k_lo) = splitint64(k);
            4.0 * (max * 0.25 - k_hi * g) - k_lo * g
        }
    } else {
        let (k_hi, k_lo) = splitint64(k - 1);
        4.0 * (min * 0.25 + k_hi * g) + k_lo * g
    })
}

/// `php_random_gammasection_closed_closed`: `[min, max]`.
pub fn closed_closed(vm: &mut Vm, engine: &Engine, min: f64, max: f64) -> PResult<f64> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max < min {
        return Ok(f64::NAN);
    }
    let k = engine.range64(vm, hi)?; // [0, hi]
    Ok(if min.abs() <= max.abs() {
        if k == hi {
            min
        } else {
            let (k_hi, k_lo) = splitint64(k);
            4.0 * (max * 0.25 - k_hi * g) - k_lo * g
        }
    } else if k == hi {
        max
    } else {
        let (k_hi, k_lo) = splitint64(k);
        4.0 * (min * 0.25 + k_hi * g) + k_lo * g
    })
}

/// `php_random_gammasection_open_closed`: `(min, max]`.
pub fn open_closed(vm: &mut Vm, engine: &Engine, min: f64, max: f64) -> PResult<f64> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 1 {
        return Ok(f64::NAN);
    }
    let k = engine.range64(vm, hi - 1)?; // [0, hi - 1]
    Ok(if min.abs() <= max.abs() {
        let (k_hi, k_lo) = splitint64(k);
        4.0 * (max * 0.25 - k_hi * g) - k_lo * g
    } else if k == hi - 1 {
        max
    } else {
        let (k_hi, k_lo) = splitint64(k + 1);
        4.0 * (min * 0.25 + k_hi * g) + k_lo * g
    })
}

/// `php_random_gammasection_open_open`: `(min, max)`.
pub fn open_open(vm: &mut Vm, engine: &Engine, min: f64, max: f64) -> PResult<f64> {
    let g = gamma_max(min, max);
    let hi = ceilint(min, max, g);
    if max <= min || hi < 2 {
        return Ok(f64::NAN);
    }
    let k = 1 + engine.range64(vm, hi - 2)?; // [1, hi - 1]
    let (k_hi, k_lo) = splitint64(k);
    Ok(if min.abs() <= max.abs() {
        4.0 * (max * 0.25 - k_hi * g) - k_lo * g
    } else {
        4.0 * (min * 0.25 + k_hi * g) + k_lo * g
    })
}
