//! Unit tests for the array functions (expectations from php-src's .phpt files).

use php_runtime::{PhpArray, Value};

use crate::testing::TestVm;

#[test]
fn smoke() {
    let mut t = TestVm::new();
    let r = t
        .call(
            "count",
            vec![Value::array(PhpArray::from_values(vec![
                1.into(),
                2.into(),
            ]))],
        )
        .unwrap();
    assert_eq!(r.as_int(), Some(2));
}
