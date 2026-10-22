//! Compiling and running code: the main script, `include`/`require`
//! (path resolution like `php_resolve_path`), `eval()`, and the request
//! shutdown sequence (`php_request_shutdown`).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use php_syntax::{ParseErrorKind, ParseOptions};

use crate::compile::ir::*;
use crate::compile::{CompileOptions, LowerError, compile_program};
use crate::errors::level;
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, Frame, PResult, Vm};

use super::{Ctx, Env, UserFn};

/// Emits the compile-time diagnostics collected by the parser.
fn emit_diagnostics(vm: &mut Vm, file: &PhpStr, diags: &[php_syntax::Diagnostic]) -> PResult<()> {
    for d in diags {
        let ty = match d.level {
            php_syntax::DiagnosticLevel::Deprecated => level::E_DEPRECATED,
            php_syntax::DiagnosticLevel::CompileWarning => level::E_COMPILE_WARNING,
            php_syntax::DiagnosticLevel::Warning => level::E_WARNING,
        };
        vm.zend_error_at(ty, file.clone(), d.line, PhpStr::from(d.message.as_str()))?;
    }
    Ok(())
}

/// How a compile failure is reported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OnParseError {
    /// Throw `ParseError` (include, eval).
    Throw,
    /// Display "Parse error" and bail out (main script).
    Fatal,
}

/// Parses and compiles source code.
pub fn compile_source(vm: &mut Vm, source: &[u8], file: PhpStr, opts: &ParseOptions, on_err: OnParseError, is_eval: bool) -> PResult<Rc<Unit>> {
    let parsed = php_syntax::parse_with(source, &file.to_string_lossy(), opts);
    let program = match parsed {
        Ok(p) => p,
        Err(e) => {
            emit_diagnostics(vm, &file, &e.diagnostics)?;
            match e.kind {
                ParseErrorKind::Compile => {
                    return Err(fatal_compile(vm, &file, e.line, &e.message));
                }
                ParseErrorKind::Parse => {
                    if on_err == OnParseError::Fatal {
                        let r = vm.php_error_cb(level::E_PARSE, file.clone(), e.line, PhpStr::from(e.message.as_str()), None);
                        return Err(r.err().unwrap_or(Flow::Fatal));
                    }
                    let ex = vm.create_exception("ParseError", e.message.as_str(), 0);
                    ex.set_prop(b"file", Value::String(file.clone()));
                    ex.set_prop(b"line", Value::Int(e.line as i64));
                    return Err(Flow::Exception(ex));
                }
            }
        }
    };
    emit_diagnostics(vm, &file, &program.diagnostics)?;
    let saved_file = std::mem::replace(&mut vm.current_file, file.clone());
    let r = compile_program(vm, &program, file.clone(), &CompileOptions { is_eval });
    vm.current_file = saved_file;
    match r {
        Ok(u) => {
            if let Some(off) = program.halt_offset {
                let name = super::eval::halt_offset_constant(&file);
                vm.define_constant(name, Value::Int(off as i64));
            }
            Ok(u)
        }
        Err(LowerError::Compile(c)) => Err(fatal_compile(vm, &file, c.line, &c.message)),
        Err(LowerError::Flow(f)) => Err(f),
    }
}

fn fatal_compile(vm: &mut Vm, file: &PhpStr, line: u32, msg: &str) -> Flow {
    match vm.zend_error_at(level::E_COMPILE_ERROR, file.clone(), line, PhpStr::from(msg)) {
        Err(f) => f,
        Ok(()) => Flow::Fatal,
    }
}

/// Runs a compiled unit in `env` (its variables bound by name).
pub fn run_unit(vm: &mut Vm, unit: &Rc<Unit>, env: &Rc<Env>) -> PResult<Value> {
    let code = &unit.main;
    let map: Vec<u32> = code.cvs.names.iter().map(|n| env.slot_for(n)).collect();
    let identity = map.iter().enumerate().all(|(i, s)| *s as usize == i) && env.len() >= map.len();
    let ufn = UserFn::new(code.clone());
    let cx = Ctx {
        env,
        code,
        ufn: &ufn,
        map: if identity { None } else { Some(&map) },
        goto: Cell::new(None),
        generator: None,
    };
    let saved_file = std::mem::replace(&mut vm.current_file, unit.file.clone());
    let saved_line = vm.current_line;
    let r = super::exec::exec_block(vm, &cx, &code.body);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    match r {
        Ok(()) => Ok(Value::Null),
        Err(Flow::Return(v)) => Ok(v.into_unref()),
        Err(Flow::Break(super::GOTO)) => Ok(Value::Null),
        Err(e) => Err(e),
    }
}

/// `expand_filepath`: absolute, normalised (`.`/`..` removed), symlinks kept.
pub fn expand_path(p: &[u8]) -> PhpStr {
    let s = String::from_utf8_lossy(p).into_owned();
    let path = if Path::new(&s).is_absolute() {
        PathBuf::from(&s)
    } else {
        std::env::current_dir().unwrap_or_default().join(&s)
    };
    let mut out: Vec<String> = Vec::new();
    for comp in path.to_string_lossy().split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c.to_string()),
        }
    }
    PhpStr::from(format!("/{}", out.join("/")))
}

fn realpath(p: &Path) -> Option<PhpStr> {
    std::fs::canonicalize(p)
        .ok()
        .map(|c| PhpStr::from(c.to_string_lossy().into_owned()))
}

/// `php_resolve_path` for include/require.
pub fn resolve_include_path(vm: &Vm, name: &[u8]) -> Option<PhpStr> {
    let s = String::from_utf8_lossy(name).into_owned();
    let is_file = |p: &Path| p.is_file();
    // Absolute or explicitly relative paths skip include_path.
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") || s == "." || s == ".." {
        let p = if s.starts_with('/') {
            PathBuf::from(&s)
        } else {
            std::env::current_dir().ok()?.join(&s)
        };
        return if is_file(&p) { realpath(&p) } else { None };
    }
    if s.contains("://") {
        return None;
    }
    let include_path = vm.ini_get("include_path").map(|v| v.to_string()).unwrap_or_else(|| ".".into());
    for dir in include_path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let base = if dir == "." {
            std::env::current_dir().ok()?
        } else {
            PathBuf::from(dir)
        };
        let p = base.join(&s);
        if is_file(&p) {
            return realpath(&p);
        }
    }
    // Fall back to the directory of the executing script.
    let cur = String::from_utf8_lossy(&vm.current_file).into_owned();
    if let Some(dir) = Path::new(&cur).parent() {
        let p = dir.join(&s);
        if is_file(&p) {
            return realpath(&p);
        }
    }
    None
}

fn op_name(op: IncludeOp) -> &'static str {
    match op {
        IncludeOp::Include => "include",
        IncludeOp::IncludeOnce => "include_once",
        IncludeOp::Require => "require",
        IncludeOp::RequireOnce => "require_once",
    }
}

/// `include`/`require`(`_once`) (`ZEND_INCLUDE_OR_EVAL`).
pub fn include(vm: &mut Vm, cx: &Ctx, op: IncludeOp, path: Value) -> PResult<Value> {
    let path = super::eval::to_str(vm, &path.into_unref())?;
    let fname = op_name(op);
    let required = matches!(op, IncludeOp::Require | IncludeOp::RequireOnce);
    let once = matches!(op, IncludeOp::IncludeOnce | IncludeOp::RequireOnce);
    if path.contains(&0) {
        return Err(vm.throw_value_error(format!("{fname}(): Argument #1 ($filename) must not contain any null bytes")));
    }
    let resolved = if path.is_empty() {
        None
    } else {
        resolve_include_path(vm, &path)
    };
    let Some(resolved) = resolved else {
        if path.is_empty() {
            vm.engine_warning(format!("{fname}(): Filename cannot be empty"))?;
        } else {
            let mut m = format!("{fname}(").into_bytes();
            m.extend_from_slice(&path);
            m.extend_from_slice(b"): Failed to open stream: No such file or directory");
            vm.engine_warning(PhpStr::from_vec(m))?;
        }
        let ip = vm.ini_get("include_path").map(|v| v.to_string()).unwrap_or_default();
        if required {
            let mut m = b"Failed opening required '".to_vec();
            m.extend_from_slice(&path);
            m.extend_from_slice(format!("' (include_path='{ip}')").as_bytes());
            return Err(vm.throw_error("Error", PhpStr::from_vec(m)));
        }
        let mut m = format!("{fname}(): Failed opening '").into_bytes();
        m.extend_from_slice(&path);
        m.extend_from_slice(format!("' for inclusion (include_path='{ip}')").as_bytes());
        vm.engine_warning(PhpStr::from_vec(m))?;
        return Ok(Value::Bool(false));
    };
    if once && vm.interp.is_included(&resolved) {
        return Ok(Value::Bool(true));
    }
    let source = match std::fs::read(resolved.to_string_lossy().as_ref()) {
        Ok(s) => s,
        Err(e) => {
            let mut m = format!("{fname}(").into_bytes();
            m.extend_from_slice(&path);
            m.extend_from_slice(format!("): Failed to open stream: {}", io_error_text(&e)).as_bytes());
            vm.engine_warning(PhpStr::from_vec(m))?;
            if required {
                let ip = vm.ini_get("include_path").map(|v| v.to_string()).unwrap_or_default();
                let mut m = b"Failed opening required '".to_vec();
                m.extend_from_slice(&path);
                m.extend_from_slice(format!("' (include_path='{ip}')").as_bytes());
                return Err(vm.throw_error("Error", PhpStr::from_vec(m)));
            }
            return Ok(Value::Bool(false));
        }
    };
    vm.interp.mark_included(resolved.clone());
    let opts = ParseOptions {
        short_open_tag: vm.ini_bool("short_open_tag"),
        ..ParseOptions::default()
    };
    let unit = compile_source(vm, &source, resolved.clone(), &opts, OnParseError::Throw, false)?;
    // Frame for backtraces: `include()` with the path argument.
    let frame = Frame {
        function: None,
        name: PhpStr::from(fname),
        class: None,
        call_type: None,
        this: None,
        call_site: Some((vm.current_file.clone(), vm.current_line)),
        args: vec![(None, Value::String(resolved.clone()))],
        is_internal: false,
        env: Some(cx.env.clone()),
    };
    vm.push_frame(frame);
    let r = run_unit(vm, &unit, cx.env);
    vm.pop_frame();
    let v = r?;
    Ok(if v.is_null() && !unit_returns(&unit) { Value::Int(1) } else { v })
}

fn unit_returns(unit: &Unit) -> bool {
    fn has_return(b: &[Stmt]) -> bool {
        b.iter().any(|s| match &s.kind {
            StmtKind::Return(_) => true,
            StmtKind::If(br, el) => br.iter().any(|(_, b)| has_return(b)) || el.as_ref().is_some_and(|b| has_return(b)),
            StmtKind::Block(b) => has_return(b),
            StmtKind::Try(t) => has_return(&t.body) || t.catches.iter().any(|c| has_return(&c.body)) || t.finally.as_ref().is_some_and(|f| has_return(f)),
            StmtKind::While(_, b) | StmtKind::DoWhile(b, _) => has_return(b),
            StmtKind::For(f) => has_return(&f.body),
            StmtKind::Foreach(f) => has_return(&f.body),
            StmtKind::Switch(sw) => sw.cases.iter().any(|c| has_return(&c.body)),
            _ => false,
        })
    }
    has_return(&unit.main.body)
}

fn io_error_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".into(),
        _ => {
            if e.raw_os_error() == Some(21) {
                return "Is a directory".into();
            }
            e.to_string()
        }
    }
}

/// `eval($code)`.
pub fn eval_string(vm: &mut Vm, cx: &Ctx, code: Value) -> PResult<Value> {
    let code = super::eval::to_str(vm, &code.into_unref())?;
    let file = PhpStr::from(format!(
        "{}({}) : eval()'d code",
        vm.current_file.to_string_lossy(),
        vm.current_line
    ));
    let opts = ParseOptions {
        start_in_scripting: true,
        short_open_tag: vm.ini_bool("short_open_tag"),
        ..ParseOptions::default()
    };
    let unit = compile_source(vm, &code, file.clone(), &opts, OnParseError::Throw, true)?;
    let frame = Frame {
        function: None,
        name: PhpStr::from("eval"),
        class: None,
        call_type: None,
        this: None,
        call_site: Some((vm.current_file.clone(), vm.current_line)),
        args: Vec::new(),
        is_internal: false,
        env: Some(cx.env.clone()),
    };
    vm.push_frame(frame);
    let r = run_unit(vm, &unit, cx.env);
    vm.pop_frame();
    r
}

// ---------------------------------------------------------------------------
// Running scripts
// ---------------------------------------------------------------------------

/// Reports an uncaught exception: the user exception handler first, then
/// php-src's "Uncaught ..." fatal error.
pub fn handle_uncaught(vm: &mut Vm, ex: crate::object::ObjRef) -> PResult<()> {
    if let Some(h) = vm.errors.exception_handler.take() {
        let r = vm.call(&h, vec![Value::Object(ex)]);
        // The handler is restored only if it was not replaced.
        if vm.errors.exception_handler.is_none() {
            vm.errors.exception_handler = None;
        }
        return match r {
            Ok(_) => {
                vm.exit_status = 255;
                Ok(())
            }
            Err(Flow::Exception(e2)) => {
                vm.report_uncaught_exception(&e2)?;
                vm.exit_status = 255;
                Ok(())
            }
            Err(f) => Err(f),
        };
    }
    vm.exit_status = 255;
    vm.report_uncaught_exception(&ex)
}

/// Runs the main script; returns after the request shutdown sequence.
pub fn run_main(vm: &mut Vm, source: &[u8], file: PhpStr, opts: &ParseOptions) {
    vm.executing = true;
    vm.current_file = file.clone();
    vm.current_line = 0;
    vm.interp.main_file = file.clone();
    let r = (|| -> PResult<Value> {
        let unit = compile_source(vm, source, file.clone(), opts, OnParseError::Fatal, false)?;
        let env = vm.global_env();
        run_unit(vm, &unit, &env)
    })();
    finish_request(vm, r);
}

/// Handles the script's outcome and runs the shutdown sequence.
pub fn finish_request(vm: &mut Vm, r: PResult<Value>) {
    match r {
        Ok(_) => {}
        Err(Flow::Exception(ex)) => {
            let _ = handle_uncaught(vm, ex);
        }
        Err(Flow::Exit(_)) => {}
        Err(Flow::Fatal) => {
            vm.exit_status = 255;
        }
        Err(_) => {}
    }
    shutdown(vm);
}

/// `php_request_shutdown`: shutdown functions, destructors, output flush.
pub fn shutdown(vm: &mut Vm) {
    let fatal = vm.exit_status == 255 && vm.errors.last_error.as_ref().is_some_and(|e| e.level & level::E_FATAL_ERRORS != 0);
    let _ = fatal;
    // 1. Shutdown functions (may register more).
    vm.interp.in_shutdown = true;
    let mut i = 0;
    loop {
        let Some((f, args)) = vm.interp.shutdown_functions.get(i).cloned() else {
            break;
        };
        i += 1;
        match vm.call(&f, args) {
            Ok(_) => {}
            Err(Flow::Exception(ex)) => {
                let _ = handle_uncaught(vm, ex);
            }
            Err(Flow::Exit(_)) => break,
            Err(Flow::Fatal) => break,
            Err(_) => {}
        }
    }
    // 2. Destructors: globals holding the only reference, in reverse order.
    if !crate::object::live_objects().is_empty() && vm.exit_status != 255 || vm.errors.last_error.as_ref().is_none_or(|e| e.level & level::E_FATAL_ERRORS == 0) {
        let r = destroy_globals(vm);
        if let Err(Flow::Exception(ex)) = r {
            let _ = handle_uncaught(vm, ex);
        }
        loop {
            match vm.call_remaining_destructors() {
                Ok(()) => break,
                Err(Flow::Exception(ex)) => {
                    let _ = handle_uncaught(vm, ex);
                }
                Err(_) => break,
            }
        }
    }
    // 3. Output buffers.
    let _ = vm.ob_end_all();
    vm.output.sink.flush();
}

/// `zend_call_destructors`: unsets global variables holding objects with a
/// single reference, last first, repeating while the table shrinks.
fn destroy_globals(vm: &mut Vm) -> PResult<()> {
    let Some(g) = vm.interp.global_env.clone() else { return Ok(()) };
    loop {
        let mut changed = false;
        let n = g.len();
        for s in (0..n).rev() {
            let candidate = {
                let vars = g.vars.borrow();
                match &vars[s] {
                    Value::Object(o) => o.refcount() == 1,
                    _ => false,
                }
            };
            if candidate {
                let old = g.unset(s as u32);
                drop(old);
                changed = true;
                vm.run_pending_destructors()?;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(())
}
