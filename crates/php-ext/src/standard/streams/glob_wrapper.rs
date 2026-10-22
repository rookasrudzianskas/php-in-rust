//! main/streams/glob_wrapper.c — the `glob://` directory wrapper.

use std::any::Any;

use php_runtime::{PResult, PhpStr, ResRef, Vm};

use super::glob::{GLOB_NOMATCH, php_glob};
use super::plain::check_open_basedir;
use super::stream::{StreamKind, StreamOps, StreamState, alloc, open};
use super::wrappers::Wrapper;

/// `glob_s_t`.
pub struct GlobOps {
    paths: Vec<Vec<u8>>,
    index: usize,
    /// Directory part of the current entry (`php_glob_stream_get_path`).
    pub path: Option<Vec<u8>>,
    /// Pattern part after the last `/` (`php_glob_stream_get_pattern`).
    pub pattern: Vec<u8>,
}

impl GlobOps {
    /// `php_glob_stream_get_count`.
    pub fn count(&self) -> usize {
        self.paths.len()
    }
}

fn path_split(full: &[u8]) -> (Vec<u8>, Vec<u8>) {
    match full.iter().rposition(|&c| c == b'/') {
        Some(pos) => {
            let file = full[pos + 1..].to_vec();
            // keep a lone leading "/" as the path
            let plen = if pos + 1 > 1 { pos } else { pos + 1 };
            (full[..plen].to_vec(), file)
        }
        None => (Vec::new(), full.to_vec()),
    }
}

impl StreamOps for GlobOps {
    fn label(&self) -> &'static str {
        "glob"
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Dir
    }

    fn can_write(&self) -> bool {
        false
    }

    fn read(&mut self, _vm: &mut Vm, _st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        Ok(-1)
    }

    fn readdir(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<Vec<u8>>> {
        if self.index < self.paths.len() {
            let (dir, file) = path_split(&self.paths[self.index]);
            self.path = Some(dir);
            self.index += 1;
            return Ok(Some(file));
        }
        self.index = self.paths.len();
        self.path = None;
        Ok(None)
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        _vm: &mut Vm,
        _st: &mut StreamState,
        _offset: i64,
        _whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        self.index = 0;
        self.path = None;
        Ok((0, None))
    }
}

pub struct GlobWrapper;

impl Wrapper for GlobWrapper {
    fn label(&self) -> &'static str {
        "glob"
    }

    fn has_opener(&self) -> bool {
        false
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

    fn has_dir_opener(&self) -> bool {
        true
    }

    fn dir_open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        options: i32,
        _ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let path = path.strip_prefix(b"glob://").unwrap_or(path);
        let mut paths = match php_glob(path, 0) {
            Ok(r) => r.paths,
            Err((code, r)) if code == GLOB_NOMATCH => r.paths,
            Err(_) => return Ok(None),
        };
        if options & open::STREAM_DISABLE_OPEN_BASEDIR == 0 {
            let mut kept = Vec::with_capacity(paths.len());
            for p in paths {
                if !check_open_basedir(vm, &p, false)? {
                    kept.push(p);
                }
            }
            paths = kept;
        }
        let pattern = match path.iter().rposition(|&c| c == b'/') {
            Some(i) => path[i + 1..].to_vec(),
            None => path.to_vec(),
        };
        let first = paths.first().cloned().unwrap_or_else(|| path.to_vec());
        let (dir, _) = path_split(&first);
        Ok(Some(alloc(
            vm,
            Box::new(GlobOps {
                paths,
                index: 0,
                path: Some(dir),
                pattern,
            }),
            b"r",
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
