//! main/streams/userspace.c — userspace stream wrappers
//! (`stream_wrapper_register`).

use std::any::Any;

use php_runtime::{PResult, PhpStr, Registry, ResRef, Vm};

use super::wrappers::Wrapper;

/// Registers `stream_wrapper_register` & co. (main/streams/userspace.c).
pub fn register(r: &mut Registry) {
    let _ = r;
}

/// A wrapper implemented by a PHP class.
pub struct UserWrapper {
    pub protocol: PhpStr,
    pub class: PhpStr,
    pub flags: i64,
}

impl Wrapper for UserWrapper {
    fn label(&self) -> &'static str {
        "user-space"
    }

    fn open(
        &self,
        _vm: &mut Vm,
        _path: &[u8],
        _mode: &[u8],
        _options: i32,
        _opened_path: &mut Option<PhpStr>,
        _ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        Ok(None)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
