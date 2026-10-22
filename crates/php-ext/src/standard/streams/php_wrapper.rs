//! ext/standard/php_fopen_wrapper.c — the `php://` wrapper.

use std::any::Any;
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};

use php_runtime::{PResult, PhpStr, ResRef, Value, Vm};

use super::memory::{self, MAX_MEM};
use super::plain::{self, StdioOps};
use super::stream::{self, StreamKind, StreamOps, StreamState, alloc};
use super::wrappers::{self, Wrapper};
use super::{context, filter, sys};

/// `php://output`: writes go through the output-buffering layer.
pub struct OutputOps;

impl StreamOps for OutputOps {
    fn label(&self) -> &'static str {
        "Output"
    }

    fn write(&mut self, vm: &mut Vm, _st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        vm.echo(buf)?;
        Ok(buf.len() as isize)
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        st.eof = true;
        Ok(-1)
    }
}

/// `php://input`: the request body (always empty for the CLI).
pub struct InputOps {
    position: i64,
}

impl StreamOps for InputOps {
    fn label(&self) -> &'static str {
        "Input"
    }

    fn can_write(&self) -> bool {
        true
    }

    fn write(&mut self, _vm: &mut Vm, _st: &mut StreamState, _buf: &[u8]) -> PResult<isize> {
        Ok(-1)
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        st.eof = true;
        Ok(0)
    }

    fn flush(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<i32> {
        Ok(-1)
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        _vm: &mut Vm,
        _st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        // the body is empty: only offsets that land on 0 succeed
        let target = match whence {
            0 | 2 => offset,
            _ => self.position + offset,
        };
        if target < 0 {
            return Ok((-1, Some(self.position)));
        }
        self.position = 0;
        Ok((0, Some(0)))
    }
}

/// Which standard descriptor a `php://std*` stream duplicates.
fn dup_std(which: u8) -> std::io::Result<OwnedFd> {
    match which {
        0 => std::io::stdin().as_fd().try_clone_to_owned(),
        1 => std::io::stdout().as_fd().try_clone_to_owned(),
        _ => std::io::stderr().as_fd().try_clone_to_owned(),
    }
}

/// Opens `php://stdin|stdout|stderr` (fd 0, 1, 2) as a STDIO stream.
pub fn open_std(vm: &mut Vm, which: u8, mode: &[u8]) -> Option<ResRef> {
    let fd = dup_std(which).ok()?;
    let res = plain::fopen_from_fd(vm, File::from(fd), mode, false);
    if let Some(h) = stream::handle_of(&res) {
        let mut s = h.borrow_mut();
        if let Some(o) = s.ops_as::<StdioOps>() {
            o.sync_output = which != 0;
            o.has_stdio = true;
        }
    }
    Some(res)
}

pub struct PhpWrapper;

/// `php_stream_apply_filter_list`: `false` when the filter limit was hit.
fn apply_filter_list(
    vm: &mut Vm,
    res: &ResRef,
    list: &[u8],
    read_chain: bool,
    write_chain: bool,
    ctx: Option<&ResRef>,
) -> PResult<bool> {
    let mut max_filter_count: i64 = 16;
    if let Some(v) = context::option(ctx, "filter", "max_filter_count") {
        max_filter_count = vm.to_int(&v)?;
    }
    let h = stream::handle_of(res).expect("open stream");
    for p in list.split(|&c| c == b'|').filter(|p| !p.is_empty()) {
        let (rc, wc) = {
            let s = h.borrow();
            (
                if read_chain {
                    s.readfilters.len() as i64
                } else {
                    0
                },
                if write_chain {
                    s.writefilters.len() as i64
                } else {
                    0
                },
            )
        };
        if rc == max_filter_count || wc == max_filter_count {
            return Ok(false);
        }
        let name = memory::url_decode(p);
        for (enabled, is_read) in [(read_chain, true), (write_chain, false)] {
            if !enabled {
                continue;
            }
            match filter::create_filter(vm, &name, &Value::Null, false)? {
                Some(f) => {
                    let mut s = h.borrow_mut();
                    s.res = Some(res.clone());
                    s.attach_filter(vm, f, is_read, true)?;
                    s.res = None;
                }
                None => {
                    vm.warning(format!(
                        "Unable to create filter ({})",
                        String::from_utf8_lossy(&name)
                    ))?;
                }
            }
        }
    }
    Ok(true)
}

impl Wrapper for PhpWrapper {
    fn label(&self) -> &'static str {
        "PHP"
    }

    fn open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        mode: &[u8],
        options: i32,
        opened_path: &mut Option<PhpStr>,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let mut path = path;
        if path.len() >= 6 && path[..6].eq_ignore_ascii_case(b"php://") {
            path = &path[6..];
        }
        let lower = path.to_ascii_lowercase();
        if lower.starts_with(b"temp") {
            let rest = &path[4..];
            let mut max_memory = MAX_MEM as i64;
            if rest.len() >= 11 && rest[..11].eq_ignore_ascii_case(b"/maxmemory:") {
                let (v, _) = php_runtime::ini::strtol(&rest[11..], 10);
                max_memory = v;
                if max_memory < 0 {
                    return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
                }
            }
            let mode_rw = memory::mode_from_str(mode);
            return Ok(Some(memory::temp_create(
                vm,
                mode_rw,
                max_memory as usize,
                None,
            )));
        }
        if lower == b"memory" {
            let mode_rw = memory::mode_from_str(mode);
            return Ok(Some(memory::memory_create(vm, mode_rw)));
        }
        if lower == b"output" {
            return Ok(Some(alloc(vm, Box::new(OutputOps), b"wb")));
        }
        let for_include_denied = options & stream::open::STREAM_OPEN_FOR_INCLUDE != 0
            && !vm.ini_bool("allow_url_include");
        if lower == b"input" {
            if for_include_denied {
                wrappers::wrapper_warn(
                    vm,
                    options,
                    "URL file-access is disabled in the server configuration",
                )?;
                return Ok(None);
            }
            return Ok(Some(alloc(vm, Box::new(InputOps { position: 0 }), b"rb")));
        }
        let fd: OwnedFd;
        let mut std_which = None;
        if lower == b"stdin" || lower == b"stdout" || lower == b"stderr" {
            if for_include_denied && lower == b"stdin" {
                wrappers::wrapper_warn(
                    vm,
                    options,
                    "URL file-access is disabled in the server configuration",
                )?;
                return Ok(None);
            }
            let which = match lower.as_slice() {
                b"stdin" => 0,
                b"stdout" => 1,
                _ => 2,
            };
            std_which = Some(which);
            match dup_std(which) {
                Ok(f) => fd = f,
                Err(_) => return Ok(None),
            }
        } else if lower.starts_with(b"fd/") {
            if for_include_denied {
                wrappers::wrapper_warn(
                    vm,
                    options,
                    "URL file-access is disabled in the server configuration",
                )?;
                return Ok(None);
            }
            let start = &path[3..];
            let digits = start
                .iter()
                .enumerate()
                .take_while(|&(i, &c)| c.is_ascii_digit() || (i == 0 && (c == b'-' || c == b'+')))
                .count();
            let ws = start.iter().take_while(|c| c.is_ascii_whitespace()).count();
            let num = &start[ws..];
            let nd = num
                .iter()
                .enumerate()
                .take_while(|&(i, &c)| c.is_ascii_digit() || (i == 0 && (c == b'-' || c == b'+')))
                .count();
            let _ = digits;
            if nd == 0 || !num[..nd].iter().any(|c| c.is_ascii_digit()) || ws + nd != start.len() {
                wrappers::log_error(
                    vm,
                    "PHP",
                    options,
                    "php://fd/ stream must be specified in the form php://fd/<orig fd>".into(),
                )?;
                return Ok(None);
            }
            let (fildes, _) = php_runtime::ini::strtol(num, 10);
            let dtablesize = sys::sys_dtablesize();
            if fildes < 0 || fildes >= dtablesize as i64 {
                wrappers::log_error(
                    vm,
                    "PHP",
                    options,
                    format!(
                        "The file descriptors must be non-negative numbers smaller than {dtablesize}"
                    ),
                )?;
                return Ok(None);
            }
            match sys::sys_dup(fildes as i32) {
                Ok(f) => fd = f,
                Err(e) => {
                    let n = sys::errno_of(&e);
                    wrappers::log_error(
                        vm,
                        "PHP",
                        options,
                        format!(
                            "Error duping file descriptor {fildes}; possibly it doesn't exist: [{n}]: {}",
                            sys::strerror(n)
                        ),
                    )?;
                    return Ok(None);
                }
            }
        } else if lower.starts_with(b"filter/") {
            let mut mode_rw = 0;
            if mode.contains(&b'r') || mode.contains(&b'+') {
                mode_rw |= filter::FILTER_READ;
            }
            if mode.contains(&b'w') || mode.contains(&b'+') || mode.contains(&b'a') {
                mode_rw |= filter::FILTER_WRITE;
            }
            let pathdup = &path[6..];
            let Some(p) = pathdup.windows(10).position(|w| w == b"/resource=") else {
                return Err(vm.throw_error("Error", "No URL resource specified"));
            };
            let Some(res) = wrappers::open_wrapper(
                vm,
                &pathdup[p + 10..],
                mode,
                options,
                Some(opened_path),
                ctx,
            )?
            else {
                return Ok(None);
            };
            let spec = &pathdup[1..p.max(1)];
            let mut ok = true;
            for tok in spec.split(|&c| c == b'/').filter(|t| !t.is_empty()) {
                ok = if tok.len() >= 5 && tok[..5].eq_ignore_ascii_case(b"read=") {
                    apply_filter_list(vm, &res, &tok[5..], true, false, ctx)?
                } else if tok.len() >= 6 && tok[..6].eq_ignore_ascii_case(b"write=") {
                    apply_filter_list(vm, &res, &tok[6..], false, true, ctx)?
                } else {
                    apply_filter_list(
                        vm,
                        &res,
                        tok,
                        mode_rw & filter::FILTER_READ != 0,
                        mode_rw & filter::FILTER_WRITE != 0,
                        ctx,
                    )?
                };
                if !ok {
                    break;
                }
            }
            if !ok {
                wrappers::log_error(vm, "PHP", options, "too many filters".into())?;
                if let Some(h) = stream::handle_of(&res) {
                    h.borrow_mut().free(vm, stream::PHP_STREAM_FREE_CLOSE)?;
                }
                res.close();
                return Ok(None);
            }
            return Ok(Some(res));
        } else {
            wrappers::wrapper_warn(vm, options, "Invalid php:// URL specified")?;
            return Ok(None);
        }

        let file = File::from(fd);
        let is_sock = file
            .metadata()
            .map(|m| {
                use std::os::unix::fs::FileTypeExt;
                m.file_type().is_socket()
            })
            .unwrap_or(false);
        if is_sock {
            return Ok(Some(super::socket::from_fd(vm, file.into(), mode)));
        }
        let res = plain::fopen_from_fd(vm, file, mode, false);
        if let Some(which) = std_which {
            if let Some(h) = stream::handle_of(&res) {
                let mut s = h.borrow_mut();
                if let Some(o) = s.ops_as::<StdioOps>() {
                    o.sync_output = which != 0;
                    o.has_stdio = true;
                }
            }
        }
        Ok(Some(res))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Whether a stream is php://output-like (for `stream_get_meta_data`).
pub fn is_output(kind: StreamKind) -> bool {
    kind == StreamKind::Other
}
