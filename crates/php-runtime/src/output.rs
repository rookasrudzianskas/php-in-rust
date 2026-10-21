//! Output buffering (port of `main/output.c`).
//!
//! All output (`echo`, `print`, error display, `printf`, ...) goes through
//! [`Vm::echo`], which feeds the handler stack top-down exactly like
//! `php_output_op`; whatever leaves the bottom handler is written to the
//! [`OutputSink`]. The `ob_*` functions map to the `Vm::ob_*` methods here.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

/// Handler operation and flag constants (`PHP_OUTPUT_HANDLER_*`).
pub mod ob {
    pub const WRITE: i64 = 0x00;
    pub const START: i64 = 0x01;
    pub const CLEAN: i64 = 0x02;
    pub const FLUSH: i64 = 0x04;
    pub const FINAL: i64 = 0x08;
    pub const CONT: i64 = WRITE;
    pub const END: i64 = FINAL;

    pub const INTERNAL: i64 = 0x0000;
    pub const USER: i64 = 0x0001;
    pub const CLEANABLE: i64 = 0x0010;
    pub const FLUSHABLE: i64 = 0x0020;
    pub const REMOVABLE: i64 = 0x0040;
    pub const STDFLAGS: i64 = 0x0070;
    pub const STARTED: i64 = 0x1000;
    pub const DISABLED: i64 = 0x2000;
    pub const PROCESSED: i64 = 0x4000;
    pub const PRODUCED_OUTPUT: i64 = 0x8000;

    /// `PHP_OUTPUT_HANDLER_ABILITY_FLAGS`.
    pub const fn ability_flags(f: i64) -> i64 {
        f & !0xf00f
    }

    pub(crate) const POP_TRY: u32 = 0x000;
    pub(crate) const POP_FORCE: u32 = 0x001;
    pub(crate) const POP_DISCARD: u32 = 0x010;
    pub(crate) const POP_SILENT: u32 = 0x100;

    pub const DEFAULT_HANDLER_NAME: &str = "default output handler";
    pub const DEVNULL_HANDLER_NAME: &str = "null output handler";

    pub(crate) const DEFAULT_SIZE: usize = 0x4000;
    pub(crate) const ALIGN: usize = 0x1000;
}

/// Destination of the final output (`sapi_module.ub_write`).
pub trait OutputSink {
    fn write(&mut self, data: &[u8]);
    fn flush(&mut self) {}
}

/// Writes to the process's stdout.
pub struct StdoutSink;

impl OutputSink for StdoutSink {
    fn write(&mut self, data: &[u8]) {
        let _ = std::io::stdout().write_all(data);
    }

    fn flush(&mut self) {
        let _ = std::io::stdout().flush();
    }
}

/// Collects output into a shared buffer (tests, embedding).
#[derive(Clone, Default)]
pub struct CaptureSink(pub Rc<RefCell<Vec<u8>>>);

impl CaptureSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    pub fn contents(&self) -> Vec<u8> {
        self.0.borrow().clone()
    }

    pub fn as_string(&self) -> String {
        String::from_utf8_lossy(&self.0.borrow()).into_owned()
    }
}

impl OutputSink for CaptureSink {
    fn write(&mut self, data: &[u8]) {
        self.0.borrow_mut().extend_from_slice(data);
    }
}

/// An internal output handler: receives the buffered data and the operation
/// flags; returns `Some(output)` or `None` on failure (buffer passes through).
pub type InternalOutputHandler = Rc<dyn Fn(&mut Vm, &[u8], i64) -> PResult<Option<Vec<u8>>>>;

/// The callback of an output handler.
#[derive(Clone)]
pub enum HandlerFunc {
    /// "default output handler": passes data through.
    Default,
    /// "null output handler": swallows everything.
    DevNull,
    /// A userland callable `function(string $buffer, int $phase): string|false`.
    User(Value),
    Internal(InternalOutputHandler),
}

/// One entry of the output handler stack (`php_output_handler`).
pub struct OutputHandler {
    pub name: PhpStr,
    pub flags: i64,
    pub level: usize,
    /// Chunk size (`0` = unlimited).
    pub size: usize,
    pub buffer: Vec<u8>,
    /// Emulated allocation size (reported as `buffer_size` by `ob_get_status`).
    pub buffer_size: usize,
    pub func: HandlerFunc,
}

fn initbuf_size(s: usize) -> usize {
    if s > 0 {
        s.div_ceil(ob::ALIGN) * ob::ALIGN
    } else {
        ob::DEFAULT_SIZE
    }
}

impl OutputHandler {
    pub fn new(
        name: impl Into<PhpStr>,
        chunk_size: usize,
        flags: i64,
        func: HandlerFunc,
    ) -> OutputHandler {
        OutputHandler {
            name: name.into(),
            flags,
            level: 0,
            size: chunk_size,
            buffer: Vec::new(),
            buffer_size: initbuf_size(chunk_size),
            func,
        }
    }

    /// `ob_get_status()` entry.
    pub fn status(&self) -> PhpArray {
        let mut a = PhpArray::new();
        a.insert_str("name", self.name.clone());
        a.insert_str("type", self.flags & 0xf);
        a.insert_str("flags", self.flags);
        a.insert_str("level", self.level as i64);
        a.insert_str("chunk_size", self.size as i64);
        a.insert_str("buffer_size", self.buffer_size as i64);
        a.insert_str("buffer_used", self.buffer.len() as i64);
        a
    }
}

/// Output state owned by the [`Vm`] (`OG(...)`).
pub struct OutputState {
    pub handlers: Vec<OutputHandler>,
    /// Level of the handler whose callback is running (`OG(running)`).
    pub running: Option<usize>,
    pub implicit_flush: bool,
    /// Output disabled (`PHP_OUTPUT_DISABLED`).
    pub disabled: bool,
    /// Handler stack active (`PHP_OUTPUT_ACTIVATED`); cleared by
    /// `php_output_deactivate`, after which output goes straight to the sink.
    pub activated: bool,
    /// Something was written to the sink (`PHP_OUTPUT_SENT`).
    pub sent: bool,
    pub sink: Box<dyn OutputSink>,
    /// Temporarily hidden top handlers (`php_output_flush` writes below the active one).
    hidden: usize,
}

impl OutputState {
    pub fn new(sink: Box<dyn OutputSink>) -> OutputState {
        OutputState {
            handlers: Vec::new(),
            running: None,
            implicit_flush: false,
            disabled: false,
            activated: true,
            sent: false,
            sink,
            hidden: 0,
        }
    }

    /// `ob_get_level()`.
    pub fn level(&self) -> usize {
        self.handlers.len()
    }

    fn visible(&self) -> usize {
        self.handlers.len() - self.hidden
    }

    fn active(&self) -> Option<&OutputHandler> {
        self.handlers.last()
    }
}

#[derive(Default)]
struct Ctx {
    op: i64,
    input: Vec<u8>,
    out: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Failure,
    Success,
    NoData,
}

impl Vm {
    /// Writes through the output-buffer stack (`php_output_write` / `PHPWRITE`).
    pub fn echo(&mut self, data: &[u8]) -> PResult<()> {
        if !self.output.activated {
            if !self.output.disabled && !data.is_empty() {
                self.output.sink.write(data);
            }
            return Ok(());
        }
        // Empty writes still reach a running handler (they count as produced output).
        if data.is_empty() && self.output.running.is_none() {
            return Ok(());
        }
        self.output_op(ob::WRITE, data)
    }

    /// `php_output_deactivate`: drops every handler (and its buffer) and
    /// sends further output directly to the sink.
    pub fn output_deactivate(&mut self) {
        self.output.activated = false;
        self.output.running = None;
        self.output.hidden = 0;
        self.output.handlers.clear();
    }

    /// Writes a Rust string through the output stack.
    pub fn echo_str(&mut self, s: &str) -> PResult<()> {
        self.echo(s.as_bytes())
    }

    /// Writes directly to the sink, bypassing buffers (`PHPWRITE_H`).
    pub fn write_unbuffered(&mut self, data: &[u8]) {
        self.output.sink.write(data);
        self.output.sent = true;
    }

    /// `flush()`: flushes the sink.
    pub fn sapi_flush(&mut self) {
        self.output.sink.flush();
    }

    fn lock_error(&mut self, op: i64) -> PResult<()> {
        if op != 0 && !self.output.handlers.is_empty() && self.output.running.is_some() {
            self.output_deactivate();
            return Err(self.php_error_docref_fatal(
                "Cannot use output buffering in output buffering display handlers",
            ));
        }
        Ok(())
    }

    fn php_error_docref_fatal(&mut self, msg: &str) -> Flow {
        match self.php_error_docref(crate::errors::level::E_ERROR, msg) {
            Err(f) => f,
            Ok(()) => Flow::Fatal,
        }
    }

    fn output_op(&mut self, op: i64, data: &[u8]) -> PResult<()> {
        self.lock_error(op)?;
        let mut ctx = Ctx {
            op,
            input: data.to_vec(),
            out: None,
        };
        let mut pending: Option<Flow> = None;
        let cnt = self.output.visible();
        if cnt > 0 {
            if cnt > 1 {
                for level in (0..cnt).rev() {
                    if level >= self.output.handlers.len()
                        || matches!(pending, Some(Flow::Fatal | Flow::Exit(_)))
                    {
                        break;
                    }
                    let was_disabled = self.output.handlers[level].flags & ob::DISABLED != 0;
                    let status = if was_disabled {
                        Status::Failure
                    } else {
                        self.handler_op(level, &mut ctx, &mut pending)?
                    };
                    match status {
                        Status::NoData => break,
                        Status::Success => {
                            if level > 0 {
                                swap(&mut ctx);
                            }
                        }
                        Status::Failure => {
                            if was_disabled {
                                if level == 0 {
                                    pass(&mut ctx);
                                }
                            } else if level > 0 {
                                swap(&mut ctx);
                            }
                        }
                    }
                }
            } else if self.output.handlers[0].flags & ob::DISABLED == 0 {
                self.handler_op(0, &mut ctx, &mut pending)?;
            } else {
                pass(&mut ctx);
            }
        } else {
            ctx.out = Some(ctx.input);
        }
        if let Some(out) = ctx.out {
            if !out.is_empty() && !self.output.disabled && !matches!(pending, Some(Flow::Fatal)) {
                self.output.sink.write(&out);
                if self.output.implicit_flush {
                    self.output.sink.flush();
                }
                self.output.sent = true;
            }
        }
        match pending {
            Some(f) => Err(f),
            None => Ok(()),
        }
    }

    /// `php_output_handler_append`: buffers input; returns `true` when the
    /// handler does not need to run yet.
    fn handler_append(&mut self, level: usize, input: &[u8]) -> bool {
        let running = self.output.running.is_some();
        let h = &mut self.output.handlers[level];
        if !input.is_empty() {
            let free = h.buffer_size.saturating_sub(h.buffer.len());
            if free <= input.len() {
                let grow_int = initbuf_size(h.size);
                let grow_buf = initbuf_size(input.len() - free);
                h.buffer_size += grow_int.max(grow_buf);
            }
            h.buffer.extend_from_slice(input);
            if h.size > 0 && h.buffer.len() >= h.size {
                return running;
            }
        }
        true
    }

    /// `php_output_handler_op`.
    fn handler_op(
        &mut self,
        level: usize,
        ctx: &mut Ctx,
        pending: &mut Option<Flow>,
    ) -> PResult<Status> {
        let original_op = ctx.op;
        if self.output.handlers[level].flags & ob::DISABLED != 0 {
            return Ok(Status::Failure);
        }
        self.lock_error(ctx.op)?;
        if ctx.op == ob::WRITE && self.output.running.is_some() {
            self.output.handlers[level].flags |= ob::PRODUCED_OUTPUT;
        }
        let input = std::mem::take(&mut ctx.input);
        if self.handler_append(level, &input) && ctx.op == ob::WRITE {
            return Ok(Status::NoData);
        }
        if self.output.handlers[level].flags & ob::STARTED == 0 {
            ctx.op |= ob::START;
        }
        self.output.running = Some(level);
        let func = self.output.handlers[level].func.clone();
        let data = self.output.handlers[level].buffer.clone();
        let mut still_have_handler = true;
        let status = match func {
            HandlerFunc::User(cb) => {
                let host = self.host.clone();
                let res = host.call_callable(
                    self,
                    &cb,
                    vec![Value::String(PhpStr::from_vec(data)), Value::Int(ctx.op)],
                );
                let present = level < self.output.handlers.len();
                match res {
                    Ok(retval) => {
                        if present && self.output.handlers[level].flags & ob::PRODUCED_OUTPUT != 0 {
                            self.output.handlers[level].flags |= ob::DISABLED;
                            self.output.handlers[level].flags &= !ob::PRODUCED_OUTPUT;
                            let name = self.output.handlers[level].name.clone();
                            let r = self.php_error_docref(
                                crate::errors::level::E_DEPRECATED,
                                format!(
                                    "Producing output from user output handler {name} is deprecated"
                                ),
                            );
                            if let Err(f) = r {
                                pending.get_or_insert(f);
                            }
                            if level < self.output.handlers.len() {
                                self.output.handlers[level].flags &= !ob::DISABLED;
                            } else {
                                still_have_handler = false;
                            }
                        }
                        match retval.unref() {
                            Value::Bool(false) => Status::Failure,
                            Value::Bool(true) => Status::NoData,
                            v => {
                                let s = match self.to_php_string(&v) {
                                    Ok(s) => s,
                                    Err(f) => {
                                        pending.get_or_insert(f);
                                        PhpStr::new()
                                    }
                                };
                                if s.is_empty() {
                                    Status::NoData
                                } else {
                                    ctx.out = Some(s.into_vec());
                                    Status::Success
                                }
                            }
                        }
                    }
                    Err(f) => {
                        pending.get_or_insert(f);
                        if !present {
                            still_have_handler = false;
                        }
                        Status::Failure
                    }
                }
            }
            HandlerFunc::Default => {
                ctx.out = Some(data);
                if ctx.out.as_ref().is_some_and(|o| !o.is_empty()) {
                    Status::Success
                } else {
                    Status::NoData
                }
            }
            HandlerFunc::DevNull => Status::NoData,
            HandlerFunc::Internal(f) => match f(self, &data, ctx.op) {
                Ok(Some(out)) => {
                    let empty = out.is_empty();
                    ctx.out = Some(out);
                    if empty {
                        Status::NoData
                    } else {
                        Status::Success
                    }
                }
                Ok(None) => Status::Failure,
                Err(e) => {
                    pending.get_or_insert(e);
                    Status::Failure
                }
            },
        };
        self.output.running = None;
        if !still_have_handler || level >= self.output.handlers.len() {
            ctx.op = original_op;
            return Ok(status);
        }
        let h = &mut self.output.handlers[level];
        h.flags |= ob::STARTED;
        match status {
            Status::Failure => {
                h.flags |= ob::DISABLED;
                ctx.out = Some(std::mem::take(&mut h.buffer));
                h.buffer_size = 0;
            }
            Status::NoData => {
                ctx.out = None;
                ctx.input.clear();
                h.buffer.clear();
                h.flags |= ob::PROCESSED;
            }
            Status::Success => {
                h.buffer.clear();
                h.flags |= ob::PROCESSED;
            }
        }
        ctx.op = original_op;
        Ok(status)
    }

    /// Pushes a handler (`php_output_handler_start`).
    pub fn ob_push(&mut self, mut h: OutputHandler) -> PResult<bool> {
        if !self.output.activated {
            return Ok(false);
        }
        self.lock_error(ob::START)?;
        h.level = self.output.handlers.len();
        self.output.handlers.push(h);
        Ok(true)
    }

    /// `ob_start($callback, $chunk_size, $flags)`. Returns `false` (after the
    /// "Failed to create buffer" notice) if the callback is invalid.
    pub fn ob_start(
        &mut self,
        callback: Option<&Value>,
        chunk_size: i64,
        flags: i64,
    ) -> PResult<bool> {
        let chunk = chunk_size.max(0) as usize;
        let handler = match callback.map(Value::unref) {
            None | Some(Value::Null) => Some(OutputHandler::new(
                ob::DEFAULT_HANDLER_NAME,
                chunk,
                ob::ability_flags(flags) | ob::INTERNAL,
                HandlerFunc::Default,
            )),
            Some(cb) => {
                let host = self.host.clone();
                match host.check_callable(self, &cb) {
                    Ok(name) => Some(OutputHandler::new(
                        name,
                        chunk,
                        ob::ability_flags(flags) | ob::USER,
                        HandlerFunc::User(cb),
                    )),
                    Err(err) => {
                        self.warning(err)?;
                        None
                    }
                }
            }
        };
        if let Some(h) = handler {
            if self.ob_push(h)? {
                return Ok(true);
            }
        }
        self.notice("Failed to create buffer")?;
        Ok(false)
    }

    /// `php_output_flush` (`ob_flush()` without the diagnostics).
    fn output_flush(&mut self) -> PResult<bool> {
        let Some(active) = self.output.active() else {
            return Ok(false);
        };
        if active.flags & ob::FLUSHABLE == 0 {
            return Ok(false);
        }
        let level = self.output.handlers.len() - 1;
        let mut ctx = Ctx {
            op: ob::FLUSH,
            ..Ctx::default()
        };
        let mut pending = None;
        self.handler_op(level, &mut ctx, &mut pending)?;
        if let Some(out) = ctx.out {
            if !out.is_empty() {
                self.output.hidden += 1;
                let r = self.output_op(ob::WRITE, &out);
                self.output.hidden -= 1;
                r?;
            }
        }
        match pending {
            Some(f) => Err(f),
            None => Ok(true),
        }
    }

    /// `ob_flush()`.
    pub fn ob_flush(&mut self) -> PResult<bool> {
        let Some(active) = self.output.active() else {
            self.notice("Failed to flush buffer. No buffer to flush")?;
            return Ok(false);
        };
        let (name, level) = (active.name.clone(), active.level);
        if !self.output_flush()? {
            self.notice(format!("Failed to flush buffer of {name} ({level})"))?;
            return Ok(false);
        }
        Ok(true)
    }

    /// `php_output_flush_all` (`flush()` of all buffers at shutdown).
    pub fn ob_flush_all(&mut self) -> PResult<()> {
        if !self.output.handlers.is_empty() {
            self.output_op(ob::FLUSH, &[])?;
        }
        Ok(())
    }

    /// `php_output_clean`.
    fn output_clean(&mut self) -> PResult<bool> {
        let Some(active) = self.output.active() else {
            return Ok(false);
        };
        if active.flags & ob::CLEANABLE == 0 {
            return Ok(false);
        }
        let level = self.output.handlers.len() - 1;
        let mut ctx = Ctx {
            op: ob::CLEAN,
            ..Ctx::default()
        };
        let mut pending = None;
        self.handler_op(level, &mut ctx, &mut pending)?;
        match pending {
            Some(f) => Err(f),
            None => Ok(true),
        }
    }

    /// `ob_clean()`.
    pub fn ob_clean(&mut self) -> PResult<bool> {
        let Some(active) = self.output.active() else {
            self.notice("Failed to delete buffer. No buffer to delete")?;
            return Ok(false);
        };
        let (name, level) = (active.name.clone(), active.level);
        if !self.output_clean()? {
            self.notice(format!("Failed to delete buffer of {name} ({level})"))?;
            return Ok(false);
        }
        Ok(true)
    }

    /// `php_output_stack_pop`.
    fn output_pop(&mut self, flags: u32) -> PResult<bool> {
        let discard = flags & ob::POP_DISCARD != 0;
        let verb = if discard { "discard" } else { "send" };
        let Some(orphan) = self.output.active() else {
            if flags & ob::POP_SILENT == 0 {
                self.notice(format!("Failed to {verb} buffer. No buffer to {verb}"))?;
            }
            return Ok(false);
        };
        if flags & ob::POP_FORCE == 0 && orphan.flags & ob::REMOVABLE == 0 {
            if flags & ob::POP_SILENT == 0 {
                let (name, level) = (orphan.name.clone(), orphan.level);
                self.notice(format!("Failed to {verb} buffer of {name} ({level})"))?;
            }
            return Ok(false);
        }
        let level = self.output.handlers.len() - 1;
        let mut ctx = Ctx {
            op: ob::FINAL,
            ..Ctx::default()
        };
        let mut pending = None;
        if self.output.handlers[level].flags & ob::DISABLED == 0 {
            if self.output.handlers[level].flags & ob::STARTED == 0 {
                ctx.op |= ob::START;
            }
            if discard {
                ctx.op |= ob::CLEAN;
            }
            self.handler_op(level, &mut ctx, &mut pending)?;
        }
        if level < self.output.handlers.len() {
            self.output.handlers.truncate(level);
        }
        if let Some(out) = ctx.out {
            if !out.is_empty() && !discard {
                self.output_op(ob::WRITE, &out)?;
            }
        }
        match pending {
            Some(f) => Err(f),
            None => Ok(true),
        }
    }

    /// `ob_end_flush()`.
    pub fn ob_end_flush(&mut self) -> PResult<bool> {
        if self.output.handlers.is_empty() {
            self.notice("Failed to delete and flush buffer. No buffer to delete or flush")?;
            return Ok(false);
        }
        self.output_pop(ob::POP_TRY)
    }

    /// `ob_end_clean()`.
    pub fn ob_end_clean(&mut self) -> PResult<bool> {
        if self.output.handlers.is_empty() {
            self.notice("Failed to delete buffer. No buffer to delete")?;
            return Ok(false);
        }
        self.output_pop(ob::POP_DISCARD | ob::POP_TRY)
    }

    /// `ob_get_contents()`.
    pub fn ob_get_contents(&self) -> Option<PhpStr> {
        self.output.active().map(|h| PhpStr::from_bytes(&h.buffer))
    }

    /// `ob_get_length()`.
    pub fn ob_get_length(&self) -> Option<i64> {
        self.output.active().map(|h| h.buffer.len() as i64)
    }

    /// `ob_get_level()`.
    pub fn ob_get_level(&self) -> i64 {
        self.output.handlers.len() as i64
    }

    /// `ob_get_clean()`: contents of the active buffer, which is then discarded.
    pub fn ob_get_clean(&mut self) -> PResult<Option<PhpStr>> {
        let Some(contents) = self.ob_get_contents() else {
            return Ok(None);
        };
        if !self.output_pop(ob::POP_DISCARD | ob::POP_TRY)? {
            if let Some(a) = self.output.active() {
                let (name, level) = (a.name.clone(), a.level);
                self.notice(format!("Failed to delete buffer of {name} ({level})"))?;
            }
        }
        Ok(Some(contents))
    }

    /// `ob_get_flush()`: contents of the active buffer, which is then flushed and removed.
    pub fn ob_get_flush(&mut self) -> PResult<Option<PhpStr>> {
        let Some(contents) = self.ob_get_contents() else {
            self.notice("Failed to delete and flush buffer. No buffer to delete or flush")?;
            return Ok(None);
        };
        if !self.output_pop(ob::POP_TRY)? {
            if let Some(a) = self.output.active() {
                let (name, level) = (a.name.clone(), a.level);
                self.notice(format!("Failed to delete buffer of {name} ({level})"))?;
            }
        }
        Ok(Some(contents))
    }

    /// `ob_list_handlers()`.
    pub fn ob_list_handlers(&self) -> PhpArray {
        PhpArray::from_values(
            self.output
                .handlers
                .iter()
                .map(|h| Value::String(h.name.clone()))
                .collect(),
        )
    }

    /// `ob_get_status($full_status)`.
    pub fn ob_get_status(&self, full: bool) -> PhpArray {
        if full {
            PhpArray::from_values(
                self.output
                    .handlers
                    .iter()
                    .map(|h| Value::array(h.status()))
                    .collect(),
            )
        } else {
            match self.output.active() {
                Some(h) => h.status(),
                None => PhpArray::new(),
            }
        }
    }

    /// `ob_implicit_flush($flag)`.
    pub fn ob_implicit_flush(&mut self, on: bool) {
        self.output.implicit_flush = on;
    }

    /// `php_output_end_all`: flushes and removes every handler (request shutdown).
    pub fn ob_end_all(&mut self) -> PResult<()> {
        while !self.output.handlers.is_empty() {
            if !self.output_pop(ob::POP_FORCE)? {
                break;
            }
        }
        Ok(())
    }

    /// `php_output_discard_all`.
    pub fn ob_discard_all(&mut self) -> PResult<()> {
        while !self.output.handlers.is_empty() {
            self.output_pop(ob::POP_DISCARD | ob::POP_FORCE)?;
        }
        Ok(())
    }

    /// `php_output_clean_all`.
    pub fn ob_clean_all(&mut self) -> PResult<()> {
        let mut pending = None;
        for level in (0..self.output.handlers.len()).rev() {
            if level >= self.output.handlers.len() {
                continue;
            }
            self.output.handlers[level].buffer.clear();
            let mut ctx = Ctx {
                op: ob::CLEAN,
                ..Ctx::default()
            };
            self.handler_op(level, &mut ctx, &mut pending)?;
        }
        match pending {
            Some(f) => Err(f),
            None => Ok(()),
        }
    }
}

fn swap(ctx: &mut Ctx) {
    ctx.input = ctx.out.take().unwrap_or_default();
}

fn pass(ctx: &mut Ctx) {
    ctx.out = Some(std::mem::take(&mut ctx.input));
}

// ---------------------------------------------------------------------------
// Userland `ob_*` functions (PHP_FUNCTIONs of main/output.c). They belong to
// ext/standard, which registers them with `register_functions`.
// ---------------------------------------------------------------------------

mod functions {
    use crate::native::{Args, NativeFunction};
    use crate::value::Value;
    use crate::vm::{PResult, Vm};

    fn str_or_false(v: Option<crate::value::PhpStr>) -> Value {
        match v {
            Some(s) => Value::String(s),
            None => Value::Bool(false),
        }
    }

    fn ob_start(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let cb = args.opt(0).cloned();
        let (chunk, flags) = (args.int(1), args.int(2));
        Ok(Value::Bool(vm.ob_start(cb.as_ref(), chunk, flags)?))
    }
    fn ob_flush(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::Bool(vm.ob_flush()?))
    }
    fn ob_clean(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::Bool(vm.ob_clean()?))
    }
    fn ob_end_flush(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::Bool(vm.ob_end_flush()?))
    }
    fn ob_end_clean(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::Bool(vm.ob_end_clean()?))
    }
    fn ob_get_flush(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(str_or_false(vm.ob_get_flush()?))
    }
    fn ob_get_clean(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(str_or_false(vm.ob_get_clean()?))
    }
    fn ob_get_contents(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(str_or_false(vm.ob_get_contents()))
    }
    fn ob_get_level(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::Int(vm.ob_get_level()))
    }
    fn ob_get_length(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(vm.ob_get_length().map_or(Value::Bool(false), Value::Int))
    }
    fn ob_list_handlers(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        Ok(Value::array(vm.ob_list_handlers()))
    }
    fn ob_get_status(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        Ok(Value::array(vm.ob_get_status(args.bool(0))))
    }
    fn ob_implicit_flush(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        vm.ob_implicit_flush(args.bool(0));
        Ok(Value::Null)
    }
    fn flush(vm: &mut Vm, _: &mut Args) -> PResult<Value> {
        vm.sapi_flush();
        Ok(Value::Null)
    }

    pub static OB_START: NativeFunction = NativeFunction {
        sig: "function ob_start($callback = null, int $chunk_size = 0, int $flags = PHP_OUTPUT_HANDLER_STDFLAGS): bool {}",
        handler: ob_start,
    };
    pub static OB_FLUSH: NativeFunction = NativeFunction {
        sig: "function ob_flush(): bool {}",
        handler: ob_flush,
    };
    pub static OB_CLEAN: NativeFunction = NativeFunction {
        sig: "function ob_clean(): bool {}",
        handler: ob_clean,
    };
    pub static OB_END_FLUSH: NativeFunction = NativeFunction {
        sig: "function ob_end_flush(): bool {}",
        handler: ob_end_flush,
    };
    pub static OB_END_CLEAN: NativeFunction = NativeFunction {
        sig: "function ob_end_clean(): bool {}",
        handler: ob_end_clean,
    };
    pub static OB_GET_FLUSH: NativeFunction = NativeFunction {
        sig: "function ob_get_flush(): string|false {}",
        handler: ob_get_flush,
    };
    pub static OB_GET_CLEAN: NativeFunction = NativeFunction {
        sig: "function ob_get_clean(): string|false {}",
        handler: ob_get_clean,
    };
    pub static OB_GET_CONTENTS: NativeFunction = NativeFunction {
        sig: "function ob_get_contents(): string|false {}",
        handler: ob_get_contents,
    };
    pub static OB_GET_LEVEL: NativeFunction = NativeFunction {
        sig: "function ob_get_level(): int {}",
        handler: ob_get_level,
    };
    pub static OB_GET_LENGTH: NativeFunction = NativeFunction {
        sig: "function ob_get_length(): int|false {}",
        handler: ob_get_length,
    };
    pub static OB_LIST_HANDLERS: NativeFunction = NativeFunction {
        sig: "function ob_list_handlers(): array {}",
        handler: ob_list_handlers,
    };
    pub static OB_GET_STATUS: NativeFunction = NativeFunction {
        sig: "function ob_get_status(bool $full_status = false): array {}",
        handler: ob_get_status,
    };
    pub static OB_IMPLICIT_FLUSH: NativeFunction = NativeFunction {
        sig: "function ob_implicit_flush(bool $enable = true): void {}",
        handler: ob_implicit_flush,
    };
    pub static FLUSH: NativeFunction = NativeFunction {
        sig: "function flush(): void {}",
        handler: flush,
    };
}

/// Registers the userland output-control functions (`ob_*`, `flush`).
pub fn register_functions(r: &mut crate::native::Registry<'_>) {
    use functions::*;
    r.functions(&[
        &OB_START,
        &OB_FLUSH,
        &OB_CLEAN,
        &OB_END_FLUSH,
        &OB_END_CLEAN,
        &OB_GET_FLUSH,
        &OB_GET_CLEAN,
        &OB_GET_CONTENTS,
        &OB_GET_LEVEL,
        &OB_GET_LENGTH,
        &OB_LIST_HANDLERS,
        &OB_GET_STATUS,
        &OB_IMPLICIT_FLUSH,
        &FLUSH,
    ]);
}
