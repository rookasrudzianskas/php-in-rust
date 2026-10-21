//! Timezone information (`parse_tz.c`): TZif parsing, offset lookup, POSIX tails.

pub mod abbr;
mod abbr_data;
pub mod db;
pub mod posix;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::tz_error;
use crate::time::{Time, TimeOffset};
use posix::{PosixStr, PosixTransitions, parse_posix_str, transitions_for_year};

pub use db::{TzDb, TzIndexEntry, builtin_db};

/// A local time type (`ttinfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TtInfo {
    /// UTC offset in seconds.
    pub offset: i32,
    /// DST flag.
    pub isdst: bool,
    /// Index into [`TzInfo::timezone_abbr`].
    pub abbr_idx: usize,
    /// Standard/wall indicator (unused).
    pub isstdcnt: u32,
    /// UT/local indicator (unused).
    pub isgmtcnt: u32,
}

/// A leap second record (`tlinfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TlInfo {
    /// Transition time.
    pub trans: i64,
    /// Correction.
    pub offset: i32,
}

/// Location information (`tlocinfo`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Location {
    /// ISO 3166 country code, `"??"` if unknown.
    pub country_code: String,
    /// Latitude in degrees.
    pub latitude: f64,
    /// Longitude in degrees.
    pub longitude: f64,
    /// Comments from zone.tab.
    pub comments: String,
}

/// TZif header counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TzCounts {
    /// UT/local indicator count.
    pub ttisgmtcnt: u64,
    /// Standard/wall indicator count.
    pub ttisstdcnt: u64,
    /// Leap second count.
    pub leapcnt: u64,
    /// Transition count.
    pub timecnt: u64,
    /// Local time type count.
    pub typecnt: u64,
    /// Abbreviation characters count.
    pub charcnt: u64,
}

/// A parsed timezone (`timelib_tzinfo`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TzInfo {
    /// The identifier as requested (PHP keeps the caller's spelling).
    pub name: String,
    /// 32-bit header counts.
    pub bit32: TzCounts,
    /// 64-bit header counts.
    pub bit64: TzCounts,
    /// Transition times.
    pub trans: Vec<i64>,
    /// Type index of each transition.
    pub trans_idx: Vec<u8>,
    /// Local time types (including types added for the POSIX string).
    pub types: Vec<TtInfo>,
    /// NUL separated abbreviation characters.
    pub timezone_abbr: Vec<u8>,
    /// Leap seconds.
    pub leap_times: Vec<TlInfo>,
    /// The "BC" flag from PHP's preamble.
    pub bc: bool,
    /// Location information.
    pub location: Location,
    /// The POSIX TZ string footer, if any.
    pub posix_string: Option<String>,
    /// Parsed POSIX string.
    pub posix_info: Option<PosixStr>,
}

/// Error from [`TzInfo::parse_builtin`]; contains a `TIMELIB_ERROR_*` code
/// (see [`crate::error::get_error_message`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TzError(pub i32);

impl std::fmt::Display for TzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(crate::error::get_error_message(self.0))
    }
}

impl std::error::Error for TzError {}

/// Offset lookup result (`timelib_get_time_zone_offset_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffsetInfo {
    /// UTC offset in seconds.
    pub offset: i32,
    /// Start of the period (`i64::MIN` when unbounded).
    pub transition_time: i64,
    /// DST flag.
    pub is_dst: bool,
}

struct Cursor<'a> {
    d: &'a [u8],
    p: usize,
}

impl Cursor<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], TzError> {
        let end = self
            .p
            .checked_add(n)
            .filter(|&e| e <= self.d.len())
            .ok_or(TzError(tz_error::CANNOT_ALLOCATE))?;
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), TzError> {
        self.bytes(n).map(|_| ())
    }
    fn u32(&mut self) -> Result<u32, TzError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64, TzError> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_be_bytes(a))
    }
    fn counts(&mut self) -> Result<TzCounts, TzError> {
        Ok(TzCounts {
            ttisgmtcnt: u64::from(self.u32()?),
            ttisstdcnt: u64::from(self.u32()?),
            leapcnt: u64::from(self.u32()?),
            timecnt: u64::from(self.u32()?),
            typecnt: u64::from(self.u32()?),
            charcnt: u64::from(self.u32()?),
        })
    }
}

fn usize_of(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

impl TzInfo {
    /// Loads a zone from the bundled database (`timelib_parse_tzfile`). The lookup is
    /// case-insensitive; the resulting `name` keeps the caller's spelling, like PHP.
    pub fn parse_builtin(timezone: &str) -> Result<TzInfo, TzError> {
        Self::parse_from_db(timezone, builtin_db())
    }

    /// Loads a zone from a database (`timelib_parse_tzfile`).
    pub fn parse_from_db(timezone: &str, db: &TzDb) -> Result<TzInfo, TzError> {
        let entry = db
            .find(timezone)
            .ok_or(TzError(tz_error::NO_SUCH_TIMEZONE))?;
        let mut tz = Self::parse_data(timezone, db.entry_data(entry))?;
        tz.name = timezone.to_string();
        Ok(tz)
    }

    /// Parses a PHP (`PHP2`) or plain TZif (`TZif2+`) blob.
    pub fn parse_data(name: &str, data: &[u8]) -> Result<TzInfo, TzError> {
        let mut c = Cursor { d: data, p: 0 };
        let mut tz = TzInfo {
            name: name.to_string(),
            ..Default::default()
        };

        // Preamble
        let is_php;
        let version;
        if data.starts_with(b"PHP") {
            is_php = true;
            let pre = c.bytes(20)?;
            version = i32::from(pre[3]) - i32::from(b'0');
            tz.bc = pre[4] == 1;
            tz.location.country_code = String::from_utf8_lossy(&pre[5..7]).into_owned();
        } else if data.starts_with(b"TZif") {
            is_php = false;
            let pre = c.bytes(20)?;
            version = match pre[4] {
                0 => 0,
                b'2' => 2,
                b'3' => 3,
                b'4' => 4,
                _ => return Err(TzError(tz_error::UNSUPPORTED_VERSION)),
            };
            tz.bc = false;
            tz.location.country_code = "??".to_string();
        } else {
            return Err(TzError(tz_error::UNSUPPORTED_VERSION));
        }
        if !(2..=4).contains(&version) {
            return Err(TzError(tz_error::UNSUPPORTED_VERSION));
        }

        // 32-bit section (skipped)
        tz.bit32 = c.counts()?;
        let b32 = tz.bit32;
        c.skip(usize_of(b32.timecnt) * 5)?;
        c.skip(usize_of(b32.typecnt) * 6)?;
        c.skip(usize_of(b32.charcnt))?;
        c.skip(usize_of(b32.leapcnt) * 8)?;
        c.skip(usize_of(b32.ttisstdcnt))?;
        c.skip(usize_of(b32.ttisgmtcnt))?;

        // 64-bit preamble
        let pre64 = c
            .bytes(5)
            .map_err(|_| TzError(tz_error::CORRUPT_NO_64BIT_PREAMBLE))?;
        if !matches!(pre64, b"TZif2" | b"TZif3" | b"TZif4") {
            return Err(TzError(tz_error::CORRUPT_NO_64BIT_PREAMBLE));
        }
        c.skip(15)?;
        tz.bit64 = c.counts()?;
        let b64 = tz.bit64;

        // Transitions
        let timecnt = usize_of(b64.timecnt);
        tz.trans = Vec::with_capacity(timecnt);
        for i in 0..timecnt {
            let t = c.i64()?;
            if i > 0 && t <= tz.trans[i - 1] {
                return Err(TzError(tz_error::CORRUPT_TRANSITIONS_DONT_INCREASE));
            }
            tz.trans.push(t);
        }
        tz.trans_idx = c.bytes(timecnt)?.to_vec();

        // Types
        let typecnt = usize_of(b64.typecnt);
        let raw = c.bytes(typecnt * 6)?;
        tz.types = raw
            .as_chunks::<6>()
            .0
            .iter()
            .map(|b| TtInfo {
                offset: i32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                isdst: b[4] != 0,
                abbr_idx: usize::from(b[5]),
                isstdcnt: 0,
                isgmtcnt: 0,
            })
            .collect();
        tz.timezone_abbr = c
            .bytes(usize_of(b64.charcnt))
            .map_err(|_| TzError(tz_error::CORRUPT_NO_ABBREVIATION))?
            .to_vec();
        for _ in 0..usize_of(b64.leapcnt) {
            let trans = c.i64()?;
            let offset = c.u32()? as i32;
            tz.leap_times.push(TlInfo { trans, offset });
        }
        let std = c.bytes(usize_of(b64.ttisstdcnt))?.to_vec();
        for (i, v) in std.iter().enumerate() {
            if let Some(t) = tz.types.get_mut(i) {
                t.isstdcnt = u32::from(*v);
            }
        }
        let gmt = c.bytes(usize_of(b64.ttisgmtcnt))?.to_vec();
        for (i, v) in gmt.iter().enumerate() {
            if let Some(t) = tz.types.get_mut(i) {
                t.isgmtcnt = u32::from(*v);
            }
        }

        // POSIX string, delimited by '\n'
        c.skip(1)?;
        let begin = c.p;
        while c.p < data.len() && data[c.p] != b'\n' {
            c.p += 1;
        }
        let posix = String::from_utf8_lossy(&data[begin..c.p]).into_owned();
        c.p = (c.p + 1).min(data.len());
        let empty_posix = posix.is_empty();
        tz.posix_string = Some(posix);
        if !empty_posix && !tz.integrate_posix_string() {
            return Err(TzError(tz_error::CORRUPT_POSIX_STRING));
        }

        if is_php {
            let lat = c.u32()?;
            let lon = c.u32()?;
            let len = c.u32()? as usize;
            tz.location.latitude = f64::from(lat) / 100_000.0 - 90.0;
            tz.location.longitude = f64::from(lon) / 100_000.0 - 180.0;
            tz.location.comments = String::from_utf8_lossy(c.bytes(len)?).into_owned();
        } else {
            tz.location.latitude = 0.0;
            tz.location.longitude = 0.0;
            tz.location.comments = "?".to_string();
        }
        Ok(tz)
    }

    /// Abbreviation string starting at `idx` in the abbreviation table.
    pub fn abbr_at(&self, idx: usize) -> &str {
        let tail = self.timezone_abbr.get(idx..).unwrap_or(&[]);
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        std::str::from_utf8(&tail[..end]).unwrap_or("")
    }

    fn find_ttinfo_index(&self, offset: i64, isdst: bool, abbr: &str) -> Option<usize> {
        self.types.iter().position(|t| {
            i64::from(t.offset) == offset && t.isdst == isdst && self.abbr_at(t.abbr_idx) == abbr
        })
    }

    fn add_new_ttinfo_index(&mut self, offset: i64, isdst: bool, abbr: &str) -> usize {
        let abbr_idx = self.timezone_abbr.len();
        self.timezone_abbr.extend_from_slice(abbr.as_bytes());
        self.timezone_abbr.push(0);
        self.bit64.charcnt = self.timezone_abbr.len() as u64;
        self.types.push(TtInfo {
            offset: offset as i32,
            isdst,
            abbr_idx,
            isstdcnt: 0,
            isgmtcnt: 0,
        });
        self.bit64.typecnt = self.types.len() as u64;
        self.types.len() - 1
    }

    fn integrate_posix_string(&mut self) -> bool {
        let Some(mut ps) = self.posix_string.as_deref().and_then(parse_posix_str) else {
            return false;
        };
        match self.find_ttinfo_index(ps.std_offset, false, &ps.std) {
            Some(i) => ps.type_index_std_type = i,
            None => {
                // Faithful to timelib: after adding a new standard type the DST type is
                // not looked up (it stays at index 0).
                ps.type_index_std_type = self.add_new_ttinfo_index(ps.std_offset, false, &ps.std);
                self.posix_info = Some(ps);
                return true;
            }
        }
        if let Some(dst) = ps.dst.clone() {
            ps.type_index_dst_type = match self.find_ttinfo_index(ps.dst_offset, true, &dst) {
                Some(i) => i,
                None => self.add_new_ttinfo_index(ps.dst_offset, true, &dst),
            };
        }
        self.posix_info = Some(ps);
        true
    }

    /// The POSIX-rule based type for `ts` (`timelib_fetch_posix_timezone_offset`).
    fn fetch_posix_offset(&self, ts: i64) -> Option<(&TtInfo, Option<i64>)> {
        let ps = self.posix_info.as_ref()?;
        if ps.dst_end.is_none() {
            let tt = self.types.get(ps.type_index_std_type)?;
            return Some((tt, self.trans.last().copied()));
        }
        let mut dummy = Time::new();
        dummy.unixtime2gmt(ts);
        let year = dummy.y;
        let mut tr = PosixTransitions::default();
        transitions_for_year(ps, year - 1, &mut tr);
        transitions_for_year(ps, year, &mut tr);
        transitions_for_year(ps, year + 1, &mut tr);
        for i in 1..tr.count {
            if ts < tr.times[i] {
                return Some((self.types.get(tr.types[i - 1])?, Some(tr.times[i - 1])));
            }
        }
        None
    }

    /// The local time type in effect at `ts` and the start of its period
    /// (`timelib_fetch_timezone_offset`; RFC 8536 section 3.2).
    pub fn fetch_offset(&self, ts: i64) -> Option<(&TtInfo, i64)> {
        if self.trans.is_empty() {
            if self.posix_info.is_some() {
                return self.fetch_posix_offset(ts).map(|(t, _)| (t, i64::MIN));
            }
            if self.types.len() == 1 {
                return Some((&self.types[0], i64::MIN));
            }
            return None;
        }
        if ts < self.trans[0] {
            return self.types.first().map(|t| (t, i64::MIN));
        }
        let last = self.trans.len() - 1;
        if ts >= self.trans[last] {
            if self.posix_info.is_some() {
                // A failed POSIX lookup leaves the transition time untouched in C, which
                // never matters because no type is returned.
                return self
                    .fetch_posix_offset(ts)
                    .map(|(t, tt)| (t, tt.unwrap_or(self.trans[last])));
            }
            let idx = usize::from(self.trans_idx[last]);
            return self.types.get(idx).map(|t| (t, self.trans[last]));
        }
        let (mut left, mut right) = (0usize, last);
        while right - left > 1 {
            let mid = (left + right) >> 1;
            if ts < self.trans[mid] {
                right = mid;
            } else {
                left = mid;
            }
        }
        let idx = usize::from(self.trans_idx[left]);
        self.types.get(idx).map(|t| (t, self.trans[left]))
    }

    /// Whether `ts` is in DST; `None` if no information (`timelib_timestamp_is_in_dst`).
    pub fn timestamp_is_in_dst(&self, ts: i64) -> Option<bool> {
        self.fetch_offset(ts).map(|(t, _)| t.isdst)
    }

    /// Offset, DST flag, abbreviation and transition time at `ts`
    /// (`timelib_get_time_zone_info`).
    pub fn get_time_zone_info(&self, ts: i64) -> TimeOffset {
        let (offset, abbr, is_dst, transition_time) = match self.fetch_offset(ts) {
            Some((t, tt)) => (t.offset, self.abbr_at(t.abbr_idx).to_string(), t.isdst, tt),
            None => (0, self.abbr_at(0).to_string(), false, 0),
        };
        let mut leap_secs = 0;
        if !self.leap_times.is_empty() {
            for i in (1..self.leap_times.len()).rev() {
                if ts > self.leap_times[i].trans {
                    leap_secs = (-self.leap_times[i].offset) as u32;
                    break;
                }
            }
        }
        TimeOffset {
            offset,
            leap_secs,
            is_dst,
            abbr,
            transition_time,
        }
    }

    /// Offset, transition time and DST flag at `ts` (`timelib_get_time_zone_offset_info`).
    pub fn offset_info(&self, ts: i64) -> Option<OffsetInfo> {
        self.fetch_offset(ts).map(|(t, tt)| OffsetInfo {
            offset: t.offset,
            transition_time: tt,
            is_dst: t.isdst,
        })
    }

    /// POSIX-rule transitions for a year (`timelib_get_transitions_for_year`); empty if
    /// the zone has no DST rule.
    pub fn transitions_for_year(&self, year: i64) -> PosixTransitions {
        let mut tr = PosixTransitions::default();
        if let Some(ps) = &self.posix_info {
            transitions_for_year(ps, year, &mut tr);
        }
        tr
    }

    /// Whether the zone data is a "slim" TZif file.
    pub fn is_slim(&self) -> bool {
        self.bit32
            == (TzCounts {
                ttisgmtcnt: 0,
                ttisstdcnt: 0,
                leapcnt: 0,
                timecnt: 0,
                typecnt: 1,
                charcnt: 1,
            })
    }
}

/// Fetches offset info for an optional zone, returning `None` for a missing zone, like
/// `timelib_get_time_zone_offset_info` with a NULL zone.
pub(crate) fn offset_info(tz: Option<&TzInfo>, ts: i64) -> Option<OffsetInfo> {
    tz.and_then(|tz| tz.offset_info(ts))
}

thread_local! {
    static TZ_CACHE: RefCell<HashMap<String, Arc<TzInfo>>> = RefCell::new(HashMap::new());
}

/// Loads a zone from the bundled database, caching per exact spelling (like PHP's
/// per-request `DATEG(tzcache)`). This is the default tz lookup used by the parsers.
pub fn builtin_tzinfo(name: &str) -> Option<Arc<TzInfo>> {
    if let Some(tz) = TZ_CACHE.with(|c| c.borrow().get(name).cloned()) {
        return Some(tz);
    }
    let tz = Arc::new(TzInfo::parse_builtin(name).ok()?);
    TZ_CACHE.with(|c| c.borrow_mut().insert(name.to_string(), tz.clone()));
    Some(tz)
}

/// Whether `id` is a valid identifier in the bundled database.
pub fn timezone_id_is_valid(id: &str) -> bool {
    builtin_db().is_valid(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amsterdam_offsets() {
        let tz = TzInfo::parse_builtin("Europe/Amsterdam").unwrap();
        // 2005-07-01 12:00 UTC: CEST
        let o = tz.get_time_zone_info(1_120_219_200);
        assert_eq!(o.offset, 7200);
        assert!(o.is_dst);
        assert_eq!(o.abbr, "CEST");
        // 2005-01-01: CET
        let o = tz.get_time_zone_info(1_104_580_800);
        assert_eq!(o.offset, 3600);
        assert!(!o.is_dst);
        assert_eq!(o.abbr, "CET");
        // Far future relies on the POSIX string: 2100-07-01
        let o = tz.get_time_zone_info(4_118_054_400);
        assert_eq!(o.abbr, "CEST");
    }

    #[test]
    fn utc_and_errors() {
        let tz = TzInfo::parse_builtin("UTC").unwrap();
        assert_eq!(tz.get_time_zone_info(0).abbr, "UTC");
        assert_eq!(
            TzInfo::parse_builtin("Nowhere/Land").unwrap_err(),
            TzError(tz_error::NO_SUCH_TIMEZONE)
        );
        let tz = TzInfo::parse_builtin("america/new_york").unwrap();
        assert_eq!(tz.name, "america/new_york");
        assert_eq!(tz.location.country_code, "US");
        assert!((tz.location.latitude - 40.71416).abs() < 1e-6);
    }

    #[test]
    fn all_zones_parse() {
        let db = builtin_db();
        for e in db.identifiers() {
            let tz = TzInfo::parse_builtin(&e.id).unwrap_or_else(|err| panic!("{}: {err}", e.id));
            let _ = tz.get_time_zone_info(0);
            let _ = tz.get_time_zone_info(2_000_000_000);
        }
    }
}

#[cfg(test)]
mod extreme_tests {
    use super::*;

    #[test]
    fn extreme_timestamps_do_not_panic() {
        let db = builtin_db();
        for e in db.identifiers() {
            let tz = TzInfo::parse_builtin(&e.id).unwrap();
            for ts in [
                i64::MIN,
                i64::MIN / 2,
                -1 << 40,
                0,
                1 << 40,
                i64::MAX / 2,
                i64::MAX,
            ] {
                let _ = tz.get_time_zone_info(ts);
            }
        }
    }
}
