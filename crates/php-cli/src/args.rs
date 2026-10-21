//! Command-line option parser for the `php` binary (CLI SAPI).
//!
//! This is a port of the option handling in `sapi/cli/php_cli.c` and of
//! `php_getopt()` from `main/getopt.c`. php runs three `php_getopt` loops over
//! the same argument vector:
//!
//! 1. `do_php_cli()` (diagnostics enabled): records `-c -n -d -S -e`, prints the
//!    usage text for `-h`/`-?` (exit 0) or for a malformed option (exit 1), and
//!    stops at the first `-i`/`-v`/`-m`.
//! 2. `do_cli()` first loop: runs `-i`/`-v`/`-m` immediately.
//! 3. `do_cli()` second loop: selects the behaviour mode and detects conflicting
//!    options.
//!
//! [`parse`] fuses them into one pass with identical observable results:
//! the first of "help / bad option / info / version / modules" wins, and mode
//! conflicts (loop 3) are only reported when none of those fired. The getopt
//! state machine itself is ported literally, quirks included (`-d=foo` means
//! `foo`, `--ini=` is an error, the "no argument" diagnostic for an unknown
//! long option names the wrong character, ...).
//!
//! # Deviations from the reference
//!
//! * `-z <file>` (zend extension) is accepted and recorded because the engine
//!   wiring asked for it; the checked-in php-src (8.7-dev) no longer has it.
//! * `-a` does not check for the readline extension; that is a property of the
//!   engine build. Callers that lack it should fail with
//!   [`INTERACTIVE_NEEDS_READLINE`] when `interactive` is set.
//! * Values that came from `argv` are kept as [`OsString`] so no bytes are lost
//!   (PHP code and `$argv` are byte strings). On Unix use
//!   `std::os::unix::ffi::OsStrExt::as_bytes`, elsewhere
//!   [`OsStr::as_encoded_bytes`].

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::ControlFlow;

/// `$argv[0]` (and `PHP_SELF`) when no script file was given, i.e. for `-r`,
/// the `-R`/`-F` line processors, reflection modes and a script read from stdin.
pub const STDIN_SCRIPT_NAME: &str = "Standard input code";

/// The CLI SAPI's built-in ini defaults (`HARDCODED_INI` in `php_cli.c`).
pub const HARDCODED_INI: &str = "html_errors=0\n\
implicit_flush=1\n\
output_buffering=0\n\
max_execution_time=0\n\
max_input_time=-1\n";

/// What php prints when `-a` is used without the readline extension.
pub const INTERACTIVE_NEEDS_READLINE: &str =
    "Interactive shell (-a) requires the readline extension.\n";

const MSG_MODE_CONFLICT: &str = "Either execute direct code, process stdin or use a file.\n";
const MSG_ONCE_F: &str = "You can use -f only once.\n";
const MSG_ONCE_R: &str = "You can use -r only once.\n";
const MSG_ONCE_RF: &str = "You can use -R or -F only once.\n";
const MSG_ONCE_B: &str = "You can use -B only once.\n";
const MSG_ONCE_E: &str = "You can use -E only once.\n";
const MSG_HIGHLIGHT_FILES: &str = "Source highlighting only works for files.\n";
const MSG_STRIP_FILES: &str = "Source stripping only works for files.\n";
const MSG_INI_ARG: &str = "Unknown argument for --ini\n";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// What the process should do, i.e. `php_cli_mode` from `cli.h` plus the
/// early-exit actions php handles before it picks a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Run a script file, or stdin when there is no file (`PHP_CLI_MODE_STANDARD`).
    /// Combined with [`CliConfig::interactive`] this is the `-a` shell.
    #[default]
    Standard,
    /// `-s`: print HTML syntax-highlighted source.
    Highlight,
    /// `-l`: syntax check only.
    Lint,
    /// `-w`: print source with comments and whitespace stripped.
    Strip,
    /// `-r <code>` (`PHP_CLI_MODE_CLI_DIRECT`).
    CliDirect,
    /// `-B`/`-R`/`-F`/`-E`: process stdin line by line.
    ProcessStdin,
    /// `--rf <name>`
    ReflectionFunction,
    /// `--rc <name>`
    ReflectionClass,
    /// `--re <name>`
    ReflectionExtension,
    /// `--rz <name>`
    ReflectionZendExtension,
    /// `--ri <name>`
    ReflectionExtInfo,
    /// `--ini`
    ShowIniConfig,
    /// `--ini=diff`
    ShowIniDiff,
    /// `-S <addr>:<port>`: the built-in web server (`do_cli_server`).
    Server,
    /// `-h`, `-?`, `--help`, `--usage`: print [`usage`] and exit 0.
    Help,
    /// `-i`: phpinfo.
    Info,
    /// `-m`: list modules.
    Modules,
    /// `-v`: print the version.
    Version,
}

/// One `-d` definition. `-d foo` is stored as `foo` = `1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniEntry {
    pub name: OsString,
    pub value: OsString,
}

impl IniEntry {
    /// Splits at the first `=` like `php_ini_builder_define()`.
    fn from_arg(arg: &[u8]) -> Self {
        match arg.iter().position(|&b| b == b'=') {
            Some(eq) => Self {
                name: from_bytes(&arg[..eq]),
                value: from_bytes(&arg[eq + 1..]),
            },
            None => Self {
                name: from_bytes(arg),
                value: OsString::from("1"),
            },
        }
    }

    /// The `name=value\n` line php feeds to its ini parser. Like
    /// `php_ini_builder_define()` it wraps the value in double quotes unless it
    /// is empty or starts with an ASCII letter, digit or quote, so
    /// `-d memory_limit=-1` becomes `memory_limit="-1"`.
    #[must_use]
    pub fn to_ini_line(&self) -> OsString {
        let value = to_bytes(&self.value);
        let mut line = to_bytes(&self.name);
        line.push(b'=');
        match value.first() {
            Some(&c) if !c.is_ascii_alphanumeric() && c != b'"' && c != b'\'' => {
                line.push(b'"');
                line.extend_from_slice(&value);
                line.push(b'"');
            }
            _ => line.extend_from_slice(&value),
        }
        line.push(b'\n');
        from_bytes(&line)
    }
}

/// Everything the command line selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    /// `argv[0]` as the process received it (use it for [`usage`]).
    pub program: OsString,
    /// Selected behaviour.
    pub mode: Mode,
    /// `-a`. Only meaningful with [`Mode::Standard`].
    pub interactive: bool,
    /// The script: `-f <file>`, `-F <file>`, or the first non-option argument.
    /// `None` means stdin for the modes that read a script (see
    /// [`CliConfig::reads_script_from_stdin`]). With `-F` this is the file run
    /// for every input line, exactly as in `php_cli.c`.
    pub script_file: Option<OsString>,
    /// `-r <code>`.
    pub direct_code: Option<OsString>,
    /// `-B <code>`: runs before the first input line.
    pub begin_code: Option<OsString>,
    /// `-R <code>`: runs for every input line.
    pub run_code: Option<OsString>,
    /// `-E <code>`: runs after the last input line.
    pub end_code: Option<OsString>,
    /// The `<name>` of `--rf`, `--rc`, `--re`, `--rz` or `--ri`; the mode says which.
    pub reflection_target: Option<OsString>,
    /// `-d` definitions in command-line order.
    pub ini_entries: Vec<IniEntry>,
    /// `-n`: do not load php.ini.
    pub no_ini: bool,
    /// `-c <path>|<file>` (the last one wins).
    pub ini_path: Option<OsString>,
    /// `-e`: extended information for debugger/profiler.
    pub extended_info: bool,
    /// Number of `-q` flags (only the built-in server's log level counts them).
    pub quiet: u32,
    /// `-H`: hide the arguments from external tools.
    pub hide_args: bool,
    /// `-C`: do not chdir to the script directory (already the default).
    pub no_chdir: bool,
    /// `-z <file>` zend extensions (see the module docs).
    pub zend_extensions: Vec<OsString>,
    /// `-S <addr>:<port>` (the last one wins). The server only runs when `mode`
    /// is [`Mode::Server`]; `-h`, `-i`, `-m` and `-v` take precedence over it.
    pub server_addr: Option<OsString>,
    /// `-t <docroot>` (the last one wins).
    pub docroot: Option<OsString>,
    /// `--repeat <count>` as `atoi` reads it; 1 when absent.
    pub repeat: i32,
    /// Everything after the script name (or after the options when there is no
    /// script). These are `$argv[1..]`. In server mode the first entry is the router.
    pub script_args: Vec<OsString>,
}

impl CliConfig {
    fn new(program: OsString) -> Self {
        Self {
            program,
            mode: Mode::Standard,
            interactive: false,
            script_file: None,
            direct_code: None,
            begin_code: None,
            run_code: None,
            end_code: None,
            reflection_target: None,
            ini_entries: Vec::new(),
            no_ini: false,
            ini_path: None,
            extended_info: false,
            quiet: 0,
            hide_args: false,
            no_chdir: false,
            zend_extensions: Vec::new(),
            server_addr: None,
            docroot: None,
            repeat: 1,
            script_args: Vec::new(),
        }
    }

    /// The contents of `$argv`: the script path (or [`STDIN_SCRIPT_NAME`])
    /// followed by [`CliConfig::script_args`].
    #[must_use]
    pub fn argv(&self) -> Vec<OsString> {
        let mut argv = Vec::with_capacity(1 + self.script_args.len());
        argv.push(
            self.script_file
                .clone()
                .unwrap_or_else(|| OsString::from(STDIN_SCRIPT_NAME)),
        );
        argv.extend(self.script_args.iter().cloned());
        argv
    }

    /// True when the script text comes from stdin: no script file, not the
    /// interactive shell, and a mode that executes/lints/highlights/strips a script.
    #[must_use]
    pub fn reads_script_from_stdin(&self) -> bool {
        self.script_file.is_none()
            && !self.interactive
            && matches!(
                self.mode,
                Mode::Standard | Mode::Highlight | Mode::Lint | Mode::Strip
            )
    }

    /// The files `-l` checks: the script file followed by every further
    /// argument up to a literal `--`. Empty means "lint stdin".
    #[must_use]
    pub fn lint_files(&self) -> Vec<&OsStr> {
        let Some(first) = &self.script_file else {
            return Vec::new();
        };
        std::iter::once(first.as_os_str())
            .chain(
                self.script_args
                    .iter()
                    .map(OsString::as_os_str)
                    .take_while(|arg| *arg != "--"),
            )
            .collect()
    }

    /// The ini text php hands to the engine as `sapi_module.ini_entries`:
    /// [`HARDCODED_INI`] (CLI only, not the web server) followed by one
    /// [`IniEntry::to_ini_line`] per `-d`.
    #[must_use]
    pub fn ini_text(&self) -> OsString {
        let mut text = Vec::new();
        if self.mode != Mode::Server {
            text.extend_from_slice(HARDCODED_INI.as_bytes());
        }
        for entry in &self.ini_entries {
            text.extend_from_slice(&to_bytes(&entry.to_ini_line()));
        }
        from_bytes(&text)
    }
}

/// Why [`parse`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliErrorKind {
    /// `php_getopt` rejected an option. php prints the diagnostic on stderr and
    /// then the usage text on stdout.
    BadOption,
    /// Contradictory options (`-r` with `-f`, `-f` twice, ...). php prints the
    /// message on stdout.
    Conflict,
}

/// A command-line error with php's message and exit status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub kind: CliErrorKind,
    /// The diagnostic, newline-terminated, worded exactly as php words it.
    pub message: String,
    /// The process exit status (always 1).
    pub exit_code: i32,
}

impl CliError {
    /// What php writes to `(stdout, stderr)` for this error. `argv0` is only
    /// used for the usage text of [`CliErrorKind::BadOption`].
    #[must_use]
    pub fn render(&self, argv0: &str) -> (String, String) {
        match self.kind {
            CliErrorKind::BadOption => (usage(argv0), self.message.clone()),
            CliErrorKind::Conflict => (self.message.clone(), String::new()),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message.trim_end_matches('\n'))
    }
}

impl Error for CliError {}

// ---------------------------------------------------------------------------
// Usage text
// ---------------------------------------------------------------------------

const USAGE_TEMPLATE: &str = "\
Usage: %s [options] [-f] <file> [--] [args...]
   %s [options] -r <code> [--] [args...]
   %s [options] [-B <begin_code>] -R <code> [-E <end_code>] [--] [args...]
   %s [options] [-B <begin_code>] -F <file> [-E <end_code>] [--] [args...]
   %s [options] -S <addr>:<port> [-t docroot] [router]
   %s [options] -- [args...]
   %s [options] -a

  -a               Run as interactive shell (requires readline extension)
  -c <path>|<file> Look for php.ini file in this directory
  -n               No configuration (ini) files will be used
  -d foo[=bar]     Define INI entry foo with value 'bar'
  -e               Generate extended information for debugger/profiler
  -f <file>        Parse and execute <file>.
  -h               This help
  -i               PHP information
  -l               Syntax check only (lint)
  -m               Show compiled in modules
  -r <code>        Run PHP <code> without using script tags <?..?>
  -B <begin_code>  Run PHP <begin_code> before processing input lines
  -R <code>        Run PHP <code> for every input line
  -F <file>        Parse and execute <file> for every input line
  -E <end_code>    Run PHP <end_code> after processing all input lines
  -H               Hide any passed arguments from external tools.
  -S <addr>:<port> Run with built-in web server.
  -t <docroot>     Specify document root <docroot> for built-in web server.
  -s               Output HTML syntax highlighted source.
  -v               Version number
  -w               Output source with stripped comments and whitespace.

  args...          Arguments passed to script. Use -- args when first argument
                   starts with - or script is read from stdin

  --ini            Show configuration file names
  --ini=diff       Show INI entries that differ from the built-in default

  --rf <name>      Show information about function <name>.
  --rc <name>      Show information about class <name>.
  --re <name>      Show information about extension <name>.
  --rz <name>      Show information about Zend extension <name>.
  --ri <name>      Show configuration for extension <name>.

  --repeat <count> Repeat script execution <count> times.
                   For internal purposes only.

";

/// The text of `php_cli_usage()`. Pass `argv[0]`: like php it prints whatever
/// follows the last `/`, and plain `php` when `argv0` contains no `/`.
#[must_use]
pub fn usage(argv0: &str) -> String {
    let prog = argv0.rfind('/').map_or("php", |slash| &argv0[slash + 1..]);
    USAGE_TEMPLATE.replace("%s", prog)
}

// ---------------------------------------------------------------------------
// Byte helpers (argv is a byte string; keep it lossless on Unix)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn to_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(unix)]
fn from_bytes(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(b).to_os_string()
}

#[cfg(not(unix))]
fn to_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

#[cfg(not(unix))]
fn from_bytes(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(b).into_owned())
}

/// `s[i]` with C-string semantics: reading past the end yields the NUL terminator.
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// C `atoi`: optional whitespace and sign, then digits; 0 when there are none.
fn atoi(s: &[u8]) -> i32 {
    let mut rest = s;
    while let [b' ' | b'\t'..=b'\r', tail @ ..] = rest {
        rest = tail;
    }
    let negative = matches!(rest.first(), Some(b'-'));
    if matches!(rest.first(), Some(b'-' | b'+')) {
        rest = &rest[1..];
    }
    let mut n: i64 = 0;
    for &digit in rest.iter().take_while(|b| b.is_ascii_digit()) {
        let d = i64::from(digit - b'0');
        n = n.saturating_mul(10);
        n = if negative {
            n.saturating_sub(d)
        } else {
            n.saturating_add(d)
        };
    }
    // `atoi` is `(int)strtol(..)`: a plain truncation of the saturated long.
    n as i32
}

// ---------------------------------------------------------------------------
// php_getopt
// ---------------------------------------------------------------------------

/// The options of `OPTIONS[]` in `php_cli.c` (`10..=16` there are the
/// reflection/`--ini`/`--repeat` pseudo characters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opt {
    Interactive,
    ProcessBegin,
    NoChdir,
    PhpIni,
    Define,
    ProcessEnd,
    ProfileInfo,
    ProcessFile,
    File,
    Help,
    Info,
    Lint,
    Modules,
    NoPhpIni,
    NoHeader,
    ProcessCode,
    HideArgs,
    Run,
    Highlight,
    Server,
    Docroot,
    Strip,
    Version,
    ReflectFunction,
    ReflectClass,
    ReflectExtension,
    ReflectZendExtension,
    ReflectExtInfo,
    Ini,
    Repeat,
    ZendExtension,
}

/// `need_param` of `opt_struct`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arg {
    /// 0: a flag.
    None,
    /// 1: `-d v`, `-dv`, `-d=v`, `--define v`, `--define=v`.
    Required,
    /// 2: only the `--name=value` form carries a value.
    Optional,
}

struct OptSpec {
    short: Option<u8>,
    long: Option<&'static str>,
    opt: Opt,
    arg: Arg,
}

const fn both(short: u8, long: &'static str, opt: Opt, arg: Arg) -> OptSpec {
    OptSpec {
        short: Some(short),
        long: Some(long),
        opt,
        arg,
    }
}

const fn long_only(long: &'static str, opt: Opt, arg: Arg) -> OptSpec {
    OptSpec {
        short: None,
        long: Some(long),
        opt,
        arg,
    }
}

/// Same order as `OPTIONS[]`; lookups take the first match.
const OPTIONS: &[OptSpec] = &[
    both(b'a', "interactive", Opt::Interactive, Arg::None),
    both(b'B', "process-begin", Opt::ProcessBegin, Arg::Required),
    both(b'C', "no-chdir", Opt::NoChdir, Arg::None),
    both(b'c', "php-ini", Opt::PhpIni, Arg::Required),
    both(b'd', "define", Opt::Define, Arg::Required),
    both(b'E', "process-end", Opt::ProcessEnd, Arg::Required),
    both(b'e', "profile-info", Opt::ProfileInfo, Arg::None),
    both(b'F', "process-file", Opt::ProcessFile, Arg::Required),
    both(b'f', "file", Opt::File, Arg::Required),
    both(b'h', "help", Opt::Help, Arg::None),
    both(b'i', "info", Opt::Info, Arg::None),
    both(b'l', "syntax-check", Opt::Lint, Arg::None),
    both(b'm', "modules", Opt::Modules, Arg::None),
    both(b'n', "no-php-ini", Opt::NoPhpIni, Arg::None),
    both(b'q', "no-header", Opt::NoHeader, Arg::None),
    both(b'R', "process-code", Opt::ProcessCode, Arg::Required),
    both(b'H', "hide-args", Opt::HideArgs, Arg::None),
    both(b'r', "run", Opt::Run, Arg::Required),
    both(b's', "syntax-highlight", Opt::Highlight, Arg::None),
    both(b's', "syntax-highlighting", Opt::Highlight, Arg::None),
    both(b'S', "server", Opt::Server, Arg::Required),
    both(b't', "docroot", Opt::Docroot, Arg::Required),
    both(b'w', "strip", Opt::Strip, Arg::None),
    both(b'?', "usage", Opt::Help, Arg::None),
    both(b'v', "version", Opt::Version, Arg::None),
    long_only("rf", Opt::ReflectFunction, Arg::Required),
    long_only("rfunction", Opt::ReflectFunction, Arg::Required),
    long_only("rc", Opt::ReflectClass, Arg::Required),
    long_only("rclass", Opt::ReflectClass, Arg::Required),
    long_only("re", Opt::ReflectExtension, Arg::Required),
    long_only("rextension", Opt::ReflectExtension, Arg::Required),
    long_only("rz", Opt::ReflectZendExtension, Arg::Required),
    long_only("rzendextension", Opt::ReflectZendExtension, Arg::Required),
    long_only("ri", Opt::ReflectExtInfo, Arg::Required),
    long_only("rextinfo", Opt::ReflectExtInfo, Arg::Required),
    long_only("ini", Opt::Ini, Arg::Optional),
    long_only("repeat", Opt::Repeat, Arg::Required),
    // Not in the 8.7-dev OPTIONS[]; see the module docs.
    OptSpec {
        short: Some(b'z'),
        long: None,
        opt: Opt::ZendExtension,
        arg: Arg::Required,
    },
];

#[derive(Debug, Clone, Copy)]
enum GetoptError {
    /// `-:`
    Colon,
    /// Unknown short option.
    NotFound,
    /// Missing value, or an unknown long option.
    NoArgument,
}

enum Step {
    /// `EOF`: a non-option, a lone `-`, `--` (already consumed) or the end.
    End,
    Opt(Opt, Option<Vec<u8>>),
    /// `PHP_GETOPT_INVALID_ARG` with the text php prints on stderr.
    Invalid(String),
}

/// `php_opt_error()`: reports `argv[optint][optchr]`.
fn getopt_error(argv: &[Vec<u8>], optint: usize, optchr: usize, kind: GetoptError) -> Step {
    let ch = String::from_utf8_lossy(&[at(&argv[optint], optchr)]).into_owned();
    let detail = match kind {
        GetoptError::Colon => ": in flags".to_owned(),
        GetoptError::NotFound => format!("option not found {ch}"),
        GetoptError::NoArgument => format!("no argument for option {ch}"),
    };
    Step::Invalid(format!(
        "Error in argument {optint}, char {}: {detail}\n",
        optchr + 1
    ))
}

/// The state of `php_getopt()` (its `static optchr`/`dash` plus `*optind`).
struct Getopt<'a> {
    argv: &'a [Vec<u8>],
    optind: usize,
    optchr: usize,
    dash: bool,
}

impl<'a> Getopt<'a> {
    fn new(argv: &'a [Vec<u8>]) -> Self {
        Self {
            argv,
            optind: 1,
            optchr: 0,
            dash: false,
        }
    }

    fn next_opt(&mut self) -> Step {
        let argv = self.argv;
        if self.optind >= argv.len() {
            return Step::End;
        }
        let arg: &[u8] = &argv[self.optind];
        if !self.dash && (at(arg, 0) != b'-' || at(arg, 1) == 0) {
            // An operand, or a lone `-`.
            return Step::End;
        }

        let is_long = at(arg, 0) == b'-' && at(arg, 1) == b'-';
        let (spec, mut arg_start) = if is_long {
            if at(arg, 2) == 0 {
                // `--` ends the options and is consumed.
                self.optind += 1;
                return Step::End;
            }
            let mut arg_start = 2;
            // `--name=value`; the `=` is only recognised before the last byte.
            let mut name_len = arg.len() - 1;
            match arg[arg_start..arg.len() - 1]
                .iter()
                .position(|&b| b == b'=')
            {
                Some(eq) => {
                    name_len = eq;
                    arg_start += 1;
                }
                None => name_len -= 1,
            }
            let name = &arg[2..2 + name_len];
            let Some(spec) = OPTIONS
                .iter()
                .find(|s| s.long.is_some_and(|long| long.as_bytes() == name))
            else {
                self.optind += 1;
                return getopt_error(argv, self.optind - 1, self.optchr, GetoptError::NoArgument);
            };
            self.optchr = 0;
            self.dash = false;
            (spec, arg_start + name_len)
        } else {
            if !self.dash {
                self.dash = true;
                self.optchr = 1;
            }
            if at(arg, self.optchr) == b':' {
                self.dash = false;
                self.optind += 1;
                return getopt_error(argv, self.optind - 1, self.optchr, GetoptError::Colon);
            }
            let ch = at(arg, self.optchr);
            let Some(spec) = OPTIONS.iter().find(|s| s.short == Some(ch)) else {
                let (err_ind, err_chr) = (self.optind, self.optchr);
                if at(arg, self.optchr + 1) == 0 {
                    self.dash = false;
                    self.optind += 1;
                } else {
                    self.optchr += 1;
                }
                return getopt_error(argv, err_ind, err_chr, GetoptError::NotFound);
            };
            (spec, 1 + self.optchr)
        };

        match spec.arg {
            Arg::None => {
                // Several short flags can share one argument; long ones cannot.
                if is_long {
                    self.optind += 1;
                } else if at(arg, self.optchr + 1) == 0 {
                    self.dash = false;
                    self.optind += 1;
                } else {
                    self.optchr += 1;
                }
                Step::Opt(spec.opt, None)
            }
            Arg::Required | Arg::Optional => {
                // `-d v`, `-d=v`, `-dv` and the long equivalents.
                self.dash = false;
                let mut optarg = None;
                if at(arg, arg_start) == 0 {
                    self.optind += 1;
                    if self.optind == argv.len() {
                        if spec.arg == Arg::Required {
                            return getopt_error(
                                argv,
                                self.optind - 1,
                                self.optchr,
                                GetoptError::NoArgument,
                            );
                        }
                    } else if spec.arg == Arg::Required {
                        optarg = Some(argv[self.optind].clone());
                        self.optind += 1;
                    }
                } else {
                    if at(arg, arg_start) == b'=' {
                        arg_start += 1;
                    }
                    optarg = Some(arg[arg_start..].to_vec());
                    self.optind += 1;
                }
                Step::Opt(spec.opt, optarg)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

struct Parser {
    cfg: CliConfig,
    /// php's `param_error`: the last conflict wins and is reported after the loop.
    conflict: Option<&'static str>,
}

impl Parser {
    /// Applies one option; `Break` means php acts on it immediately.
    fn apply(&mut self, opt: Opt, arg: Option<Vec<u8>>) -> ControlFlow<()> {
        let Self { cfg, conflict } = self;
        let value = arg.as_deref().map(from_bytes).unwrap_or_default();
        match opt {
            Opt::Help => {
                cfg.mode = Mode::Help;
                return ControlFlow::Break(());
            }
            Opt::Info => {
                cfg.mode = Mode::Info;
                return ControlFlow::Break(());
            }
            Opt::Version => {
                cfg.mode = Mode::Version;
                return ControlFlow::Break(());
            }
            Opt::Modules => {
                cfg.mode = Mode::Modules;
                return ControlFlow::Break(());
            }

            // Options handled in do_php_cli() and do_cli_server().
            Opt::PhpIni => cfg.ini_path = Some(value),
            Opt::NoPhpIni => cfg.no_ini = true,
            Opt::Define => cfg
                .ini_entries
                .push(IniEntry::from_arg(&arg.unwrap_or_default())),
            Opt::ProfileInfo => cfg.extended_info = true,
            Opt::Server => cfg.server_addr = Some(value),
            Opt::Docroot => cfg.docroot = Some(value),
            Opt::NoHeader => cfg.quiet += 1,
            Opt::ZendExtension => cfg.zend_extensions.push(value),

            // Options handled in do_cli().
            Opt::NoChdir => cfg.no_chdir = true,
            Opt::HideArgs => cfg.hide_args = true,
            Opt::Repeat => cfg.repeat = atoi(&arg.unwrap_or_default()),

            Opt::Interactive => {
                if !cfg.interactive {
                    if cfg.mode == Mode::Standard {
                        cfg.interactive = true;
                    } else {
                        *conflict = Some(MSG_MODE_CONFLICT);
                    }
                }
            }
            Opt::ProcessFile => {
                if cfg.mode == Mode::ProcessStdin {
                    if cfg.run_code.is_some() || cfg.script_file.is_some() {
                        *conflict = Some(MSG_ONCE_RF);
                        return ControlFlow::Continue(());
                    }
                } else if cfg.mode != Mode::Standard {
                    *conflict = Some(MSG_MODE_CONFLICT);
                    return ControlFlow::Continue(());
                }
                cfg.mode = Mode::ProcessStdin;
                cfg.script_file = Some(value);
            }
            Opt::File => {
                if matches!(cfg.mode, Mode::CliDirect | Mode::ProcessStdin) {
                    *conflict = Some(MSG_MODE_CONFLICT);
                } else if cfg.script_file.is_some() {
                    *conflict = Some(MSG_ONCE_F);
                } else {
                    cfg.script_file = Some(value);
                }
            }
            Opt::Lint => {
                if cfg.mode == Mode::Standard {
                    cfg.mode = Mode::Lint;
                }
            }
            Opt::Run => {
                if cfg.mode == Mode::CliDirect {
                    if cfg.direct_code.is_some() || cfg.script_file.is_some() {
                        *conflict = Some(MSG_ONCE_R);
                        return ControlFlow::Continue(());
                    }
                } else if cfg.mode != Mode::Standard || cfg.interactive {
                    *conflict = Some(MSG_MODE_CONFLICT);
                    return ControlFlow::Continue(());
                }
                cfg.mode = Mode::CliDirect;
                cfg.direct_code = Some(value);
            }
            Opt::ProcessCode => {
                if cfg.mode == Mode::ProcessStdin {
                    if cfg.run_code.is_some() || cfg.script_file.is_some() {
                        *conflict = Some(MSG_ONCE_RF);
                        return ControlFlow::Continue(());
                    }
                } else if cfg.mode != Mode::Standard {
                    *conflict = Some(MSG_MODE_CONFLICT);
                    return ControlFlow::Continue(());
                }
                cfg.mode = Mode::ProcessStdin;
                cfg.run_code = Some(value);
            }
            Opt::ProcessBegin => {
                if cfg.mode == Mode::ProcessStdin {
                    if cfg.begin_code.is_some() {
                        *conflict = Some(MSG_ONCE_B);
                        return ControlFlow::Continue(());
                    }
                } else if cfg.mode != Mode::Standard || cfg.interactive {
                    *conflict = Some(MSG_MODE_CONFLICT);
                    return ControlFlow::Continue(());
                }
                cfg.mode = Mode::ProcessStdin;
                cfg.begin_code = Some(value);
            }
            Opt::ProcessEnd => {
                if cfg.mode == Mode::ProcessStdin {
                    if cfg.end_code.is_some() {
                        *conflict = Some(MSG_ONCE_E);
                        return ControlFlow::Continue(());
                    }
                } else if cfg.mode != Mode::Standard || cfg.interactive {
                    *conflict = Some(MSG_MODE_CONFLICT);
                    return ControlFlow::Continue(());
                }
                cfg.mode = Mode::ProcessStdin;
                cfg.end_code = Some(value);
            }
            Opt::Highlight => {
                if matches!(cfg.mode, Mode::CliDirect | Mode::ProcessStdin) {
                    *conflict = Some(MSG_HIGHLIGHT_FILES);
                } else {
                    cfg.mode = Mode::Highlight;
                }
            }
            Opt::Strip => {
                if matches!(cfg.mode, Mode::CliDirect | Mode::ProcessStdin) {
                    *conflict = Some(MSG_STRIP_FILES);
                } else {
                    cfg.mode = Mode::Strip;
                }
            }
            Opt::ReflectFunction => Self::reflect(cfg, Mode::ReflectionFunction, value),
            Opt::ReflectClass => Self::reflect(cfg, Mode::ReflectionClass, value),
            Opt::ReflectExtension => Self::reflect(cfg, Mode::ReflectionExtension, value),
            Opt::ReflectZendExtension => Self::reflect(cfg, Mode::ReflectionZendExtension, value),
            Opt::ReflectExtInfo => Self::reflect(cfg, Mode::ReflectionExtInfo, value),
            Opt::Ini => match arg.as_deref() {
                None => cfg.mode = Mode::ShowIniConfig,
                Some(b"diff") => cfg.mode = Mode::ShowIniDiff,
                Some(_) => *conflict = Some(MSG_INI_ARG),
            },
        }
        ControlFlow::Continue(())
    }

    /// The reflection options override whatever mode was selected before.
    fn reflect(cfg: &mut CliConfig, mode: Mode, name: OsString) {
        cfg.mode = mode;
        cfg.reflection_target = Some(name);
    }
}

/// Parses a full argument vector, `args[0]` being the program path as given to
/// `main`.
///
/// Returns `Ok` for every command line php would act on, including `-h`
/// ([`Mode::Help`]), `-v`, `-m` and `-i`, and `Err` for the ones it rejects
/// with exit status 1.
///
/// # Errors
///
/// [`CliErrorKind::BadOption`] for an unknown option or a missing option
/// value, [`CliErrorKind::Conflict`] for contradictory options.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<CliConfig, CliError> {
    let mut os_args: Vec<OsString> = args.into_iter().collect();
    if os_args.is_empty() {
        os_args.push(OsString::from("php"));
    }
    let argv: Vec<Vec<u8>> = os_args.iter().map(|a| to_bytes(a)).collect();

    let mut parser = Parser {
        cfg: CliConfig::new(os_args[0].clone()),
        conflict: None,
    };
    let mut getopt = Getopt::new(&argv);
    loop {
        match getopt.next_opt() {
            Step::End => break,
            Step::Invalid(message) => {
                return Err(CliError {
                    kind: CliErrorKind::BadOption,
                    message,
                    exit_code: 1,
                });
            }
            Step::Opt(opt, arg) => {
                if parser.apply(opt, arg).is_break() {
                    // -h, -i, -v, -m: php acts at once and ignores the rest.
                    return Ok(parser.cfg);
                }
            }
        }
    }

    let Parser { mut cfg, conflict } = parser;
    let mut optind = getopt.optind;

    if cfg.server_addr.is_some() {
        // do_cli_server() runs instead of do_cli(): no conflict checks, and the
        // first operand is the router script.
        cfg.mode = Mode::Server;
        cfg.script_args = os_args[optind..].to_vec();
        return Ok(cfg);
    }
    if let Some(message) = conflict {
        return Err(CliError {
            kind: CliErrorKind::Conflict,
            message: message.to_owned(),
            exit_code: 1,
        });
    }

    // The first operand is the script unless one was given already, the code
    // comes from the command line / stdin, or `--` was the last thing consumed.
    let takes_script = optind < argv.len()
        && cfg.script_file.is_none()
        && !matches!(cfg.mode, Mode::CliDirect | Mode::ProcessStdin)
        && argv[optind - 1] != b"--";
    if takes_script {
        cfg.script_file = Some(os_args[optind].clone());
        optind += 1;
    }
    cfg.script_args = os_args[optind..].to_vec();
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("php")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    fn ok(args: &[&str]) -> CliConfig {
        parse(argv(args)).unwrap_or_else(|e| panic!("{args:?} was rejected: {e:?}"))
    }

    fn err(args: &[&str]) -> CliError {
        match parse(argv(args)) {
            Ok(cfg) => panic!("{args:?} was accepted: {cfg:#?}"),
            Err(e) => e,
        }
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn entry(name: &str, value: &str) -> IniEntry {
        IniEntry {
            name: name.into(),
            value: value.into(),
        }
    }

    fn assert_bad_option(args: &[&str], message: &str) {
        let e = err(args);
        assert_eq!(e.kind, CliErrorKind::BadOption, "{args:?}");
        assert_eq!(e.message, message, "{args:?}");
        assert_eq!(e.exit_code, 1);
    }

    fn assert_conflict(args: &[&str], message: &str) {
        let e = err(args);
        assert_eq!(e.kind, CliErrorKind::Conflict, "{args:?}");
        assert_eq!(e.message, message, "{args:?}");
        assert_eq!(e.exit_code, 1);
    }

    // -- modes --------------------------------------------------------------

    #[test]
    fn no_arguments_read_a_script_from_stdin() {
        let cfg = ok(&[]);
        assert_eq!(cfg.program, "php");
        assert_eq!(cfg.mode, Mode::Standard);
        assert_eq!(cfg.script_file, None);
        assert!(cfg.reads_script_from_stdin());
        assert_eq!(cfg.argv(), os(&["Standard input code"]));
        assert_eq!(cfg.repeat, 1);
    }

    #[test]
    fn an_empty_argument_vector_is_tolerated() {
        let cfg = parse(Vec::new()).unwrap();
        assert_eq!(cfg.program, "php");
        assert_eq!(cfg.mode, Mode::Standard);
    }

    #[test]
    fn script_file_forms() {
        for args in [
            &["script.php"][..],
            &["-f", "script.php"],
            &["-fscript.php"],
            &["-f=script.php"],
            &["--file", "script.php"],
            &["--file=script.php"],
            &["-n", "script.php"],
        ] {
            let cfg = ok(args);
            assert_eq!(cfg.mode, Mode::Standard, "{args:?}");
            assert_eq!(cfg.script_file.as_deref(), Some("script.php".as_ref()));
            assert!(cfg.script_args.is_empty());
            assert!(!cfg.reads_script_from_stdin());
            assert_eq!(cfg.argv(), os(&["script.php"]));
        }
    }

    #[test]
    fn script_arguments_follow_the_script_name() {
        let cfg = ok(&["a.php", "x", "y"]);
        assert_eq!(cfg.argv(), os(&["a.php", "x", "y"]));
        let cfg = ok(&["-f", "a.php", "x", "y"]);
        assert_eq!(cfg.argv(), os(&["a.php", "x", "y"]));
    }

    #[test]
    fn dashes_after_the_script_name_belong_to_the_script() {
        let cfg = ok(&["a.php", "-n", "-x", "--foo", "-r", "--", "-"]);
        assert!(!cfg.no_ini);
        assert_eq!(cfg.mode, Mode::Standard);
        assert_eq!(
            cfg.argv(),
            os(&["a.php", "-n", "-x", "--foo", "-r", "--", "-"])
        );
        let cfg = ok(&["-n", "a.php", "-h", "-v"]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.mode, Mode::Standard);
        assert_eq!(cfg.script_args, os(&["-h", "-v"]));
    }

    #[test]
    fn double_dash_ends_options_and_reads_stdin() {
        // sapi/cli/tests/015.phpt
        let cfg = ok(&["--", "foo", "bar", "baz"]);
        assert_eq!(cfg.script_file, None);
        assert!(cfg.reads_script_from_stdin());
        assert_eq!(
            cfg.argv(),
            os(&["Standard input code", "foo", "bar", "baz"])
        );

        let cfg = ok(&["-n", "--", "-x", "--", "y"]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.argv(), os(&["Standard input code", "-x", "--", "y"]));
    }

    #[test]
    fn double_dash_after_the_script_is_a_script_argument() {
        let cfg = ok(&["a.php", "--", "x"]);
        assert_eq!(cfg.argv(), os(&["a.php", "--", "x"]));
    }

    #[test]
    fn double_dash_between_f_and_arguments() {
        let cfg = ok(&["-f", "a.php", "--", "-x"]);
        assert_eq!(cfg.argv(), os(&["a.php", "-x"]));
    }

    #[test]
    fn a_value_that_is_double_dash_still_blocks_the_script() {
        // php looks at argv[optind - 1], which here is `-d`'s value.
        let cfg = ok(&["-d", "--", "a.php"]);
        assert_eq!(cfg.script_file, None);
        assert_eq!(cfg.script_args, os(&["a.php"]));
        assert_eq!(cfg.ini_entries, [entry("--", "1")]);
    }

    #[test]
    fn a_lone_dash_is_an_operand_not_stdin() {
        let cfg = ok(&["-"]);
        assert_eq!(cfg.script_file.as_deref(), Some("-".as_ref()));
        let cfg = ok(&["-n", "-", "x"]);
        assert_eq!(cfg.script_file.as_deref(), Some("-".as_ref()));
        assert_eq!(cfg.script_args, os(&["x"]));
    }

    #[test]
    fn run_code() {
        for args in [
            &["-r", "echo 1;"][..],
            &["-recho 1;"],
            &["-r=echo 1;"],
            &["--run", "echo 1;"],
            &["--run=echo 1;"],
        ] {
            let cfg = ok(args);
            assert_eq!(cfg.mode, Mode::CliDirect, "{args:?}");
            assert_eq!(cfg.direct_code.as_deref(), Some("echo 1;".as_ref()));
            assert_eq!(cfg.script_file, None);
            assert!(!cfg.reads_script_from_stdin());
            assert_eq!(cfg.argv(), os(&["Standard input code"]));
        }
    }

    #[test]
    fn run_code_with_arguments() {
        let cfg = ok(&["-r", "var_dump($argv);", "a", "-b", "--c"]);
        assert_eq!(cfg.mode, Mode::CliDirect);
        assert_eq!(cfg.argv(), os(&["Standard input code", "a", "-b", "--c"]));

        let cfg = ok(&["-r", "code", "--", "-a", "b"]);
        assert_eq!(cfg.argv(), os(&["Standard input code", "-a", "b"]));
    }

    #[test]
    fn run_code_takes_the_next_argument_whatever_it_looks_like() {
        let cfg = ok(&["-r", "-h"]);
        assert_eq!(cfg.mode, Mode::CliDirect);
        assert_eq!(cfg.direct_code.as_deref(), Some("-h".as_ref()));
        let cfg = ok(&["-r", ""]);
        assert_eq!(cfg.direct_code.as_deref(), Some("".as_ref()));
    }

    #[test]
    fn line_processing_modes() {
        let cfg = ok(&["-R", "echo $argn;"]);
        assert_eq!(cfg.mode, Mode::ProcessStdin);
        assert_eq!(cfg.run_code.as_deref(), Some("echo $argn;".as_ref()));
        assert_eq!(cfg.script_file, None);
        assert!(!cfg.reads_script_from_stdin());

        let cfg = ok(&["-B", "begin();", "-R", "run();", "-E", "end();", "x", "y"]);
        assert_eq!(cfg.mode, Mode::ProcessStdin);
        assert_eq!(cfg.begin_code.as_deref(), Some("begin();".as_ref()));
        assert_eq!(cfg.run_code.as_deref(), Some("run();".as_ref()));
        assert_eq!(cfg.end_code.as_deref(), Some("end();".as_ref()));
        // With -B/-R/-E the operands are all `$argv`, never a script.
        assert_eq!(cfg.argv(), os(&["Standard input code", "x", "y"]));

        let cfg = ok(&["-F", "line.php"]);
        assert_eq!(cfg.mode, Mode::ProcessStdin);
        assert_eq!(cfg.script_file.as_deref(), Some("line.php".as_ref()));
        assert_eq!(cfg.run_code, None);
        assert_eq!(cfg.argv(), os(&["line.php"]));

        let cfg = ok(&["-E", "end();"]);
        assert_eq!(cfg.mode, Mode::ProcessStdin);
        assert_eq!(cfg.end_code.as_deref(), Some("end();".as_ref()));
        let cfg = ok(&[
            "--process-begin=b",
            "--process-code",
            "r",
            "--process-end=e",
        ]);
        assert_eq!(cfg.begin_code.as_deref(), Some("b".as_ref()));
        assert_eq!(cfg.run_code.as_deref(), Some("r".as_ref()));
        assert_eq!(cfg.end_code.as_deref(), Some("e".as_ref()));
        let cfg = ok(&["--process-file", "f.php"]);
        assert_eq!(cfg.script_file.as_deref(), Some("f.php".as_ref()));
    }

    #[test]
    fn lint_highlight_strip() {
        for (flag, long, mode) in [
            ("-l", "--syntax-check", Mode::Lint),
            ("-s", "--syntax-highlight", Mode::Highlight),
            ("-s", "--syntax-highlighting", Mode::Highlight),
            ("-w", "--strip", Mode::Strip),
        ] {
            for f in [flag, long] {
                let cfg = ok(&[f, "a.php"]);
                assert_eq!(cfg.mode, mode, "{f}");
                assert_eq!(cfg.script_file.as_deref(), Some("a.php".as_ref()));
                let cfg = ok(&[f]);
                assert_eq!(cfg.mode, mode, "{f}");
                assert!(cfg.reads_script_from_stdin());
            }
        }
    }

    #[test]
    fn mode_switches_that_php_silently_ignores_or_overrides() {
        // sapi/cli/tests/012-2.phpt: "-r 3 -l" ignores the lint request.
        assert_eq!(ok(&["-r", "echo 3;", "-l"]).mode, Mode::CliDirect);
        // -l only applies while the mode is still standard; -s/-w win over it.
        assert_eq!(ok(&["-s", "-l", "a.php"]).mode, Mode::Highlight);
        assert_eq!(ok(&["-l", "-s", "a.php"]).mode, Mode::Highlight);
        assert_eq!(ok(&["-l", "-w", "a.php"]).mode, Mode::Strip);
        assert_eq!(ok(&["-w", "-s", "a.php"]).mode, Mode::Highlight);
        // Reflection and --ini override whatever came before.
        assert_eq!(
            ok(&["-r", "x", "--rf", "strlen"]).mode,
            Mode::ReflectionFunction
        );
        assert_eq!(ok(&["-l", "--ini"]).mode, Mode::ShowIniConfig);
    }

    #[test]
    fn interactive_shell() {
        let cfg = ok(&["-a"]);
        assert_eq!(cfg.mode, Mode::Standard);
        assert!(cfg.interactive);
        assert!(!cfg.reads_script_from_stdin());
        assert!(ok(&["--interactive"]).interactive);
        assert!(ok(&["-a", "-a"]).interactive);
        assert!(ok(&["-n", "-a", "-C"]).interactive);
    }

    #[test]
    fn early_exit_modes() {
        for (args, mode) in [
            (&["-h"][..], Mode::Help),
            (&["-?"], Mode::Help),
            (&["--help"], Mode::Help),
            (&["--usage"], Mode::Help),
            (&["-i"], Mode::Info),
            (&["--info"], Mode::Info),
            (&["-m"], Mode::Modules),
            (&["--modules"], Mode::Modules),
            (&["-v"], Mode::Version),
            (&["--version"], Mode::Version),
            // sapi/cli/tests/015.phpt: operands after --version are ignored.
            (&["--version", "foo", "bar", "baz"], Mode::Version),
        ] {
            assert_eq!(ok(args).mode, mode, "{args:?}");
        }
    }

    #[test]
    fn the_first_early_exit_wins() {
        assert_eq!(ok(&["-v", "-h"]).mode, Mode::Version);
        assert_eq!(ok(&["-h", "-v"]).mode, Mode::Help);
        assert_eq!(ok(&["-m", "-v", "-i"]).mode, Mode::Modules);
        assert_eq!(ok(&["-nv"]).mode, Mode::Version);
        assert_eq!(ok(&["-vh"]).mode, Mode::Version);
        // Options after the early exit are never looked at.
        assert_eq!(ok(&["-v", "--bogus"]).mode, Mode::Version);
        assert_eq!(ok(&["-h", "-x"]).mode, Mode::Help);
        // Conflicts found before the exit do not matter either.
        assert_eq!(ok(&["-r", "a", "-f", "b", "-h"]).mode, Mode::Help);
        assert_eq!(ok(&["-r", "a", "-r", "b", "-v"]).mode, Mode::Version);
        // ... but a bad option before it does.
        assert_bad_option(
            &["--bogus", "-v"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        // -S is dropped when php falls back to the CLI SAPI.
        let cfg = ok(&["-S", "localhost:8000", "-v"]);
        assert_eq!(cfg.mode, Mode::Version);
    }

    #[test]
    fn show_ini() {
        assert_eq!(ok(&["--ini"]).mode, Mode::ShowIniConfig);
        assert_eq!(ok(&["--ini=diff"]).mode, Mode::ShowIniDiff);
        // The value is optional, so `--ini diff` does not take `diff`.
        let cfg = ok(&["--ini", "diff"]);
        assert_eq!(cfg.mode, Mode::ShowIniConfig);
        assert_eq!(cfg.script_file.as_deref(), Some("diff".as_ref()));
        assert_conflict(&["--ini=nope"], "Unknown argument for --ini\n");
        assert_conflict(&["--ini=diff=x"], "Unknown argument for --ini\n");
        // A trailing `=` is not recognised as an assignment.
        assert_bad_option(
            &["--ini="],
            "Error in argument 1, char 1: no argument for option -\n",
        );
    }

    #[test]
    fn reflection_modes() {
        for (names, mode) in [
            (["--rf", "--rfunction"], Mode::ReflectionFunction),
            (["--rc", "--rclass"], Mode::ReflectionClass),
            (["--re", "--rextension"], Mode::ReflectionExtension),
            (["--rz", "--rzendextension"], Mode::ReflectionZendExtension),
            (["--ri", "--rextinfo"], Mode::ReflectionExtInfo),
        ] {
            for name in names {
                for args in [vec![name, "Target"], vec![&format!("{name}=Target")[..]]] {
                    let cfg = ok(&args);
                    assert_eq!(cfg.mode, mode, "{args:?}");
                    assert_eq!(cfg.reflection_target.as_deref(), Some("Target".as_ref()));
                    assert!(!cfg.reads_script_from_stdin());
                }
            }
        }
        // Method names and empty names are passed through (--re "" in 006.phpt).
        let cfg = ok(&["--rf", "ReflectionMethod::__construct"]);
        assert_eq!(
            cfg.reflection_target.as_deref(),
            Some("ReflectionMethod::__construct".as_ref())
        );
        assert_eq!(
            ok(&["--re", ""]).reflection_target.as_deref(),
            Some("".as_ref())
        );
        // The last reflection option wins.
        let cfg = ok(&["--rf", "f", "--rc", "C"]);
        assert_eq!(cfg.mode, Mode::ReflectionClass);
        assert_eq!(cfg.reflection_target.as_deref(), Some("C".as_ref()));
    }

    #[test]
    fn built_in_server() {
        let cfg = ok(&["-S", "localhost:8000"]);
        assert_eq!(cfg.mode, Mode::Server);
        assert_eq!(cfg.server_addr.as_deref(), Some("localhost:8000".as_ref()));
        assert_eq!(cfg.docroot, None);
        assert!(cfg.script_args.is_empty());

        let cfg = ok(&["-S", "0.0.0.0:80", "-t", "/srv/www", "router.php"]);
        assert_eq!(cfg.mode, Mode::Server);
        assert_eq!(cfg.docroot.as_deref(), Some("/srv/www".as_ref()));
        assert_eq!(cfg.script_args, os(&["router.php"]));
        assert_eq!(cfg.script_file, None);

        let cfg = ok(&["-t", "/a", "--server=h:1", "--docroot", "/b", "-S", "h:2"]);
        assert_eq!(cfg.server_addr.as_deref(), Some("h:2".as_ref()));
        assert_eq!(cfg.docroot.as_deref(), Some("/b".as_ref()));

        // -q counts (it lowers the server log level).
        assert_eq!(ok(&["-S", "h:1", "-q", "-q"]).quiet, 2);
        // do_cli() never runs, so its conflict checks never fire.
        assert_eq!(ok(&["-S", "h:1", "-r", "a", "-r", "b"]).mode, Mode::Server);
        // Without -S, -t is merely recorded.
        let cfg = ok(&["-t", "/x", "a.php"]);
        assert_eq!(cfg.mode, Mode::Standard);
        assert_eq!(cfg.docroot.as_deref(), Some("/x".as_ref()));
    }

    #[test]
    fn plain_flags_and_values() {
        let cfg = ok(&[
            "-n", "-c", "/etc/php", "-e", "-q", "-H", "-C", "-z", "ext.so", "a.php",
        ]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.ini_path.as_deref(), Some("/etc/php".as_ref()));
        assert!(cfg.extended_info);
        assert_eq!(cfg.quiet, 1);
        assert!(cfg.hide_args);
        assert!(cfg.no_chdir);
        assert_eq!(cfg.zend_extensions, os(&["ext.so"]));
        assert_eq!(cfg.script_file.as_deref(), Some("a.php".as_ref()));

        let cfg = ok(&[
            "--no-php-ini",
            "--php-ini=/p",
            "--profile-info",
            "--no-header",
            "--hide-args",
            "--no-chdir",
            "-c",
            "/last",
        ]);
        assert!(cfg.no_ini && cfg.extended_info && cfg.hide_args && cfg.no_chdir);
        assert_eq!(cfg.quiet, 1);
        assert_eq!(cfg.ini_path.as_deref(), Some("/last".as_ref()));
    }

    #[test]
    fn repeat_uses_atoi() {
        assert_eq!(ok(&["--repeat", "3", "a.php"]).repeat, 3);
        assert_eq!(ok(&["--repeat=2"]).repeat, 2);
        assert_eq!(ok(&["--repeat", "abc"]).repeat, 0);
        assert_eq!(ok(&["--repeat", "  -7x"]).repeat, -7);
        assert_eq!(ok(&["--repeat", "+4"]).repeat, 4);
        assert_eq!(atoi(b""), 0);
        assert_eq!(atoi(b"12 34"), 12);
    }

    // -- -d forms -----------------------------------------------------------

    #[test]
    fn define_forms() {
        for args in [
            &["-dfoo=bar"][..],
            &["-d", "foo=bar"],
            &["-d=foo=bar"], // php_getopt skips a leading `=`
            &["--define", "foo=bar"],
            &["--define=foo=bar"],
            &["-nd", "foo=bar"],
            &["-ndfoo=bar"],
        ] {
            assert_eq!(ok(args).ini_entries, [entry("foo", "bar")], "{args:?}");
        }
    }

    #[test]
    fn define_without_a_value_means_one() {
        let cfg = ok(&["-d", "display_errors"]);
        assert_eq!(cfg.ini_entries, [entry("display_errors", "1")]);
        assert_eq!(
            cfg.ini_text(),
            format!("{HARDCODED_INI}display_errors=1\n").as_str()
        );
    }

    #[test]
    fn define_keeps_order_and_duplicates() {
        // sapi/cli/tests/003.phpt: the later definition wins in the ini parser.
        let cfg = ok(&[
            "-n",
            "-d",
            "max_execution_time=500",
            "-d",
            "max_execution_time=555",
            "-r",
            "x",
        ]);
        assert_eq!(
            cfg.ini_entries,
            [
                entry("max_execution_time", "500"),
                entry("max_execution_time", "555")
            ]
        );
        assert_eq!(cfg.mode, Mode::CliDirect);
    }

    #[test]
    fn define_splits_at_the_first_equals_only() {
        let cfg = ok(&["-d", "a=b=c", "-d", "empty=", "-d", "=v"]);
        assert_eq!(
            cfg.ini_entries,
            [entry("a", "b=c"), entry("empty", ""), entry("", "v")]
        );
    }

    #[test]
    fn define_quotes_values_like_the_ini_builder() {
        let line = |arg: &str| ok(&["-d", arg]).ini_entries[0].to_ini_line();
        assert_eq!(line("foo=bar"), "foo=bar\n");
        assert_eq!(line("foo=1"), "foo=1\n");
        assert_eq!(line("foo"), "foo=1\n");
        assert_eq!(line("foo="), "foo=\n");
        assert_eq!(line("foo=\"a b\""), "foo=\"a b\"\n");
        assert_eq!(line("foo='x'"), "foo='x'\n");
        assert_eq!(line("memory_limit=-1"), "memory_limit=\"-1\"\n");
        assert_eq!(
            line("error_reporting=E_ALL & ~E_NOTICE"),
            "error_reporting=E_ALL & ~E_NOTICE\n"
        );
        assert_eq!(
            line("include_path=.:/usr/share"),
            "include_path=\".:/usr/share\"\n"
        );
        assert_eq!(line("v=/test/path"), "v=\"/test/path\"\n");
    }

    #[test]
    fn ini_text_matches_sapi_ini_entries() {
        let cfg = ok(&["-d", "a=b", "-d", "c=/d", "-n", "x.php"]);
        assert_eq!(
            cfg.ini_text(),
            format!("{HARDCODED_INI}a=b\nc=\"/d\"\n").as_str()
        );
        assert_eq!(ok(&[]).ini_text(), HARDCODED_INI);
        // The web server SAPI does not get the CLI defaults.
        let cfg = ok(&["-S", "h:1", "-d", "a=b"]);
        assert_eq!(cfg.ini_text(), "a=b\n");
    }

    // -- combined flags ------------------------------------------------------

    #[test]
    fn combined_short_flags() {
        let cfg = ok(&["-nq"]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.quiet, 1);

        let cfg = ok(&["-nqeHC", "a.php"]);
        assert!(cfg.no_ini && cfg.extended_info && cfg.hide_args && cfg.no_chdir);
        assert_eq!(cfg.script_file.as_deref(), Some("a.php".as_ref()));

        // The first flag that takes a value ends the cluster.
        let cfg = ok(&["-nqd", "foo=bar", "-l", "a.php"]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.ini_entries, [entry("foo", "bar")]);
        assert_eq!(cfg.mode, Mode::Lint);
        let cfg = ok(&["-ndfoo=bar", "-n"]);
        assert_eq!(cfg.ini_entries, [entry("foo", "bar")]);
        let cfg = ok(&["-qrecho 1;", "x"]);
        assert_eq!(cfg.direct_code.as_deref(), Some("echo 1;".as_ref()));
        assert_eq!(cfg.script_args, os(&["x"]));
        let cfg = ok(&["-qr", "echo 1;"]);
        assert_eq!(cfg.direct_code.as_deref(), Some("echo 1;".as_ref()));
        // sapi/cli/tests/003-2.phpt
        let cfg = ok(&["-nd", "max_execution_time=111", "-r", "x"]);
        assert!(cfg.no_ini);
        assert_eq!(cfg.ini_entries, [entry("max_execution_time", "111")]);
    }

    #[test]
    fn long_options_do_not_combine_or_abbreviate() {
        assert_bad_option(
            &["--no-php-ini-x"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        assert_bad_option(
            &["--no-php"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        // A value on a flag is ignored.
        assert!(ok(&["--no-php-ini=1", "a.php"]).no_ini);
    }

    // -- errors ---------------------------------------------------------------

    #[test]
    fn getopt_diagnostics() {
        assert_bad_option(&["-x"], "Error in argument 1, char 2: option not found x\n");
        assert_bad_option(
            &["-nx"],
            "Error in argument 1, char 3: option not found x\n",
        );
        assert_bad_option(
            &["-n", "-x"],
            "Error in argument 2, char 2: option not found x\n",
        );
        assert_bad_option(
            &["-xn"],
            "Error in argument 1, char 2: option not found x\n",
        );
        assert_bad_option(
            &["-d"],
            "Error in argument 1, char 2: no argument for option d\n",
        );
        assert_bad_option(
            &["-nd"],
            "Error in argument 1, char 3: no argument for option d\n",
        );
        assert_bad_option(
            &["-n", "-r"],
            "Error in argument 2, char 2: no argument for option r\n",
        );
        assert_bad_option(
            &["--define"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        assert_bad_option(
            &["--notexisting", "foo", "bar", "baz"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        assert_bad_option(&["-:"], "Error in argument 1, char 2: : in flags\n");
        assert_bad_option(
            &["-n-"],
            "Error in argument 1, char 3: option not found -\n",
        );
        assert_bad_option(
            &["---"],
            "Error in argument 1, char 1: no argument for option -\n",
        );
        // php_getopt remembers the column of the previous short cluster, so the
        // message for this unknown long option points at the wrong character.
        assert_bad_option(
            &["-nq", "--bogus"],
            "Error in argument 2, char 3: no argument for option b\n",
        );
    }

    #[test]
    fn bad_options_render_the_usage_on_stdout() {
        let e = err(&["-x"]);
        let (stdout, stderr) = e.render("/usr/bin/php");
        assert_eq!(stderr, "Error in argument 1, char 2: option not found x\n");
        assert_eq!(stdout, usage("/usr/bin/php"));
        assert_eq!(
            e.to_string(),
            "Error in argument 1, char 2: option not found x"
        );
    }

    #[test]
    fn conflicts_render_on_stdout_only() {
        let e = err(&["-r", "a", "-r", "b"]);
        let (stdout, stderr) = e.render("php");
        assert_eq!(stdout, "You can use -r only once.\n");
        assert_eq!(stderr, "");
    }

    #[test]
    fn repeated_options_sapi_cli_tests_012() {
        let once_rf = "You can use -R or -F only once.\n";
        assert_conflict(&["-F", "some.php", "-F", "some.php"], once_rf);
        assert_conflict(&["-F", "some.php", "-R", "some.php"], once_rf);
        assert_conflict(&["-R", "some.php", "-F", "some.php"], once_rf);
        assert_conflict(&["-R", "some.php", "-R", "some.php"], once_rf);
        assert_conflict(
            &["-f", "some.php", "-f", "some.php"],
            "You can use -f only once.\n",
        );
        assert_conflict(&["-B", "", "-B", ""], "You can use -B only once.\n");
        assert_conflict(&["-E", "", "-E", ""], "You can use -E only once.\n");
        assert_conflict(&["-r", "", "-r", ""], "You can use -r only once.\n");
    }

    #[test]
    fn incompatible_modes_sapi_cli_tests_012_2() {
        let conflict = "Either execute direct code, process stdin or use a file.\n";
        assert_conflict(&["-r", "echo 1;", "-F", "some.php"], conflict);
        assert_conflict(&["-r", "echo 2;", "-f", "some.php"], conflict);
        assert_conflict(&["-r", "echo 4;", "-R", "some.php"], conflict);
        assert_conflict(&["-r", "echo 5;", "-B", ""], conflict);
        assert_conflict(&["-a", "-B", ""], conflict);
        assert_conflict(&["-r", "echo 6;", "-E", ""], conflict);
        assert_conflict(&["-a", "-E", ""], conflict);
        assert_conflict(
            &["-r", "echo 7;", "-s"],
            "Source highlighting only works for files.\n",
        );
        assert_conflict(
            &["-r", "echo 8;", "-w"],
            "Source stripping only works for files.\n",
        );
        assert_conflict(&["-l", "-r", "echo 9;"], conflict);
        // sapi/cli/tests/009.phpt
        assert_conflict(&["-a", "-r", "echo hello;"], conflict);
        assert_conflict(&["-r", "echo hello;", "-a"], conflict);
    }

    #[test]
    fn more_conflicts() {
        let conflict = "Either execute direct code, process stdin or use a file.\n";
        assert_conflict(&["-R", "x", "-r", "y"], conflict);
        assert_conflict(&["-F", "x", "-f", "y"], conflict);
        assert_conflict(&["-B", "x", "-f", "y"], conflict);
        assert_conflict(&["-s", "-B", "x"], conflict);
        assert_conflict(&["-l", "-R", "x"], conflict);
        assert_conflict(&["-F", "a", "-F", "b", "-r", "c"], conflict);
        assert_conflict(
            &["-B", "x", "-s"],
            "Source highlighting only works for files.\n",
        );
        assert_conflict(
            &["-R", "x", "-w"],
            "Source stripping only works for files.\n",
        );
        // `-f x` before `-r` is accepted (php still opens the file)...
        let cfg = ok(&["-f", "a", "-r", "b"]);
        assert_eq!(cfg.mode, Mode::CliDirect);
        assert_eq!(cfg.script_file.as_deref(), Some("a".as_ref()));
        assert_eq!(cfg.direct_code.as_deref(), Some("b".as_ref()));
        // ... but then a second -r is a plain "only once" violation.
        assert_conflict(
            &["-f", "a", "-r", "b", "-r", "c"],
            "You can use -r only once.\n",
        );
    }

    #[test]
    fn the_last_conflict_wins() {
        assert_conflict(
            &["-r", "a", "-f", "b", "-s"],
            "Source highlighting only works for files.\n",
        );
        assert_conflict(
            &["-r", "a", "-s", "-f", "b"],
            "Either execute direct code, process stdin or use a file.\n",
        );
    }

    #[test]
    fn a_conflict_does_not_hide_a_bad_option() {
        // The bad option is found by the first loop, before conflicts matter.
        assert_bad_option(
            &["-r", "a", "-r", "b", "-x"],
            "Error in argument 5, char 2: option not found x\n",
        );
    }

    #[test]
    fn mixing_a_script_with_f_is_not_a_conflict() {
        let cfg = ok(&["-F", "line.php", "extra", "args"]);
        assert_eq!(cfg.argv(), os(&["line.php", "extra", "args"]));
        // -f before -R: the file is still opened, the -R code is what runs.
        let cfg = ok(&["-f", "a.php", "-R", "x"]);
        assert_eq!(cfg.script_file.as_deref(), Some("a.php".as_ref()));
        assert_eq!(cfg.run_code.as_deref(), Some("x".as_ref()));
        assert_conflict(
            &["-f", "a.php", "-R", "x", "-F", "b.php"],
            "You can use -R or -F only once.\n",
        );
    }

    // -- lint ------------------------------------------------------------------

    #[test]
    fn lint_checks_every_operand() {
        // sapi/cli/tests/024.phpt
        let cfg = ok(&["-n", "-l", "good.php", "bad.php", "some.unknown"]);
        assert_eq!(cfg.mode, Mode::Lint);
        assert_eq!(
            cfg.lint_files(),
            ["good.php", "bad.php", "some.unknown"].map(OsStr::new)
        );
        let cfg = ok(&["-l", "-f", "a.php", "b.php"]);
        assert_eq!(cfg.lint_files(), ["a.php", "b.php"].map(OsStr::new));
        let cfg = ok(&["-l", "a.php", "--", "b.php"]);
        assert_eq!(cfg.lint_files(), ["a.php"].map(OsStr::new));
        assert!(ok(&["-l"]).lint_files().is_empty());
        assert!(ok(&["-l", "--", "x"]).lint_files().is_empty());
    }

    // -- usage -------------------------------------------------------------------

    #[test]
    fn usage_text() {
        let text = usage("php");
        assert!(text.starts_with("Usage: php [options] [-f] <file> [--] [args...]\n"));
        assert!(text.contains("\n   php [options] -r <code> [--] [args...]\n"));
        assert!(text.contains("\n   php [options] -a\n\n  -a               Run as"));
        assert!(text.contains("\n  -d foo[=bar]     Define INI entry foo with value 'bar'\n"));
        assert!(text.contains("\n  --ini=diff       Show INI entries that differ"));
        assert!(text.ends_with("                   For internal purposes only.\n\n"));
        assert_eq!(text.lines().count(), 45);
        assert!(!text.contains('%'));
    }

    #[test]
    fn usage_program_name_comes_from_the_last_slash() {
        assert!(usage("/usr/local/bin/php8.7").starts_with("Usage: php8.7 [options]"));
        assert!(usage("./php").starts_with("Usage: php [options]"));
        // Without a slash php prints "php", not argv[0].
        assert!(usage("php-cli").starts_with("Usage: php [options]"));
        assert_eq!(usage("a/b").matches("b [options]").count(), 7);
    }

    // -- bytes ----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn non_utf8_arguments_survive() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"echo \"\xff\xfe\";");
        let cfg = parse(vec![
            "php".into(),
            "-r".into(),
            raw.to_os_string(),
            raw.into(),
        ])
        .unwrap();
        assert_eq!(cfg.direct_code.as_deref(), Some(raw));
        assert_eq!(cfg.script_args, [raw.to_os_string()]);

        let arg = OsStr::from_bytes(b"k=\xe9").to_os_string();
        let cfg = parse(vec!["php".into(), "-d".into(), arg]).unwrap();
        assert_eq!(cfg.ini_entries[0].value.as_bytes(), b"\xe9");
        // 0xe9 is not alphanumeric in the C locale, so php quotes it.
        assert_eq!(cfg.ini_entries[0].to_ini_line().as_bytes(), b"k=\"\xe9\"\n");

        let script = OsStr::from_bytes(b"\xc3\xa4.php").to_os_string(); // "ä.php"
        let cfg = parse(vec!["php".into(), script.clone()]).unwrap();
        assert_eq!(cfg.script_file, Some(script));
    }

    #[test]
    fn multibyte_option_character_is_reported_per_byte() {
        // php_getopt walks bytes; the diagnostic names the first byte only.
        let e = err(&["-ä"]);
        assert_eq!(e.kind, CliErrorKind::BadOption);
        assert!(
            e.message
                .starts_with("Error in argument 1, char 2: option not found ")
        );
    }
}
