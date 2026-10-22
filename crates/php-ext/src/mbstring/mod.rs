//! ext/mbstring: multibyte string functions over a port of libmbfl.
//!
//! * [`mbfl`] — encoding descriptors and registry, conversion buffers, the
//!   legacy filters and `mbfl_strcut`, languages.
//! * [`filters`] — the per-encoding converters (`libmbfl/filters`).
//! * [`tables`] — conversion and Unicode tables generated from php-src.
//! * [`php_unicode`] — case mapping (`php_unicode.c`).
//! * The PHP functions of `mbstring.c` and `mb_gpc.c`, grouped by topic.
//!
//! `mb_ereg*`/`mb_regex_*` are not provided: php-src only defines them when
//! built with Oniguruma, whose regex semantics are not available here.

pub mod conversion;
pub mod filters;
pub mod globals;
pub mod gpc;
pub mod info;
pub mod mail;
pub mod mbfl;
pub mod mime;
pub mod output;
pub mod php_unicode;
pub mod strings;
pub mod tables;

#[cfg(test)]
mod tests;

use php_runtime::{NativeFunction, Registry};

macro_rules! native {
    ($name:ident, $sig:expr, $handler:path) => {
        pub static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

native!(
    MB_LANGUAGE,
    "mb_language(?string $language = null): string|bool",
    info::mb_language
);
native!(
    MB_INTERNAL_ENCODING,
    "mb_internal_encoding(?string $encoding = null): string|bool",
    info::mb_internal_encoding
);
native!(
    MB_HTTP_INPUT,
    "mb_http_input(?string $type = null): array|string|false",
    info::mb_http_input
);
native!(
    MB_HTTP_OUTPUT,
    "mb_http_output(?string $encoding = null): string|bool",
    info::mb_http_output
);
native!(
    MB_DETECT_ORDER,
    "mb_detect_order(array|string|null $encoding = null): array|bool",
    info::mb_detect_order
);
native!(
    MB_SUBSTITUTE_CHARACTER,
    "mb_substitute_character(string|int|null $substitute_character = null): string|int|bool",
    info::mb_substitute_character
);
native!(
    MB_PREFERRED_MIME_NAME,
    "mb_preferred_mime_name(string $encoding): string|false",
    info::mb_preferred_mime_name
);
native!(
    MB_PARSE_STR,
    "/** @param array $result */ mb_parse_str(string $string, &$result): bool",
    gpc::mb_parse_str
);
native!(
    MB_OUTPUT_HANDLER,
    "mb_output_handler(string $string, int $status): string",
    output::mb_output_handler
);
native!(
    MB_STR_SPLIT,
    "mb_str_split(string $string, int $length = 1, ?string $encoding = null): array",
    strings::mb_str_split
);
native!(
    MB_STRLEN,
    "mb_strlen(string $string, ?string $encoding = null): int",
    strings::mb_strlen
);
native!(
    MB_STRPOS,
    "mb_strpos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false",
    strings::mb_strpos
);
native!(
    MB_STRRPOS,
    "mb_strrpos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false",
    strings::mb_strrpos
);
native!(
    MB_STRIPOS,
    "mb_stripos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false",
    strings::mb_stripos
);
native!(
    MB_STRRIPOS,
    "mb_strripos(string $haystack, string $needle, int $offset = 0, ?string $encoding = null): int|false",
    strings::mb_strripos
);
native!(
    MB_STRSTR,
    "mb_strstr(string $haystack, string $needle, bool $before_needle = false, ?string $encoding = null): string|false",
    strings::mb_strstr
);
native!(
    MB_STRRCHR,
    "mb_strrchr(string $haystack, string $needle, bool $before_needle = false, ?string $encoding = null): string|false",
    strings::mb_strrchr
);
native!(
    MB_STRISTR,
    "mb_stristr(string $haystack, string $needle, bool $before_needle = false, ?string $encoding = null): string|false",
    strings::mb_stristr
);
native!(
    MB_STRRICHR,
    "mb_strrichr(string $haystack, string $needle, bool $before_needle = false, ?string $encoding = null): string|false",
    strings::mb_strrichr
);
native!(
    MB_SUBSTR_COUNT,
    "mb_substr_count(string $haystack, string $needle, ?string $encoding = null): int",
    strings::mb_substr_count
);
native!(
    MB_SUBSTR,
    "mb_substr(string $string, int $start, ?int $length = null, ?string $encoding = null): string",
    strings::mb_substr
);
native!(
    MB_STRCUT,
    "mb_strcut(string $string, int $start, ?int $length = null, ?string $encoding = null): string",
    strings::mb_strcut
);
native!(
    MB_STRWIDTH,
    "mb_strwidth(string $string, ?string $encoding = null): int",
    strings::mb_strwidth
);
native!(
    MB_STRIMWIDTH,
    "mb_strimwidth(string $string, int $start, int $width, string $trim_marker = \"\", ?string $encoding = null): string",
    strings::mb_strimwidth
);
native!(
    MB_CONVERT_ENCODING,
    "mb_convert_encoding(array|string $string, string $to_encoding, array|string|null $from_encoding = null): array|string|false",
    conversion::mb_convert_encoding
);
native!(
    MB_CONVERT_CASE,
    "mb_convert_case(string $string, int $mode, ?string $encoding = null): string",
    strings::mb_convert_case
);
native!(
    MB_STRTOUPPER,
    "mb_strtoupper(string $string, ?string $encoding = null): string",
    strings::mb_strtoupper
);
native!(
    MB_STRTOLOWER,
    "mb_strtolower(string $string, ?string $encoding = null): string",
    strings::mb_strtolower
);
native!(
    MB_UCFIRST,
    "mb_ucfirst(string $string, ?string $encoding = null): string",
    strings::mb_ucfirst
);
native!(
    MB_LCFIRST,
    "mb_lcfirst(string $string, ?string $encoding = null): string",
    strings::mb_lcfirst
);
native!(
    MB_TRIM,
    "mb_trim(string $string, ?string $characters = null, ?string $encoding = null): string",
    strings::mb_trim
);
native!(
    MB_LTRIM,
    "mb_ltrim(string $string, ?string $characters = null, ?string $encoding = null): string",
    strings::mb_ltrim
);
native!(
    MB_RTRIM,
    "mb_rtrim(string $string, ?string $characters = null, ?string $encoding = null): string",
    strings::mb_rtrim
);
native!(
    MB_DETECT_ENCODING,
    "mb_detect_encoding(string $string, array|string|null $encodings = null, bool $strict = false): string|false",
    conversion::mb_detect_encoding
);
native!(
    MB_LIST_ENCODINGS,
    "mb_list_encodings(): array",
    info::mb_list_encodings
);
native!(
    MB_ENCODING_ALIASES,
    "mb_encoding_aliases(string $encoding): array",
    info::mb_encoding_aliases
);
native!(
    MB_ENCODE_MIMEHEADER,
    "mb_encode_mimeheader(string $string, ?string $charset = null, ?string $transfer_encoding = null, string $newline = \"\\r\\n\", int $indent = 0): string",
    mime::mb_encode_mimeheader
);
native!(
    MB_DECODE_MIMEHEADER,
    "mb_decode_mimeheader(string $string): string",
    mime::mb_decode_mimeheader
);
native!(
    MB_CONVERT_KANA,
    "mb_convert_kana(string $string, string $mode = \"KV\", ?string $encoding = null): string",
    conversion::mb_convert_kana
);
native!(
    MB_CONVERT_VARIABLES,
    "mb_convert_variables(string $to_encoding, array|string $from_encoding, mixed &$var, mixed &...$vars): string|false",
    conversion::mb_convert_variables
);
native!(
    MB_ENCODE_NUMERICENTITY,
    "mb_encode_numericentity(string $string, array $map, ?string $encoding = null, bool $hex = false): string",
    conversion::mb_encode_numericentity
);
native!(
    MB_DECODE_NUMERICENTITY,
    "mb_decode_numericentity(string $string, array $map, ?string $encoding = null): string",
    conversion::mb_decode_numericentity
);
native!(
    MB_SEND_MAIL,
    "mb_send_mail(string $to, string $subject, string $message, array|string $additional_headers = [], ?string $additional_params = null): bool",
    mail::mb_send_mail
);
native!(
    MB_GET_INFO,
    "mb_get_info(string $type = \"all\"): array|string|int|false|null",
    info::mb_get_info
);
native!(
    MB_CHECK_ENCODING,
    "mb_check_encoding(array|string|null $value = null, ?string $encoding = null): bool",
    strings::mb_check_encoding
);
native!(
    MB_SCRUB,
    "mb_scrub(string $string, ?string $encoding = null): string",
    strings::mb_scrub
);
native!(
    MB_ORD,
    "mb_ord(string $string, ?string $encoding = null): int|false",
    strings::mb_ord
);
native!(
    MB_CHR,
    "mb_chr(int $codepoint, ?string $encoding = null): string|false",
    strings::mb_chr
);
native!(
    MB_STR_PAD,
    "mb_str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT, ?string $encoding = null): string",
    strings::mb_str_pad
);

pub static FUNCTIONS: [&NativeFunction; 49] = [
    &MB_LANGUAGE,
    &MB_INTERNAL_ENCODING,
    &MB_HTTP_INPUT,
    &MB_HTTP_OUTPUT,
    &MB_DETECT_ORDER,
    &MB_SUBSTITUTE_CHARACTER,
    &MB_PREFERRED_MIME_NAME,
    &MB_PARSE_STR,
    &MB_OUTPUT_HANDLER,
    &MB_STR_SPLIT,
    &MB_STRLEN,
    &MB_STRPOS,
    &MB_STRRPOS,
    &MB_STRIPOS,
    &MB_STRRIPOS,
    &MB_STRSTR,
    &MB_STRRCHR,
    &MB_STRISTR,
    &MB_STRRICHR,
    &MB_SUBSTR_COUNT,
    &MB_SUBSTR,
    &MB_STRCUT,
    &MB_STRWIDTH,
    &MB_STRIMWIDTH,
    &MB_CONVERT_ENCODING,
    &MB_CONVERT_CASE,
    &MB_STRTOUPPER,
    &MB_STRTOLOWER,
    &MB_UCFIRST,
    &MB_LCFIRST,
    &MB_TRIM,
    &MB_LTRIM,
    &MB_RTRIM,
    &MB_DETECT_ENCODING,
    &MB_LIST_ENCODINGS,
    &MB_ENCODING_ALIASES,
    &MB_ENCODE_MIMEHEADER,
    &MB_DECODE_MIMEHEADER,
    &MB_CONVERT_KANA,
    &MB_CONVERT_VARIABLES,
    &MB_ENCODE_NUMERICENTITY,
    &MB_DECODE_NUMERICENTITY,
    &MB_SEND_MAIL,
    &MB_GET_INFO,
    &MB_CHECK_ENCODING,
    &MB_SCRUB,
    &MB_ORD,
    &MB_CHR,
    &MB_STR_PAD,
];

pub fn register(r: &mut Registry) {
    r.functions(&FUNCTIONS);
    r.ini_entries(&globals::INI_ENTRIES);
    // PHP_UNICODE_CASE_* (php_unicode.h)
    r.constant("MB_CASE_UPPER", 0i64);
    r.constant("MB_CASE_LOWER", 1i64);
    r.constant("MB_CASE_TITLE", 2i64);
    r.constant("MB_CASE_FOLD", 3i64);
    r.constant("MB_CASE_UPPER_SIMPLE", 4i64);
    r.constant("MB_CASE_LOWER_SIMPLE", 5i64);
    r.constant("MB_CASE_TITLE_SIMPLE", 6i64);
    r.constant("MB_CASE_FOLD_SIMPLE", 7i64);
}
