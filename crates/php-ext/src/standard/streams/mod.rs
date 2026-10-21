//! main/streams + ext/standard stream wrappers (plain files, php://, data:, glob:).
//!
//! The functions below are the interface other modules use to do stream I/O
//! (e.g. `fprintf`, `md5_file`, `hash_file`). The streams work package owns
//! this module and must keep these signatures.

pub mod glob;
use php_runtime::{PResult, Registry, Value, Vm};

/// `fwrite()` semantics on a stream resource: returns the number of bytes
/// written, or `None` when the write failed (any warning has been emitted).
/// A value that is not a stream resource raises PHP's TypeError.
pub fn write_resource(vm: &mut Vm, stream: &Value, data: &[u8]) -> PResult<Option<usize>> {
    let _ = (vm, stream, data);
    todo!("implemented by the streams work package")
}

/// `file_get_contents($path)` semantics for any supported wrapper, emitting
/// PHP's "Failed to open stream" warnings. `None` on failure.
pub fn read_all(vm: &mut Vm, path: &[u8]) -> PResult<Option<Vec<u8>>> {
    let _ = (vm, path);
    todo!("implemented by the streams work package")
}

pub fn register(_r: &mut Registry) {}
