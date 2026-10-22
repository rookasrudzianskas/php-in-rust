//! Stream-layer unit tests (expectations from ext/standard/tests/{file,streams}).

use std::cell::Cell;

use crate::testing::TestVm;
use php_runtime::{PResult, Value, Vm};

use super::stream::{self, StreamOps, StreamState};

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

thread_local! {
    static CLOSED_WITH_VM: Cell<u32> = const { Cell::new(0) };
}

/// Ops whose close needs the VM (like a userspace stream).
struct NeedsVm;

impl StreamOps for NeedsVm {
    fn label(&self) -> &'static str {
        "needs-vm"
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        st.eof = true;
        Ok(0)
    }

    fn close(&mut self, vm: &mut Vm, _st: &mut StreamState, _close_handle: bool) -> PResult<i32> {
        vm.echo(b"closed")?;
        CLOSED_WITH_VM.with(|c| c.set(c.get() + 1));
        Ok(0)
    }

    fn needs_vm_to_close(&self) -> bool {
        true
    }
}

#[test]
fn implicitly_dropped_streams_are_closed_later_with_the_vm() {
    let mut t = TestVm::new();
    let res = stream::alloc(&mut t.vm, Box::new(NeedsVm), b"r");
    assert!(super::handle_of(&res).is_some());
    drop(res);
    assert_eq!(CLOSED_WITH_VM.with(Cell::get), 0);
    super::process_deferred_closes(&mut t.vm).unwrap();
    assert_eq!(CLOSED_WITH_VM.with(Cell::get), 1);
    assert_eq!(t.output(), "closed");
    // explicit closes run right away and are not repeated
    let res = stream::alloc(&mut t.vm, Box::new(NeedsVm), b"r");
    assert!(super::close(&mut t.vm, &res).unwrap());
    drop(res);
    super::process_deferred_closes(&mut t.vm).unwrap();
    assert_eq!(CLOSED_WITH_VM.with(Cell::get), 2);
}

#[test]
fn public_api_roundtrip_on_memory_streams() {
    let mut t = TestVm::new();
    let res = super::open_memory(&mut t.vm, b"line1\nline2\n");
    assert_eq!(
        super::read_line(&mut t.vm, &res, None).unwrap().as_deref(),
        Some(&b"line1\n"[..])
    );
    assert_eq!(super::tell(&res), Some(6));
    assert_eq!(
        super::read(&mut t.vm, &res, 3).unwrap().as_deref(),
        Some(&b"lin"[..])
    );
    assert_eq!(super::seek(&mut t.vm, &res, 0, 2).unwrap(), 0);
    assert_eq!(super::write(&mut t.vm, &res, b"x").unwrap(), Some(1));
    assert_eq!(super::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
    assert_eq!(
        super::read_to_end(&mut t.vm, &res).unwrap().as_deref(),
        Some(&b"line1\nline2\nx"[..])
    );
    assert!(super::eof(&mut t.vm, &res).unwrap());
    assert_eq!(
        super::stat(&mut t.vm, &res).unwrap().map(|s| s.size),
        Some(13)
    );
    assert!(super::close(&mut t.vm, &res).unwrap());
    assert!(super::handle_of(&res).is_none());
    let (cls, msg) = crate::testing::thrown(
        super::write_resource(&mut t.vm, &Value::Resource(res), b"x").map(|_| Value::Null),
    );
    assert_eq!(cls, "TypeError");
    assert!(msg.ends_with("must be an open stream resource"), "{msg}");
}

#[test]
fn read_all_and_open_for_include() {
    let mut t = TestVm::new();
    assert_eq!(
        super::read_all(&mut t.vm, b"data:,abc").unwrap().as_deref(),
        Some(&b"abc"[..])
    );
    assert_eq!(
        super::read_all(&mut t.vm, b"/definitely/missing").unwrap(),
        None
    );
    assert!(
        t.output()
            .contains("Failed to open stream: No such file or directory")
    );
    let dir = std::env::temp_dir().join(format!("php-rs-inc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.php"), "<?php").unwrap();
    t.vm.ini_set("include_path", dir.to_string_lossy().into_owned());
    let (data, path) = super::open_for_include(&mut t.vm, b"x.php")
        .unwrap()
        .unwrap();
    assert_eq!(data, b"<?php");
    assert!(path.as_bytes().ends_with(b"/x.php"), "{path:?}");
    // directories are not includable
    let d = dir.to_string_lossy().into_owned();
    assert!(
        super::open_for_include(&mut t.vm, d.as_bytes())
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
