//! The `Pdo\Sqlite::openBlob()` stream (`php_stream_pdosqlite3_ops`).

use std::ffi::c_int;

use php_runtime::{PResult, ResRef, Vm};

use crate::sqlite3::capi::{Blob, ffi};
use crate::standard::streams::stream::{self, StatBuf, StreamOps, StreamState};

/// `php_stream_pdosqlite3_data`.
pub struct BlobStream {
    blob: Blob,
    position: usize,
    size: usize,
    flags: c_int,
}

/// Wraps an open blob in a stream resource.
pub fn open_stream(vm: &mut Vm, blob: Blob, flags: c_int, mode: &[u8]) -> ResRef {
    let size = blob.bytes().max(0) as usize;
    stream::alloc(
        vm,
        Box::new(BlobStream {
            blob,
            position: 0,
            size,
            flags,
        }),
        mode,
    )
}

impl StreamOps for BlobStream {
    fn label(&self) -> &'static str {
        "PDOSQLite"
    }

    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        if self.flags & ffi::SQLITE_OPEN_READONLY != 0 {
            vm.warning("Can't write to blob stream: is open as read only")?;
            return Ok(-1);
        }
        if self.position + buf.len() > self.size {
            vm.warning("It is not possible to increase the size of a BLOB")?;
            return Ok(-1);
        }
        if self.blob.write(buf, self.position as c_int) != ffi::SQLITE_OK {
            return Ok(-1);
        }
        if self.position + buf.len() >= self.size {
            st.eof = true;
            self.position = self.size;
        } else {
            self.position += buf.len();
        }
        Ok(buf.len() as isize)
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        let mut count = buf.len();
        if self.position + count >= self.size {
            count = self.size - self.position;
            st.eof = true;
        }
        if count > 0 {
            if self.blob.read(&mut buf[..count], self.position as c_int) != ffi::SQLITE_OK {
                return Ok(-1);
            }
            self.position += count;
        }
        Ok(count as isize)
    }

    fn close(&mut self, _vm: &mut Vm, _st: &mut StreamState, _close_handle: bool) -> PResult<i32> {
        // An error still closes the blob.
        self.blob.close();
        Ok(0)
    }

    fn can_seek(&self) -> bool {
        true
    }

    /// `php_pdosqlite3_stream_seek`.
    fn seek(
        &mut self,
        _vm: &mut Vm,
        st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        let size = self.size as i64;
        let pos = self.position as i64;
        let (ret, newoff) = match whence {
            libc_seek::SEEK_CUR => {
                if offset < 0 {
                    if pos < -offset {
                        self.position = 0;
                        (-1, -1)
                    } else {
                        self.position = (pos + offset) as usize;
                        st.eof = false;
                        (0, self.position as i64)
                    }
                } else if pos + offset > size {
                    self.position = self.size;
                    (-1, -1)
                } else {
                    self.position = (pos + offset) as usize;
                    st.eof = false;
                    (0, self.position as i64)
                }
            }
            libc_seek::SEEK_SET => {
                if offset < 0 || size < offset {
                    self.position = self.size;
                    (-1, -1)
                } else {
                    self.position = offset as usize;
                    st.eof = false;
                    (0, self.position as i64)
                }
            }
            libc_seek::SEEK_END => {
                if offset > 0 {
                    self.position = self.size;
                    (-1, -1)
                } else if size < -offset {
                    self.position = 0;
                    (-1, -1)
                } else {
                    self.position = (size + offset) as usize;
                    st.eof = false;
                    (0, self.position as i64)
                }
            }
            _ => (-1, self.position as i64),
        };
        Ok((ret, Some(newoff)))
    }

    fn stat(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        Ok(Some(StatBuf {
            size: self.size as i64,
            ..StatBuf::default()
        }))
    }
}

/// `whence` values of `fseek()`.
mod libc_seek {
    pub const SEEK_SET: i32 = 0;
    pub const SEEK_CUR: i32 = 1;
    pub const SEEK_END: i32 = 2;
}
