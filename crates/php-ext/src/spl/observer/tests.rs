//! Tests of spl_observer.c, ported from ext/spl/tests/SplObjectStorage/*.phpt,
//! multiple_iterator_001.phpt and the related bug/gh tests.

use crate::testing::TestVm;

#[test]
fn classes_are_registered() {
    let t = TestVm::new();
    for c in [
        "SplObserver",
        "SplSubject",
        "SplObjectStorage",
        "MultipleIterator",
    ] {
        assert!(t.vm.find_class(c.as_bytes()).is_some(), "{c}");
    }
}
