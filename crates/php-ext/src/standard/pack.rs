//! ext/standard/pack.c — `pack()`.
//!
//! Binary serialisation driven by Perl-style format strings. Each format
//! code may be followed by an endianness modifier (`<` little, `>` big; only
//! for `s S l L q Q f d`) and a repeater (a decimal count or `*`).
//!
//! Sizes follow the platforms PHP supports: `sizeof(int) == sizeof(float) == 4`,
//! `sizeof(double) == 8` and a 64-bit `zend_long`.

use php_runtime::{Args, Flow, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// Byte order of a multi-byte value (`php_pack_endianness`; the
/// `PHP_NO_ENDIAN_MODIFIER` state is `Option::None`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endian {
    Little,
    Big,
}

/// `MACHINE_LITTLE_ENDIAN`.
const MACHINE_LITTLE_ENDIAN: bool = cfg!(target_endian = "little");

/// `PHP_MACHINE_ENDIAN`.
pub const MACHINE_ENDIAN: Endian = if MACHINE_LITTLE_ENDIAN {
    Endian::Little
} else {
    Endian::Big
};

const INT_MAX: i64 = i32::MAX as i64;
/// `sizeof(int)`.
const INT_SIZE: i64 = 4;
/// `sizeof(float)`.
const FLOAT_SIZE: i64 = 4;
/// `sizeof(double)`.
const DOUBLE_SIZE: i64 = 8;

/// `php_pack()`: stores the low `size` bytes (1..=8) of `value` in `out` in the
/// given byte order, with the same `zend_ulong` swaps and shifts as the C code.
pub fn pack_int(value: i64, size: usize, endian: Endian, out: &mut [u8]) {
    debug_assert!((1..=8).contains(&size));
    let mut zl = value as u64;
    if (endian == Endian::Little) != MACHINE_LITTLE_ENDIAN {
        zl = zl.swap_bytes();
        if MACHINE_LITTLE_ENDIAN {
            zl >>= (8 - size) * 8;
        }
    } else if !MACHINE_LITTLE_ENDIAN {
        zl <<= (8 - size) * 8;
    }
    out[..size].copy_from_slice(&zl.to_ne_bytes()[..size]);
}

/// `php_pack_copy_float(is_little_endian)` for `Some`, a plain `memcpy` of the
/// machine representation for `None`.
fn float_bytes(v: f32, little: Option<bool>) -> [u8; 4] {
    let mut bits = v.to_bits();
    if little.is_some_and(|l| l != MACHINE_LITTLE_ENDIAN) {
        bits = bits.swap_bytes();
    }
    bits.to_ne_bytes()
}

/// `php_pack_copy_double` / machine `memcpy`.
fn double_bytes(v: f64, little: Option<bool>) -> [u8; 8] {
    let mut bits = v.to_bits();
    if little.is_some_and(|l| l != MACHINE_LITTLE_ENDIAN) {
        bits = bits.swap_bytes();
    }
    bits.to_ne_bytes()
}

/// Byte order of a float/double code: `little_code`/`big_code` have inherent
/// endianness, otherwise the modifier decides; `None` means machine order.
fn float_order(code: u8, little_code: u8, big_code: u8, endian: Option<Endian>) -> Option<bool> {
    if code == little_code || endian == Some(Endian::Little) {
        Some(true)
    } else if code == big_code || endian == Some(Endian::Big) {
        Some(false)
    } else {
        None
    }
}

/// Builds a message that embeds format-code bytes verbatim (C's `%c`).
fn msg(parts: &[&[u8]]) -> PhpStr {
    PhpStr::from_vec(parts.concat())
}

/// `"Type %c: <text>"`.
fn type_msg(code: u8, text: &str) -> PhpStr {
    msg(&[b"Type ", &[code], b": ", text.as_bytes()])
}

fn unsupported_modifier(vm: &mut Vm, code: u8) -> Flow {
    vm.throw_value_error(msg(&[
        b"Endianness modifier is not supported for format code '",
        &[code],
        b"'",
    ]))
}

fn inherent_endianness(vm: &mut Vm, endian: Endian, code: u8) -> Flow {
    let modifier = match endian {
        Endian::Little => b'<',
        Endian::Big => b'>',
    };
    vm.throw_value_error(msg(&[
        b"Endianness modifier '",
        &[modifier],
        b"' cannot be applied to format code '",
        &[code],
        b"' which already has inherent endianness",
    ]))
}

/// Reads an endianness modifier at `fmt[*i]`, advancing past it.
fn parse_modifier(fmt: &[u8], i: &mut usize) -> Option<Endian> {
    let endian = match fmt.get(*i) {
        Some(b'<') => Endian::Little,
        Some(b'>') => Endian::Big,
        _ => return None,
    };
    *i += 1;
    Some(endian)
}

/// `strtol(s, NULL, 10)` over the leading ASCII digits of `s`: saturates at
/// `LONG_MAX` like `strtol` does on overflow.
fn strtol_digits(s: &[u8]) -> i64 {
    s.iter()
        .take_while(|d| d.is_ascii_digit())
        .fold(0i64, |acc, &d| {
            acc.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        })
}

/// One preprocessed `pack()` format code.
struct PackCode {
    code: u8,
    arg: i64,
    endian: Option<Endian>,
}

/// `PHP_FUNCTION(pack)`.
fn pack(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let format = args.str(0).clone();
    let fmt = format.as_bytes();
    let mut argv: Vec<Value> = args.rest(1).iter().map(Value::unref).collect();
    let num_args = argv.len() as i64;
    let mut codes: Vec<PackCode> = Vec::with_capacity(fmt.len());
    let mut currentarg: i64 = 0;

    // Preprocess the format into codes, repeaters and modifiers.
    let mut i = 0;
    while i < fmt.len() {
        let code = fmt[i];
        i += 1;
        let endian = parse_modifier(fmt, &mut i);
        let mut arg: i64 = 1;
        if let Some(&c) = fmt.get(i) {
            if c == b'*' {
                arg = -1;
                i += 1;
            } else if c.is_ascii_digit() {
                // atoi() is strtol() truncated to int: huge repeaters can turn
                // negative and then behave like '*'.
                arg = i64::from(strtol_digits(&fmt[i..]) as i32);
                while i < fmt.len() && fmt[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }

        // Codes consuming `arg` values ('*' = all remaining ones).
        let mut consume = |arg: &mut i64| -> bool {
            if *arg < 0 {
                *arg = num_args - currentarg;
            }
            if currentarg > INT_MAX - *arg {
                return false;
            }
            currentarg += *arg;
            currentarg <= num_args
        };

        let enough = match code {
            b'x' | b'X' | b'@' => {
                if endian.is_some() {
                    return Err(unsupported_modifier(vm, code));
                }
                if arg < 0 {
                    vm.warning(type_msg(code, "'*' ignored"))?;
                    arg = 1;
                }
                true
            }
            b'a' | b'A' | b'Z' | b'h' | b'H' => {
                if endian.is_some() {
                    return Err(unsupported_modifier(vm, code));
                }
                if currentarg >= num_args {
                    return Err(vm.throw_value_error(type_msg(code, "not enough arguments")));
                }
                if arg < 0 {
                    // try_convert_to_string(): the argument is converted in place.
                    let idx = currentarg as usize;
                    let s = vm.to_php_string(&argv[idx])?;
                    arg = s.len() as i64;
                    if code == b'Z' {
                        // Z is always NUL-terminated: pack("Z*", "aa") === "aa\0".
                        arg += 1;
                    }
                    argv[idx] = Value::String(s);
                }
                currentarg += 1;
                true
            }
            b'J' | b'P' | b'n' | b'N' | b'v' | b'V' | b'g' | b'G' | b'e' | b'E' => {
                if let Some(e) = endian {
                    return Err(inherent_endianness(vm, e, code));
                }
                consume(&mut arg)
            }
            b'q' | b'Q' | b's' | b'S' | b'l' | b'L' | b'f' | b'd' => consume(&mut arg),
            b'c' | b'C' | b'i' | b'I' => {
                if endian.is_some() {
                    return Err(unsupported_modifier(vm, code));
                }
                consume(&mut arg)
            }
            _ => return Err(vm.throw_value_error(type_msg(code, "unknown format code"))),
        };
        if !enough {
            return Err(vm.throw_value_error(type_msg(code, "too few arguments")));
        }
        codes.push(PackCode { code, arg, endian });
    }

    if currentarg < num_args {
        vm.warning(format!("{} arguments unused", num_args - currentarg))?;
    }

    // Calculate the output length; INC_OUTPUTPOS keeps it within INT_MAX.
    let mut outputpos: i64 = 0;
    let mut outputsize: i64 = 0;
    for c in &codes {
        let (code, arg) = (c.code, c.arg);
        let step = match code {
            b'h' | b'H' => Some((arg / 2 + arg % 2, 1)),
            b'a' | b'A' | b'Z' | b'c' | b'C' | b'x' => Some((arg, 1)),
            b's' | b'S' | b'n' | b'v' => Some((arg, 2)),
            b'i' | b'I' => Some((arg, INT_SIZE)),
            b'l' | b'L' | b'N' | b'V' => Some((arg, 4)),
            b'q' | b'Q' | b'J' | b'P' => Some((arg, 8)),
            b'f' | b'g' | b'G' => Some((arg, FLOAT_SIZE)),
            b'd' | b'e' | b'E' => Some((arg, DOUBLE_SIZE)),
            b'X' => {
                outputpos -= arg;
                if outputpos < 0 {
                    vm.warning(type_msg(code, "outside of string"))?;
                    outputpos = 0;
                }
                None
            }
            b'@' => {
                outputpos = arg;
                None
            }
            _ => None,
        };
        if let Some((count, width)) = step {
            if count < 0 || (INT_MAX - outputpos) / width < count {
                return Err(
                    vm.throw_value_error(type_msg(code, "integer overflow in format string"))
                );
            }
            outputpos += count * width;
        }
        outputsize = outputsize.max(outputpos);
    }

    // Do the actual packing. Every position stays at or below the one
    // computed above, so `out` never needs to grow.
    let mut out = vec![0u8; outputsize as usize];
    let mut pos: i64 = 0;
    let mut values = argv.iter();
    for c in &codes {
        let code = c.code;
        let mut arg = c.arg;
        let mut next_value = || {
            values
                .next()
                .expect("argument count checked while preprocessing")
        };
        match code {
            b'a' | b'A' | b'Z' => {
                let max_copy = if code == b'Z' { (arg - 1).max(0) } else { arg };
                let s = vm.to_php_string(next_value())?;
                let copy = (s.len() as i64).min(max_copy) as usize;
                let start = pos as usize;
                out[start..start + copy].copy_from_slice(&s[..copy]);
                out[start + copy..start + arg as usize].fill(if code == b'A' { b' ' } else { 0 });
                pos += arg;
            }
            b'h' | b'H' => {
                let mut nibbleshift = if code == b'h' { 0 } else { 4 };
                let mut first = true;
                let s = vm.to_php_string(next_value())?;
                pos -= 1;
                if arg > s.len() as i64 {
                    vm.warning(type_msg(code, "not enough characters in string"))?;
                    arg = s.len() as i64;
                }
                for &ch in &s[..arg as usize] {
                    let n = match ch {
                        b'0'..=b'9' => ch - b'0',
                        b'A'..=b'F' => ch - b'A' + 10,
                        b'a'..=b'f' => ch - b'a' + 10,
                        _ => {
                            vm.warning(msg(&[b"Type ", &[code], b": illegal hex digit ", &[ch]]))?;
                            0
                        }
                    };
                    if first {
                        pos += 1;
                        out[pos as usize] = 0;
                        first = false;
                    } else {
                        first = true;
                    }
                    out[pos as usize] |= n << nibbleshift;
                    nibbleshift = (nibbleshift + 4) & 7;
                }
                pos += 1;
            }
            b'c' | b'C' | b'i' | b'I' | b's' | b'S' | b'n' | b'v' | b'l' | b'L' | b'N' | b'V'
            | b'q' | b'Q' | b'J' | b'P' => {
                let (size, inherent) = match code {
                    b'c' | b'C' => (1, Some(MACHINE_ENDIAN)),
                    b'i' | b'I' => (INT_SIZE as usize, Some(MACHINE_ENDIAN)),
                    b'n' => (2, Some(Endian::Big)),
                    b'v' => (2, Some(Endian::Little)),
                    b's' | b'S' => (2, None),
                    b'N' => (4, Some(Endian::Big)),
                    b'V' => (4, Some(Endian::Little)),
                    b'l' | b'L' => (4, None),
                    b'J' => (8, Some(Endian::Big)),
                    b'P' => (8, Some(Endian::Little)),
                    _ => (8, None),
                };
                let endian = inherent.or(c.endian).unwrap_or(MACHINE_ENDIAN);
                for _ in 0..arg {
                    let v = vm.to_int(next_value())?;
                    let start = pos as usize;
                    pack_int(v, size, endian, &mut out[start..start + size]);
                    pos += size as i64;
                }
            }
            b'f' | b'g' | b'G' => {
                let order = float_order(code, b'g', b'G', c.endian);
                for _ in 0..arg {
                    let v = vm.to_float(next_value())? as f32;
                    let start = pos as usize;
                    out[start..start + 4].copy_from_slice(&float_bytes(v, order));
                    pos += FLOAT_SIZE;
                }
            }
            b'd' | b'e' | b'E' => {
                let order = float_order(code, b'e', b'E', c.endian);
                for _ in 0..arg {
                    let v = vm.to_float(next_value())?;
                    let start = pos as usize;
                    out[start..start + 8].copy_from_slice(&double_bytes(v, order));
                    pos += DOUBLE_SIZE;
                }
            }
            b'x' => {
                let start = pos as usize;
                out[start..start + arg as usize].fill(0);
                pos += arg;
            }
            b'X' => pos = (pos - arg).max(0),
            b'@' => {
                if arg > pos {
                    out[pos as usize..arg as usize].fill(0);
                }
                pos = arg;
            }
            _ => {}
        }
    }

    out.truncate(pos as usize);
    Ok(Value::String(PhpStr::from_vec(out)))
}

pub static PACK: NativeFunction = NativeFunction {
    sig: "pack(string $format, mixed ...$values): string",
    handler: pack,
};

pub fn register(r: &mut Registry) {
    r.function(&PACK);
}

#[cfg(test)]
mod tests;
