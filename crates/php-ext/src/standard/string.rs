//! ext/standard/string.c — split into submodules under `string/`.
//!
//! * `trim`: `trim`/`rtrim`/`ltrim`/`chop`, `strspn`/`strcspn`
//! * `search`: `strpos` family, `strstr` family, `substr`, `substr_count`,
//!   `strpbrk`, `substr_compare`, `str_contains` & co
//! * `replace`: `str_replace`/`str_ireplace`, `strtr`, `substr_replace`
//! * `split`: `explode`, `implode`, `str_split`, `chunk_split`, `wordwrap`,
//!   `nl2br`, `str_word_count`
//! * `misc`: hex, case mapping, slashes, padding, repetition, ...
//! * `strip_tags`, `similar`, `hebrev`, `locale`, `parse_str`, `path`,
//!   `strtok`, `utf8`: the remaining functions, one module each

mod hebrev;
mod locale;
mod misc;
mod parse_str;
mod path;
mod replace;
mod search;
mod similar;
mod split;
mod strip_tags;
mod strtok;
mod trim;
mod utf8;
pub mod util;

use php_runtime::Registry;

pub use misc::{php_addcslashes, php_addslashes, php_stripcslashes, php_stripslashes};
pub use search::php_substr;
pub use split::{php_explode, php_implode};
pub use trim::php_trim;

pub fn register(r: &mut Registry) {
    trim::register(r);
    search::register(r);
    replace::register(r);
    split::register(r);
    misc::register(r);
    hebrev::register(r);
    locale::register(r);
    parse_str::register(r);
    path::register(r);
    similar::register(r);
    strip_tags::register(r);
    strtok::register(r);
    utf8::register(r);
}
