//! Parse error/warning reporting (`timelib_error_container`) and error codes.

/// Maximum number of errors (and separately warnings) recorded (`TIMELIB_MAX_ERRORS`).
pub const MAX_ERRORS: usize = 1000;

/// Warning/error codes (`TIMELIB_WARN_*` / `TIMELIB_ERR_*`).
#[allow(missing_docs)]
pub mod code {
    pub const WARN_DOUBLE_TZ: i32 = 0x101;
    pub const WARN_INVALID_TIME: i32 = 0x102;
    pub const WARN_INVALID_DATE: i32 = 0x103;
    pub const WARN_TRAILING_DATA: i32 = 0x11a;

    pub const ERR_DOUBLE_TZ: i32 = 0x201;
    pub const ERR_TZID_NOT_FOUND: i32 = 0x202;
    pub const ERR_DOUBLE_TIME: i32 = 0x203;
    pub const ERR_DOUBLE_DATE: i32 = 0x204;
    pub const ERR_UNEXPECTED_CHARACTER: i32 = 0x205;
    pub const ERR_EMPTY_STRING: i32 = 0x206;
    pub const ERR_UNEXPECTED_DATA: i32 = 0x207;
    pub const ERR_NO_TEXTUAL_DAY: i32 = 0x208;
    pub const ERR_NO_TWO_DIGIT_DAY: i32 = 0x209;
    pub const ERR_NO_THREE_DIGIT_DAY_OF_YEAR: i32 = 0x20a;
    pub const ERR_NO_TWO_DIGIT_MONTH: i32 = 0x20b;
    pub const ERR_NO_TEXTUAL_MONTH: i32 = 0x20c;
    pub const ERR_NO_TWO_DIGIT_YEAR: i32 = 0x20d;
    pub const ERR_NO_FOUR_DIGIT_YEAR: i32 = 0x20e;
    pub const ERR_NO_TWO_DIGIT_HOUR: i32 = 0x20f;
    pub const ERR_HOUR_LARGER_THAN_12: i32 = 0x210;
    pub const ERR_MERIDIAN_BEFORE_HOUR: i32 = 0x211;
    pub const ERR_NO_MERIDIAN: i32 = 0x212;
    pub const ERR_NO_TWO_DIGIT_MINUTE: i32 = 0x213;
    pub const ERR_NO_TWO_DIGIT_SECOND: i32 = 0x214;
    pub const ERR_NO_SIX_DIGIT_MICROSECOND: i32 = 0x215;
    pub const ERR_NO_SEP_SYMBOL: i32 = 0x216;
    pub const ERR_EXPECTED_ESCAPE_CHAR: i32 = 0x217;
    pub const ERR_NO_ESCAPED_CHAR: i32 = 0x218;
    pub const ERR_WRONG_FORMAT_SEP: i32 = 0x219;
    pub const ERR_TRAILING_DATA: i32 = 0x21a;
    pub const ERR_DATA_MISSING: i32 = 0x21b;
    pub const ERR_NO_THREE_DIGIT_MILLISECOND: i32 = 0x21c;
    pub const ERR_NO_FOUR_DIGIT_YEAR_ISO: i32 = 0x21d;
    pub const ERR_NO_TWO_DIGIT_WEEK: i32 = 0x21e;
    pub const ERR_INVALID_WEEK: i32 = 0x21f;
    pub const ERR_NO_DAY_OF_WEEK: i32 = 0x220;
    pub const ERR_INVALID_DAY_OF_WEEK: i32 = 0x221;
    pub const ERR_INVALID_SPECIFIER: i32 = 0x222;
    pub const ERR_INVALID_TZ_OFFSET: i32 = 0x223;
    pub const ERR_FORMAT_LITERAL_MISMATCH: i32 = 0x224;
    pub const ERR_MIX_ISO_WITH_NATURAL: i32 = 0x225;
    pub const ERR_NUMBER_OUT_OF_RANGE: i32 = 0x226;
}

/// One warning or error (`timelib_error_message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorMessage {
    /// One of the [`code`] constants (0 for ISO interval errors, which carry no code).
    pub error_code: i32,
    /// Byte position in the (whitespace-trimmed for strtotime) input.
    pub position: usize,
    /// The byte at that position (0 at the end of the input).
    pub character: u8,
    /// Human readable message, exactly as PHP reports it.
    pub message: &'static str,
}

/// Collected warnings and errors (`timelib_error_container`). PHP's
/// `DateTime::getLastErrors()` / `date_parse()` expose these.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorContainer {
    /// Errors (parsing failed if non-empty).
    pub errors: Vec<ErrorMessage>,
    /// Warnings.
    pub warnings: Vec<ErrorMessage>,
}

impl ErrorContainer {
    /// Number of errors.
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Number of warnings.
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// Whether there are no errors and no warnings.
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }

    pub(crate) fn add_error(
        &mut self,
        code: i32,
        position: usize,
        character: u8,
        message: &'static str,
    ) {
        if self.errors.len() >= MAX_ERRORS {
            return;
        }
        self.errors.push(ErrorMessage {
            error_code: code,
            position,
            character,
            message,
        });
    }

    pub(crate) fn add_warning(
        &mut self,
        code: i32,
        position: usize,
        character: u8,
        message: &'static str,
    ) {
        if self.warnings.len() >= MAX_ERRORS {
            return;
        }
        self.warnings.push(ErrorMessage {
            error_code: code,
            position,
            character,
            message,
        });
    }
}

/// Error codes for tz/duration operations (`TIMELIB_ERROR_*`).
#[allow(missing_docs)]
pub mod tz_error {
    pub const NO_ERROR: i32 = 0x00;
    pub const CANNOT_ALLOCATE: i32 = 0x01;
    pub const CORRUPT_TRANSITIONS_DONT_INCREASE: i32 = 0x02;
    pub const CORRUPT_NO_64BIT_PREAMBLE: i32 = 0x03;
    pub const CORRUPT_NO_ABBREVIATION: i32 = 0x04;
    pub const UNSUPPORTED_VERSION: i32 = 0x05;
    pub const NO_SUCH_TIMEZONE: i32 = 0x06;
    pub const SLIM_FILE: i32 = 0x07;
    pub const CORRUPT_POSIX_STRING: i32 = 0x08;
    pub const EMPTY_POSIX_STRING: i32 = 0x09;
}

const ERROR_MESSAGES: [&str; 18] = [
    "No error",
    "Cannot allocate buffer for parsing",
    "Corrupt tzfile: The transitions in the file don't always increase",
    "Corrupt tzfile: The expected 64-bit preamble is missing",
    "Corrupt tzfile: No abbreviation could be found for a transition",
    "The version used in this timezone identifier is unsupported",
    "No timezone with this name could be found",
    "A 'slim' timezone file has been detected",
    "The embedded POSIX string is not valid",
    "The embedded POSIX string is empty",
    "Seconds value out of range",
    "Nanoseconds value out of range",
    "Division by zero",
    "Integer Overflow",
    "The ISO 8601 duration string may only contain the period (P) aspect",
    "The ISO 8601 duration string is missing the period (P) aspect",
    "The ISO 8601 duration string may only contain the time (T) aspect",
    "The ISO 8601 duration string could not be parsed",
];

/// Message for a `TIMELIB_ERROR_*` code (`timelib_get_error_message`).
pub fn get_error_message(error_code: i32) -> &'static str {
    usize::try_from(error_code)
        .ok()
        .and_then(|i| ERROR_MESSAGES.get(i))
        .copied()
        .unwrap_or("Unknown error code")
}
