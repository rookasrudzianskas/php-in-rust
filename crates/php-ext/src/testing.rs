//! Test harness for extension unit tests: a [`Vm`] with every module
//! registered and stdout captured.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::output::CaptureSink;
use php_runtime::{Flow, PResult, PhpStr, Value, Vm};

pub struct TestVm {
    pub vm: Vm,
    pub out: CaptureSink,
    pub log: Rc<RefCell<Vec<u8>>>,
}

struct LogWriter(Rc<RefCell<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TestVm {
    pub fn new() -> TestVm {
        let out = CaptureSink::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut vm = Vm::with_sink(Box::new(out.clone()));
        vm.stderr = Box::new(LogWriter(log.clone()));
        vm.executing = true;
        vm.current_file = PhpStr::from("/t.php");
        vm.current_line = 3;
        crate::register_all(&mut vm);
        TestVm { vm, out, log }
    }

    /// Output written so far (drained), including displayed warnings.
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.out.take()).into_owned()
    }

    pub fn call(&mut self, name: &str, args: Vec<Value>) -> PResult<Value> {
        self.vm.call_native(name, args)
    }
}

/// Class and message of a thrown exception.
pub fn thrown(r: PResult<Value>) -> (String, String) {
    match r {
        Err(Flow::Exception(e)) => (e.class().name.to_string(), Vm::exception_message(&e)),
        Err(other) => panic!("expected exception, got {other:?}"),
        Ok(v) => panic!("expected exception, got value {v:?}"),
    }
}
