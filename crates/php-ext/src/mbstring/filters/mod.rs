//! The per-encoding converters (`ext/mbstring/libmbfl/filters`): the fast
//! `mb_*_to_wchar` / `mb_wchar_to_*` functions, specialised checks and cuts,
//! and the legacy filters `mbfl_strcut` still needs.

pub mod base64;
pub mod cjk;
pub mod eightbit;
pub mod htmlent;
pub mod mblen;
pub mod pass;
pub mod qprint;
pub mod sevenbit;
pub mod singlebyte;
pub mod ucs2;
pub mod ucs4;
pub mod utf16;
pub mod utf32;
pub mod utf7;
pub mod utf7imap;
pub mod utf8;
pub mod uuencode;
pub mod wchar;
