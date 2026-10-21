//! Execution of a single `.phpt` test: the port of run-tests.php's `run_test()`.
//!
//! Flow: parse -> (SKIP for CGI/PHPDBG/REDIRECTTEST) -> EXTENSIONS check ->
//! environment and INI setup -> SKIPIF -> write `--FILE--` next to the test ->
//! run `php -n -d ... -f test.php` -> CLEAN -> compare -> artifacts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use regex::bytes::Regex as BytesRegex;

use crate::diff::{FirstDiff, LineMode, generate_diff};
use crate::exec::{self, Outcome, Spec, StderrMode, StdinMode, StdoutMode};
use crate::expect::{Matcher, PatternError, normalize_expected, normalize_output, php_trim};
use crate::ini::{IniSettings, default_ini_overwrites, extension_ini_overrides};
use crate::phpt::TestFile;
use crate::report::{Status, TestResult};
use crate::shell::{escape_arg, ltrim_php, php_empty, rtrim_php, trim_php};

/// Variables that describe a CGI request; SKIPIF and CLEAN must not see them.
const REQUEST_VARS: [&str; 4] = [
    "REQUEST_METHOD",
    "QUERY_STRING",
    "PATH_TRANSLATED",
    "SCRIPT_FILENAME",
];

/// Variables run-tests resets before applying a test's `--ENV--`.
const RESET_VARS: [&str; 8] = [
    "REDIRECT_STATUS",
    "QUERY_STRING",
    "PATH_TRANSLATED",
    "SCRIPT_FILENAME",
    "REQUEST_METHOD",
    "CONTENT_TYPE",
    "CONTENT_LENGTH",
    "TZ",
];

/// Everything the runner needs to know, shared by all tests of a run.
#[derive(Debug, Clone)]
pub struct Config {
    /// The PHP binary under test (absolute).
    pub php: PathBuf,
    /// Repository root: display names are relative to it.
    pub root: PathBuf,
    /// Working directory of every child process (`TEST_PHP_SRCDIR`).
    pub srcdir: PathBuf,
    pub timeout: Duration,
    /// Keep generated `.php` / `.skip.php` / `.clean.php` files and always
    /// write `.out` / `.exp`.
    pub keep_all: bool,
    /// Do not execute `--CLEAN--` sections.
    pub no_clean: bool,
    /// Lines of context in diffs.
    pub context: usize,
    /// `name=value` settings passed as `-d` to every run.
    pub ini_overwrites: Vec<String>,
    /// Base environment of the test processes.
    pub env: BTreeMap<String, String>,
    /// Lower-cased names of the extensions the binary has loaded, if known.
    pub extensions: Option<BTreeSet<String>>,
}

impl Config {
    /// Defaults mirroring run-tests.php (30 s timeout, `$ini_overwrites`).
    pub fn new(php: PathBuf, root: PathBuf) -> Self {
        let env = base_environment(&php);
        Self {
            srcdir: root.clone(),
            php,
            root,
            timeout: Duration::from_secs(30),
            keep_all: false,
            no_clean: false,
            context: 3,
            ini_overwrites: default_ini_overwrites(),
            env,
            extensions: None,
        }
    }

    /// Apply run-tests' extension-dependent INI overrides.
    pub fn apply_extension_overrides(&mut self) {
        let Some(exts) = &self.extensions else { return };
        let extra: Vec<&str> = exts
            .iter()
            .flat_map(|e| extension_ini_overrides(e).iter().copied())
            .collect();
        self.ini_overwrites
            .extend(extra.into_iter().map(String::from));
    }
}

/// Environment handed to tests: the runner's own, hardened like run-tests.
pub fn base_environment(php: &Path) -> BTreeMap<String, String> {
    use std::io::IsTerminal;

    let mut env: BTreeMap<String, String> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    for var in ["SSH_CLIENT", "SSH_AUTH_SOCK", "SSH_TTY", "SSH_CONNECTION"] {
        env.insert(var.to_string(), "deleted".to_string());
    }
    if env.get("TEMP").is_none_or(String::is_empty) {
        env.insert("TEMP".to_string(), temp_dir_string());
    }
    let php = php.to_string_lossy().into_owned();
    env.insert("TEST_PHP_EXECUTABLE_ESCAPED".to_string(), escape_arg(&php));
    env.insert("TEST_PHP_EXECUTABLE".to_string(), php);
    env.entry("SKIP_ONLINE_TESTS".to_string())
        .or_insert_with(|| "1".to_string());
    let all_tty = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal();
    if !all_tty {
        env.insert("SKIP_IO_CAPTURE_TESTS".to_string(), "1".to_string());
    }
    env
}

/// `sys_get_temp_dir()`: no trailing slash.
pub fn temp_dir_string() -> String {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Split an extension listing (`php -m` output or a comma list) into
/// normalised lower-case names (`Zend OPcache` becomes `opcache`).
pub fn parse_extension_list(text: &str) -> BTreeSet<String> {
    text.split([',', '\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .map(|l| {
            let l = l.to_lowercase();
            if l == "zend opcache" {
                "opcache".to_string()
            } else {
                l
            }
        })
        .collect()
}

/// Ask the binary which extensions it has loaded: `php -n -m`, falling back
/// to `get_loaded_extensions()` through `-r`.
pub fn probe_extensions(cfg: &Config) -> Option<BTreeSet<String>> {
    let env: Vec<(OsString, OsString)> = cfg
        .env
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    let run = |args: &[&str]| {
        exec::run(&Spec {
            program: cfg.php.clone(),
            args: args.iter().map(OsString::from).collect(),
            env: env.clone(),
            cwd: cfg.srcdir.clone(),
            stdin: StdinMode::Null,
            stdout: StdoutMode::Capture,
            stderr: StderrMode::Discard,
            timeout: Duration::from_secs(30),
        })
    };
    let attempts: [&[&str]; 2] = [
        &["-n", "-d", "display_errors=0", "-m"],
        &[
            "-n",
            "-d",
            "display_errors=0",
            "-r",
            "echo implode(',', get_loaded_extensions());",
        ],
    ];
    for args in attempts {
        let out = run(args);
        if out.spawn_error.is_some() || out.timed_out || out.exit_code != Some(0) {
            continue;
        }
        let exts = parse_extension_list(&String::from_utf8_lossy(&out.data));
        if !exts.is_empty() {
            return Some(exts);
        }
    }
    None
}

static SKIP_REASON: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?i-u)^skip\s*(.+)").expect("static regex"));
static INFO_REASON: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?i-u)^info\s*(.+)").expect("static regex"));
static WARN_REASON: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?i-u)^warn\s+(.+)").expect("static regex"));
static FLAKY_FUNCTIONS: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"(?i-u)\b(disk_free_space|hrtime|microtime|sleep|usleep)\(")
        .expect("static regex")
});
static FLAKY_OUTPUT: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(
        r"(?i-u)\b(404: page not found|address already in use|connection refused|deadlock|mailbox already exists|timed out)\b",
    )
    .expect("static regex")
});
static MAIL_PLACEHOLDER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{MAIL:(\S+)\}").expect("static regex"));
static ENV_PLACEHOLDER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{ENV:(\S+)\}").expect("static regex"));

/// `is_flaky()`: tests worth retrying once when they fail.
fn is_flaky(tf: &TestFile) -> bool {
    if tf.has("FLAKY") {
        return true;
    }
    const PERF_MARKER: &[u8] = b"SKIP_PERF_SENSITIVE";
    if tf
        .get("SKIPIF")
        .is_some_and(|s| s.windows(PERF_MARKER.len()).any(|w| w == PERF_MARKER))
    {
        return true;
    }
    tf.get("FILE")
        .is_some_and(|code| FLAKY_FUNCTIONS.is_match(code))
}

/// `error_may_be_retried()`.
fn error_may_be_retried(tf: &TestFile, output: &[u8]) -> bool {
    FLAKY_OUTPUT.is_match(output) || is_flaky(tf)
}

/// PHP truthiness of a string (`if ($clean_output)`).
fn truthy(s: &str) -> bool {
    !php_empty(s)
}

/// Captured bytes plus the notes run-tests appends (timeout / `Termsig=`).
fn annotated_output(out: &mut Outcome) -> Vec<u8> {
    let mut data = std::mem::take(&mut out.data);
    if out.timed_out {
        data.extend_from_slice(b"\n ** ERROR: process timed out **\n");
    } else if let Some(sig) = out.termsig() {
        data.extend_from_slice(format!("\nTermsig={sig}\n").as_bytes());
    }
    data
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
}

fn describe_first(first: &FirstDiff) -> String {
    let show = |b: &[u8]| truncate_chars(&String::from_utf8_lossy(b), 80);
    match (&first.expected, &first.actual) {
        (Some(e), Some(a)) => format!(
            "line {}: expected `{}`, got `{}`",
            first.line,
            show(e),
            show(a)
        ),
        (Some(e), None) => format!("line {}: missing `{}`", first.line, show(e)),
        (None, Some(a)) => format!("line {}: unexpected `{}`", first.line, show(a)),
        (None, None) => "output differs".to_string(),
    }
}

enum Attempt {
    Done(Box<TestResult>),
    Retry,
}

/// Runs tests; share one instance between worker threads.
pub struct Runner {
    cfg: Config,
    skip_cache: Mutex<HashMap<String, String>>,
}

impl Runner {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            skip_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Display name of a test: relative to the repository root when possible.
    pub fn display_name(&self, path: &Path) -> String {
        path.strip_prefix(&self.cfg.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    /// Run one test (retrying once if it looks flaky) and time it.
    pub fn run_test(&self, path: &Path) -> TestResult {
        let start = Instant::now();
        let attempt = match self.attempt(path, false) {
            Attempt::Retry => self.attempt(path, true),
            done => done,
        };
        let mut result = match attempt {
            Attempt::Done(r) => *r,
            Attempt::Retry => {
                let mut r = TestResult::new(path, &self.display_name(path), Status::Bork);
                r.reason = "internal error: retry requested twice".to_string();
                r
            }
        };
        result.duration = start.elapsed();
        result
    }

    fn php_spec(
        &self,
        args: Vec<OsString>,
        env: &BTreeMap<String, String>,
        stdin: StdinMode,
        stdout: StdoutMode,
        stderr: StderrMode,
    ) -> Spec {
        Spec {
            program: self.cfg.php.clone(),
            args,
            env: child_env(env),
            cwd: self.cfg.srcdir.clone(),
            stdin,
            stdout,
            stderr,
            timeout: self.cfg.timeout,
        }
    }

    /// Run a helper script (SKIPIF / CLEAN): stdout only, no stdin.
    fn run_script(
        &self,
        args: Vec<OsString>,
        env: &BTreeMap<String, String>,
    ) -> Result<Vec<u8>, String> {
        let spec = self.php_spec(
            args,
            env,
            StdinMode::Null,
            StdoutMode::Capture,
            StderrMode::Discard,
        );
        let mut out = exec::run(&spec);
        if let Some(e) = out.spawn_error.take() {
            return Err(e);
        }
        Ok(annotated_output(&mut out))
    }

    /// `SkipCache::checkSkip()`: evaluate a SKIPIF script, caching per
    /// (command, directory, code) so identical scripts run once.
    fn check_skip(
        &self,
        args: &[String],
        code: &[u8],
        file: &Path,
        env: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let dir = file.parent().unwrap_or(Path::new("."));
        let key = format!(
            "{}\0{}\0{}",
            args.join("\0"),
            dir.display(),
            String::from_utf8_lossy(code)
        );
        let cached = self
            .skip_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some(result) = cached {
            if self.cfg.keep_all {
                let _ = std::fs::write(file, code);
            }
            return Ok(result);
        }

        std::fs::write(file, code).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        let mut run_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        run_args.push(file.as_os_str().to_os_string());
        let result = self.run_script(run_args, env);
        if !self.cfg.keep_all {
            let _ = std::fs::remove_file(file);
        }
        let output = String::from_utf8_lossy(php_trim(&result?)).into_owned();
        if output.starts_with("nocache") {
            return Ok(String::new());
        }
        self.skip_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, output.clone());
        Ok(output)
    }

    fn attempt(&self, path: &Path, retried: bool) -> Attempt {
        let name = self.display_name(path);
        let finish = |status: Status, title: &str, reason: String, info: String| {
            let mut r = TestResult::new(path, &name, status);
            r.title = title.to_string();
            r.reason = reason;
            r.info = info;
            Attempt::Done(Box::new(r))
        };

        let mut tf = match TestFile::load(path) {
            Ok(tf) => tf,
            Err(b) => return finish(Status::Bork, "", b.0, String::new()),
        };
        let title = tf.title();
        let skip = |reason: &str| {
            finish(
                Status::Skip,
                &title,
                reason.to_string(),
                format!("reason: {reason}"),
            )
        };

        if tf.has("REDIRECTTEST") {
            return skip("REDIRECTTEST is not supported");
        }
        // CGI-only sections (GET, POST, COOKIE, ...) need php-cgi.
        if tf.is_cgi() {
            return skip("CGI not available");
        }
        if tf.has("PHPDBG") {
            return skip("phpdbg not available");
        }

        let (capture_stdin, capture_stdout, capture_stderr) = if tf.has("CAPTURE_STDIO") {
            let capture = tf.text("CAPTURE_STDIO").to_lowercase();
            (
                capture.contains("stdin"),
                capture.contains("stdout"),
                capture.contains("stderr"),
            )
        } else {
            (true, true, true)
        };

        // Artifact files live next to the test, like run-tests.
        let dir = tf.dir();
        let file_name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = file_name
            .strip_suffix("phpt")
            .unwrap_or(&file_name)
            .to_string();
        let artifact = |suffix: &str| dir.join(format!("{stem}{suffix}"));
        let diff_file = artifact("diff");
        let log_file = artifact("log");
        let exp_file = artifact("exp");
        let stdin_file = artifact("stdin");
        let out_file = artifact("out");
        let sh_file = artifact("sh");
        let test_file = artifact("php");
        let skipif_file = artifact("skip.php");
        let clean_file = artifact("clean.php");
        for old in [
            &diff_file,
            &log_file,
            &exp_file,
            &stdin_file,
            &out_file,
            &sh_file,
            &test_file,
            &skipif_file,
            &clean_file,
        ] {
            let _ = std::fs::remove_file(old);
        }

        // ---- environment -------------------------------------------------
        let mut env = self.cfg.env.clone();
        for var in RESET_VARS {
            env.insert(var.to_string(), String::new());
        }
        let dir_str = dir.to_string_lossy().into_owned();
        if tf.not_empty("ENV") {
            let text = tf.text("ENV").replace("{PWD}", &dir_str);
            for line in text.split('\n') {
                if let Some((key, value)) = trim_php(line).split_once('=')
                    && !php_empty(key)
                {
                    env.insert(key.to_string(), value.to_string());
                }
            }
        }

        // ---- required extensions ----------------------------------------
        if tf.has("EXTENSIONS") {
            let wanted: Vec<String> = tf
                .text("EXTENSIONS")
                .split(['\n', '\r'])
                .map(|e| trim_php(e).to_string())
                .filter(|e| !e.is_empty())
                .collect();
            if !wanted.is_empty() {
                let Some(loaded) = &self.cfg.extensions else {
                    return skip("unable to determine the loaded extensions of the PHP binary");
                };
                let missing: Vec<&str> = wanted
                    .iter()
                    .map(String::as_str)
                    .filter(|e| !loaded.contains(&e.to_lowercase()))
                    .collect();
                if !missing.is_empty() {
                    return skip(&format!(
                        "Required extension{} missing: {}",
                        if missing.len() > 1 { "s" } else { "" },
                        missing.join(", ")
                    ));
                }
            }
        }

        // ---- ini settings ------------------------------------------------
        let mut ini = IniSettings::new();
        ini.apply_lines(&self.cfg.ini_overwrites);
        let orig_ini_args = ini.to_args();
        if tf.has("INI") {
            let mut text = tf
                .text("INI")
                .replace("{PWD}", &dir_str)
                .replace("{TMP}", &temp_dir_string());
            text = MAIL_PLACEHOLDER
                .replace_all(&text, "tee $1 >/dev/null")
                .into_owned();
            let mut missing_env = None;
            text = ENV_PLACEHOLDER
                .replace_all(&text, |caps: &regex::Captures<'_>| {
                    match self.cfg.env.get(&caps[1]) {
                        Some(v) => v.clone(),
                        None => {
                            missing_env = Some(caps[1].to_string());
                            String::new()
                        }
                    }
                })
                .into_owned();
            if let Some(var) = missing_env {
                return skip(&format!("Environment variable {var} is not set"));
            }
            ini.apply_lines(text.split(['\n', '\r']).filter(|l| !l.is_empty()));
        }
        env.insert(
            "TEST_PHP_EXTRA_ARGS".to_string(),
            format!("{} {}", escape_arg("-n"), ini.to_params()),
        );

        // ---- SKIPIF ------------------------------------------------------
        let mut warn_info: Option<String> = None;
        if tf.not_empty("SKIPIF") {
            let mut skip_env = env.clone();
            for var in REQUEST_VARS {
                skip_env.remove(var);
            }
            skip_env.insert("TEST_PHP_EVALUATING_SKIPIF".to_string(), "1".to_string());

            let mut args: Vec<String> = vec!["-n".into(), "-q".into()];
            args.extend(orig_ini_args.iter().cloned());
            for extra in [
                "-d",
                "opcache.file_cache=",
                "-d",
                "opcache.file_cache_only=0",
                "-d",
                "display_errors=1",
                "-d",
                "display_startup_errors=0",
            ] {
                args.push(extra.to_string());
            }
            let code = tf.get("SKIPIF").unwrap_or_default().to_vec();
            let output = match self.check_skip(&args, &code, &skipif_file, &skip_env) {
                Ok(o) => o,
                Err(e) => {
                    return finish(
                        Status::Bork,
                        &title,
                        format!("cannot run SKIPIF: {e}"),
                        String::new(),
                    );
                }
            };
            let out_bytes = output.as_bytes();
            let starts = |prefix: &str| {
                output.len() >= prefix.len()
                    && output.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            };

            if starts("skip") {
                let reason = SKIP_REASON
                    .captures(out_bytes)
                    .map(|m| String::from_utf8_lossy(&m[1]).into_owned())
                    .unwrap_or_default();
                return finish(
                    Status::Skip,
                    &title,
                    reason.clone(),
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!("reason: {reason}")
                    },
                );
            }
            if starts("info") && INFO_REASON.is_match(out_bytes) {
                // Informational only.
            } else if starts("warn")
                && let Some(m) = WARN_REASON.captures(out_bytes)
            {
                warn_info = Some(format!("(warn: {})", String::from_utf8_lossy(&m[1])));
            } else if starts("xfail") {
                tf.set("XFAIL", ltrim_php(&output[5..]).as_bytes());
            } else if starts("xleak") {
                tf.set("XLEAK", ltrim_php(&output[5..]).as_bytes());
            } else if starts("flaky") {
                tf.set("FLAKY", ltrim_php(&output[5..]).as_bytes());
            } else if !output.is_empty() {
                return finish(
                    Status::Bork,
                    &title,
                    format!(
                        "invalid output from SKIPIF: {}",
                        truncate_chars(&output, 200)
                    ),
                    String::new(),
                );
            }
        }

        // ---- run the test ------------------------------------------------
        let file_section = tf.get("FILE").unwrap_or_default().to_vec();
        if let Err(e) = std::fs::write(&test_file, &file_section) {
            return finish(
                Status::Bork,
                &title,
                format!("cannot write {}: {e}", test_file.display()),
                String::new(),
            );
        }
        let test_file_str = test_file.to_string_lossy().into_owned();

        env.insert("REDIRECT_STATUS".to_string(), "1".to_string());
        for var in ["PATH_TRANSLATED", "SCRIPT_FILENAME"] {
            if env.get(var).is_none_or(|v| php_empty(v)) {
                env.insert(var.to_string(), test_file_str.clone());
            }
        }
        env.insert("HTTP_COOKIE".to_string(), String::new());
        env.insert("REQUEST_METHOD".to_string(), "GET".to_string());
        env.insert("CONTENT_TYPE".to_string(), String::new());
        env.insert("CONTENT_LENGTH".to_string(), String::new());

        let mut php_args: Vec<String> = vec!["-n".to_string()];
        php_args.extend(ini.to_args());
        php_args.push("-f".to_string());
        php_args.push(test_file_str.clone());

        let (program_args, shell_line): (Vec<OsString>, String) = if tf.has("ARGS") {
            // The raw ARGS section is shell syntax, exactly as in run-tests.
            let php = self.cfg.php.to_string_lossy();
            let line = format!(
                "{} {}",
                escape_arg(&php),
                php_args
                    .iter()
                    .map(|a| escape_arg(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let mut script = format!("exec {line} -- ").into_bytes();
            script.extend_from_slice(tf.get("ARGS").unwrap_or_default());
            (vec![OsString::from("-c"), bytes_to_os(script)], line)
        } else {
            (php_args.iter().map(OsString::from).collect(), String::new())
        };

        let stdin = if capture_stdin {
            StdinMode::Data(tf.get("STDIN").unwrap_or_default().to_vec())
        } else {
            StdinMode::Inherit
        };
        let (stdout_mode, stderr_mode) = match (capture_stdout, capture_stderr) {
            (true, true) => (StdoutMode::Capture, StderrMode::Merge),
            (true, false) => (StdoutMode::Capture, StderrMode::Inherit),
            (false, true) => (StdoutMode::Inherit, StderrMode::Capture),
            (false, false) => (StdoutMode::Inherit, StderrMode::Inherit),
        };
        let mut spec = self.php_spec(program_args, &env, stdin, stdout_mode, stderr_mode);
        if tf.has("ARGS") {
            spec.program = PathBuf::from("/bin/sh");
        }
        let mut outcome = exec::run(&spec);
        if let Some(e) = outcome.spawn_error.take() {
            return finish(
                Status::Bork,
                &title,
                format!("cannot execute php: {e}"),
                String::new(),
            );
        }
        if exec::interrupted() {
            let _ = std::fs::remove_file(&test_file);
            return finish(
                Status::Bork,
                &title,
                "interrupted".to_string(),
                String::new(),
            );
        }
        let timed_out = outcome.timed_out;
        let overflowed = outcome.overflowed;
        let raw_output = annotated_output(&mut outcome);

        // ---- CLEAN -------------------------------------------------------
        let mut clean_output: Option<String> = None;
        if tf.not_empty("CLEAN") && (!self.cfg.no_clean || self.cfg.keep_all) {
            let code = php_trim(tf.get("CLEAN").unwrap_or_default()).to_vec();
            let _ = std::fs::write(&clean_file, &code);
            if !self.cfg.no_clean {
                let mut clean_env = env.clone();
                for var in REQUEST_VARS {
                    clean_env.remove(var);
                }
                let mut args: Vec<OsString> = vec!["-n".into(), "-q".into()];
                args.extend(orig_ini_args.iter().map(OsString::from));
                for extra in [
                    "-d",
                    "opcache.file_cache=",
                    "-d",
                    "opcache.file_cache_only=0",
                ] {
                    args.push(extra.into());
                }
                args.push(clean_file.as_os_str().to_os_string());
                clean_output = match self.run_script(args, &clean_env) {
                    Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(e) => Some(format!("cannot run CLEAN: {e}")),
                };
            }
            if !self.cfg.keep_all {
                let _ = std::fs::remove_file(&clean_file);
            }
        }

        // ---- compare -----------------------------------------------------
        let output = normalize_output(&raw_output);
        let mut pattern_error: Option<PatternError> = None;
        let (wanted, line_mode, matched) = if let Some(section) = tf.get("EXPECTF") {
            let wanted = normalize_expected(section);
            let matched = match Matcher::expectf(&wanted) {
                Ok(m) => m.is_match(&output),
                Err(e) => {
                    pattern_error = Some(e);
                    false
                }
            };
            (wanted, LineMode::Format, matched)
        } else if let Some(section) = tf.get("EXPECTREGEX") {
            let wanted = normalize_expected(section);
            let matched = match Matcher::regex(&wanted) {
                Ok(m) => m.is_match(&output),
                Err(e) => {
                    pattern_error = Some(e);
                    false
                }
            };
            (wanted, LineMode::Exact, matched)
        } else {
            let wanted = normalize_expected(tf.get("EXPECT").unwrap_or_default());
            let matched = output == wanted;
            (wanted, LineMode::Exact, matched)
        };
        let passed = matched && !timed_out && !overflowed;

        if !passed
            && !retried
            && !timed_out
            && !overflowed
            && pattern_error.is_none()
            && error_may_be_retried(&tf, &output)
        {
            return Attempt::Retry;
        }

        if self.cfg.keep_all {
            let _ = std::fs::write(&exp_file, &wanted);
            let _ = std::fs::write(&out_file, &output);
        }

        if passed {
            if !self.cfg.keep_all {
                let _ = std::fs::remove_file(&test_file);
            }
            if let Some(clean) = clean_output.as_deref().filter(|c| truthy(c)) {
                return finish(
                    Status::Bork,
                    &title,
                    format!(
                        "invalid output from CLEAN: {}",
                        truncate_chars(clean.trim(), 200)
                    ),
                    String::new(),
                );
            }
            if tf.has("XFAIL") {
                return finish(
                    Status::XPass,
                    &title,
                    "XFAIL section but test passes".to_string(),
                    "(warn: XFAIL section but test passes)".to_string(),
                );
            }
            if retried {
                return finish(
                    Status::Warn,
                    &title,
                    "passed on retry attempt".to_string(),
                    "(warn: Test passed on retry attempt)".to_string(),
                );
            }
            return finish(Status::Pass, &title, String::new(), String::new());
        }

        // ---- failure: write artifacts -----------------------------------
        let report = generate_diff(&wanted, line_mode, &output, self.cfg.context);
        let _ = std::fs::write(&exp_file, &wanted);
        let _ = std::fs::write(&out_file, &output);
        let _ = std::fs::write(&diff_file, &report.text);
        let mut log = b"\n---- EXPECTED OUTPUT\n".to_vec();
        log.extend_from_slice(&wanted);
        log.extend_from_slice(b"\n---- ACTUAL OUTPUT\n");
        log.extend_from_slice(&output);
        log.extend_from_slice(b"\n---- FAILED\n");
        let _ = std::fs::write(&log_file, log);
        if tf.has("STDIN") {
            let _ = std::fs::write(&stdin_file, tf.get("STDIN").unwrap_or_default());
        }
        self.write_repro_script(
            &sh_file,
            &env,
            &spec,
            &shell_line,
            &tf,
            capture_stdout && capture_stderr,
        );

        let has_xfail = tf.has("XFAIL");
        let status = if has_xfail {
            Status::XFail
        } else if timed_out {
            Status::Timeout
        } else {
            Status::Fail
        };
        let reason = if timed_out {
            format!("timed out after {}s", self.cfg.timeout.as_secs())
        } else if overflowed {
            "output exceeded the capture limit".to_string()
        } else if let Some(e) = &pattern_error {
            format!("cannot use expectation: {e}")
        } else {
            report
                .first
                .as_ref()
                .map_or_else(|| "output differs".to_string(), describe_first)
        };
        let info = if has_xfail {
            format!("  XFAIL REASON: {}", rtrim_php(&tf.text("XFAIL")))
        } else if matches!(status, Status::Timeout) {
            format!("({reason})")
        } else {
            warn_info.unwrap_or_default()
        };

        let mut result = TestResult::new(path, &name, status);
        result.title = title.clone();
        result.reason = reason;
        result.info = info;
        result.diff = Some(report.text);
        Attempt::Done(Box::new(result))
    }

    /// `.sh` file that re-runs the failed test with the same environment.
    fn write_repro_script(
        &self,
        file: &Path,
        env: &BTreeMap<String, String>,
        spec: &Spec,
        shell_line: &str,
        tf: &TestFile,
        merged: bool,
    ) {
        let mut script = String::from("#!/bin/sh\n\nunset $(env | cut -d= -f1)\n");
        for (k, v) in env {
            if !v.is_empty() {
                script.push_str(&format!("export {k}={}\n", escape_arg(v)));
            }
        }
        let command = if tf.has("ARGS") {
            format!(
                "{shell_line} -- {}",
                String::from_utf8_lossy(tf.get("ARGS").unwrap_or_default()).trim_end()
            )
        } else {
            let mut parts = vec![escape_arg(&spec.program.to_string_lossy())];
            parts.extend(spec.args.iter().map(|a| escape_arg(&a.to_string_lossy())));
            parts.join(" ")
        };
        script.push_str(&format!(
            "\nexec {command}{}\n",
            if merged { " 2>&1" } else { "" }
        ));
        if std::fs::write(file, script).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

/// Environment for `Spec`: empty values are not passed (as with `proc_open`).
fn child_env(env: &BTreeMap<String, String>) -> Vec<(OsString, OsString)> {
    env.iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
}

#[cfg(unix)]
fn bytes_to_os(bytes: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes)
}

#[cfg(not(unix))]
fn bytes_to_os(bytes: Vec<u8>) -> OsString {
    OsString::from(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> TestFile {
        TestFile::parse(Path::new("/x/t.phpt"), src.as_bytes()).unwrap()
    }

    #[test]
    fn extension_list_parsing() {
        let modules = "[PHP Modules]\nCore\ndate\nJSON\n\n[Zend Modules]\nZend OPcache\n";
        let set = parse_extension_list(modules);
        assert!(set.contains("core") && set.contains("date") && set.contains("json"));
        assert!(set.contains("opcache"));
        assert!(!set.iter().any(|e| e.starts_with('[')));
        let set = parse_extension_list("Core,standard,SPL");
        assert_eq!(set.len(), 3);
        assert!(set.contains("spl"));
        assert!(parse_extension_list("").is_empty());
    }

    #[test]
    fn flaky_detection() {
        let base = "--TEST--\nt\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n";
        assert!(!is_flaky(&parse(base)));
        assert!(is_flaky(&parse(
            "--TEST--\nt\n--FILE--\n<?php usleep(10);\n--EXPECT--\n1\n"
        )));
        assert!(is_flaky(&parse(
            "--TEST--\nt\n--FILE--\n<?php $t = microtime(true);\n--EXPECT--\n1\n"
        )));
        assert!(!is_flaky(&parse(
            "--TEST--\nt\n--FILE--\n<?php function mysleep() {}\n--EXPECT--\n1\n"
        )));
        assert!(is_flaky(&parse(
            "--TEST--\nt\n--FLAKY--\nwhy\n--FILE--\nx\n--EXPECT--\n1\n"
        )));
        assert!(is_flaky(&parse(
            "--TEST--\nt\n--SKIPIF--\n<?php getenv('SKIP_PERF_SENSITIVE');\n--FILE--\nx\n--EXPECT--\n1\n"
        )));
    }

    #[test]
    fn flaky_output_detection() {
        let tf = parse("--TEST--\nt\n--FILE--\nx\n--EXPECT--\n1\n");
        assert!(error_may_be_retried(&tf, b"... Connection refused ..."));
        assert!(error_may_be_retried(&tf, b"deadlock found"));
        assert!(!error_may_be_retried(&tf, b"all fine"));
        assert!(!error_may_be_retried(&tf, b"undeadlocked"));
    }

    #[test]
    fn skip_reason_extraction() {
        let m = SKIP_REASON.captures(b"skip  Not supported here").unwrap();
        assert_eq!(&m[1], b"Not supported here");
        let m = SKIP_REASON.captures(b"SKIP: x").unwrap();
        assert_eq!(&m[1], b": x");
        assert!(SKIP_REASON.captures(b"skip").is_none());
        assert!(WARN_REASON.is_match(b"warn something"));
        assert!(!WARN_REASON.is_match(b"warn"));
    }

    #[test]
    fn annotated_output_appends_run_tests_notes() {
        let mut out = Outcome {
            data: b"x".to_vec(),
            timed_out: true,
            ..Outcome::default()
        };
        assert_eq!(
            annotated_output(&mut out),
            b"x\n ** ERROR: process timed out **\n"
        );
        let mut out = Outcome {
            data: b"x".to_vec(),
            signal: Some(11),
            ..Outcome::default()
        };
        assert_eq!(annotated_output(&mut out), b"x\nTermsig=11\n");
        let mut out = Outcome {
            data: b"x".to_vec(),
            exit_code: Some(1),
            ..Outcome::default()
        };
        assert_eq!(annotated_output(&mut out), b"x");
    }

    #[test]
    fn truncation_is_char_aware() {
        assert_eq!(truncate_chars("héllo", 10), "héllo");
        assert_eq!(truncate_chars("héllo wörld", 3), "hél...");
    }

    #[test]
    fn child_env_drops_empty_values() {
        let mut env = BTreeMap::new();
        env.insert("A".to_string(), "1".to_string());
        env.insert("B".to_string(), String::new());
        let out = child_env(&env);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, OsString::from("A"));
    }

    #[test]
    fn base_environment_sets_run_tests_variables() {
        let env = base_environment(Path::new("/opt/my php/bin/php"));
        assert_eq!(env["TEST_PHP_EXECUTABLE"], "/opt/my php/bin/php");
        assert_eq!(env["TEST_PHP_EXECUTABLE_ESCAPED"], "'/opt/my php/bin/php'");
        assert_eq!(env["SSH_AUTH_SOCK"], "deleted");
        assert!(!env["TEMP"].is_empty());
        assert!(env.contains_key("SKIP_ONLINE_TESTS"));
    }
}
