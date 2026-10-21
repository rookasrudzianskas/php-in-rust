//! Timezone abbreviation table (`timezonemap.h`, `fallbackmap.h`) and lookups.

use super::abbr_data::{FALLBACK_MAP, TIMEZONE_MAP};
use super::db::strcasecmp;

/// One abbreviation entry (`timelib_tz_lookup_table`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TzLookupEntry {
    /// Lower-case abbreviation, e.g. `"cest"`.
    pub name: &'static str,
    /// Whether this is a DST abbreviation.
    pub dst: bool,
    /// UTC offset in seconds (including DST).
    pub gmtoffset: i32,
    /// An identifier using this abbreviation; `None` for the military single-letter
    /// zones (PHP reports `timezone_id => NULL` for those).
    pub full_tz_name: Option<&'static str>,
}

static UTC_ENTRY: TzLookupEntry = TzLookupEntry {
    name: "utc",
    dst: false,
    gmtoffset: 0,
    full_tz_name: Some("UTC"),
};

/// Searches the abbreviation tables (`abbr_search`). `gmtoffset == -1` means "any".
pub(crate) fn abbr_search(
    word: &[u8],
    gmtoffset: i64,
    isdst: i32,
) -> Option<&'static TzLookupEntry> {
    if strcasecmp(b"utc", word).is_eq() || strcasecmp(b"gmt", word).is_eq() {
        return Some(&UTC_ENTRY);
    }
    let mut first_found = None;
    for tp in TIMEZONE_MAP {
        if strcasecmp(word, tp.name.as_bytes()).is_eq() {
            if first_found.is_none() {
                first_found = Some(tp);
                if gmtoffset == -1 {
                    return Some(tp);
                }
            }
            if i64::from(tp.gmtoffset) == gmtoffset {
                return Some(tp);
            }
        }
    }
    if first_found.is_some() {
        return first_found;
    }
    FALLBACK_MAP
        .iter()
        .find(|fmp| i64::from(fmp.gmtoffset) == gmtoffset && i32::from(fmp.dst) == isdst)
}

/// Finds a timezone identifier from an abbreviation and/or offset
/// (`timelib_timezone_id_from_abbr`, used by `timezone_name_from_abbr()`).
pub fn timezone_id_from_abbr(abbr: &str, gmtoffset: i64, isdst: i32) -> Option<&'static str> {
    abbr_search(abbr.as_bytes(), gmtoffset, isdst).and_then(|tp| tp.full_tz_name)
}

/// The full abbreviation table (`timelib_timezone_abbreviations_list`, used by
/// `DateTimeZone::listAbbreviations()`).
pub fn timezone_abbreviations_list() -> &'static [TzLookupEntry] {
    TIMEZONE_MAP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(timezone_id_from_abbr("CET", -1, 0), Some("Europe/Berlin"));
        assert_eq!(timezone_id_from_abbr("utc", -1, 0), Some("UTC"));
        assert_eq!(timezone_id_from_abbr("", 3600, 0), Some("Europe/Paris"));
        assert_eq!(
            timezone_id_from_abbr("", -18000, 1),
            Some("America/Chicago")
        );
        assert_eq!(timezone_id_from_abbr("xyz", 12345, 0), None);
        // military zones are found but have no identifier
        assert_eq!(timezone_id_from_abbr("a", -1, 0), None);
        assert!(!timezone_abbreviations_list().is_empty());
    }
}
