//! The slice of `php_stream` that `image.c` relies on.
//!
//! getimagesize() is written against PHP's stream layer and its quirks are
//! observable (sequential reads, seeks that may fail, the `eof` flag, the
//! read buffer being dropped by a seek, `php_stream_gets`). This module
//! reproduces `main/streams/streams.c` (`php_stream_read`, `_seek`, `_getc`,
//! `_eof`, `_get_line`, `fill_read_buffer`) on top of either a plain file
//! (`plain_wrapper.c`: buffered, 8 KiB chunks, lazily read from disk) or a
//! memory stream (`memory.c`: unbuffered), so that the format parsers can be
//! ported statement by statement.

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};

use php_runtime::PhpStr;

/// `stream->chunk_size`.
const CHUNK_SIZE: usize = 8192;

enum Backend {
    /// `php_stream_fopen` on a local file (buffered).
    File(File),
    /// `php_stream_memory_open(TEMP_STREAM_READONLY, ...)` (unbuffered).
    Memory { data: PhpStr, fpos: u64 },
}

/// A readable, seekable PHP stream.
pub struct Stream {
    backend: Backend,
    /// `stream->readbuf` (valid data is `buf[readpos..writepos]`).
    buf: Vec<u8>,
    readpos: usize,
    writepos: usize,
    /// `stream->position`: may become -1 after a failed memory seek, like php-src.
    position: i64,
    eof: bool,
    /// Notices raised while reading (`Read of %zu bytes failed ...`), to be
    /// reported by the owner of the `Vm` through [`Stream::take_notices`].
    notices: Vec<String>,
}

/// `strerror()` text of an I/O error (without Rust's ` (os error N)` suffix).
pub fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

impl Stream {
    pub fn from_file(file: File) -> Stream {
        Stream::new(Backend::File(file))
    }

    /// `php_stream_memory_open(TEMP_STREAM_READONLY, data)`.
    pub fn from_memory(data: PhpStr) -> Stream {
        Stream::new(Backend::Memory { data, fpos: 0 })
    }

    fn new(backend: Backend) -> Stream {
        Stream {
            backend,
            buf: Vec::new(),
            readpos: 0,
            writepos: 0,
            position: 0,
            eof: false,
            notices: Vec::new(),
        }
    }

    /// Notices produced by the stream layer since the last call.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    fn buffered(&self) -> bool {
        matches!(self.backend, Backend::File(_))
    }

    /// `stream->ops->read` into `out`. `None` is a read error (`ret < 0`).
    fn raw_read(&mut self, out: &mut [u8]) -> Option<usize> {
        match &mut self.backend {
            Backend::Memory { data, fpos } => {
                let len = data.len() as u64;
                if *fpos >= len {
                    self.eof = true;
                    return Some(0);
                }
                let n = (out.len() as u64).min(len - *fpos) as usize;
                let start = *fpos as usize;
                out[..n].copy_from_slice(&data.as_bytes()[start..start + n]);
                *fpos += n as u64;
                Some(n)
            }
            Backend::File(file) => {
                let mut res = file.read(out);
                if matches!(&res, Err(e) if e.kind() == ErrorKind::Interrupted) {
                    res = file.read(out);
                }
                match res {
                    Ok(0) => {
                        self.eof = true;
                        Some(0)
                    }
                    Ok(n) => Some(n),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => Some(0),
                    Err(e) if e.kind() == ErrorKind::Interrupted => None,
                    Err(e) => {
                        let errno = e.raw_os_error().unwrap_or(0);
                        self.notices.push(format!(
                            "Read of {} bytes failed with errno={} {}",
                            out.len(),
                            errno,
                            strerror(&e)
                        ));
                        // EBADF (9) does not mark the stream as exhausted.
                        if errno != 9 {
                            self.eof = true;
                        }
                        None
                    }
                }
            }
        }
    }

    /// `php_stream_fill_read_buffer` for an unfiltered stream. `false` on failure.
    fn fill_read_buffer(&mut self, size: usize) -> bool {
        if self.writepos - self.readpos >= size {
            return true;
        }
        // Reduce buffer memory consumption (drop the consumed prefix).
        if self.readpos > 0 {
            self.buf.copy_within(self.readpos..self.writepos, 0);
            self.writepos -= self.readpos;
            self.readpos = 0;
        }
        if self.buf.len() < self.writepos + CHUNK_SIZE {
            self.buf.resize(self.writepos + CHUNK_SIZE, 0);
        }
        let start = self.writepos;
        let mut tmp = std::mem::take(&mut self.buf);
        let res = self.raw_read(&mut tmp[start..start + CHUNK_SIZE]);
        self.buf = tmp;
        match res {
            Some(n) => {
                self.writepos += n;
                true
            }
            None => false,
        }
    }

    /// `php_stream_read`: reads up to `out.len()` bytes (greedy for files and
    /// memory streams). Returns the number of bytes read, or -1 on error.
    pub fn read(&mut self, out: &mut [u8]) -> isize {
        let mut size = out.len();
        let mut off = 0usize;
        let mut didread: isize = 0;
        while size > 0 {
            if self.writepos > self.readpos {
                let toread = (self.writepos - self.readpos).min(size);
                out[off..off + toread]
                    .copy_from_slice(&self.buf[self.readpos..self.readpos + toread]);
                self.readpos += toread;
                size -= toread;
                off += toread;
                didread += toread as isize;
            }
            if size == 0 {
                break;
            }
            let toread;
            if !self.buffered() {
                match self.raw_read(&mut out[off..off + size]) {
                    Some(n) => toread = n,
                    None => {
                        if didread == 0 {
                            return -1;
                        }
                        break;
                    }
                }
            } else {
                if !self.fill_read_buffer(size) {
                    if didread == 0 {
                        return -1;
                    }
                    break;
                }
                toread = (self.writepos - self.readpos).min(size);
                if toread > 0 {
                    out[off..off + toread]
                        .copy_from_slice(&self.buf[self.readpos..self.readpos + toread]);
                    self.readpos += toread;
                }
            }
            if toread > 0 {
                didread += toread as isize;
                off += toread;
                size -= toread;
            } else {
                // EOF.
                break;
            }
        }
        if didread > 0 {
            self.position += didread as i64;
        }
        didread
    }

    /// Reads exactly `N` bytes (`php_stream_read(...) == N`).
    pub fn read_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut a = [0u8; N];
        if self.read(&mut a) == N as isize {
            Some(a)
        } else {
            None
        }
    }

    /// `php_stream_getc`: the next byte, or -1 (`EOF`).
    pub fn getc(&mut self) -> i32 {
        let mut b = [0u8; 1];
        if self.read(&mut b) > 0 {
            b[0] as i32
        } else {
            -1
        }
    }

    /// `stream->ops->seek(stream, offset, SEEK_SET, &stream->position)`.
    fn raw_seek_set(&mut self, offset: i64) -> i32 {
        match &mut self.backend {
            Backend::Memory { fpos, .. } => {
                if offset < 0 {
                    *fpos = 0;
                    self.position = -1;
                    -1
                } else {
                    *fpos = offset as u64;
                    self.position = offset;
                    self.eof = false;
                    0
                }
            }
            Backend::File(file) => {
                if offset < 0 {
                    return -1;
                }
                match file.seek(SeekFrom::Start(offset as u64)) {
                    Ok(_) => {
                        self.position = offset;
                        self.eof = false;
                        0
                    }
                    Err(_) => -1,
                }
            }
        }
    }

    fn seek_absolute(&mut self, offset: i64) -> i32 {
        let ret = self.raw_seek_set(offset);
        // Invalidate the buffer contents.
        self.readpos = 0;
        self.writepos = 0;
        ret
    }

    /// `php_stream_seek(stream, offset, SEEK_CUR)`. 0 on success.
    pub fn seek_cur(&mut self, offset: i64) -> i32 {
        if self.buffered() && offset > 0 && offset <= (self.writepos - self.readpos) as i64 {
            self.readpos += offset as usize;
            self.position += offset;
            self.eof = false;
            return 0;
        }
        let target = if offset > i64::MAX.saturating_sub(self.position) {
            i64::MAX
        } else {
            self.position.wrapping_add(offset)
        };
        self.seek_absolute(target)
    }

    /// `php_stream_seek(stream, offset, SEEK_SET)`. 0 on success.
    pub fn seek_set(&mut self, offset: i64) -> i32 {
        if self.buffered()
            && offset > self.position
            && offset <= self.position + (self.writepos - self.readpos) as i64
        {
            self.readpos += (offset - self.position) as usize;
            self.position = offset;
            self.eof = false;
            return 0;
        }
        if offset < 0 {
            return -1;
        }
        self.seek_absolute(offset)
    }

    /// `php_stream_rewind`.
    pub fn rewind(&mut self) -> i32 {
        self.seek_set(0)
    }

    /// `php_stream_eof`.
    pub fn eof(&self) -> bool {
        if self.writepos > self.readpos {
            return false;
        }
        self.eof
    }

    /// `php_stream_gets(stream, NULL, 0)`: the next line including its `\n`,
    /// or `None` at the end of the stream.
    pub fn gets(&mut self) -> Option<Vec<u8>> {
        let mut line: Vec<u8> = Vec::new();
        loop {
            let avail = self.writepos - self.readpos;
            if avail > 0 {
                let window = &self.buf[self.readpos..self.writepos];
                let (cpysz, done) = match window.iter().position(|&b| b == b'\n') {
                    Some(i) => (i + 1, true),
                    None => (avail, false),
                };
                line.extend_from_slice(&window[..cpysz]);
                self.position += cpysz as i64;
                self.readpos += cpysz;
                if done {
                    break;
                }
            } else if self.eof {
                break;
            } else {
                // A failure that is not fatal simply leaves the buffer empty.
                self.fill_read_buffer(CHUNK_SIZE);
                if self.writepos == self.readpos {
                    break;
                }
            }
        }
        if line.is_empty() { None } else { Some(line) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(data: &[u8]) -> Stream {
        Stream::from_memory(PhpStr::from(data))
    }

    #[test]
    fn memory_read_seek_eof() {
        let mut s = mem(b"abcdef");
        assert_eq!(s.getc(), b'a' as i32);
        let mut b = [0u8; 3];
        assert_eq!(s.read(&mut b), 3);
        assert_eq!(&b, b"bcd");
        assert!(!s.eof());
        let mut big = [0u8; 8];
        assert_eq!(s.read(&mut big), 2);
        assert!(s.eof());
        assert_eq!(s.getc(), -1);
        assert_eq!(s.rewind(), 0);
        assert!(!s.eof());
        assert_eq!(s.seek_cur(2), 0);
        assert_eq!(s.getc(), b'c' as i32);
        // Forward seeks past the end succeed; reads then return nothing.
        assert_eq!(s.seek_cur(100), 0);
        assert_eq!(s.getc(), -1);
    }

    #[test]
    fn memory_negative_seek_breaks_position() {
        let mut s = mem(b"abcdef");
        assert_eq!(s.seek_cur(-1), -1);
        // php-src leaves stream->position at -1 after a failed memory seek.
        assert_eq!(s.position, -1);
    }

    #[test]
    fn gets_splits_lines() {
        let mut s = mem(b"one\ntwo\nthree");
        assert_eq!(s.gets().as_deref(), Some(&b"one\n"[..]));
        assert_eq!(s.gets().as_deref(), Some(&b"two\n"[..]));
        assert_eq!(s.gets().as_deref(), Some(&b"three"[..]));
        assert_eq!(s.gets(), None);
    }

    #[test]
    fn file_stream_buffers_and_seeks() {
        let path =
            std::env::temp_dir().join(format!("php_image_stream_{}.bin", std::process::id()));
        let data: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut s = Stream::from_file(File::open(&path).unwrap());
        let mut b = [0u8; 10];
        assert_eq!(s.read(&mut b), 10);
        assert_eq!(&b[..], &data[..10]);
        assert_eq!(s.seek_cur(9000), 0);
        assert_eq!(s.read(&mut b), 10);
        assert_eq!(&b[..], &data[9010..9020]);
        assert_eq!(s.seek_set(19995), 0);
        let mut big = [0u8; 10];
        assert_eq!(s.read(&mut big), 5);
        assert!(s.eof());
        assert_eq!(s.rewind(), 0);
        assert_eq!(s.getc(), data[0] as i32);
        let _ = std::fs::remove_file(&path);
    }
}
