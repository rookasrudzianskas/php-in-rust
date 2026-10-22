//! Tests of the iterator classes, ported from the php-src `.phpt` files
//! (user classes are simulated with `crate::spl::testutil::user_class`).

/// The source of `tests/phpt/ext/spl/tests/<name>`.
macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/phpt/ext/spl/tests/",
            $name
        ))
    };
}

mod dual;
mod helpers;
mod recursive;
