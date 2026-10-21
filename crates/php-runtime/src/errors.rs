//! Error reporting: `zend_error` / `php_error_docref` / `php_error_cb`
//! (`Zend/zend.c`, `main/main.c`).
//!
//! * [`Vm::zend_error`] — engine-style errors ("Undefined variable $x").
//! * [`Vm::php_error_docref`] / [`Vm::warning`] / [`Vm::notice`] / [`Vm::deprecated`] —
//!   errors raised by built-in functions, prefixed with `"fname(): "`.
//! * Display format follows `php_error_cb` with `html_errors=0`:
//!   `"{prepend}\nWarning: {msg} in {file} on line {n}\n{append}"`, written
//!   through the output-buffer stack (`php_printf`).
//! * User error handlers (`set_error_handler`) are invoked through the
//!   [`crate::vm::Host`]; returning `false` falls back to the default display.
//! * Fatal errors (`E_ERROR`, ...) display and then unwind with [`Flow::Fatal`].

use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

/// `E_*` error levels (`Zend/zend_errors.h`).
pub mod level {
    pub const E_ERROR: i64 = 1 << 0;
    pub const E_WARNING: i64 = 1 << 1;
    pub const E_PARSE: i64 = 1 << 2;
    pub const E_NOTICE: i64 = 1 << 3;
    pub const E_CORE_ERROR: i64 = 1 << 4;
    pub const E_CORE_WARNING: i64 = 1 << 5;
    pub const E_COMPILE_ERROR: i64 = 1 << 6;
    pub const E_COMPILE_WARNING: i64 = 1 << 7;
    pub const E_USER_ERROR: i64 = 1 << 8;
    pub const E_USER_WARNING: i64 = 1 << 9;
    pub const E_USER_NOTICE: i64 = 1 << 10;
    pub const E_STRICT: i64 = 1 << 11;
    pub const E_RECOVERABLE_ERROR: i64 = 1 << 12;
    pub const E_DEPRECATED: i64 = 1 << 13;
    pub const E_USER_DEPRECATED: i64 = 1 << 14;
    /// Do not bail out after a fatal error (used for uncaught exceptions).
    pub const E_DONT_BAIL: i64 = 1 << 15;
    pub const E_ALL: i64 = E_ERROR
        | E_WARNING
        | E_PARSE
        | E_NOTICE
        | E_CORE_ERROR
        | E_CORE_WARNING
        | E_COMPILE_ERROR
        | E_COMPILE_WARNING
        | E_USER_ERROR
        | E_USER_WARNING
        | E_USER_NOTICE
        | E_RECOVERABLE_ERROR
        | E_DEPRECATED
        | E_USER_DEPRECATED;
    pub const E_CORE: i64 = E_CORE_ERROR | E_CORE_WARNING;
    pub const E_FATAL_ERRORS: i64 =
        E_ERROR | E_CORE_ERROR | E_COMPILE_ERROR | E_USER_ERROR | E_RECOVERABLE_ERROR | E_PARSE;
}

use level::*;

/// Label used in the displayed message (`php_error_cb`).
pub fn error_type_str(ty: i64) -> &'static str {
    match ty {
        E_ERROR | E_CORE_ERROR | E_COMPILE_ERROR | E_USER_ERROR => "Fatal error",
        E_RECOVERABLE_ERROR => "Recoverable fatal error",
        E_WARNING | E_CORE_WARNING | E_COMPILE_WARNING | E_USER_WARNING => "Warning",
        E_PARSE => "Parse error",
        E_NOTICE | E_USER_NOTICE => "Notice",
        E_DEPRECATED | E_USER_DEPRECATED => "Deprecated",
        _ => "Unknown error",
    }
}

/// The last reported error (`error_get_last`).
#[derive(Clone, Debug)]
pub struct LastError {
    pub level: i64,
    pub message: PhpStr,
    pub file: PhpStr,
    pub line: u32,
}

/// A registered `set_error_handler` callback.
#[derive(Clone, Debug)]
pub struct UserErrorHandler {
    pub callback: Value,
    pub mask: i64,
}

/// `EG(error_handling)`: some internal constructors turn warnings into exceptions.
#[derive(Clone, Debug, Default)]
pub enum ErrorHandlingMode {
    #[default]
    Normal,
    /// Warnings throw an exception of the given class (`EH_THROW`).
    Throw(PhpStr),
}

/// Where displayed errors go (`display_errors`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayErrors {
    Off,
    Stdout,
    Stderr,
}

/// Error-reporting state owned by the [`Vm`].
#[derive(Debug)]
pub struct ErrorState {
    /// `EG(error_reporting)`; mirrors the `error_reporting` ini, modified by `@`.
    pub reporting: i64,
    pub display: DisplayErrors,
    pub html_errors: bool,
    pub log_errors: bool,
    pub ignore_repeated: bool,
    pub ignore_repeated_source: bool,
    pub fatal_error_backtraces: bool,
    pub prepend: Option<PhpStr>,
    pub append: Option<PhpStr>,
    pub last_error: Option<LastError>,
    /// Current user error handler and the stack saved by `set_error_handler`.
    pub user_handler: Option<UserErrorHandler>,
    pub user_handler_stack: Vec<Option<UserErrorHandler>>,
    /// Current exception handler (`set_exception_handler`) and its stack.
    pub exception_handler: Option<Value>,
    pub exception_handler_stack: Vec<Option<Value>>,
    pub mode: ErrorHandlingMode,
    /// Errors collected while `record_errors` is active (e.g. during compilation).
    pub recorded: Option<Vec<LastError>>,
}

impl Default for ErrorState {
    fn default() -> Self {
        ErrorState {
            reporting: E_ALL,
            display: DisplayErrors::Stdout,
            html_errors: false,
            log_errors: false,
            ignore_repeated: false,
            ignore_repeated_source: false,
            fatal_error_backtraces: true,
            prepend: None,
            append: None,
            last_error: None,
            user_handler: None,
            user_handler_stack: Vec::new(),
            exception_handler: None,
            exception_handler_stack: Vec::new(),
            mode: ErrorHandlingMode::Normal,
            recorded: None,
        }
    }
}

/// Saved `error_reporting` value for an `@` silence region.
#[derive(Clone, Copy, Debug)]
#[must_use = "pass the token to Vm::end_silence"]
pub struct SilenceToken(i64);

/// `htmlspecialchars` with `ENT_COMPAT` (for `html_errors=1`).
pub fn escape_html(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        match c {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// `smart_str_append_escaped` (used for string arguments in stack traces).
pub fn append_escaped(out: &mut Vec<u8>, s: &[u8]) {
    for &c in s {
        if c < 32 || c == b'\\' || c > 126 {
            out.push(b'\\');
            match c {
                b'\n' => out.push(b'n'),
                b'\r' => out.push(b'r'),
                b'\t' => out.push(b't'),
                0x0c => out.push(b'f'),
                0x0b => out.push(b'v'),
                b'\\' => out.push(b'\\'),
                0x1b => out.push(b'e'),
                _ => {
                    const HEX: &[u8; 16] = b"0123456789ABCDEF";
                    out.push(b'x');
                    out.push(HEX[(c >> 4) as usize]);
                    out.push(HEX[(c & 0xf) as usize]);
                }
            }
        } else {
            out.push(c);
        }
    }
}

/// `build_trace_args`: renders one argument of a stack-trace frame.
pub fn append_trace_arg(out: &mut Vec<u8>, arg: &Value, max_len: usize, precision: i64) {
    match &*arg.deref() {
        Value::Undef | Value::Null => out.extend_from_slice(b"NULL"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(d) => crate::value::float_fmt::append_double(out, *d, precision, true),
        Value::String(s) => {
            out.push(b'\'');
            let n = s.len().min(max_len);
            append_escaped(out, &s[..n]);
            if s.len() > max_len {
                out.extend_from_slice(b"...");
            }
            out.push(b'\'');
        }
        Value::Array(_) => out.extend_from_slice(b"Array"),
        Value::Object(o) => {
            let cls = o.class();
            if cls.is_enum() {
                if let Some(Value::String(case)) = o.get_prop(b"name") {
                    out.extend_from_slice(&cls.name);
                    out.extend_from_slice(b"::");
                    out.extend_from_slice(&case);
                    return;
                }
            }
            out.extend_from_slice(b"Object(");
            let name = cls.name.as_bytes();
            let cut = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            out.extend_from_slice(&name[..cut]);
            out.push(b')');
        }
        Value::Resource(r) => {
            out.extend_from_slice(b"Resource id #");
            out.extend_from_slice(r.id().to_string().as_bytes());
        }
        Value::Ref(_) => unreachable!("references never nest"),
    }
}

/// `zend_trace_to_string`: renders a `debug_backtrace()`-style array
/// (`#0 file(line): Class->func(args)` lines, optionally ending with `#n {main}`).
pub fn trace_to_string(
    trace: &PhpArray,
    include_main: bool,
    max_len: usize,
    precision: i64,
) -> PhpStr {
    let mut out = Vec::new();
    let mut num = 0usize;
    for (_, frame) in trace.iter() {
        let frame = frame.deref();
        let Value::Array(f) = &*frame else { continue };
        out.push(b'#');
        out.extend_from_slice(num.to_string().as_bytes());
        out.push(b' ');
        match f.get_str(b"file") {
            Some(Value::String(file)) => {
                let line = f.get_str(b"line").and_then(Value::as_int).unwrap_or(0);
                out.extend_from_slice(file);
                out.push(b'(');
                out.extend_from_slice(line.to_string().as_bytes());
                out.extend_from_slice(b"): ");
            }
            Some(_) => out.extend_from_slice(b"[unknown file]: "),
            None => out.extend_from_slice(b"[internal function]: "),
        }
        if let Some(Value::String(c)) = f.get_str(b"class") {
            let cut = c.iter().position(|&b| b == 0).unwrap_or(c.len());
            out.extend_from_slice(&c[..cut]);
        }
        if let Some(Value::String(t)) = f.get_str(b"type") {
            out.extend_from_slice(t);
        }
        if let Some(Value::String(func)) = f.get_str(b"function") {
            out.extend_from_slice(func);
        }
        out.push(b'(');
        if let Some(Value::Array(args)) = f.get_str(b"args") {
            let mut first = true;
            for (k, a) in args.iter() {
                if !first {
                    out.extend_from_slice(b", ");
                }
                first = false;
                if let crate::value::ArrayKey::Str(name) = &k {
                    out.extend_from_slice(name);
                    out.extend_from_slice(b": ");
                }
                append_trace_arg(&mut out, a, max_len, precision);
            }
        }
        out.extend_from_slice(b")\n");
        num += 1;
    }
    if include_main {
        out.push(b'#');
        out.extend_from_slice(num.to_string().as_bytes());
        out.extend_from_slice(b" {main}");
    }
    PhpStr::from_vec(out)
}

fn is_warning_level(ty: i64) -> bool {
    matches!(
        ty,
        E_WARNING | E_CORE_WARNING | E_COMPILE_WARNING | E_USER_WARNING
    )
}

impl Vm {
    /// `zend_error(type, msg)`: an engine error without function prefix, at
    /// the current file/line. Returns `Err` if a user handler threw or the
    /// error is fatal.
    pub fn zend_error(&mut self, ty: i64, msg: impl Into<PhpStr>) -> PResult<()> {
        let (file, line) = self.error_location(ty);
        self.zend_error_at(ty, file, line, msg.into())
    }

    /// Engine warning (`zend_error(E_WARNING, ...)`).
    pub fn engine_warning(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.zend_error(E_WARNING, msg)
    }

    /// Engine notice (`zend_error(E_NOTICE, ...)`).
    pub fn engine_notice(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.zend_error(E_NOTICE, msg)
    }

    /// Engine deprecation (`zend_error(E_DEPRECATED, ...)`).
    pub fn engine_deprecated(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.zend_error(E_DEPRECATED, msg)
    }

    /// `php_error_docref(NULL, type, msg)`: prefixes `"fname(): "` with the
    /// active function (`Class::method` for methods, `main` at top level).
    pub fn php_error_docref(&mut self, ty: i64, msg: impl Into<PhpStr>) -> PResult<()> {
        let msg = msg.into();
        let mut origin = self.error_origin().into_bytes();
        let mut msg = msg.to_vec();
        if self.errors.html_errors {
            origin = escape_html(&origin);
            msg = escape_html(&msg);
        }
        let mut full = Vec::with_capacity(origin.len() + msg.len() + 4);
        full.extend_from_slice(&origin);
        full.extend_from_slice(b": ");
        full.extend_from_slice(&msg);
        self.zend_error(ty, PhpStr::from_vec(full))
    }

    /// Built-in function warning: `"Warning: fname(): msg in FILE on line N"`.
    pub fn warning(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.php_error_docref(E_WARNING, msg)
    }

    /// Built-in function notice.
    pub fn notice(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.php_error_docref(E_NOTICE, msg)
    }

    /// Built-in function deprecation.
    pub fn deprecated(&mut self, msg: impl Into<PhpStr>) -> PResult<()> {
        self.php_error_docref(E_DEPRECATED, msg)
    }

    /// `zend_error_noreturn(E_ERROR, msg)`: displays a fatal error and returns
    /// the [`Flow::Fatal`] to unwind with (`return Err(vm.fatal_error(..))`).
    pub fn fatal_error(&mut self, msg: impl Into<PhpStr>) -> Flow {
        match self.zend_error(E_ERROR, msg) {
            Err(f) => f,
            Ok(()) => Flow::Fatal,
        }
    }

    /// `"fname()"` origin for docref-style messages (`php_verror`): the active
    /// function's declaring class and name, `main()` for top-level code.
    pub fn error_origin(&self) -> String {
        match self.frames.last() {
            Some(f) => {
                let name = if f.name.is_empty() {
                    "main".into()
                } else {
                    f.name.to_string_lossy()
                };
                let args = if self.ini_bool("error_include_args") {
                    let mut out = Vec::new();
                    let max_len =
                        self.ini_int("zend.exception_string_param_max_len").max(0) as usize;
                    for (i, (n, v)) in f.args.iter().enumerate() {
                        if i > 0 {
                            out.extend_from_slice(b", ");
                        }
                        if let Some(n) = n {
                            out.extend_from_slice(n);
                            out.extend_from_slice(b": ");
                        }
                        append_trace_arg(&mut out, v, max_len, self.precision);
                    }
                    String::from_utf8_lossy(&out).into_owned()
                } else {
                    String::new()
                };
                match f.function.as_ref().and_then(|func| func.scope.as_ref()) {
                    Some(scope) => format!("{scope}::{name}({args})"),
                    None => format!("{name}({args})"),
                }
            }
            None if self.executing => "main()".to_string(),
            None => "Unknown".to_string(),
        }
    }

    fn error_location(&self, ty: i64) -> (PhpStr, u32) {
        if ty & (E_CORE_ERROR | E_CORE_WARNING) != 0 || !self.executing {
            return (PhpStr::from("Unknown"), 0);
        }
        (self.current_file.clone(), self.current_line)
    }

    /// `zend_error_zstr_at`: full error pipeline with an explicit location.
    pub fn zend_error_at(
        &mut self,
        orig_ty: i64,
        file: PhpStr,
        line: u32,
        msg: PhpStr,
    ) -> PResult<()> {
        let ty = orig_ty & E_ALL;

        // Emit delayed errors before handling a fatal error.
        if ty & E_FATAL_ERRORS != 0 && orig_ty & E_DONT_BAIL == 0 {
            if let Some(rec) = self.errors.recorded.take() {
                let handler = self.errors.user_handler.take();
                let mut r = Ok(());
                for e in &rec {
                    r = self.zend_error_at(e.level, e.file.clone(), e.line, e.message.clone());
                    if r.is_err() {
                        break;
                    }
                }
                self.errors.user_handler = handler;
                self.errors.recorded = Some(rec);
                r?;
            }
        }

        if let Some(rec) = &mut self.errors.recorded {
            rec.push(LastError {
                level: ty,
                message: msg.clone(),
                file: file.clone(),
                line,
            });
            if ty & E_FATAL_ERRORS == 0 || orig_ty & E_DONT_BAIL != 0 {
                return Ok(());
            }
        }

        let backtrace = if self.errors.fatal_error_backtraces
            && ty & E_FATAL_ERRORS != 0
            && orig_ty & E_DONT_BAIL == 0
        {
            let ignore_args = self.ini_bool("zend.exception_ignore_args");
            let bt = self.backtrace(0, ignore_args, 0);
            Some(bt)
        } else {
            None
        };

        let use_user_handler = matches!(self.errors.mode, ErrorHandlingMode::Normal)
            && self
                .errors
                .user_handler
                .as_ref()
                .is_some_and(|h| h.mask & ty != 0)
            && !matches!(
                ty,
                E_ERROR
                    | E_PARSE
                    | E_CORE_ERROR
                    | E_CORE_WARNING
                    | E_COMPILE_ERROR
                    | E_COMPILE_WARNING
            );

        if use_user_handler {
            let handler = self.errors.user_handler.take().expect("checked");
            let args = vec![
                Value::Int(ty),
                Value::String(msg.clone()),
                Value::String(file.clone()),
                Value::Int(line as i64),
            ];
            let host = self.host.clone();
            let res = host.call_callable(self, &handler.callback, args);
            if self.errors.user_handler.is_none() {
                self.errors.user_handler = Some(handler);
            }
            if matches!(res?, Value::Bool(false)) {
                self.php_error_cb(orig_ty, file, line, msg, backtrace)?;
            }
            return Ok(());
        }
        self.php_error_cb(orig_ty, file, line, msg, backtrace)
    }

    /// The default error callback (`php_error_cb`): records, displays, logs,
    /// and bails out on fatal errors.
    pub fn php_error_cb(
        &mut self,
        orig_ty: i64,
        file: PhpStr,
        line: u32,
        msg: PhpStr,
        backtrace: Option<PhpArray>,
    ) -> PResult<()> {
        let ty = orig_ty & E_ALL;
        let display = if self.errors.ignore_repeated {
            match &self.errors.last_error {
                Some(last) => {
                    last.message != msg
                        || (!self.errors.ignore_repeated_source
                            && (last.line != line || last.file != file))
                }
                None => true,
            }
        } else {
            true
        };

        if let ErrorHandlingMode::Throw(class) = &self.errors.mode {
            if is_warning_level(ty) {
                let class = class.clone();
                let ex = self.create_exception(&class.to_string_lossy(), msg, 0);
                if ex.class().instance_of_name(b"ErrorException") {
                    ex.set_prop(b"severity", Value::Int(ty));
                }
                return Err(Flow::Exception(ex));
            }
        }

        let trace_str = match &backtrace {
            Some(bt) => {
                let max_len = self.ini_int("zend.exception_string_param_max_len").max(0) as usize;
                trace_to_string(bt, true, max_len, self.precision)
            }
            None => PhpStr::new(),
        };

        if display {
            self.errors.last_error = Some(LastError {
                level: ty,
                message: msg.clone(),
                file: file.clone(),
                line,
            });
        }

        if display && ((self.errors.reporting & ty) != 0 || (ty & E_CORE) != 0) {
            let label = error_type_str(ty);
            if self.errors.log_errors {
                let mut log = format!("PHP {label}:  ").into_bytes();
                log.extend_from_slice(&msg);
                log.extend_from_slice(format!(" in {file} on line {line}").as_bytes());
                if !trace_str.is_empty() {
                    log.extend_from_slice(b"\nStack trace:\n");
                    log.extend_from_slice(&trace_str);
                }
                log.push(b'\n');
                self.write_log(&log);
            }
            if self.errors.display != DisplayErrors::Off {
                let prepend = self.errors.prepend.clone().unwrap_or_default();
                let append = self.errors.append.clone().unwrap_or_default();
                let mut out = Vec::with_capacity(msg.len() + 64);
                if self.errors.html_errors {
                    // Only E_ERROR/E_PARSE are escaped here; docref messages were
                    // escaped by php_error_docref.
                    let shown = if matches!(ty, E_ERROR | E_PARSE) {
                        escape_html(&msg)
                    } else {
                        msg.to_vec()
                    };
                    out.extend_from_slice(&prepend);
                    out.extend_from_slice(b"<br />\n<b>");
                    out.extend_from_slice(label.as_bytes());
                    out.extend_from_slice(b"</b>:  ");
                    out.extend_from_slice(&shown);
                    out.extend_from_slice(b" in <b>");
                    out.extend_from_slice(&file);
                    out.extend_from_slice(format!("</b> on line <b>{line}</b><br />").as_bytes());
                    if !trace_str.is_empty() && !matches!(ty, E_ERROR | E_PARSE) {
                        out.extend_from_slice(b"\nStack trace:\n");
                        out.extend_from_slice(&trace_str);
                    }
                    out.push(b'\n');
                    out.extend_from_slice(&append);
                } else {
                    out.extend_from_slice(&prepend);
                    out.push(b'\n');
                    out.extend_from_slice(label.as_bytes());
                    out.extend_from_slice(b": ");
                    out.extend_from_slice(&msg);
                    out.extend_from_slice(b" in ");
                    out.extend_from_slice(&file);
                    out.extend_from_slice(format!(" on line {line}").as_bytes());
                    if !trace_str.is_empty() {
                        out.extend_from_slice(b"\nStack trace:\n");
                        out.extend_from_slice(&trace_str);
                    }
                    out.push(b'\n');
                    out.extend_from_slice(&append);
                }
                if self.errors.display == DisplayErrors::Stderr && !self.errors.html_errors {
                    // CLI writes "label: msg in file on line N\n" to stderr without the leading newline.
                    let start = prepend.len() + 1;
                    let end = out.len() - append.len();
                    let chunk = out[start..end].to_vec();
                    self.write_log(&chunk);
                } else {
                    self.echo(&out)?;
                }
            }
        }

        if matches!(
            ty,
            E_CORE_ERROR | E_ERROR | E_RECOVERABLE_ERROR | E_PARSE | E_COMPILE_ERROR | E_USER_ERROR
        ) {
            self.exit_status = 255;
            if orig_ty & E_DONT_BAIL == 0 {
                crate::object::mark_all_destructed();
                return Err(Flow::Fatal);
            }
        }
        Ok(())
    }

    /// Writes to the error log sink (stderr for the CLI).
    pub fn write_log(&mut self, bytes: &[u8]) {
        let _ = self.stderr.write_all(bytes);
        let _ = self.stderr.flush();
    }

    /// `@`-operator start (`ZEND_BEGIN_SILENCE`): masks all non-fatal errors.
    pub fn begin_silence(&mut self) -> SilenceToken {
        let saved = self.errors.reporting;
        if saved & !E_FATAL_ERRORS != 0 {
            self.errors.reporting &= E_FATAL_ERRORS;
        }
        SilenceToken(saved)
    }

    /// `@`-operator end (`ZEND_END_SILENCE`): restores `error_reporting`
    /// unless the silenced code changed it.
    pub fn end_silence(&mut self, token: SilenceToken) {
        let cur = self.errors.reporting;
        if cur & !E_FATAL_ERRORS == 0 && token.0 & !E_FATAL_ERRORS != 0 {
            self.errors.reporting = token.0;
        }
    }

    /// `error_get_last()` as an array (or `None`).
    pub fn error_get_last(&self) -> Option<PhpArray> {
        let e = self.errors.last_error.as_ref()?;
        let mut a = PhpArray::new();
        a.insert_str("type", e.level);
        a.insert_str("message", e.message.clone());
        a.insert_str("file", e.file.clone());
        a.insert_str("line", e.line as i64);
        Some(a)
    }

    /// `set_error_handler()`: installs a handler, returning the previous callback.
    pub fn set_error_handler(&mut self, callback: Option<Value>, mask: i64) -> Option<Value> {
        let prev = self.errors.user_handler.take();
        self.errors.user_handler_stack.push(prev.clone());
        self.errors.user_handler = callback.map(|callback| UserErrorHandler { callback, mask });
        prev.map(|h| h.callback)
    }

    /// `restore_error_handler()`.
    pub fn restore_error_handler(&mut self) {
        self.errors.user_handler = self.errors.user_handler_stack.pop().flatten();
    }

    /// `set_exception_handler()`: returns the previous handler.
    pub fn set_exception_handler(&mut self, callback: Option<Value>) -> Option<Value> {
        let prev = self.errors.exception_handler.take();
        self.errors.exception_handler_stack.push(prev.clone());
        self.errors.exception_handler = callback;
        prev
    }

    /// `restore_exception_handler()`.
    pub fn restore_exception_handler(&mut self) {
        self.errors.exception_handler = self.errors.exception_handler_stack.pop().flatten();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(E_ALL, 30719);
        assert_eq!(E_FATAL_ERRORS, 4437);
        assert_eq!(error_type_str(E_USER_DEPRECATED), "Deprecated");
        assert_eq!(
            error_type_str(E_RECOVERABLE_ERROR),
            "Recoverable fatal error"
        );
    }

    #[test]
    fn escaping() {
        let mut v = Vec::new();
        append_escaped(&mut v, b"a\nb\\c\x01\xff");
        assert_eq!(v, b"a\\nb\\\\c\\x01\\xFF");
    }
}
