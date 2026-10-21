//! Unix timestamp to broken-down time (`unixtime2tm.c`).

use std::sync::Arc;

use crate::time::{
    AbbrInfo, DAYS_PER_ERA, HINNANT_EPOCH_SHIFT, SECS_PER_DAY, Time, YEARS_PER_ERA, ZoneType,
};
use crate::tz::{TzInfo, offset_info};

/// Converts days since 1970-01-01 to `(y, m, d)` (`timelib_date_from_epoch_days`;
/// Howard Hinnant's `civil_from_days`).
pub fn date_from_epoch_days(epoch_days: i64) -> (i64, i64, i64) {
    let days = epoch_days.wrapping_add(HINNANT_EPOCH_SHIFT);
    let era = if days >= 0 {
        days
    } else {
        days.wrapping_sub(DAYS_PER_ERA - 1)
    } / DAYS_PER_ERA;
    let day_of_era = days.wrapping_sub(era.wrapping_mul(DAYS_PER_ERA)) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let mut y = (year_of_era as i64).wrapping_add(era.wrapping_mul(YEARS_PER_ERA));
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_portion = (5 * day_of_year + 2) / 153;
    let d = (day_of_year - (153 * month_portion + 2) / 5 + 1) as i64;
    let m = if month_portion < 10 {
        month_portion as i64 + 3
    } else {
        month_portion as i64 - 9
    };
    if m <= 2 {
        y = y.wrapping_add(1);
    }
    (y, m, d)
}

/// Converts a Unix timestamp to a UTC date `(y, m, d)` (`timelib_unixtime2date`).
pub fn unixtime2date(ts: i64) -> (i64, i64, i64) {
    let mut epoch_days = ts / SECS_PER_DAY;
    if ts % SECS_PER_DAY < 0 {
        epoch_days -= 1;
    }
    date_from_epoch_days(epoch_days)
}

impl Time {
    /// Sets the fields to the UTC representation of `ts` (`timelib_unixtime2gmt`).
    pub fn unixtime2gmt(&mut self, ts: i64) {
        let (y, m, d) = unixtime2date(ts);
        self.y = y;
        self.m = m;
        self.d = d;
        let mut remainder = ts % SECS_PER_DAY;
        if remainder < 0 {
            remainder += SECS_PER_DAY;
        }
        let hours = remainder / 3600;
        let minutes = (remainder - hours * 3600) / 60;
        let seconds = remainder % 60;
        self.h = hours;
        self.i = minutes;
        self.s = seconds;
        self.z = 0;
        self.dst = 0;
        self.sse = ts;
        self.sse_uptodate = true;
        self.tim_uptodate = true;
        self.is_localtime = false;
    }

    /// Recomputes the broken-down fields from `sse`, keeping the zone
    /// (`timelib_update_from_sse`).
    pub fn update_from_sse(&mut self) {
        let z = self.z;
        let dst = self.dst;
        let sse = self.sse;
        match self.zone_type {
            ZoneType::Abbr | ZoneType::Offset => {
                let local = self
                    .sse
                    .wrapping_add(i64::from(self.z))
                    .wrapping_add(i64::from(self.dst) * 3600);
                self.unixtime2gmt(local);
            }
            ZoneType::Id => {
                let offset = offset_info(self.tz_info.as_deref(), self.sse).map_or(0, |o| o.offset);
                self.unixtime2gmt(self.sse.wrapping_add(i64::from(offset)));
            }
            ZoneType::None => self.unixtime2gmt(self.sse),
        }
        self.sse = sse;
        self.is_localtime = true;
        self.have_zone = 1;
        self.z = z;
        self.dst = dst;
    }

    /// Sets the fields to the local representation of `ts` in the time's zone
    /// (`timelib_unixtime2local`).
    pub fn unixtime2local(&mut self, ts: i64) {
        match self.zone_type {
            ZoneType::Abbr | ZoneType::Offset => {
                let z = self.z;
                let dst = self.dst;
                self.unixtime2gmt(
                    ts.wrapping_add(i64::from(self.z))
                        .wrapping_add(i64::from(self.dst) * 3600),
                );
                self.sse = ts;
                self.z = z;
                self.dst = dst;
            }
            ZoneType::Id => {
                let tz = self.tz_info.clone();
                let off = tz
                    .as_deref()
                    .map(|tz| tz.get_time_zone_info(ts))
                    .unwrap_or_default();
                self.unixtime2gmt(ts.wrapping_add(i64::from(off.offset)));
                self.sse = ts;
                self.dst = i32::from(off.is_dst);
                self.z = off.offset;
                self.tz_info = tz;
                self.tz_abbr_update(&off.abbr);
            }
            ZoneType::None => {
                self.is_localtime = false;
                self.have_zone = 0;
                return;
            }
        }
        self.is_localtime = true;
        self.have_zone = 1;
    }

    /// Switches to a plain UTC offset zone (`timelib_set_timezone_from_offset`).
    pub fn set_timezone_from_offset(&mut self, utc_offset: i64) {
        self.tz_abbr = None;
        self.z = utc_offset as i32;
        self.have_zone = 1;
        self.zone_type = ZoneType::Offset;
        self.dst = 0;
        self.tz_info = None;
    }

    /// Switches to an abbreviation zone (`timelib_set_timezone_from_abbr`).
    pub fn set_timezone_from_abbr(&mut self, abbr_info: &AbbrInfo) {
        self.tz_abbr = Some(abbr_info.abbr.clone());
        self.z = abbr_info.utc_offset as i32;
        self.have_zone = 1;
        self.zone_type = ZoneType::Abbr;
        self.dst = abbr_info.dst;
        self.tz_info = None;
    }

    /// Switches to an identifier zone, recomputing offset/DST/abbreviation for the
    /// current `sse` (`timelib_set_timezone`).
    pub fn set_timezone(&mut self, tz: &Arc<TzInfo>) {
        let off = tz.get_time_zone_info(self.sse);
        self.z = off.offset;
        self.dst = i32::from(off.is_dst);
        self.tz_info = Some(tz.clone());
        self.tz_abbr = Some(off.abbr);
        self.have_zone = 1;
        self.zone_type = ZoneType::Id;
    }

    /// Converts to local time (`localtime == true`, requires a zone) or to UTC
    /// (`timelib_apply_localtime`). Returns `false` if no zone is available.
    pub fn apply_localtime(&mut self, localtime: bool) -> bool {
        if localtime {
            if self.tz_info.is_none() {
                return false;
            }
            self.unixtime2local(self.sse);
        } else {
            self.unixtime2gmt(self.sse);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days() {
        assert_eq!(date_from_epoch_days(0), (1970, 1, 1));
        assert_eq!(date_from_epoch_days(-1), (1969, 12, 31));
        assert_eq!(date_from_epoch_days(11016), (2000, 2, 29));
        assert_eq!(unixtime2date(-1), (1969, 12, 31));
    }

    #[test]
    fn gmt() {
        let mut t = Time::new();
        t.unixtime2gmt(1_234_567_890);
        assert_eq!((t.y, t.m, t.d, t.h, t.i, t.s), (2009, 2, 13, 23, 31, 30));
        t.unixtime2gmt(-1);
        assert_eq!((t.y, t.m, t.d, t.h, t.i, t.s), (1969, 12, 31, 23, 59, 59));
    }
}
