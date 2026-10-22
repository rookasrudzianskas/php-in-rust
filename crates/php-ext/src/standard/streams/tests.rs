//! Stream-layer unit tests (expectations from ext/standard/tests/{file,streams}).

use crate::testing::TestVm;
use php_runtime::Value;

#[test]
fn std_handles_are_resources_1_to_3() {
    let t = TestVm::new();
    for (name, id) in [("STDIN", 1), ("STDOUT", 2), ("STDERR", 3)] {
        match t.vm.constant(name) {
            Some(Value::Resource(r)) => {
                assert_eq!(r.type_name(), "stream");
                assert_eq!(r.id(), id, "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}
