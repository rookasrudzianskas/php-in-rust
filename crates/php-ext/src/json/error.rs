//! `php_json_error_code` and `php_json_error_details` (`php_json.h`, `json.c`).

/// `php_json_error_code`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i64)]
pub enum ErrorCode {
    #[default]
    None = 0,
    Depth = 1,
    StateMismatch = 2,
    CtrlChar = 3,
    Syntax = 4,
    Utf8 = 5,
    Recursion = 6,
    InfOrNan = 7,
    UnsupportedType = 8,
    InvalidPropertyName = 9,
    Utf16 = 10,
    NonBackedEnum = 11,
}

impl ErrorCode {
    /// `php_json_get_error_msg()`.
    pub fn message(self) -> &'static str {
        match self {
            ErrorCode::None => "No error",
            ErrorCode::Depth => "Maximum stack depth exceeded",
            ErrorCode::StateMismatch => "State mismatch (invalid or malformed JSON)",
            ErrorCode::CtrlChar => "Control character error, possibly incorrectly encoded",
            ErrorCode::Syntax => "Syntax error",
            ErrorCode::Utf8 => "Malformed UTF-8 characters, possibly incorrectly encoded",
            ErrorCode::Recursion => "Recursion detected",
            ErrorCode::InfOrNan => "Inf and NaN cannot be JSON encoded",
            ErrorCode::UnsupportedType => "Type is not supported",
            ErrorCode::InvalidPropertyName => "The decoded property name is invalid",
            ErrorCode::Utf16 => "Single unpaired UTF-16 surrogate in unicode escape",
            ErrorCode::NonBackedEnum => "Non-backed enums have no default serialization",
        }
    }
}

/// `php_json_error_details`: the code plus the 1-based line and column of the
/// failing token (both `0` when there is no location).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ErrorDetails {
    pub code: ErrorCode,
    pub line: u64,
    pub column: u64,
}

impl ErrorDetails {
    pub fn new(code: ErrorCode) -> ErrorDetails {
        ErrorDetails {
            code,
            line: 0,
            column: 0,
        }
    }

    /// `php_json_get_error_msg_with_location()`.
    pub fn message_with_location(&self) -> String {
        let base = self.code.message();
        if self.line > 0 && self.column > 0 {
            format!("{base} near location {}:{}", self.line, self.column)
        } else {
            base.to_string()
        }
    }
}
