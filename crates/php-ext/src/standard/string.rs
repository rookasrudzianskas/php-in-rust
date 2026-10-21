//! ext/standard/string.c — split into submodules under `string/`.

mod hebrev;
mod locale;
mod parse_str;
mod path;
mod similar;
mod strip_tags;
mod strtok;
mod utf8;

use php_runtime::Registry;

pub fn register(r: &mut Registry) {
    hebrev::register(r);
    locale::register(r);
    parse_str::register(r);
    path::register(r);
    similar::register(r);
    strip_tags::register(r);
    strtok::register(r);
    utf8::register(r);
}
