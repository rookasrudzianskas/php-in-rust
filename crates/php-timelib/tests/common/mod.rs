//! Shared helpers for tests ported from `ext/date/tests/*.phpt`.
//!
//! These mirror the PHP functions used by the phpt files (with an explicit default
//! timezone and an explicit "now"), so expected outputs can be copied verbatim.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::sync::Arc;

use php_timelib::php::{self, PhpInterval, PhpTimeZone};
use php_timelib::{
    ErrorContainer, RelTime, SPECIAL_FIRST_DAY_OF_MONTH, Time, TzInfo, UNSET, ZoneType,
    builtin_tzinfo,
};

/// A fixed "now" used by tests whose expectations do not depend on the current time
/// (2005-07-14 22:30:41 UTC).
pub const NOW: i64 = 1_121_380_241;

/// Loads a zone from the bundled database.
pub fn tz(name: &str) -> Arc<TzInfo> {
    builtin_tzinfo(name).unwrap_or_else(|| panic!("unknown zone {name}"))
}

/// `strtotime($s, $now)` with `date_default_timezone_set($zone)`.
pub fn strtotime(s: &str, now: i64, zone: &str) -> Option<i64> {
    php::php_strtotime(s, now, &tz(zone))
}

/// `strtotime($s)` for inputs that do not depend on "now".
pub fn strtotime_abs(s: &str, zone: &str) -> Option<i64> {
    strtotime(s, NOW, zone)
}

/// `date($fmt, $ts)` with `date_default_timezone_set($zone)`.
pub fn date(fmt: &str, ts: i64, zone: &str) -> String {
    String::from_utf8(php_timelib::format_timestamp(
        fmt.as_bytes(),
        ts,
        Some(&tz(zone)),
    ))
    .unwrap()
}

/// `gmdate($fmt, $ts)`.
pub fn gmdate(fmt: &str, ts: i64) -> String {
    String::from_utf8(php_timelib::format_timestamp(fmt.as_bytes(), ts, None)).unwrap()
}

/// `mktime($h, $i, $s, $m, $d, $y)` with all arguments given.
pub fn mktime(h: i64, i: i64, s: i64, m: i64, d: i64, y: i64, zone: &str) -> i64 {
    php::php_mktime(
        h,
        Some(i),
        Some(s),
        Some(m),
        Some(d),
        Some(y),
        NOW,
        Some(&tz(zone)),
    )
    .unwrap()
}

/// `gmmktime($h, $i, $s, $m, $d, $y)` with all arguments given.
pub fn gmmktime(h: i64, i: i64, s: i64, m: i64, d: i64, y: i64) -> i64 {
    php::php_mktime(h, Some(i), Some(s), Some(m), Some(d), Some(y), NOW, None).unwrap()
}

/// `idate($fmt, $ts)` with `date_default_timezone_set($zone)`.
pub fn idate(fmt: char, ts: i64, zone: &str) -> Option<i32> {
    let mut t = Time {
        tz_info: Some(tz(zone)),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    php_timelib::idate(fmt as u8, &t)
}

/// A `DateTime` object.
#[derive(Clone, Debug)]
pub struct DT {
    pub t: Time,
    pub default_zone: String,
}

impl DT {
    /// `new DateTime($s)` with `date_default_timezone_set($zone)`; panics on errors.
    pub fn new(s: &str, zone: &str) -> DT {
        Self::try_new(s, None, zone).unwrap_or_else(|e| panic!("new DateTime({s:?}): {e:?}"))
    }

    /// `new DateTime($s, new DateTimeZone($tz))`.
    pub fn new_in(s: &str, tz: &str, zone: &str) -> DT {
        let z = php::timezone_open(tz).unwrap();
        Self::try_new(s, Some(&z), zone).unwrap()
    }

    /// `new DateTime($s, $tz)` returning the parse errors on failure.
    pub fn try_new(s: &str, tz: Option<&PhpTimeZone>, zone: &str) -> Result<DT, ErrorContainer> {
        php::date_initialize(s.as_bytes(), None, tz, &self::tz(zone), (NOW, 0)).map(|(t, _)| DT {
            t,
            default_zone: zone.to_string(),
        })
    }

    /// `DateTime::createFromFormat($f, $s)`; `None` on errors. Note: unspecified fields
    /// are taken from [`NOW`].
    pub fn from_format(f: &str, s: &str, tz: Option<&str>, zone: &str) -> Option<DT> {
        let z = tz.map(|t| php::timezone_open(t).unwrap());
        php::date_initialize(
            s.as_bytes(),
            Some(f.as_bytes()),
            z.as_ref(),
            &self::tz(zone),
            (NOW, 0),
        )
        .ok()
        .map(|(t, _)| DT {
            t,
            default_zone: zone.to_string(),
        })
    }

    /// `DateTime::format($f)`.
    pub fn format(&self, f: &str) -> String {
        String::from_utf8(php_timelib::date_format(
            f.as_bytes(),
            &self.t,
            self.t.is_localtime,
        ))
        .unwrap()
    }

    /// `DateTime::modify($s)`; returns false on failure.
    pub fn modify(&mut self, s: &str) -> bool {
        php::date_modify(&mut self.t, s.as_bytes()).is_ok()
    }

    /// `DateTime::setTimezone(new DateTimeZone($tz))`.
    pub fn set_timezone(&mut self, tz: &str) {
        php::date_timezone_set(&mut self.t, &php::timezone_open(tz).unwrap());
    }

    /// `DateTime::add(new DateInterval($spec))`.
    pub fn add(&mut self, spec: &str) {
        let iv = php::date_interval_create(spec).unwrap();
        self.t = php::date_add(&self.t, &iv);
    }

    /// `DateTime::add($interval)`.
    pub fn add_interval(&mut self, iv: &PhpInterval) {
        self.t = php::date_add(&self.t, iv);
    }

    /// `DateTime::sub(new DateInterval($spec))`.
    pub fn sub(&mut self, spec: &str) {
        let iv = php::date_interval_create(spec).unwrap();
        self.t = php::date_sub(&self.t, &iv).unwrap();
    }

    /// `DateTime::sub($interval)`.
    pub fn sub_interval(&mut self, iv: &PhpInterval) {
        self.t = php::date_sub(&self.t, iv).unwrap();
    }

    /// `$this->diff($other)`.
    pub fn diff(&self, other: &DT) -> PhpInterval {
        php::date_diff(&self.t, &other.t, false)
    }

    /// `DateTime::getTimestamp()`.
    pub fn timestamp(&self) -> i64 {
        self.t.sse
    }

    /// `DateTime::getOffset()`.
    pub fn offset(&self) -> i64 {
        php::date_offset_get(&self.t)
    }

    /// `DateTime::setDate()`.
    pub fn set_date(&mut self, y: i64, m: i64, d: i64) {
        php::date_date_set(&mut self.t, y, m, d);
    }

    /// `DateTime::setISODate()`.
    pub fn set_isodate(&mut self, y: i64, w: i64, d: i64) {
        php::date_isodate_set(&mut self.t, y, w, d);
    }

    /// `DateTime::setTime()`.
    pub fn set_time(&mut self, h: i64, i: i64, s: i64, us: i64) {
        php::date_time_set(&mut self.t, h, i, s, us);
    }

    /// `DateTime::setTimestamp()`.
    pub fn set_timestamp(&mut self, ts: i64) {
        php::date_timestamp_set(&mut self.t, ts);
    }

    /// The object's properties as `var_dump()` shows them (`date_object_to_hash`).
    pub fn props(&self) -> Vec<(K, V)> {
        let mut a = vec![(K::S("date".into()), V::Str(self.format("x-m-d H:i:s.u")))];
        if self.t.is_localtime {
            a.push((
                K::S("timezone_type".into()),
                V::Int(i64::from(self.t.zone_type.as_i32())),
            ));
            let name = match self.t.zone_type {
                ZoneType::Id => self.t.tz_name().unwrap_or("").to_string(),
                ZoneType::Offset => php_timelib::format::tz_offset_str(i64::from(self.t.z)),
                ZoneType::Abbr => self.t.tz_abbr.clone().unwrap_or_default(),
                ZoneType::None => String::new(),
            };
            a.push((K::S("timezone".into()), V::Str(name)));
        }
        a
    }

    /// `var_dump($dt)` for an object of class `class` with handle `id`.
    pub fn var_dump(&self, class: &str, id: u32) -> String {
        V::Obj(class.to_string(), id, self.props()).var_dump()
    }

    /// `DateTime::getTimezone()->getName()`.
    pub fn timezone_name(&self) -> String {
        PhpTimeZone::from_time(&self.t)
            .map(|z| z.name())
            .unwrap_or_default()
    }
}

/// `DateInterval::format($f)`.
pub fn interval_format(iv: &PhpInterval, f: &str) -> String {
    String::from_utf8(php_timelib::interval_format(f.as_bytes(), &iv.diff)).unwrap()
}

/// The properties of a `DateInterval` as `var_dump()` shows them.
pub fn interval_props(iv: &PhpInterval) -> Vec<(K, V)> {
    let d = &iv.diff;
    vec![
        (K::S("y".into()), V::Int(d.y)),
        (K::S("m".into()), V::Int(d.m)),
        (K::S("d".into()), V::Int(d.d)),
        (K::S("h".into()), V::Int(d.h)),
        (K::S("i".into()), V::Int(d.i)),
        (K::S("s".into()), V::Int(d.s)),
        (K::S("f".into()), V::Float(d.us as f64 / 1_000_000.0)),
        (K::S("invert".into()), V::Int(i64::from(d.invert))),
        (
            K::S("days".into()),
            if d.days != UNSET {
                V::Int(d.days)
            } else {
                V::Bool(false)
            },
        ),
        (K::S("from_string".into()), V::Bool(false)),
    ]
}

/// `var_dump($interval)`.
pub fn interval_var_dump(iv: &PhpInterval, id: u32) -> String {
    V::Obj("DateInterval".into(), id, interval_props(iv)).var_dump()
}

/// `$interval->days` as PHP prints it with var_dump (`int(N)` or `bool(false)`).
pub fn interval_days(iv: &RelTime) -> Option<i64> {
    (iv.days != UNSET).then_some(iv.days)
}

// ---------------------------------------------------------------------------------
// A tiny PHP value model for var_dump/print_r output of date_parse() & friends.

/// A PHP value.
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// An ordered array; keys are either ints or strings.
    Arr(Vec<(K, V)>),
    /// An object: class name, handle (`#N`) and properties.
    Obj(String, u32, Vec<(K, V)>),
}

/// An array key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum K {
    I(i64),
    S(String),
}

fn arr_set(a: &mut Vec<(K, V)>, k: K, v: V) {
    if let Some(e) = a.iter_mut().find(|(ek, _)| *ek == k) {
        e.1 = v;
    } else {
        a.push((k, v));
    }
}

fn php_float(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

impl V {
    /// `var_dump()` output.
    pub fn var_dump(&self) -> String {
        let mut out = String::new();
        self.dump_into(&mut out, 0);
        out
    }

    fn dump_into(&self, out: &mut String, indent: usize) {
        let pad = " ".repeat(indent);
        match self {
            V::Null => {
                let _ = writeln!(out, "{pad}NULL");
            }
            V::Bool(b) => {
                let _ = writeln!(out, "{pad}bool({b})");
            }
            V::Int(i) => {
                let _ = writeln!(out, "{pad}int({i})");
            }
            V::Float(f) => {
                let _ = writeln!(out, "{pad}float({})", php_float(*f));
            }
            V::Str(s) => {
                let _ = writeln!(out, "{pad}string({}) \"{s}\"", s.len());
            }
            V::Arr(_) | V::Obj(..) => {
                let a = match self {
                    V::Arr(a) => {
                        let _ = writeln!(out, "{pad}array({}) {{", a.len());
                        a
                    }
                    V::Obj(class, id, a) => {
                        let _ = writeln!(out, "{pad}object({class})#{id} ({}) {{", a.len());
                        a
                    }
                    _ => unreachable!(),
                };
                for (k, v) in a {
                    match k {
                        K::I(i) => {
                            let _ = writeln!(out, "{pad}  [{i}]=>");
                        }
                        K::S(s) => {
                            let _ = writeln!(out, "{pad}  [\"{s}\"]=>");
                        }
                    }
                    v.dump_into(out, indent + 2);
                }
                let _ = writeln!(out, "{pad}}}");
            }
        }
    }

    /// `print_r()` output.
    pub fn print_r(&self) -> String {
        let mut out = String::new();
        self.print_r_into(&mut out, 0);
        out
    }

    fn print_r_into(&self, out: &mut String, indent: usize) {
        match self {
            V::Bool(true) => out.push('1'),
            V::Bool(false) | V::Null => {}
            V::Int(i) => {
                let _ = write!(out, "{i}");
            }
            V::Float(f) => out.push_str(&php_float(*f)),
            V::Str(s) => out.push_str(s),
            V::Arr(_) | V::Obj(..) => {
                let pad = " ".repeat(indent);
                let a = match self {
                    V::Arr(a) => {
                        let _ = writeln!(out, "Array\n{pad}(");
                        a
                    }
                    V::Obj(class, _, a) => {
                        let _ = writeln!(out, "{class} Object\n{pad}(");
                        a
                    }
                    _ => unreachable!(),
                };
                for (k, v) in a {
                    match k {
                        K::I(i) => {
                            let _ = write!(out, "{pad}    [{i}] => ");
                        }
                        K::S(s) => {
                            let _ = write!(out, "{pad}    [{s}] => ");
                        }
                    }
                    v.print_r_into(out, indent + 8);
                    out.push('\n');
                }
                let _ = writeln!(out, "{pad})");
            }
        }
    }
}

fn errors_into(a: &mut Vec<(K, V)>, e: &ErrorContainer) {
    let mut w = Vec::new();
    for m in &e.warnings {
        arr_set(
            &mut w,
            K::I(m.position as i64),
            V::Str(m.message.to_string()),
        );
    }
    let mut er = Vec::new();
    for m in &e.errors {
        arr_set(
            &mut er,
            K::I(m.position as i64),
            V::Str(m.message.to_string()),
        );
    }
    a.push((
        K::S("warning_count".into()),
        V::Int(e.warnings.len() as i64),
    ));
    a.push((K::S("warnings".into()), V::Arr(w)));
    a.push((K::S("error_count".into()), V::Int(e.errors.len() as i64)));
    a.push((K::S("errors".into()), V::Arr(er)));
}

fn parsed_to_value(t: &Time, e: &ErrorContainer) -> V {
    let mut a = Vec::new();
    let el = |v: i64| {
        if v == UNSET {
            V::Bool(false)
        } else {
            V::Int(v)
        }
    };
    a.push((K::S("year".into()), el(t.y)));
    a.push((K::S("month".into()), el(t.m)));
    a.push((K::S("day".into()), el(t.d)));
    a.push((K::S("hour".into()), el(t.h)));
    a.push((K::S("minute".into()), el(t.i)));
    a.push((K::S("second".into()), el(t.s)));
    a.push((
        K::S("fraction".into()),
        if t.us == UNSET {
            V::Bool(false)
        } else {
            V::Float(t.us as f64 / 1_000_000.0)
        },
    ));
    errors_into(&mut a, e);
    a.push((K::S("is_localtime".into()), V::Bool(t.is_localtime)));
    if t.is_localtime {
        a.push((
            K::S("zone_type".into()),
            V::Int(i64::from(t.zone_type.as_i32())),
        ));
        match t.zone_type {
            ZoneType::Offset => {
                a.push((K::S("zone".into()), el(i64::from(t.z))));
                a.push((K::S("is_dst".into()), V::Bool(t.dst != 0)));
            }
            ZoneType::Id => {
                if let Some(ab) = &t.tz_abbr {
                    a.push((K::S("tz_abbr".into()), V::Str(ab.clone())));
                }
                if let Some(n) = t.tz_name() {
                    a.push((K::S("tz_id".into()), V::Str(n.to_string())));
                }
            }
            ZoneType::Abbr => {
                a.push((K::S("zone".into()), el(i64::from(t.z))));
                a.push((K::S("is_dst".into()), V::Bool(t.dst != 0)));
                a.push((
                    K::S("tz_abbr".into()),
                    V::Str(t.tz_abbr.clone().unwrap_or_default()),
                ));
            }
            ZoneType::None => {}
        }
    }
    if t.have_relative {
        let r = &t.relative;
        let mut rel = vec![
            (K::S("year".into()), V::Int(r.y)),
            (K::S("month".into()), V::Int(r.m)),
            (K::S("day".into()), V::Int(r.d)),
            (K::S("hour".into()), V::Int(r.h)),
            (K::S("minute".into()), V::Int(r.i)),
            (K::S("second".into()), V::Int(r.s)),
        ];
        if r.have_weekday_relative {
            rel.push((K::S("weekday".into()), V::Int(i64::from(r.weekday))));
        }
        if r.have_special_relative && r.special.kind == php_timelib::SPECIAL_WEEKDAY {
            rel.push((K::S("weekdays".into()), V::Int(r.special.amount)));
        }
        if r.first_last_day_of != 0 {
            let k = if r.first_last_day_of == SPECIAL_FIRST_DAY_OF_MONTH {
                "first_day_of_month"
            } else {
                "last_day_of_month"
            };
            rel.push((K::S(k.into()), V::Bool(true)));
        }
        a.push((K::S("relative".into()), V::Arr(rel)));
    }
    V::Arr(a)
}

/// `date_parse($s)` as a PHP value.
pub fn date_parse(s: &str) -> V {
    let r = php_timelib::strtotime(s, &builtin_tzinfo);
    parsed_to_value(&r.time, &r.errors)
}

/// `date_parse_from_format($f, $s)` as a PHP value.
pub fn date_parse_from_format(f: &str, s: &str) -> V {
    let r = php_timelib::parse_from_format(f, s, &builtin_tzinfo);
    parsed_to_value(&r.time, &r.errors)
}

/// `DateTime::getLastErrors()` after parsing `s` (strtotime grammar, or
/// createFromFormat when `format` is given); `None` when PHP returns `false` (no
/// warnings and no errors).
pub fn last_errors(format: Option<&str>, s: &str) -> Option<V> {
    let r = match format {
        Some(f) => php_timelib::parse_from_format(f, s, &builtin_tzinfo),
        None => php_timelib::strtotime(s, &builtin_tzinfo),
    };
    if r.errors.is_empty() {
        return None;
    }
    let mut a = Vec::new();
    errors_into(&mut a, &r.errors);
    Some(V::Arr(a))
}

/// Asserts equality of multi-line text with a readable diff.
#[track_caller]
pub fn assert_text(actual: &str, expected: &str) {
    let a = actual.trim_end();
    let e = expected.trim_end();
    if a != e {
        let mut msg = String::new();
        for (i, (la, le)) in a.lines().zip(e.lines()).enumerate() {
            if la != le {
                let _ = writeln!(
                    msg,
                    "first difference at line {}:\n  actual:   {la:?}\n  expected: {le:?}",
                    i + 1
                );
                break;
            }
        }
        panic!("text mismatch\n{msg}--- actual ---\n{a}\n--- expected ---\n{e}");
    }
}
