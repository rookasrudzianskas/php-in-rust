//! Sunrise/sunset and twilight calculations (`astro.c`, after Paul Schlyter's public
//! domain `sunriset.c`), plus the PHP-level wrappers used by `date_sunrise()`,
//! `date_sunset()` and `date_sun_info()`.

use std::sync::Arc;

use crate::time::{Time, ZoneType};
use crate::tz::TzInfo;

const PI: f64 = std::f64::consts::PI;
const RADEG: f64 = 180.0 / PI;
const DEGRAD: f64 = PI / 180.0;
const INV360: f64 = 1.0 / 360.0;

fn sind(x: f64) -> f64 {
    (x * DEGRAD).sin()
}
fn cosd(x: f64) -> f64 {
    (x * DEGRAD).cos()
}
fn atan2d(y: f64, x: f64) -> f64 {
    RADEG * y.atan2(x)
}
fn acosd(x: f64) -> f64 {
    RADEG * x.acos()
}

/// Reduce angle to within 0..360 degrees.
fn astro_revolution(x: f64) -> f64 {
    x - 360.0 * (x * INV360).floor()
}

/// Reduce angle to within -180..+180 degrees.
fn astro_rev180(x: f64) -> f64 {
    x - 360.0 * (x * INV360 + 0.5).floor()
}

/// Greenwich Mean Sidereal Time at 0h UT.
fn astro_gmst0(d: f64) -> f64 {
    astro_revolution((180.0 + 356.0470 + 282.9404) + (0.985_600_258_5 + 4.70935E-5) * d)
}

/// The Sun's ecliptic longitude and distance at `d` days since 2000 Jan 0.0.
fn astro_sunpos(d: f64) -> (f64, f64) {
    let m = astro_revolution(356.0470 + 0.985_600_258_5 * d);
    let w = 282.9404 + 4.70935E-5 * d;
    let e = 0.016709 - 1.151E-9 * d;
    let ee = m + e * RADEG * sind(m) * (1.0 + e * cosd(m));
    let x = cosd(ee) - e;
    let y = (1.0 - e * e).sqrt() * sind(ee);
    let r = (x * x + y * y).sqrt();
    let v = atan2d(y, x);
    let mut lon = v + w;
    if lon >= 360.0 {
        lon -= 360.0;
    }
    (lon, r)
}

/// Right ascension, declination and distance of the Sun.
fn astro_sun_ra_dec(d: f64) -> (f64, f64, f64) {
    let (lon, r) = astro_sunpos(d);
    let x = r * cosd(lon);
    let y0 = r * sind(lon);
    let obl_ecl = 23.4393 - 3.563E-7 * d;
    let z = y0 * sind(obl_ecl);
    let y = y0 * cosd(obl_ecl);
    let ra = atan2d(y, x);
    let dec = atan2d(z, (x * x + y * y).sqrt());
    (ra, dec, r)
}

/// Converts a Unix timestamp to a Julian day (`timelib_ts_to_julianday`).
pub fn ts_to_julianday(ts: i64) -> f64 {
    ts as f64 / 86400.0 + 2_440_587.5
}

/// Days since J2000.0 (`timelib_ts_to_j2000`).
pub fn ts_to_j2000(ts: i64) -> f64 {
    ts_to_julianday(ts) - 2_451_545.0
}

/// Result of [`astro_rise_set_altitude`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RiseSet {
    /// 0: the sun rises and sets; +1: always above the altitude; -1: always below.
    pub rc: i32,
    /// Rise time in decimal hours UTC (only meaningful when `rc == 0`).
    pub h_rise: f64,
    /// Set time in decimal hours UTC (only meaningful when `rc == 0`).
    pub h_set: f64,
    /// Rise timestamp.
    pub ts_rise: i64,
    /// Set timestamp.
    pub ts_set: i64,
    /// Transit (sun at south) timestamp.
    pub ts_transit: i64,
}

/// Computes rise/set times for the sun crossing `altit` degrees
/// (`timelib_astro_rise_set_altitude`). `upper_limb` selects the upper limb instead of
/// the centre. Like timelib, this normalises `t_loc` to 12:00 local time (its `sse` is
/// restored afterwards).
pub fn astro_rise_set_altitude(
    t_loc: &mut Time,
    lon: f64,
    lat: f64,
    altit: f64,
    upper_limb: bool,
) -> RiseSet {
    let mut res = RiseSet::default();
    let mut altit = altit;

    // Normalize time
    let old_sse = t_loc.sse;
    t_loc.h = 12;
    t_loc.i = 0;
    t_loc.s = 0;
    t_loc.update_ts(None);

    // Calculate TS belonging to UTC 00:00 of the current day
    let mut t_utc = Time::new();
    t_utc.y = t_loc.y;
    t_utc.m = t_loc.m;
    t_utc.d = t_loc.d;
    t_utc.update_ts(None);

    // Compute d of 12h local mean solar time
    let timestamp = t_utc.sse;
    let d = ts_to_j2000(timestamp) + 2.0 - lon / 360.0;

    // Compute local sidereal time of this moment
    let sidtime = astro_revolution(astro_gmst0(d) + 180.0 + lon);

    // Compute Sun's RA + Decl + distance at this moment
    let (s_ra, sdec, sr) = astro_sun_ra_dec(d);

    // Compute time when Sun is at south - in hours UT
    let tsouth = 12.0 - astro_rev180(sidtime - s_ra) / 15.0;

    // Compute the Sun's apparent radius, degrees
    let sradius = 0.2666 / sr;

    if upper_limb {
        altit -= sradius;
    }

    let cost = (sind(altit) - sind(lat) * sind(sdec)) / (cosd(lat) * cosd(sdec));
    res.ts_transit = (t_utc.sse as f64 + tsouth * 3600.0) as i64;
    if cost >= 1.0 {
        res.rc = -1;
        let v = (t_utc.sse as f64 + tsouth * 3600.0) as i64;
        res.ts_rise = v;
        res.ts_set = v;
    } else if cost <= -1.0 {
        res.rc = 1;
        res.ts_rise = t_loc.sse - 12 * 3600;
        res.ts_set = t_loc.sse + 12 * 3600;
    } else {
        let t = acosd(cost) / 15.0;
        res.ts_rise = ((tsouth - t) * 3600.0 + t_utc.sse as f64) as i64;
        res.ts_set = ((tsouth + t) * 3600.0 + t_utc.sse as f64) as i64;
        res.h_rise = tsouth - t;
        res.h_set = tsouth + t;
    }

    t_loc.sse = old_sse;
    res
}

/// Value of one `date_sun_info()` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunValue {
    /// A timestamp.
    Time(i64),
    /// `true`: the sun is always above the horizon (polar day); `false`: always below.
    Always(bool),
}

/// `date_sun_info()` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SunInfo {
    /// `sunrise`.
    pub sunrise: SunValue,
    /// `sunset`.
    pub sunset: SunValue,
    /// `transit`.
    pub transit: i64,
    /// `civil_twilight_begin`.
    pub civil_twilight_begin: SunValue,
    /// `civil_twilight_end`.
    pub civil_twilight_end: SunValue,
    /// `nautical_twilight_begin`.
    pub nautical_twilight_begin: SunValue,
    /// `nautical_twilight_end`.
    pub nautical_twilight_end: SunValue,
    /// `astronomical_twilight_begin`.
    pub astronomical_twilight_begin: SunValue,
    /// `astronomical_twilight_end`.
    pub astronomical_twilight_end: SunValue,
}

fn local_time(ts: i64, tz: &Arc<TzInfo>) -> Time {
    let mut t = Time {
        tz_info: Some(tz.clone()),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    t
}

/// `date_sun_info($timestamp, $latitude, $longitude)` in zone `tz`.
pub fn sun_info(ts: i64, latitude: f64, longitude: f64, tz: &Arc<TzInfo>) -> SunInfo {
    let mut t = local_time(ts, tz);
    let pair = |rs: RiseSet| match rs.rc {
        -1 => (SunValue::Always(false), SunValue::Always(false)),
        1 => (SunValue::Always(true), SunValue::Always(true)),
        _ => (SunValue::Time(rs.ts_rise), SunValue::Time(rs.ts_set)),
    };
    let rs = astro_rise_set_altitude(&mut t, longitude, latitude, -35.0 / 60.0, true);
    let (sunrise, sunset) = pair(rs);
    let transit = rs.ts_transit;
    let (civil_twilight_begin, civil_twilight_end) = pair(astro_rise_set_altitude(
        &mut t, longitude, latitude, -6.0, false,
    ));
    let (nautical_twilight_begin, nautical_twilight_end) = pair(astro_rise_set_altitude(
        &mut t, longitude, latitude, -12.0, false,
    ));
    let (astronomical_twilight_begin, astronomical_twilight_end) = pair(astro_rise_set_altitude(
        &mut t, longitude, latitude, -18.0, false,
    ));
    SunInfo {
        sunrise,
        sunset,
        transit,
        civil_twilight_begin,
        civil_twilight_end,
        nautical_twilight_begin,
        nautical_twilight_end,
        astronomical_twilight_begin,
        astronomical_twilight_end,
    }
}

/// Return format of [`sunrise_sunset`] (`SUNFUNCS_RET_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunFormat {
    /// `SUNFUNCS_RET_TIMESTAMP` (0).
    Timestamp,
    /// `SUNFUNCS_RET_STRING` (1): `"HH:MM"`.
    String,
    /// `SUNFUNCS_RET_DOUBLE` (2): decimal hours.
    Double,
}

/// Result of [`sunrise_sunset`].
#[derive(Clone, Debug, PartialEq)]
pub enum SunResult {
    /// A timestamp.
    Timestamp(i64),
    /// `"HH:MM"`.
    String(String),
    /// Decimal hours.
    Double(f64),
}

/// `date_sunrise()` / `date_sunset()` (`php_do_date_sunrise_sunset`). `zenith` is in
/// degrees; `gmt_offset` is in hours (`None` = the zone's offset at the epoch, which
/// is what PHP computes). Returns `None` where PHP returns `false`.
#[allow(clippy::too_many_arguments)]
pub fn sunrise_sunset(
    ts: i64,
    format: SunFormat,
    latitude: f64,
    longitude: f64,
    zenith: f64,
    gmt_offset: Option<f64>,
    tz: &Arc<TzInfo>,
    calc_sunset: bool,
) -> Option<SunResult> {
    let altitude = 90.0 - zenith;
    if !latitude.is_finite() || !longitude.is_finite() {
        return None;
    }
    let mut t = Time {
        tz_info: Some(tz.clone()),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    let gmt_offset = gmt_offset.unwrap_or_else(|| t.current_offset() as f64 / 3600.0);
    t.unixtime2local(ts);
    let rs = astro_rise_set_altitude(&mut t, longitude, latitude, altitude, true);
    if rs.rc != 0 {
        return None;
    }
    if format == SunFormat::Timestamp {
        return Some(SunResult::Timestamp(if calc_sunset {
            rs.ts_set
        } else {
            rs.ts_rise
        }));
    }
    let mut n = if calc_sunset { rs.h_set } else { rs.h_rise } + gmt_offset;
    if !(0.0..=24.0).contains(&n) {
        n -= (n / 24.0).floor() * 24.0;
    }
    if !(0.0..=24.0).contains(&n) {
        return None;
    }
    Some(match format {
        SunFormat::String => {
            let h = n as i32;
            SunResult::String(format!(
                "{:02}:{:02}",
                h,
                (60.0 * (n - f64::from(h))) as i32
            ))
        }
        _ => SunResult::Double(n),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::builtin_tzinfo;

    #[test]
    fn sun_info_utc() {
        // From ext/date/tests/date_sun_info_001.phpt: strtotime("2006-12-12") in UTC
        let tz = builtin_tzinfo("UTC").unwrap();
        let si = sun_info(1_165_881_600, 31.7667, 35.2333, &tz);
        assert_eq!(si.sunrise, SunValue::Time(1_165_897_761));
        assert_eq!(si.sunset, SunValue::Time(1_165_934_160));
        assert_eq!(si.transit, 1_165_915_961);
        assert_eq!(si.civil_twilight_begin, SunValue::Time(1_165_896_156));
        assert_eq!(si.civil_twilight_end, SunValue::Time(1_165_935_765));
        assert_eq!(si.nautical_twilight_begin, SunValue::Time(1_165_894_334));
        assert_eq!(si.nautical_twilight_end, SunValue::Time(1_165_937_588));
        assert_eq!(
            si.astronomical_twilight_begin,
            SunValue::Time(1_165_892_551)
        );
        assert_eq!(si.astronomical_twilight_end, SunValue::Time(1_165_939_371));
    }
}
