//! Smoke tests of the basic UTF-8 functions.

use super::*;

#[test]
fn strlen_utf8() {
    let mut t = TestVm::new();
    // mb_strlen.phpt
    assert_eq!(
        int(&call(
            &mut t,
            "mb_strlen",
            vec![s("日本語テキストです。0123456789。")]
        )),
        21
    );
    assert_eq!(
        int(&call(&mut t, "mb_strlen", vec![s("abc"), s("UTF-8")])),
        3
    );
    assert_eq!(vd(&mut t, "mb_strlen", vec![b(b"\xff\xfe")]), "int(2)\n");
}
