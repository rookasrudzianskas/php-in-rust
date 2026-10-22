//! main/streams/memory.c — `php://memory`, `php://temp` and the RFC 2397
//! `data:` wrapper.

use std::any::Any;

use php_runtime::{PResult, PhpArray, PhpStr, ResRef, Value, Vm};

use super::stream::{
    self, CastAs, OptParam, StatBuf, StreamHandle, StreamKind, StreamOps, StreamState, alloc,
    flags, opt,
};
use super::wrappers::{self, Wrapper};

pub const TEMP_STREAM_DEFAULT: i32 = 0x0;
pub const TEMP_STREAM_READONLY: i32 = 0x1;
pub const TEMP_STREAM_APPEND: i32 = 0x4;
/// `PHP_STREAM_MAX_MEM`.
pub const MAX_MEM: usize = 2 * 1024 * 1024;

/// `php_stream_mode_from_str`.
pub fn mode_from_str(mode: &[u8]) -> i32 {
    if mode.contains(&b'a') {
        TEMP_STREAM_APPEND
    } else if mode.contains(&b'w') || mode.contains(&b'+') {
        TEMP_STREAM_DEFAULT
    } else {
        TEMP_STREAM_READONLY
    }
}

/// `_php_stream_mode_to_str`.
pub fn mode_to_str(mode: i32) -> &'static [u8] {
    if mode == TEMP_STREAM_READONLY {
        b"rb"
    } else if mode == TEMP_STREAM_APPEND {
        b"a+b"
    } else {
        b"w+b"
    }
}

/// `php_stream_memory_data`.
pub struct MemoryOps {
    pub data: Vec<u8>,
    pub fpos: usize,
    pub mode: i32,
}

impl StreamOps for MemoryOps {
    fn label(&self) -> &'static str {
        "MEMORY"
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Memory
    }

    fn write(&mut self, _vm: &mut Vm, _st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        if self.mode & TEMP_STREAM_READONLY != 0 {
            return Ok(-1);
        }
        if self.mode & TEMP_STREAM_APPEND != 0 {
            self.fpos = self.data.len();
        }
        if self.fpos + buf.len() > self.data.len() {
            self.data.resize(self.fpos + buf.len(), 0);
        }
        self.data[self.fpos..self.fpos + buf.len()].copy_from_slice(buf);
        self.fpos += buf.len();
        Ok(buf.len() as isize)
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        if self.fpos >= self.data.len() {
            st.eof = true;
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - self.fpos);
        buf[..n].copy_from_slice(&self.data[self.fpos..self.fpos + n]);
        self.fpos += n;
        Ok(n as isize)
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
        let len = self.data.len() as i64;
        let cur = self.fpos as i64;
        let ok = |s: &mut Self, st: &mut StreamState, p: i64| {
            s.fpos = p as usize;
            st.eof = false;
            st.fatal_error = false;
            (0, Some(p))
        };
        Ok(match whence {
            1 => {
                if offset < 0 && cur < -offset {
                    self.fpos = 0;
                    (-1, Some(-1))
                } else {
                    ok(self, st, cur + offset)
                }
            }
            0 => {
                if offset < 0 {
                    self.fpos = 0;
                    (-1, Some(-1))
                } else {
                    ok(self, st, offset)
                }
            }
            2 => {
                if offset > 0 {
                    ok(self, st, len + offset)
                } else if len < -offset {
                    self.fpos = 0;
                    (-1, Some(-1))
                } else {
                    ok(self, st, len + offset)
                }
            }
            _ => (-1, Some(cur)),
        })
    }

    fn stat(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        let mode = if self.mode & TEMP_STREAM_READONLY != 0 {
            0o444
        } else {
            0o666
        };
        Ok(Some(StatBuf {
            mode: mode | 0o100000,
            size: self.data.len() as i64,
            nlink: 1,
            rdev: -1,
            dev: 0xC,
            blksize: -1,
            blocks: -1,
            ..StatBuf::default()
        }))
    }

    fn set_option(
        &mut self,
        _vm: &mut Vm,
        _st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        if option == opt::TRUNCATE_API {
            match value {
                opt::TRUNCATE_SUPPORTED => return Ok(opt::RETURN_OK),
                opt::TRUNCATE_SET_SIZE => {
                    if self.mode & TEMP_STREAM_READONLY != 0 {
                        return Ok(opt::RETURN_ERR);
                    }
                    let OptParam::Size(n) = param else {
                        return Ok(opt::RETURN_ERR);
                    };
                    let n = *n as usize;
                    self.data.resize(n, 0);
                    if n < self.fpos {
                        self.fpos = n;
                    }
                    return Ok(opt::RETURN_OK);
                }
                _ => {}
            }
        }
        Ok(opt::RETURN_NOTIMPL)
    }
}

/// `php_stream_memory_create`.
pub fn memory_create(vm: &mut Vm, mode: i32) -> ResRef {
    let res = alloc(
        vm,
        Box::new(MemoryOps {
            data: Vec::new(),
            fpos: 0,
            mode,
        }),
        mode_to_str(mode),
    );
    if let Some(h) = stream::handle_of(&res) {
        h.borrow_mut().st.flags |= flags::NO_BUFFER;
    }
    res
}

/// `php_stream_memory_open`: a memory stream over `data`.
pub fn memory_open(vm: &mut Vm, mode: i32, data: Vec<u8>) -> ResRef {
    let res = memory_create(vm, mode);
    if let Some(h) = stream::handle_of(&res) {
        if let Some(m) = h.borrow_mut().ops_as::<MemoryOps>() {
            m.data = data;
        }
    }
    res
}

/// `php_stream_temp_data`.
pub struct TempOps {
    /// The enclosed memory (later: file) stream and its resource.
    pub inner: Option<(ResRef, StreamHandle)>,
    pub smax: usize,
    pub mode: i32,
    pub meta: Option<PhpArray>,
    pub tmpdir: Option<PhpStr>,
    /// `php_stream_rfc2397_ops` (read-only "RFC2397" label).
    pub rfc2397: bool,
}

impl TempOps {
    fn inner(&self) -> Option<StreamHandle> {
        self.inner.as_ref().map(|(_, h)| h.clone())
    }

    fn inner_is_memory(&self) -> bool {
        self.inner()
            .is_some_and(|h| h.borrow().kind() == StreamKind::Memory)
    }
}

impl StreamOps for TempOps {
    fn label(&self) -> &'static str {
        if self.rfc2397 { "RFC2397" } else { "TEMP" }
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Temp
    }

    fn can_write(&self) -> bool {
        !self.rfc2397
    }

    fn write(&mut self, vm: &mut Vm, _st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        let Some(h) = self.inner() else {
            return Ok(-1);
        };
        if self.inner_is_memory() {
            let pos = h.borrow().tell();
            if pos as usize + buf.len() >= self.smax {
                let membuf = h
                    .borrow_mut()
                    .ops_as::<MemoryOps>()
                    .map(|m| m.data.clone())
                    .unwrap_or_default();
                let dir = self.tmpdir.clone();
                let Some(file) = super::plain::fopen_temporary_file(
                    vm,
                    dir.as_ref().map(|d| d.as_bytes()),
                    b"php",
                )?
                else {
                    vm.warning(
                        "Unable to create temporary file, Check permissions in temporary files directory.",
                    )?;
                    return Ok(0);
                };
                let fh = stream::handle_of(&file).expect("fresh");
                {
                    let mut f = fh.borrow_mut();
                    f.write(vm, &membuf)?;
                    f.seek(vm, pos, 0)?;
                }
                h.borrow_mut().free(vm, stream::PHP_STREAM_FREE_CLOSE)?;
                self.inner = Some((file, fh));
            }
        }
        let h = self.inner().expect("inner");
        let mut s = h.borrow_mut();
        s.write(vm, buf)
    }

    fn read(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        let Some(h) = self.inner() else {
            return Ok(-1);
        };
        let mut s = h.borrow_mut();
        let got = s.read(vm, buf)?;
        st.eof = s.st.eof;
        Ok(got)
    }

    fn close(&mut self, vm: &mut Vm, _st: &mut StreamState, close_handle: bool) -> PResult<i32> {
        let Some((res, h)) = self.inner.take() else {
            return Ok(0);
        };
        let r = h.borrow_mut().free(
            vm,
            stream::PHP_STREAM_FREE_CLOSE
                | if close_handle {
                    0
                } else {
                    stream::PHP_STREAM_FREE_PRESERVE_HANDLE
                },
        )?;
        res.close();
        Ok(r)
    }

    fn flush(&mut self, vm: &mut Vm, _st: &mut StreamState) -> PResult<i32> {
        match self.inner() {
            Some(h) => h.borrow_mut().flush(vm),
            None => Ok(-1),
        }
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        let Some(h) = self.inner() else {
            return Ok((-1, Some(-1)));
        };
        let mut s = h.borrow_mut();
        let ret = s.seek(vm, offset, whence)?;
        let pos = s.tell();
        st.eof = s.st.eof;
        Ok((ret, Some(pos)))
    }

    fn cast_fd(&mut self, as_: CastAs) -> Option<std::os::fd::RawFd> {
        let h = self.inner()?;
        let mut s = h.borrow_mut();
        if s.kind() == StreamKind::Stdio {
            return s.cast_fd(as_);
        }
        None
    }

    fn stat(&mut self, vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        match self.inner() {
            Some(h) => h.borrow_mut().stat(vm),
            None => Ok(None),
        }
    }

    fn set_option(
        &mut self,
        vm: &mut Vm,
        _st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        if option == opt::META_DATA_API {
            if let (Some(meta), OptParam::Meta(a)) = (&self.meta, param) {
                for (k, v) in meta.iter() {
                    a.insert(k.clone(), v.clone());
                }
            }
            return Ok(opt::RETURN_OK);
        }
        match self.inner() {
            Some(h) => h.borrow_mut().set_option(vm, option, value, param),
            None => Ok(opt::RETURN_NOTIMPL),
        }
    }
}

/// `php_stream_temp_create_ex`.
pub fn temp_create(vm: &mut Vm, mode: i32, max_memory: usize, tmpdir: Option<PhpStr>) -> ResRef {
    let res = alloc(
        vm,
        Box::new(TempOps {
            inner: None,
            smax: max_memory,
            mode,
            meta: None,
            tmpdir,
            rfc2397: false,
        }),
        mode_to_str(mode),
    );
    let inner = memory_create(vm, mode);
    let ih = stream::handle_of(&inner).expect("fresh");
    if let Some(h) = stream::handle_of(&res) {
        let mut s = h.borrow_mut();
        s.st.flags |= flags::NO_BUFFER;
        if let Some(t) = s.ops_as::<TempOps>() {
            t.inner = Some((inner, ih));
        }
    }
    res
}

/// `php_stream_temp_open`: a temp stream holding `data`, rewound.
pub fn temp_open(vm: &mut Vm, mode: i32, max_memory: usize, data: &[u8]) -> PResult<ResRef> {
    let res = temp_create(vm, mode, max_memory, None);
    if let Some(h) = stream::handle_of(&res) {
        let mut s = h.borrow_mut();
        if !data.is_empty() {
            let stream::Stream { ops, st, .. } = &mut *s;
            ops.write(vm, st, data)?;
            ops.seek(vm, st, 0, 0)?;
        }
        if let Some(t) = s.ops_as::<TempOps>() {
            t.mode = mode;
        }
    }
    Ok(res)
}

/// `php_url_decode`.
pub fn url_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < s.len()
                && s[i + 1].is_ascii_hexdigit()
                && s[i + 2].is_ascii_hexdigit() =>
            {
                let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(h(s[i + 1]) * 16 + h(s[i + 2]));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// `php_base64_decode_ex(str, len, strict=1)`.
pub fn base64_decode_strict(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut i = 0;
    let mut padding = 0;
    let mut j = 0usize;
    while i < input.len() {
        let c = input[i];
        i += 1;
        if c == b'=' {
            padding += 1;
            continue;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => {
                // strict mode skips whitespace only
                if matches!(c, b'\t' | b'\n' | b'\r' | b' ') {
                    continue;
                }
                return None;
            }
        };
        if padding > 0 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        j += 1;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // fail if the input is truncated (only one char in last group)
    if j % 4 == 1 {
        return None;
    }
    // fail if the padding length is wrong (not VV==, VVV=), but accept zero padding
    if padding > 0 && (padding > 2 || !(j + padding).is_multiple_of(4)) {
        return None;
    }
    Some(out)
}

/// The RFC 2397 `data:` wrapper.
pub struct DataWrapper;

impl Wrapper for DataWrapper {
    fn label(&self) -> &'static str {
        "RFC2397"
    }

    fn is_url(&self) -> bool {
        true
    }

    fn open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        mode: &[u8],
        options: i32,
        _opened_path: &mut Option<PhpStr>,
        _ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let fail = |vm: &mut Vm, msg: &str| -> PResult<Option<ResRef>> {
            wrappers::log_error(vm, "RFC2397", options, msg.to_string())?;
            Ok(None)
        };
        if !path.starts_with(b"data:") {
            return Ok(None);
        }
        let mut p = &path[5..];
        if p.len() >= 2 && p[0] == b'/' && p[1] == b'/' {
            p = &p[2..];
        }
        let Some(comma) = p.iter().position(|&c| c == b',') else {
            return fail(vm, "rfc2397: no comma in URL");
        };
        let mut meta = PhpArray::new();
        let mut base64 = false;
        if comma != 0 {
            let mut m = &p[..comma];
            let semi = m.iter().position(|&c| c == b';');
            let sep = m.iter().position(|&c| c == b'/');
            if semi.is_none() && sep.is_none() {
                return fail(vm, "rfc2397: illegal media type");
            }
            match semi {
                None => {
                    meta.insert_str("mediatype", Value::bytes(m));
                    m = &m[m.len()..];
                }
                Some(s) if sep.is_some_and(|sp| sp < s) => {
                    meta.insert_str("mediatype", Value::bytes(&m[..s]));
                    m = &m[s..];
                }
                Some(s) => {
                    if s != 0 || m != b";base64" {
                        return fail(vm, "rfc2397: illegal media type");
                    }
                }
            }
            // parameters and ';base64'
            while m.first() == Some(&b';') {
                m = &m[1..];
                let eq = m.iter().position(|&c| c == b'=');
                let semi = m.iter().position(|&c| c == b';');
                if eq.is_none() || semi.is_some_and(|s| s < eq.unwrap_or(0)) {
                    if m != b"base64" {
                        return fail(vm, "rfc2397: illegal parameter");
                    }
                    base64 = true;
                    m = &m[m.len()..];
                    break;
                }
                let eq = eq.expect("checked");
                let vend = semi.unwrap_or(m.len());
                let name = &m[..eq];
                let value = &m[eq + 1..vend];
                if name != b"mediatype" {
                    meta.insert(php_runtime::ArrayKey::from_bytes(name), Value::bytes(value));
                }
                m = &m[vend..];
            }
            if !m.is_empty() {
                return fail(vm, "rfc2397: illegal URL");
            }
        }
        meta.insert_str("base64", base64);
        let data = &p[comma + 1..];
        let decoded = if base64 {
            match base64_decode_strict(data) {
                Some(d) => d,
                None => return fail(vm, "rfc2397: unable to decode"),
            }
        } else {
            url_decode(data)
        };
        let res = temp_create(vm, 0, usize::MAX, None);
        let h = stream::handle_of(&res).expect("fresh");
        {
            let mut s = h.borrow_mut();
            {
                let stream::Stream { ops, st, .. } = &mut *s;
                ops.write(vm, st, &decoded)?;
                ops.seek(vm, st, 0, 0)?;
            }
            let m = if mode.len() > 15 { &mode[..15] } else { mode };
            s.st.mode = PhpStr::from_bytes(m);
            if let Some(t) = s.ops_as::<TempOps>() {
                t.rfc2397 = true;
                t.mode = if mode.first() == Some(&b'r') && mode.get(1) != Some(&b'+') {
                    TEMP_STREAM_READONLY
                } else {
                    0
                };
                t.meta = Some(meta);
            }
        }
        Ok(Some(res))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
