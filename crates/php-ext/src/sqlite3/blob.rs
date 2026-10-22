//! `SQLite3::openBlob()` and the `SQLite3` blob stream
//! (`php_stream_sqlite3_ops`).

use php_runtime::{Args, PResult, Value, Vm};

use super::Pending;
use super::capi::{Blob, ffi};
use super::db::open_db;
use crate::standard::streams::stream::{self, StatBuf, StreamOps, StreamState};

/// `php_stream_sqlite3_data`.
struct BlobStream {
    blob: Blob,
    position: usize,
    size: usize,
    /// The `$flags` passed to `openBlob()`.
    flags: i64,
}

impl BlobStream {
    /// A failed seek: the position is clamped and `*newoffs = -1`.
    fn seek_failed(&mut self, position: usize) -> (i32, Option<i64>) {
        self.position = position;
        (-1, Some(-1))
    }

    fn seek_to(&mut self, st: &mut StreamState, position: usize) -> (i32, Option<i64>) {
        self.position = position;
        st.eof = false;
        st.fatal_error = false;
        (0, Some(position as i64))
    }
}

impl StreamOps for BlobStream {
    fn label(&self) -> &'static str {
        "SQLite3"
    }

    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        if self.flags & i64::from(ffi::SQLITE_OPEN_READONLY) != 0 {
            vm.warning("Can't write to blob stream: is open as read only")?;
            return Ok(-1);
        }
        let count = buf.len();
        if self.position + count > self.size {
            vm.warning("It is not possible to increase the size of a BLOB")?;
            return Ok(-1);
        }
        if self.blob.write(buf, self.position as i32) != ffi::SQLITE_OK {
            return Ok(-1);
        }
        if self.position + count >= self.size {
            st.eof = true;
            self.position = self.size;
        } else {
            self.position += count;
        }
        Ok(count as isize)
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        let mut count = buf.len();
        if self.position + count >= self.size {
            count = self.size - self.position;
            st.eof = true;
        }
        if count > 0 {
            if self.blob.read(&mut buf[..count], self.position as i32) != ffi::SQLITE_OK {
                return Ok(-1);
            }
            self.position += count;
        }
        Ok(count as isize)
    }

    fn close(&mut self, _vm: &mut Vm, _st: &mut StreamState, _close_handle: bool) -> PResult<i32> {
        // An error still closes the handle.
        self.blob.close();
        Ok(0)
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        _vm: &mut Vm,
        st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        let (pos, size) = (self.position as u64, self.size as u64);
        let magnitude = offset.unsigned_abs();
        Ok(match whence {
            // SEEK_CUR
            1 => {
                if offset < 0 {
                    if pos < magnitude {
                        self.seek_failed(0)
                    } else {
                        self.seek_to(st, (pos - magnitude) as usize)
                    }
                } else if pos + magnitude > size {
                    self.seek_failed(self.size)
                } else {
                    self.seek_to(st, (pos + magnitude) as usize)
                }
            }
            // SEEK_SET: a negative offset is a huge size_t.
            0 => {
                if offset < 0 || size < magnitude {
                    self.seek_failed(self.size)
                } else {
                    self.seek_to(st, offset as usize)
                }
            }
            // SEEK_END
            2 => {
                if offset > 0 {
                    self.seek_failed(self.size)
                } else if size < magnitude {
                    self.seek_failed(0)
                } else {
                    self.seek_to(st, (size - magnitude) as usize)
                }
            }
            _ => (-1, Some(self.position as i64)),
        })
    }

    fn stat(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        Ok(Some(StatBuf {
            size: self.size as i64,
            ..StatBuf::default()
        }))
    }
}

/// `SQLite3::openBlob()`.
pub(crate) fn open_blob(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let table = args.str(0).clone();
    let column = args.str(1).clone();
    let rowid = args.int(2);
    let dbname = args.str(3).clone();
    let flags = args.int(4);
    if dbname.contains(&0) {
        return Err(vm.argument_value_error(4, "must not contain any null bytes"));
    }
    let this = args
        .this()
        .cloned()
        .expect("SQLite3 method called without $this");
    let (conn, shared) = open_db(vm, &this)?;
    let writable = flags & i64::from(ffi::SQLITE_OPEN_READWRITE) != 0;
    match conn.blob_open(&dbname, &table, &column, rowid, writable) {
        Err(_) => {
            let mut p = Pending::new(&conn);
            let msg = format!("Unable to open blob: {}", conn.errmsg());
            p.error(vm, shared.exceptions.get(), i64::from(conn.errcode()), msg);
            p.finish(Value::Bool(false))
        }
        Ok(blob) => {
            let size = usize::try_from(blob.bytes()).unwrap_or(0);
            let ops = BlobStream {
                blob,
                position: 0,
                size,
                flags,
            };
            let mode: &[u8] = if writable { b"r+b" } else { b"rb" };
            let res = stream::alloc(vm, Box::new(ops), mode);
            Ok(Value::Resource(res))
        }
    }
}
