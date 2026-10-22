//! `strftime()` / `gmstrftime()`: the C library's `strftime(3)` in the "C"
//! locale (glibc semantics, including its `_ - 0 ^ #` flags and field
//! widths), fed with the `struct tm` that `php_strftime` builds.

use php_timelib::dow::isoweek_from_date;
use php_timelib::format::{DAY_FULL_NAMES, DAY_SHORT_NAMES, MON_FULL_NAMES, MON_SHORT_NAMES};

/// The `struct tm` fields `php_strftime` fills in.
#[derive(Clone, Debug, Default)]
pub struct Tm {
    pub sec: i64,
    pub min: i64,
    pub hour: i64,
    pub mday: i64,
    /// 0-11.
    pub mon: i64,
    /// Years since 1900.
    pub year: i64,
    pub wday: i64,
    pub yday: i64,
    pub isdst: i64,
    pub gmtoff: i64,
    pub zone: String,
    /// The timestamp (`%s`).
    pub ts: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pad {
    Default,
    Space,
    Zero,
    None,
}

struct Flags {
    pad: Pad,
    upcase: bool,
    swapcase: bool,
    width: Option<usize>,
}

fn name<'a>(names: &[&'a str], i: i64) -> &'a str {
    usize::try_from(i)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .unwrap_or("?")
}

/// Appends a number padded to `digits` (with `default_pad` unless a flag
/// overrides it).
fn push_num(out: &mut Vec<u8>, f: &Flags, n: i64, digits: usize, default_pad: Pad) {
    let pad = match f.pad {
        Pad::Default => default_pad,
        p => p,
    };
    let neg = n < 0;
    let digits_str = n.unsigned_abs().to_string();
    let width = f.width.unwrap_or(digits);
    let len = digits_str.len() + usize::from(neg);
    let fill = width.saturating_sub(len);
    match pad {
        Pad::None => {
            if neg {
                out.push(b'-');
            }
            out.extend_from_slice(digits_str.as_bytes());
        }
        Pad::Space | Pad::Default => {
            out.extend(std::iter::repeat_n(b' ', fill));
            if neg {
                out.push(b'-');
            }
            out.extend_from_slice(digits_str.as_bytes());
        }
        Pad::Zero => {
            if neg {
                out.push(b'-');
            }
            out.extend(std::iter::repeat_n(b'0', fill));
            out.extend_from_slice(digits_str.as_bytes());
        }
    }
}

/// Appends a string field, honouring the width (space padding unless `0`)
/// and the case flags.
fn push_str(out: &mut Vec<u8>, f: &Flags, s: &str, swap_means_lower: bool) {
    let s: String = if f.upcase {
        s.to_ascii_uppercase()
    } else if f.swapcase {
        if swap_means_lower {
            s.to_ascii_lowercase()
        } else {
            s.to_ascii_uppercase()
        }
    } else {
        s.to_string()
    };
    if let Some(w) = f.width
        && w > s.len()
    {
        let c = if f.pad == Pad::Zero { b'0' } else { b' ' };
        out.extend(std::iter::repeat_n(c, w - s.len()));
    }
    out.extend_from_slice(s.as_bytes());
}

fn iso_week(tm: &Tm) -> (i64, i64) {
    isoweek_from_date(tm.year + 1900, tm.mon + 1, tm.mday)
}

/// Formats `tm` according to `format` (`strftime(3)` in the C locale).
pub fn strftime(format: &[u8], tm: &Tm) -> Vec<u8> {
    let mut out = Vec::with_capacity(format.len() * 2);
    format_into(&mut out, format, tm);
    out
}

fn format_into(out: &mut Vec<u8>, format: &[u8], tm: &Tm) {
    let mut i = 0;
    while i < format.len() {
        let c = format[i];
        if c != b'%' {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut f = Flags {
            pad: Pad::Default,
            upcase: false,
            swapcase: false,
            width: None,
        };
        while let Some(&fc) = format.get(i) {
            match fc {
                b'_' => f.pad = Pad::Space,
                b'-' => f.pad = Pad::None,
                b'0' => f.pad = Pad::Zero,
                b'^' => f.upcase = true,
                b'#' => f.swapcase = true,
                _ => break,
            }
            i += 1;
        }
        let wstart = i;
        while format.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i > wstart {
            let w: usize = std::str::from_utf8(&format[wstart..i])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(usize::MAX);
            f.width = Some(w.min(1 << 16));
        }
        // E and O modifiers have no effect in the C locale.
        while matches!(format.get(i), Some(b'E' | b'O')) {
            i += 1;
        }
        let Some(&conv) = format.get(i) else {
            // A trailing incomplete sequence is copied verbatim.
            out.extend_from_slice(&format[start..]);
            break;
        };
        i += 1;
        let hour12 = if tm.hour % 12 == 0 { 12 } else { tm.hour % 12 };
        let year = tm.year + 1900;
        match conv {
            b'%' => push_str(out, &f, "%", false),
            b'a' => push_str(out, &f, name(&DAY_SHORT_NAMES, tm.wday), false),
            b'A' => push_str(out, &f, name(&DAY_FULL_NAMES, tm.wday), false),
            b'b' | b'h' => push_str(out, &f, name(&MON_SHORT_NAMES, tm.mon), false),
            b'B' => push_str(out, &f, name(&MON_FULL_NAMES, tm.mon), false),
            b'c' => {
                let mut s = Vec::new();
                format_into(&mut s, b"%a %b %e %H:%M:%S %Y", tm);
                push_str(out, &f, &String::from_utf8_lossy(&s), false);
            }
            b'C' => push_num(out, &f, year.div_euclid(100), 2, Pad::Zero),
            b'd' => push_num(out, &f, tm.mday, 2, Pad::Zero),
            b'D' => push_sub(out, &f, b"%m/%d/%y", tm),
            b'e' => push_num(out, &f, tm.mday, 2, Pad::Space),
            b'F' => push_sub(out, &f, b"%Y-%m-%d", tm),
            b'g' => push_num(out, &f, iso_week(tm).1.rem_euclid(100), 2, Pad::Zero),
            b'G' => push_num(out, &f, iso_week(tm).1, 1, Pad::Zero),
            b'H' => push_num(out, &f, tm.hour, 2, Pad::Zero),
            b'I' => push_num(out, &f, hour12, 2, Pad::Zero),
            b'j' => push_num(out, &f, tm.yday + 1, 3, Pad::Zero),
            b'k' => push_num(out, &f, tm.hour, 2, Pad::Space),
            b'l' => push_num(out, &f, hour12, 2, Pad::Space),
            b'm' => push_num(out, &f, tm.mon + 1, 2, Pad::Zero),
            b'M' => push_num(out, &f, tm.min, 2, Pad::Zero),
            b'n' => push_str(out, &f, "\n", false),
            b'p' => push_str(out, &f, if tm.hour >= 12 { "PM" } else { "AM" }, true),
            b'P' => push_str(out, &f, if tm.hour >= 12 { "pm" } else { "am" }, false),
            b'r' => push_sub(out, &f, b"%I:%M:%S %p", tm),
            b'R' => push_sub(out, &f, b"%H:%M", tm),
            b's' => push_num(out, &f, tm.ts, 1, Pad::Zero),
            b'S' => push_num(out, &f, tm.sec, 2, Pad::Zero),
            b't' => push_str(out, &f, "\t", false),
            b'T' => push_sub(out, &f, b"%H:%M:%S", tm),
            b'u' => push_num(out, &f, (tm.wday + 6) % 7 + 1, 1, Pad::Zero),
            b'U' => push_num(out, &f, (tm.yday - tm.wday + 7) / 7, 2, Pad::Zero),
            b'V' => push_num(out, &f, iso_week(tm).0, 2, Pad::Zero),
            b'w' => push_num(out, &f, tm.wday, 1, Pad::Zero),
            b'W' => push_num(
                out,
                &f,
                (tm.yday - (tm.wday - 1 + 7) % 7 + 7) / 7,
                2,
                Pad::Zero,
            ),
            b'x' => push_sub(out, &f, b"%m/%d/%y", tm),
            b'X' => push_sub(out, &f, b"%H:%M:%S", tm),
            b'y' => push_num(out, &f, year.rem_euclid(100), 2, Pad::Zero),
            b'Y' => push_num(out, &f, year, 1, Pad::Zero),
            b'z' => {
                let off = tm.gmtoff;
                let sign = if off < 0 { "-" } else { "+" };
                let a = off.abs();
                let v = (a / 3600) * 100 + (a / 60) % 60;
                let mut digits = Vec::new();
                push_num(
                    &mut digits,
                    &Flags {
                        pad: Pad::Zero,
                        upcase: false,
                        swapcase: false,
                        width: Some(f.width.map_or(4, |w| w.saturating_sub(1))),
                    },
                    v,
                    4,
                    Pad::Zero,
                );
                out.extend_from_slice(sign.as_bytes());
                out.extend_from_slice(&digits);
            }
            b'Z' => push_str(out, &f, &tm.zone, true),
            _ => out.extend_from_slice(&format[start..i]),
        }
    }
}

/// A composite conversion (`%D`, `%T`, ...), padded as a whole.
fn push_sub(out: &mut Vec<u8>, f: &Flags, sub: &[u8], tm: &Tm) {
    let mut s = Vec::new();
    format_into(&mut s, sub, tm);
    push_str(out, f, &String::from_utf8_lossy(&s), false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm() -> Tm {
        // 2008-08-08 08:08:08 (Friday), day 220 of the year.
        Tm {
            sec: 8,
            min: 8,
            hour: 8,
            mday: 8,
            mon: 7,
            year: 108,
            wday: 5,
            yday: 220,
            isdst: 0,
            gmtoff: 19800,
            zone: "IST".into(),
            ts: 1_218_162_488,
        }
    }

    fn s(f: &str) -> String {
        String::from_utf8(strftime(f.as_bytes(), &tm())).unwrap()
    }

    #[test]
    fn conversions() {
        assert_eq!(s("%a %A %b %B %h"), "Fri Friday Aug August Aug");
        assert_eq!(s("%c"), "Fri Aug  8 08:08:08 2008");
        assert_eq!(s("%C %d %D %e %F"), "20 08 08/08/08  8 2008-08-08");
        assert_eq!(s("%g %G %V %u %w"), "08 2008 32 5 5");
        assert_eq!(s("%H %I %j %k %l %m %M"), "08 08 221  8  8 08 08");
        assert_eq!(s("%p %r %R %S %T"), "AM 08:08:08 AM 08:08 08 08:08:08");
        assert_eq!(s("%U %W %x %X %y %Y"), "31 31 08/08/08 08:08:08 08 2008");
        assert_eq!(s("%z %Z %% %n%t"), "+0530 IST % \n\t");
        assert_eq!(s("%Q %"), "%Q %");
        assert_eq!(s("%-d %_m %^a %#Z %10A"), "8  8 FRI ist     Friday");
    }
}
