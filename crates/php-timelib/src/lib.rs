//! Port of [timelib](https://github.com/derickr/timelib) (PHP's `ext/date/lib`):
//! date/time parsing, formatting, timezones, intervals and astronomical helpers.
//!
//! The PHP-facing `date()`, `strtotime()`, `DateTime`, `DateTimeZone`,
//! `DateInterval` and `DatePeriod` are built on top of this crate. Behaviour follows
//! timelib 2026.02 as bundled with php-src, including its quirks:
//!
//! * [`strtotime`] is a faithful port of the re2c scanner in `parse_date.re`
//!   (longest-match, first-rule-wins), producing a [`Time`] plus an
//!   [`ErrorContainer`] with positions identical to PHP's `date_parse()`.
//! * [`parse_from_format`] implements `DateTime::createFromFormat()`.
//! * [`strtointerval`] parses ISO 8601 intervals (`R5/2008-03-01T13:00:00Z/P1D`).
//! * [`Time::update_ts`] / [`Time::update_from_sse`] convert between broken-down
//!   time and Unix timestamps, applying relative parts exactly like `tm2unixtime.c`.
//! * [`diff`], [`add`], [`sub`], [`add_wall`], [`sub_wall`] implement
//!   `DateTime::diff/add/sub`.
//! * The timezone database is PHP's bundled one (see [`builtin_db`]), stored in
//!   `data/tzdb.bin` (regenerate with `examples/gen_tzdb.rs`).
//!
//! ```
//! use php_timelib::{builtin_tzinfo, php_strtotime, format_timestamp};
//!
//! let tz = builtin_tzinfo("Europe/Amsterdam").unwrap();
//! let ts = php_strtotime("2005-07-14 22:30:41", 0, &tz).unwrap();
//! assert_eq!(ts, 1121373041);
//! let s = format_timestamp(b"D, d M Y H:i:s T", ts, Some(&tz));
//! assert_eq!(s, b"Thu, 14 Jul 2005 22:30:41 CEST");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod astro;
pub mod dow;
pub mod error;
pub mod format;
pub mod interval;
pub mod parse;
pub mod php;
pub mod time;
mod tm2unixtime;
pub mod tz;
mod unixtime2tm;

pub use astro::{
    RiseSet, SunFormat, SunInfo, SunResult, SunValue, astro_rise_set_altitude, sun_info,
    sunrise_sunset, ts_to_j2000, ts_to_julianday,
};
pub use dow::{
    date_from_isodate, day_of_week, day_of_year, daynr_from_weeknr, days_in_month, is_leap,
    iso_day_of_week, isodate_from_date, isoweek_from_date, valid_date, valid_time,
};
pub use error::{ErrorContainer, ErrorMessage, get_error_message};
pub use format::{date_format, format_timestamp, idate, interval_format};
pub use interval::{add, add_wall, diff, diff_days, sub, sub_wall};
pub use parse::TzGetter;
pub use parse::from_format::{
    DEFAULT_FORMAT_CONFIG, FormatConfig, FormatSpecifier, parse_from_format,
    parse_from_format_with_map,
};
pub use parse::iso_interval::{IsoInterval, strtointerval};
pub use parse::strtotime::{ParseResult, strtotime};
pub use php::{php_mktime, php_strtotime};
pub use time::{
    AbbrInfo, NO_CLONE, OVERRIDE_TIME, RelTime, SPECIAL_DAY_OF_WEEK_IN_MONTH,
    SPECIAL_FIRST_DAY_OF_MONTH, SPECIAL_LAST_DAY_OF_MONTH, SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH,
    SPECIAL_WEEKDAY, SpecialRelative, Time, TimeOffset, UNSET, ZoneType, decimal_hour_to_hms,
    hms_to_decimal_hour, hms_to_seconds, hmsf_to_decimal_hour,
};
pub use tm2unixtime::do_rel_normalize;
pub use tz::abbr::{TzLookupEntry, timezone_abbreviations_list, timezone_id_from_abbr};
pub use tz::{
    Location, OffsetInfo, TzDb, TzError, TzIndexEntry, TzInfo, builtin_db, builtin_tzinfo,
    timezone_id_is_valid,
};
pub use unixtime2tm::{date_from_epoch_days, unixtime2date};

/// timelib version (`TIMELIB_VERSION`).
pub const TIMELIB_VERSION: u32 = 202602;
/// timelib version string (`TIMELIB_ASCII_VERSION`).
pub const TIMELIB_ASCII_VERSION: &str = "2026.02";
