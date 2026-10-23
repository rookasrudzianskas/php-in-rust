//! The `php` command-line binary (`sapi/cli/php_cli.c`).

#[allow(dead_code)]
mod args;

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::io::{BufWriter, Read, Stdout, Write};
use std::process::ExitCode;
use std::rc::Rc;

use php_runtime::interp::include;
use php_runtime::output::OutputSink;
use php_runtime::value::{ArrayKey, PhpArray};
use php_runtime::{PhpStr, Value, Vm};
use php_syntax::ParseOptions;

/// Stack of the interpreter thread: deep PHP recursion runs on it. Only the
/// pages actually touched use memory.
const STACK_SIZE: usize = 4 << 30;

/// Native stack PHP code may use before "Maximum call stack size reached"
/// (zend.max_allowed_stack_size); keeps runaway recursion from committing
/// gigabytes of memory.
const MAX_STACK_USE: usize = 512 << 20;

/// Counts heap use so `memory_limit` is enforced.
#[global_allocator]
static GLOBAL: php_runtime::alloc::TrackingAllocator = php_runtime::alloc::TrackingAllocator;

fn to_bytes(s: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.as_encoded_bytes().to_vec()
    }
}

/// Buffered stdout shared by the output sink and the error log.
#[derive(Clone)]
struct SharedOut(Rc<RefCell<BufWriter<Stdout>>>);

impl OutputSink for SharedOut {
    fn write(&mut self, data: &[u8]) {
        let _ = self.0.borrow_mut().write_all(data);
    }
    /// Implicit flushes (CLI `implicit_flush=1`) are deferred: the buffer
    /// is flushed when full, before writes to stderr and at exit.
    fn flush(&mut self) {}
}

/// stderr that flushes pending stdout first (keeps merged output ordered).
struct ErrOut(SharedOut);

impl Write for ErrOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.0.borrow_mut().flush();
        std::io::stderr().write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

/// include/require through the stream layer (wrappers, include_path).
fn include_opener(vm: &mut Vm, path: &[u8], _use_include_path: bool) -> php_runtime::PResult<Option<(PhpStr, Vec<u8>)>> {
    Ok(php_ext::standard::streams::open_for_include(vm, path)?.map(|(data, resolved)| (resolved, data)))
}

fn deferred_closes(vm: &mut Vm) -> php_runtime::PResult<()> {
    php_ext::standard::streams::process_deferred_closes(vm)
}

fn new_vm(config: &args::CliConfig, out: &SharedOut) -> Vm {
    let mut vm = Vm::with_sink(Box::new(out.clone()));
    vm.stderr = Box::new(ErrOut(out.clone()));
    php_runtime::interp::install(&mut vm);
    php_ext::register_all(&mut vm);
    php_runtime::interp::install_standard(&mut vm);
    vm.hooks.include_opener = Some(include_opener);
    vm.hooks.on_statement_boundary(deferred_closes);
    vm.hooks.on_request_shutdown(deferred_closes);
    for e in &config.ini_entries {
        let name = to_bytes(&e.name);
        let value = ini_entry_value(&mut vm, e).unwrap_or_else(|| to_bytes(&e.value));
        php_ext::standard::basic::set_cfg_entry(&mut vm, PhpStr::from_vec(name.clone()), Value::String(PhpStr::from_vec(value.clone())));
        vm.ini_set_system(&name, PhpStr::from_vec(value));
    }
    vm
}

/// The value of a `-d name=value` entry as php's ini scanner reads the line
/// it builds for it (quotes stripped, constants such as `E_ALL` and
/// expressions evaluated).
fn ini_entry_value(vm: &mut Vm, e: &args::IniEntry) -> Option<Vec<u8>> {
    let line = to_bytes(&e.to_ini_line());
    let mut out = None;
    let ok = php_ext::standard::streams::ini::parse_ini_with(vm, &line, Some(b"Unknown"), 0, &mut |vm, ev| {
        if let php_ext::standard::streams::ini::IniEvent::Entry { value, .. } = ev {
            out = Some(match value {
                Some(v) => vm.to_php_string(v)?.to_vec(),
                None => Vec::new(),
            });
        }
        Ok(())
    })
    .ok()?;
    if ok { out } else { None }
}

/// `$_SERVER`, `$argv`, `$argc` and the other superglobals.
fn setup_globals(vm: &mut Vm, config: &args::CliConfig, script: &[u8]) {
    php_ext::standard::basic::set_argv(vm, config.argv().iter().map(|a| PhpStr::from_vec(to_bytes(a))).collect());
    php_ext::standard::basic::set_main_script(vm, PhpStr::from_bytes(script));
    let g = vm.global_env();
    let argv: Vec<Value> = config.argv().iter().map(|a| Value::String(PhpStr::from_vec(to_bytes(a)))).collect();
    let argc = argv.len() as i64;
    let argv_arr = Value::array(PhpArray::from_values(argv));
    let empty = || Value::empty_array();
    for name in ["_GET", "_POST", "_COOKIE", "_FILES"] {
        let s = g.slot_for(name.as_bytes());
        g.set(s, empty());
    }
    let s = g.slot_for(b"argv");
    g.set(s, argv_arr.clone());
    let s = g.slot_for(b"argc");
    g.set(s, Value::Int(argc));
    let mut env_arr = PhpArray::new();
    for (k, v) in std::env::vars_os() {
        env_arr.insert(
            ArrayKey::from_php_str(PhpStr::from_vec(to_bytes(&k))),
            Value::String(PhpStr::from_vec(to_bytes(&v))),
        );
    }
    let mut server = env_arr.clone();
    let script = PhpStr::from_bytes(script);
    for k in ["PHP_SELF", "SCRIPT_NAME", "SCRIPT_FILENAME", "PATH_TRANSLATED"] {
        server.insert(ArrayKey::from(k), Value::String(script.clone()));
    }
    server.insert(ArrayKey::from("DOCUMENT_ROOT"), Value::empty_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    server.insert(ArrayKey::from("REQUEST_TIME_FLOAT"), Value::Float(now.as_secs_f64()));
    server.insert(ArrayKey::from("REQUEST_TIME"), Value::Int(now.as_secs() as i64));
    server.insert(ArrayKey::from("argv"), argv_arr);
    server.insert(ArrayKey::from("argc"), Value::Int(argc));
    let s = g.slot_for(b"_SERVER");
    g.set(s, Value::array(server));
    let order = vm.ini_get("variables_order").map(|v| v.to_vec()).unwrap_or_default();
    let s = g.slot_for(b"_ENV");
    if order.contains(&b'E') {
        g.set(s, Value::array(env_arr));
    } else {
        g.set(s, empty());
    }
    let s = g.slot_for(b"_REQUEST");
    g.set(s, empty());
}

fn version_banner() -> String {
    format!(
        "PHP {} (cli) (built: Jan  1 2026 00:00:00) (NTS)\nCopyright (c) The PHP Group\nZend Engine v{}, Copyright (c) Zend Technologies\n",
        php_runtime::vm::version::VERSION,
        php_runtime::vm::version::ZEND_VERSION
    )
}

fn run(config: args::CliConfig, argv0: String) -> i32 {
    let out = SharedOut(Rc::new(RefCell::new(BufWriter::with_capacity(1 << 16, std::io::stdout()))));
    let code = run_inner(&config, &argv0, &out);
    let _ = out.0.borrow_mut().flush();
    code
}

fn run_inner(config: &args::CliConfig, argv0: &str, out: &SharedOut) -> i32 {
    let mut w = out.clone();
    match config.mode {
        args::Mode::Help => {
            w.write(args::usage(argv0).as_bytes());
            return 0;
        }
        args::Mode::Version => {
            w.write(version_banner().as_bytes());
            return 0;
        }
        _ => {}
    }
    let mut vm = new_vm(config, out);
    // Stack-overflow guard for deep recursion.
    let top = {
        let x = 0u8;
        std::hint::black_box(&x) as *const u8 as usize
    };
    vm.interp.stack_limit.set(top.saturating_sub(MAX_STACK_USE));
    php_runtime::alloc::set_limit(vm.ini_int("memory_limit"));
    match config.mode {
        args::Mode::Modules => {
            let mut mods: Vec<&str> = vm.modules.clone();
            mods.sort_by_key(|m| m.to_ascii_lowercase());
            mods.dedup();
            let mut s = String::from("[PHP Modules]\n");
            for m in mods {
                s.push_str(m);
                s.push('\n');
            }
            s.push_str("\n[Zend Modules]\n\n");
            w.write(s.as_bytes());
            0
        }
        args::Mode::Info => {
            let mut s = String::from("phpinfo()\n");
            s.push_str(&format!("PHP Version => {}\n\n", php_runtime::vm::version::VERSION));
            s.push_str(&format!("System => {}\n", php_runtime::builtins::PHP_OS));
            s.push_str("Server API => Command Line Interface\n");
            s.push_str("Virtual Directory Support => disabled\n");
            s.push_str("Configuration File (php.ini) Path => /usr/local/etc/php\n");
            s.push_str("Loaded Configuration File => (none)\n");
            s.push_str(&format!("Zend Extension Build => API420250101,NTS\nPHP Extension Build => API20250101,NTS\nDebug Build => no\nThread Safety => disabled\n\n"));
            s.push_str("\n");
            let mut mods: Vec<&str> = vm.modules.clone();
            mods.sort_by_key(|m| m.to_ascii_lowercase());
            mods.dedup();
            for m in mods {
                s.push_str(&format!("{m}\n\n"));
            }
            w.write(s.as_bytes());
            0
        }
        args::Mode::Lint => lint(&mut vm, config),
        args::Mode::CliDirect => {
            let code = to_bytes(config.direct_code.as_deref().unwrap_or_default());
            setup_globals(&mut vm, config, args::STDIN_SCRIPT_NAME.as_bytes());
            let opts = ParseOptions {
                start_in_scripting: true,
                short_open_tag: vm.ini_bool("short_open_tag"),
                ..ParseOptions::default()
            };
            include::run_main(&mut vm, &code, PhpStr::from("Command line code"), &opts);
            vm.exit_status
        }
        args::Mode::Standard => {
            let (source, file, shown) = match &config.script_file {
                Some(f) => {
                    let path = to_bytes(f);
                    match std::fs::read(f) {
                        Ok(s) => (s, include::expand_path(&path), path),
                        Err(_) => {
                            w.write(format!("Could not open input file: {}\n", f.to_string_lossy()).as_bytes());
                            return 1;
                        }
                    }
                }
                None => {
                    let mut s = Vec::new();
                    let _ = std::io::stdin().read_to_end(&mut s);
                    (s, PhpStr::from(args::STDIN_SCRIPT_NAME), args::STDIN_SCRIPT_NAME.as_bytes().to_vec())
                }
            };
            if config.script_file.is_some() {
                vm.interp.mark_included(file.clone());
            }
            setup_globals(&mut vm, config, &shown);
            let opts = ParseOptions {
                skip_shebang: true,
                short_open_tag: vm.ini_bool("short_open_tag"),
                ..ParseOptions::default()
            };
            include::run_main(&mut vm, &source, file, &opts);
            vm.exit_status
        }
        _ => {
            w.write(b"This mode is not supported\n");
            1
        }
    }
}

fn lint(vm: &mut Vm, config: &args::CliConfig) -> i32 {
    let files: Vec<OsString> = if config.script_file.is_some() {
        config.lint_files().into_iter().map(OsString::from).collect()
    } else {
        Vec::new()
    };
    let mut status = 0;
    let inputs: Vec<(Vec<u8>, String)> = if files.is_empty() {
        let mut s = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut s);
        vec![(s, args::STDIN_SCRIPT_NAME.to_string())]
    } else {
        let mut v = Vec::new();
        for f in &files {
            match std::fs::read(f) {
                Ok(s) => v.push((s, f.to_string_lossy().into_owned())),
                Err(_) => {
                    let _ = vm.echo(format!("Could not open input file: {}\n", f.to_string_lossy()).as_bytes());
                    return 1;
                }
            }
        }
        v
    };
    vm.executing = false;
    for (src, name) in inputs {
        let opts = ParseOptions {
            skip_shebang: true,
            short_open_tag: vm.ini_bool("short_open_tag"),
            ..ParseOptions::default()
        };
        match php_syntax::parse_with(&src, &name, &opts) {
            Ok(_) => {
                let _ = vm.echo(format!("No syntax errors detected in {name}\n").as_bytes());
            }
            Err(e) => {
                let ty = if e.kind == php_syntax::ParseErrorKind::Parse {
                    php_runtime::errors::level::E_PARSE
                } else {
                    php_runtime::errors::level::E_COMPILE_ERROR
                };
                let _ = vm.php_error_cb(
                    ty | php_runtime::errors::level::E_DONT_BAIL,
                    PhpStr::from(name.as_str()),
                    e.line,
                    PhpStr::from(e.message.as_str()),
                    None,
                );
                let _ = vm.echo(format!("Errors parsing {name}\n").as_bytes());
                status = 255;
            }
        }
    }
    let _ = vm.ob_end_all();
    status
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let argv0 = argv.first().map_or_else(|| "php".to_owned(), |a| a.to_string_lossy().into_owned());
    let config = match args::parse(argv) {
        Ok(c) => c,
        Err(error) => {
            let (stdout, stderr) = error.render(&argv0);
            let _ = std::io::stderr().write_all(stderr.as_bytes());
            let _ = std::io::stdout().write_all(stdout.as_bytes());
            return ExitCode::from(u8::try_from(error.exit_code).unwrap_or(1));
        }
    };
    let child = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || run(config, argv0))
        .expect("spawn interpreter thread");
    let code = child.join().unwrap_or(255);
    ExitCode::from((code & 0xff) as u8)
}
