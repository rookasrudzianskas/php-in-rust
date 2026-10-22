//! Port of libbcmath (`ext/bcmath/libbcmath/src/*.c`): arbitrary precision
//! decimal arithmetic on digit arrays.
//!
//! A [`BcNum`] mirrors `bc_struct`: `len` digits before the decimal point
//! (at least one), `scale` digits after it, one byte (0..=9) per digit in
//! `digits` (`n_value`) and a sign. Every routine reproduces the *shape*
//! (`len`/`scale`) of the C result, not just its value, because
//! `BcMath\Number` and the square root iteration observe it.
//!
//! The SIMD/SWAR fast paths of php-src only change speed, so add and subtract
//! work digit by digit; multiplication and division use [`super::digits`].

use std::cmp::{max, min};

use super::digits::{div_digits, mul_digits, pow_digits};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sign {
    Plus,
    Minus,
}

/// `bc_num`.
#[derive(Clone, Debug)]
pub struct BcNum {
    pub sign: Sign,
    /// `n_len`: number of digits before the decimal point.
    pub len: usize,
    /// `n_scale`: number of digits after the decimal point.
    pub scale: usize,
    /// `n_value`: exactly `len + scale` digits.
    pub digits: Vec<u8>,
}

/// `zend_enum_RoundingMode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoundMode {
    HalfAwayFromZero,
    HalfTowardsZero,
    HalfEven,
    HalfOdd,
    TowardsZero,
    AwayFromZero,
    NegativeInfinity,
    PositiveInfinity,
}

impl RoundMode {
    /// The mode for the name of a `RoundingMode` enum case.
    pub fn from_case_name(name: &str) -> Option<RoundMode> {
        Some(match name {
            "HalfAwayFromZero" => RoundMode::HalfAwayFromZero,
            "HalfTowardsZero" => RoundMode::HalfTowardsZero,
            "HalfEven" => RoundMode::HalfEven,
            "HalfOdd" => RoundMode::HalfOdd,
            "TowardsZero" => RoundMode::TowardsZero,
            "AwayFromZero" => RoundMode::AwayFromZero,
            "NegativeInfinity" => RoundMode::NegativeInfinity,
            "PositiveInfinity" => RoundMode::PositiveInfinity,
            _ => return None,
        })
    }
}

/// `bc_raise_status` (without `BC_RAISE_STATUS_OK`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RaiseError {
    LenOverflow,
    ScaleOverflow,
    FullLenOverflow,
    DivideByZero,
    /// `safe_emalloc(count, size, 0)` overflowed `size_t`.
    AllocOverflow {
        count: usize,
        size: usize,
    },
    /// An allocation of this many bytes exceeds the memory limit.
    Alloc(usize),
}

/// `raise_mod_status` (without `OK`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RaiseModError {
    BaseHasFractional,
    ExpoHasFractional,
    ExpoIsNegative,
    ModHasFractional,
    ModIsZero,
}

/// An allocation that would exceed the memory limit (`bytes` requested).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AllocError(pub usize);

/// Maximum number of digits a scale is extended by (`BC_MATH_NUMBER_EXPAND_SCALE`).
pub const EXPAND_SCALE: usize = 10;

impl BcNum {
    /// `bc_new_num`: a zero with `len` integer and `scale` fractional digits.
    pub fn new(len: usize, scale: usize) -> BcNum {
        BcNum {
            sign: Sign::Plus,
            len,
            scale,
            digits: vec![0; len + scale],
        }
    }

    /// `BCG(_zero_)`.
    pub fn zero() -> BcNum {
        BcNum::new(1, 0)
    }

    /// `BCG(_one_)`.
    pub fn one() -> BcNum {
        BcNum {
            sign: Sign::Plus,
            len: 1,
            scale: 0,
            digits: vec![1],
        }
    }

    /// `bc_long2num`.
    pub fn from_long(lval: i64) -> BcNum {
        if lval == 0 {
            return BcNum::zero();
        }
        let mut magnitude = lval.unsigned_abs();
        let mut digits = Vec::new();
        while magnitude > 0 {
            digits.push((magnitude % 10) as u8);
            magnitude /= 10;
        }
        digits.reverse();
        BcNum {
            sign: if lval < 0 { Sign::Minus } else { Sign::Plus },
            len: digits.len(),
            scale: 0,
            digits,
        }
    }

    /// `bc_int2num`.
    pub fn from_int(val: i32) -> BcNum {
        let mut n = BcNum::from_long(i64::from(val));
        if n.digits.is_empty() {
            n = BcNum::zero();
        }
        n
    }

    /// `bc_num2long`: the integer part, or 0 when it does not fit.
    pub fn to_long(&self) -> i64 {
        let mut val: i64 = 0;
        for &n in &self.digits[..self.len] {
            if val > i64::MAX / 10 {
                return 0;
            }
            val *= 10;
            if val > i64::MAX - i64::from(n) {
                return 0;
            }
            val += i64::from(n);
        }
        if self.sign == Sign::Plus { val } else { -val }
    }

    /// `bc_is_zero_for_scale`.
    pub fn is_zero_for_scale(&self, scale: usize) -> bool {
        let scale = min(scale, self.scale);
        self.digits[..self.len + scale].iter().all(|&d| d == 0)
    }

    /// `bc_is_zero`.
    pub fn is_zero(&self) -> bool {
        self.is_zero_for_scale(self.scale)
    }

    /// `bc_is_near_zero`: all digits (up to `scale`) zero except a final 1.
    pub fn is_near_zero(&self, scale: usize) -> bool {
        let scale = min(scale, self.scale);
        let total = self.len + scale;
        match self.digits[..total].iter().position(|&d| d != 0) {
            None => true,
            Some(i) => i + 1 == total && self.digits[i] == 1,
        }
    }

    /// `bc_is_neg`.
    pub fn is_neg(&self) -> bool {
        self.sign == Sign::Minus
    }

    /// `_bc_rm_leading_zeros`.
    pub fn rm_leading_zeros(&mut self) {
        let zeros = self
            .digits
            .iter()
            .take(self.len.saturating_sub(1))
            .take_while(|&&d| d == 0)
            .count();
        if zeros > 0 {
            self.digits.drain(..zeros);
            self.len -= zeros;
        }
    }

    /// `bc_rm_trailing_zeros`.
    pub fn rm_trailing_zeros(&mut self) {
        while self.scale > 0 && self.digits.last() == Some(&0) {
            self.digits.pop();
            self.scale -= 1;
        }
    }

    /// Assigns a smaller `n_scale` (the extra digits are no longer part of the number).
    pub fn truncate_scale(&mut self, scale: usize) {
        if scale < self.scale {
            self.scale = scale;
            self.digits.truncate(self.len + scale);
        }
    }

    /// `bc_str2num`: parses `s`; returns the number and its full scale (the
    /// number of fractional digits written), or `None` if malformed.
    pub fn parse(s: &[u8], scale: usize, auto_scale: bool) -> Option<(BcNum, usize)> {
        let end = s.len();
        let at = |i: usize| s.get(i).copied();
        let count_digits = |mut i: usize| {
            while at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
            i
        };
        let skip_zero_reverse = |mut scanner: usize, stop: usize| {
            while scanner > stop && s[scanner - 1] == b'0' {
                scanner -= 1;
            }
            scanner
        };

        let mut ptr = 0;
        if matches!(at(ptr), Some(b'+' | b'-')) {
            ptr += 1;
        }
        while at(ptr) == Some(b'0') {
            ptr += 1;
        }
        let integer_ptr = ptr;
        ptr = count_digits(ptr);
        let mut digits = ptr - integer_ptr;
        let decimal_point = at(ptr) == Some(b'.');
        if !decimal_point && ptr != end {
            return None;
        }

        let mut str_scale = 0;
        let mut full_scale = 0;
        let mut fractional_ptr = 0;
        let mut fractional_end = 0;
        if decimal_point {
            fractional_ptr = ptr + 1;
            fractional_end = fractional_ptr;
            if fractional_ptr != end {
                fractional_end = count_digits(fractional_ptr);
                if fractional_end != end {
                    return None;
                }
                full_scale = fractional_end - fractional_ptr;
                fractional_end = skip_zero_reverse(fractional_end, fractional_ptr);
                str_scale = fractional_end - fractional_ptr;
                // A smaller manual scale truncates (123.0001 with scale 2 -> 123).
                if str_scale > scale && !auto_scale {
                    fractional_end -= str_scale - scale;
                    str_scale = scale;
                    if str_scale > 0 {
                        let new_end = skip_zero_reverse(fractional_end, fractional_ptr);
                        str_scale -= fractional_end - new_end;
                        fractional_end = new_end;
                    }
                }
            }
        }

        if digits + str_scale == 0 {
            return Some((BcNum::zero(), full_scale));
        }
        let zero_int = digits == 0;
        if zero_int {
            digits = 1;
        }
        let mut out = Vec::with_capacity(digits + str_scale);
        if zero_int {
            out.push(0);
            out.extend(s[fractional_ptr..fractional_end].iter().map(|c| c - b'0'));
        } else {
            out.extend(
                s[integer_ptr..integer_ptr + digits]
                    .iter()
                    .map(|c| c - b'0'),
            );
            if str_scale > 0 {
                out.extend(s[fractional_ptr..fractional_end].iter().map(|c| c - b'0'));
            }
        }
        let sign = if s.first() == Some(&b'-') {
            Sign::Minus
        } else {
            Sign::Plus
        };
        Some((
            BcNum {
                sign,
                len: digits,
                scale: str_scale,
                digits: out,
            },
            full_scale,
        ))
    }

    /// `bc_num2str_ex`: formats with exactly `scale` fractional digits
    /// (truncating or zero padding).
    pub fn to_string_scale(&self, scale: usize) -> Vec<u8> {
        let min_scale = min(self.scale, scale);
        let signch = self.sign != Sign::Plus && !self.is_zero_for_scale(min_scale);
        let mut out = Vec::with_capacity(self.len + scale + usize::from(signch) + 1);
        if signch {
            out.push(b'-');
        }
        out.extend(self.digits[..self.len].iter().map(|d| d + b'0'));
        if scale > 0 {
            out.push(b'.');
            out.extend(
                self.digits[self.len..self.len + min_scale]
                    .iter()
                    .map(|d| d + b'0'),
            );
            out.resize(out.len() + scale.saturating_sub(self.scale), b'0');
        }
        out
    }

    /// Digit of this number at `pos` places from the least significant digit
    /// of a number with `total_scale` fractional digits (0 outside).
    fn digit_from_right(&self, pos: usize, total_scale: usize) -> u8 {
        let shift = total_scale - self.scale;
        if pos < shift {
            return 0;
        }
        let from_end = pos - shift;
        if from_end >= self.digits.len() {
            0
        } else {
            self.digits[self.digits.len() - 1 - from_end]
        }
    }
}

/// `_bc_do_compare` result for "left magnitude is greater".
fn compare_result(left_abs_greater: bool, use_sign: bool, left_sign: Sign) -> i32 {
    if left_abs_greater {
        if !use_sign || left_sign == Sign::Plus {
            1
        } else {
            -1
        }
    } else if !use_sign || left_sign == Sign::Plus {
        -1
    } else {
        1
    }
}

/// `_bc_do_compare`: -1, 0 or 1; `use_sign = false` compares magnitudes.
pub fn do_compare(n1: &BcNum, n2: &BcNum, scale: usize, use_sign: bool) -> i32 {
    if use_sign && n1.sign != n2.sign {
        // scale and n_scale differ only in Number objects (0.00 <=> -0.00).
        if (n1.scale > scale || n2.scale > scale)
            && n1.len == 1
            && n2.len == 1
            && n1.digits[0] == 0
            && n2.digits[0] == 0
            && n1.is_zero_for_scale(scale)
            && n2.is_zero_for_scale(scale)
        {
            return 0;
        }
        return if n1.sign == Sign::Plus { 1 } else { -1 };
    }

    if n1.len != n2.len {
        return compare_result(n1.len > n2.len, use_sign, n1.sign);
    }

    let n1_scale = min(n1.scale, scale);
    let n2_scale = min(n2.scale, scale);
    let count = n1.len + min(n1_scale, n2_scale);
    if let Some(i) = (0..count).find(|&i| n1.digits[i] != n2.digits[i]) {
        return compare_result(n1.digits[i] > n2.digits[i], use_sign, n1.sign);
    }

    if n1_scale > n2_scale {
        if n1.digits[count..n1.len + n1_scale].iter().any(|&d| d != 0) {
            return compare_result(true, use_sign, n1.sign);
        }
    } else if n2_scale > n1_scale && n2.digits[count..n2.len + n2_scale].iter().any(|&d| d != 0) {
        return compare_result(false, use_sign, n1.sign);
    }
    0
}

/// `bc_compare`.
pub fn compare(n1: &BcNum, n2: &BcNum, scale: usize) -> i32 {
    do_compare(n1, n2, scale, true)
}

/// `_bc_do_add`: adds magnitudes (signs ignored).
fn do_add(n1: &BcNum, n2: &BcNum) -> BcNum {
    let sum_len = max(n1.len, n2.len) + 1;
    let sum_scale = max(n1.scale, n2.scale);
    let mut digits = vec![0u8; sum_len + sum_scale];
    let mut carry = 0u8;
    for pos in 0..digits.len() {
        let mut v =
            n1.digit_from_right(pos, sum_scale) + n2.digit_from_right(pos, sum_scale) + carry;
        if v >= 10 {
            v -= 10;
            carry = 1;
        } else {
            carry = 0;
        }
        let last = digits.len() - 1;
        digits[last - pos] = v;
    }
    let mut sum = BcNum {
        sign: Sign::Plus,
        len: sum_len,
        scale: sum_scale,
        digits,
    };
    sum.rm_leading_zeros();
    sum
}

/// `_bc_do_sub`: subtracts magnitudes; `n1` must not be smaller than `n2`.
fn do_sub(n1: &BcNum, n2: &BcNum) -> BcNum {
    let diff_len = max(n1.len, n2.len);
    let diff_scale = max(n1.scale, n2.scale);
    let mut digits = vec![0u8; diff_len + diff_scale];
    let mut borrow = 0i8;
    for pos in 0..digits.len() {
        let mut v = n1.digit_from_right(pos, diff_scale) as i8
            - n2.digit_from_right(pos, diff_scale) as i8
            - borrow;
        if v < 0 {
            v += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        let last = digits.len() - 1;
        digits[last - pos] = v as u8;
    }
    let mut diff = BcNum {
        sign: Sign::Plus,
        len: diff_len,
        scale: diff_scale,
        digits,
    };
    diff.rm_leading_zeros();
    diff
}

/// `bc_add`.
pub fn add(n1: &BcNum, n2: &BcNum, scale_min: usize) -> BcNum {
    if n1.sign == n2.sign {
        let mut sum = do_add(n1, n2);
        sum.sign = n1.sign;
        return sum;
    }
    // subtraction must be done
    match do_compare(n1, n2, scale_min, false) {
        -1 => {
            let mut sum = do_sub(n2, n1);
            sum.sign = n2.sign;
            sum
        }
        0 => BcNum::new(1, max(scale_min, max(n1.scale, n2.scale))),
        _ => {
            let mut sum = do_sub(n1, n2);
            sum.sign = n1.sign;
            sum
        }
    }
}

/// `bc_sub`.
pub fn sub(n1: &BcNum, n2: &BcNum, scale_min: usize) -> BcNum {
    if n1.sign != n2.sign {
        let mut diff = do_add(n1, n2);
        diff.sign = n1.sign;
        return diff;
    }
    match do_compare(n1, n2, scale_min, false) {
        -1 => {
            let mut diff = do_sub(n2, n1);
            diff.sign = if n2.sign == Sign::Plus {
                Sign::Minus
            } else {
                Sign::Plus
            };
            diff
        }
        0 => BcNum::new(1, max(scale_min, max(n1.scale, n2.scale))),
        _ => {
            let mut diff = do_sub(n1, n2);
            diff.sign = n1.sign;
            diff
        }
    }
}

/// `bc_multiply`.
pub fn multiply(n1: &BcNum, n2: &BcNum, scale: usize) -> BcNum {
    let full_scale = n1.scale + n2.scale;
    let prod_scale = min(full_scale, max(scale, max(n1.scale, n2.scale)));
    let digits = mul_digits(&n1.digits, &n2.digits);
    let len = n1.len + n2.len;
    let mut prod = BcNum {
        sign: if n1.sign == n2.sign {
            Sign::Plus
        } else {
            Sign::Minus
        },
        len,
        scale: full_scale,
        digits,
    };
    prod.truncate_scale(prod_scale);
    prod.rm_leading_zeros();
    if prod.is_zero() {
        prod.sign = Sign::Plus;
        prod.truncate_scale(0);
    }
    prod
}

/// `bc_divide`: `None` when dividing by zero.
pub fn divide(numerator: &BcNum, divisor: &BcNum, scale: usize) -> Option<BcNum> {
    if divisor.is_zero() {
        return None;
    }
    let quot_scale = scale;

    // If numerator is zero, the quotient is always zero.
    if numerator.is_zero() {
        return Some(BcNum::zero());
    }

    let sign_of = |quot: &BcNum| {
        if quot.is_zero() || numerator.sign == divisor.sign {
            Sign::Plus
        } else {
            Sign::Minus
        }
    };

    // If divisor is 1 / -1, the quotient's digits are the numerator's.
    if do_compare(divisor, &BcNum::one(), divisor.scale, false) == 0 {
        let qs = min(numerator.scale, quot_scale);
        let mut quot = BcNum {
            sign: Sign::Plus,
            len: numerator.len,
            scale: qs,
            digits: numerator.digits[..numerator.len + qs].to_vec(),
        };
        quot.sign = sign_of(&quot);
        return Some(quot);
    }

    let mut numerator_size = numerator.len + quot_scale + divisor.scale;

    // check and remove numerator leading zeros
    let mut numerator_leading_zeros = 0;
    while numerator.digits[numerator_leading_zeros] == 0 {
        numerator_leading_zeros += 1;
        if numerator_leading_zeros == numerator_size {
            return Some(BcNum::zero());
        }
    }
    numerator_size -= numerator_leading_zeros;

    // check and remove divisor leading zeros
    let mut divisor_start = 0;
    let mut divisor_size = divisor.len + divisor.scale;
    while divisor.digits[divisor_start] == 0 {
        divisor_start += 1;
        divisor_size -= 1;
    }

    if divisor_size > numerator_size {
        return Some(BcNum::zero());
    }

    // check and remove divisor trailing zeros (leave at least one digit)
    let divisor_trailing_zeros = (1..divisor_size)
        .rev()
        .take_while(|&i| divisor.digits[divisor_start + i] == 0)
        .count();
    divisor_size -= divisor_trailing_zeros;
    numerator_size -= divisor_trailing_zeros;

    let quot_size = numerator_size - divisor_size + 1;
    let (quot_len, total) = if quot_size > quot_scale {
        (quot_size - quot_scale, quot_size)
    } else {
        (1, 1 + quot_scale)
    };

    // Size that can be read from the numerator.
    let numerator_readable_size = numerator.len + numerator.scale - numerator_leading_zeros;
    let numerator_digits = &numerator.digits[numerator_leading_zeros..];
    let divisor_digits = &divisor.digits[divisor_start..divisor_start + divisor_size];

    // If divisor is 1 here, return the numerator with the decimal point moved.
    if divisor_size == 1 && divisor_digits[0] == 1 {
        let mut quot = BcNum {
            sign: Sign::Plus,
            len: quot_len,
            scale: total - quot_len,
            digits: vec![0; total],
        };
        // The leading `quot_scale - quot_size + 1` digits stay zero.
        let lead = if quot_size <= quot_scale {
            quot_scale - quot_size + 1
        } else {
            0
        };
        let used = min(quot_size, numerator_readable_size);
        quot.digits[lead..lead + used].copy_from_slice(&numerator_digits[..used]);
        if used < quot.len {
            // e.g. 12.3 / 0.01 <=> 1230
            quot.scale = 0;
            quot.digits.truncate(quot.len);
        } else {
            let filled = lead + used;
            quot.scale = filled - quot.len;
            quot.digits.truncate(filled);
        }
        quot.sign = sign_of(&quot);
        return Some(quot);
    }

    // The numerator reads as the first `numerator_size` digits, padded with
    // zeros when the number is shorter.
    let mut a = vec![0u8; numerator_size];
    let used = min(numerator_size, numerator_readable_size);
    a[..used].copy_from_slice(&numerator_digits[..used]);
    let q = div_digits(&a, divisor_digits);
    let mut quot = BcNum {
        sign: Sign::Plus,
        len: quot_len,
        scale: quot_scale,
        digits: vec![0; total],
    };
    let at = total - q.len();
    quot.digits[at..].copy_from_slice(&q);
    quot.rm_leading_zeros();
    if quot.is_zero() {
        quot.sign = Sign::Plus;
        quot.truncate_scale(0);
    } else {
        quot.sign = sign_of(&quot);
    }
    Some(quot)
}

/// `bc_divmod`: quotient (scale 0) and remainder; `None` when dividing by zero.
pub fn divmod(num1: &BcNum, num2: &BcNum, scale: usize) -> Option<(BcNum, BcNum)> {
    if num2.is_zero() {
        return None;
    }
    // Calculate final scale.
    let rscale = max(num1.scale, num2.scale + scale);
    let temp = divide(num1, num2, 0)?;
    let quotient = temp.clone();
    let product = multiply(&temp, num2, rscale);
    let mut rem = sub(num1, &product, rscale);

    // The value of rscale changes during processing; `scale` is intended here.
    rem.truncate_scale(min(scale, rem.scale));
    if rem.is_zero() {
        rem.sign = Sign::Plus;
        rem.truncate_scale(0);
    }
    Some((quotient, rem))
}

/// `bc_modulo`.
pub fn modulo(num1: &BcNum, num2: &BcNum, scale: usize) -> Option<BcNum> {
    divmod(num1, num2, scale).map(|(_, rem)| rem)
}

/// `bc_floor_or_ceil`.
pub fn floor_or_ceil(num: &BcNum, is_floor: bool) -> BcNum {
    let mut result = BcNum {
        sign: num.sign,
        len: num.len,
        scale: 0,
        digits: num.digits[..num.len].to_vec(),
    };

    // Positive and flooring (or negative and ceiling): nothing else to do.
    let keep = num.scale == 0 || result.sign == if is_floor { Sign::Plus } else { Sign::Minus };
    if !keep && num.digits[num.len..].iter().any(|&d| d != 0) {
        // Increment the absolute value of the result by 1.
        let mut tmp = do_add(&result, &BcNum::one());
        tmp.sign = result.sign;
        result = tmp;
    }
    if result.is_zero() {
        result.sign = Sign::Plus;
    }
    result
}

/// `bc_round`: the rounded number and the scale of the value after rounding.
/// `limit` is the memory limit used to reject absurd precisions.
pub fn round(
    num: &BcNum,
    precision: i64,
    mode: RoundMode,
    limit: Option<usize>,
) -> Result<(BcNum, usize), AllocError> {
    // Rounding to an integer part larger than the number (0.1 with precision -3).
    if precision < 0 && (num.len as u128) < (-(i128::from(precision) + 1)) as u128 + 1 {
        match mode {
            RoundMode::HalfAwayFromZero
            | RoundMode::HalfTowardsZero
            | RoundMode::HalfEven
            | RoundMode::HalfOdd
            | RoundMode::TowardsZero => return Ok((BcNum::zero(), 0)),
            RoundMode::PositiveInfinity => {
                if num.sign == Sign::Minus {
                    return Ok((BcNum::zero(), 0));
                }
            }
            RoundMode::NegativeInfinity => {
                if num.sign == Sign::Plus {
                    return Ok((BcNum::zero(), 0));
                }
            }
            RoundMode::AwayFromZero => {}
        }
        if num.is_zero() {
            return Ok((BcNum::zero(), 0));
        }
        // precision -3 becomes 1000.
        let magnitude = precision.unsigned_abs() as usize;
        check_limit(magnitude.saturating_add(1), limit)?;
        let mut result = BcNum::new(magnitude + 1, 0);
        result.digits[0] = 1;
        result.sign = num.sign;
        return Ok((result, 0));
    }

    // Pads with zeros when the number has fewer digits (bcadd('1', '1', 4) is 2.0000).
    if precision >= 0 && (num.scale as u128) <= precision as u128 {
        let precision = precision as usize;
        if num.scale == precision {
            return Ok((num.clone(), precision));
        }
        check_limit(num.len.saturating_add(precision), limit)?;
        let mut result = BcNum::new(num.len, precision);
        result.sign = num.sign;
        result.digits[..num.len + num.scale].copy_from_slice(&num.digits);
        return Ok((result, precision));
    }

    // No underflow: the negative case returned early above.
    let rounded_len = (num.len as i64 + precision) as usize;
    let mut result = if rounded_len == 0 {
        BcNum::new(1, 0)
    } else {
        let mut r = BcNum::new(num.len, if precision > 0 { precision as usize } else { 0 });
        r.digits[..rounded_len].copy_from_slice(&num.digits[..rounded_len]);
        r
    };
    result.sign = num.sign;

    let first = num.digits[rounded_len];
    enum Step {
        Up,
        CheckZero,
        Loop,
    }
    // Cases that can be determined without looping.
    let step = match mode {
        RoundMode::HalfAwayFromZero => {
            if first >= 5 {
                Step::Up
            } else {
                Step::CheckZero
            }
        }
        RoundMode::HalfTowardsZero | RoundMode::HalfEven | RoundMode::HalfOdd => {
            if first > 5 {
                Step::Up
            } else if first < 5 {
                Step::CheckZero
            } else {
                Step::Loop
            }
        }
        RoundMode::PositiveInfinity => {
            if num.sign != Sign::Plus {
                Step::CheckZero
            } else if first > 0 {
                Step::Up
            } else {
                Step::Loop
            }
        }
        RoundMode::NegativeInfinity => {
            if num.sign != Sign::Minus {
                Step::CheckZero
            } else if first > 0 {
                Step::Up
            } else {
                Step::Loop
            }
        }
        RoundMode::TowardsZero => Step::CheckZero,
        RoundMode::AwayFromZero => {
            if first > 0 {
                Step::Up
            } else {
                Step::Loop
            }
        }
    };

    let up = match step {
        Step::Up => true,
        Step::CheckZero => false,
        Step::Loop => {
            // Loop through the remaining digits.
            let rest = &num.digits[rounded_len + 1..];
            if rest.iter().any(|&d| d != 0) {
                true
            } else {
                match mode {
                    RoundMode::HalfTowardsZero
                    | RoundMode::PositiveInfinity
                    | RoundMode::NegativeInfinity
                    | RoundMode::AwayFromZero => false,
                    RoundMode::HalfEven => {
                        !(rounded_len == 0 || num.digits[rounded_len - 1] & 1 == 0)
                    }
                    RoundMode::HalfOdd => {
                        !(rounded_len != 0 && num.digits[rounded_len - 1] & 1 == 1)
                    }
                    _ => unreachable!("handled without looping"),
                }
            }
        }
    };

    if up {
        let tmp = if rounded_len == 0 {
            let mut t = BcNum::new(num.len + 1, 0);
            t.digits[0] = 1;
            t.sign = num.sign;
            t
        } else {
            let mut scaled_one = BcNum::new(result.len, result.scale);
            scaled_one.digits[rounded_len - 1] = 1;
            let mut t = do_add(&result, &scaled_one);
            t.sign = result.sign;
            t
        };
        result = tmp;
    }

    let scale = result.scale;
    if result.is_zero() {
        result.sign = Sign::Plus;
        result.truncate_scale(0);
    }
    Ok((result, scale))
}

fn check_limit(bytes: usize, limit: Option<usize>) -> Result<(), AllocError> {
    match limit {
        Some(l) if bytes > l => Err(AllocError(bytes)),
        _ => Ok(()),
    }
}

/// `bc_raise`: `base` raised to the integer `exponent`.
pub fn raise(
    base: &BcNum,
    exponent: i64,
    scale: usize,
    limit: Option<usize>,
) -> Result<BcNum, RaiseError> {
    // Special case if exponent is a zero.
    if exponent == 0 {
        return Ok(BcNum::one());
    }

    let is_neg = exponent < 0;
    let exponent = exponent.unsigned_abs();
    let rscale = scale;

    if base.is_zero() {
        // If the exponent is negative, it divides by 0.
        if is_neg {
            return Err(RaiseError::DivideByZero);
        }
        return Ok(BcNum::zero());
    }

    // check overflow
    let exp = exponent as usize;
    if base.len > usize::MAX / exp {
        return Err(RaiseError::LenOverflow);
    }
    if base.scale > usize::MAX / exp {
        return Err(RaiseError::ScaleOverflow);
    }

    let base_len = base.len + base.scale;
    let power_len = base.len * exp;
    let power_scale = base.scale * exp;
    if power_len > usize::MAX - power_scale {
        return Err(RaiseError::FullLenOverflow);
    }
    let power_full_len = power_len + power_scale;

    let power_sign = if base.sign == Sign::Minus && exponent & 1 == 1 {
        Sign::Minus
    } else {
        Sign::Plus
    };

    // bc_standard_raise allocates three BC_VECTOR buffers (8 digits per element).
    if !(base_len <= 8 && power_full_len <= 16) {
        let leading = base.digits.iter().take_while(|&&d| d == 0).count();
        let arr_size = (base_len - leading).div_ceil(8);
        let count = arr_size * exp;
        match count.checked_mul(8 * 3) {
            None => return Err(RaiseError::AllocOverflow { count, size: 24 }),
            Some(bytes) => {
                if matches!(limit, Some(l) if bytes > l) {
                    return Err(RaiseError::Alloc(bytes));
                }
            }
        }
    }

    // The integer power of the digits, written right-aligned into
    // `power_full_len` digits (at least `power_scale + 1`).
    let leading = base.digits.iter().take_while(|&&d| d == 0).count();
    let p = pow_digits(&base.digits[leading..], exponent);
    let total = max(p.len(), power_scale + 1);
    let mut digits = vec![0u8; total];
    digits[total - p.len()..].copy_from_slice(&p);
    let mut power = BcNum {
        sign: Sign::Plus,
        len: total - power_scale,
        scale: power_scale,
        digits,
    };
    power.rm_leading_zeros();
    if power.is_zero() {
        power.sign = Sign::Plus;
        power.truncate_scale(0);
    } else {
        power.sign = power_sign;
    }

    if is_neg {
        match divide(&BcNum::one(), &power, rscale) {
            Some(q) => Ok(q),
            None => Err(RaiseError::DivideByZero),
        }
    } else {
        power.truncate_scale(min(scale, power.scale));
        if power.is_zero() {
            power.sign = Sign::Plus;
        }
        Ok(power)
    }
}

/// `bc_raisemod`.
pub fn raisemod(
    base: &BcNum,
    expo: &BcNum,
    modulus: &BcNum,
    scale: usize,
) -> Result<BcNum, RaiseModError> {
    if base.scale != 0 {
        return Err(RaiseModError::BaseHasFractional);
    }
    if expo.scale != 0 {
        return Err(RaiseModError::ExpoHasFractional);
    }
    if expo.is_neg() {
        return Err(RaiseModError::ExpoIsNegative);
    }
    if modulus.scale != 0 {
        return Err(RaiseModError::ModHasFractional);
    }
    if modulus.is_zero() {
        return Err(RaiseModError::ModIsZero);
    }

    // Any integer number mod 1 (or -1) must be equal to 0.
    if do_compare(modulus, &BcNum::one(), modulus.scale, false) == 0 {
        return Ok(BcNum::new(1, scale));
    }

    let two = BcNum::from_int(2);
    let mut power = base.clone();
    let mut exponent = expo.clone();
    let mut temp = BcNum::one();
    while !exponent.is_zero() {
        let (quot, parity) = divmod(&exponent, &two, 0).expect("divisor is non-zero");
        exponent = quot;
        if !parity.is_zero() {
            temp = multiply(&temp, &power, scale);
            temp = modulo(&temp, modulus, scale).expect("modulus is non-zero");
        }
        power = multiply(&power, &power, scale);
        power = modulo(&power, modulus, scale).expect("modulus is non-zero");
    }
    Ok(temp)
}

/// `bc_sqrt`: `None` for a negative number.
pub fn sqrt(num: &BcNum, scale: usize) -> Option<BcNum> {
    if num.is_neg() {
        return None;
    }
    // Square root of 0 is 0.
    if num.is_zero() {
        return Some(BcNum::zero());
    }
    let num_cmp_one = compare(num, &BcNum::one(), num.scale);
    // Square root of 1 is 1.
    if num_cmp_one == 0 {
        return Some(BcNum::one());
    }

    let rscale = max(scale, num.scale);
    let mut point5 = BcNum::new(1, 1);
    point5.digits[1] = 5;

    // Calculate the initial guess.
    let mut cscale;
    let mut guess;
    if num_cmp_one < 0 {
        // The number is between 0 and 1.  Guess should start at 1.
        guess = BcNum::one();
        cscale = num.scale;
    } else {
        // The number is greater than 1.  Guess should start at 10^(exp/2).
        let ten = BcNum::from_int(10);
        let mut guess1 = BcNum::from_long(num.len as i64);
        guess1 = multiply(&guess1, &point5, 0);
        guess1.truncate_scale(0);
        guess = raise(&ten, guess1.to_long(), 0, None).expect("small power of ten");
        cscale = 3;
    }

    // Find the square root using Newton's algorithm.
    loop {
        let guess1 = guess.clone();
        guess = divide(num, &guess, cscale).expect("guess is non-zero");
        guess = add(&guess, &guess1, 0);
        guess = multiply(&guess, &point5, cscale);
        let diff = sub(&guess, &guess1, cscale + 1);
        if diff.is_near_zero(cscale) {
            if cscale < rscale + 1 {
                cscale = min(cscale * 3, rscale + 1);
            } else {
                break;
            }
        }
    }

    Some(divide(&guess, &BcNum::one(), rscale).expect("one is non-zero"))
}
