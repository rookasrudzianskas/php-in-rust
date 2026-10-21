//! Command line handling and orchestration of a run.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::exec;
use crate::report::{self, Status, TestResult};
use crate::runner::{self, Config, Runner};
use crate::sched::{Scheduler, collect_conflicts};

const USAGE: &str = "\
Synopsis:
    php-run-tests [options] <test files or directories>...

Runs php-src style .phpt tests against a PHP binary (port of run-tests.php).

Options:
    -p <php>              PHP binary to test (default: $TEST_PHP_EXECUTABLE, then
                          $CARGO_TARGET_DIR/{release,debug}/php, then
                          target/release/php and target/debug/php in the workspace root)
    -j<N>                 Run N tests in parallel (default: number of CPU cores)
    -q, --quiet           Only print problems (hide PASS and SKIP lines)
    --show-diff           Print the diff of every failing test
    --keep-all            Keep generated .php/.skip.php/.clean.php files and write
                          .out/.exp even for passing tests
    --timeout <secs>      Per-process timeout (default: 30)
    --filter <substr>     Only run tests whose path contains <substr>
    --json <file>         Write machine-readable results to <file>
    --passing-list <file> Regression mode: run exactly the tests listed in <file>
                          (paths relative to the repository root, '#' comments
                          allowed); exit non-zero unless all of them PASS
    --update-passing <file>
                          After the run, rewrite <file> with every test that
                          passes (sorted; entries not part of this run are kept
                          if their file still exists)
    --summary-by-dir      Print pass/fail counts per directory
    -d <name=value>       Extra INI setting passed to every php invocation
    --srcdir <dir>        Working directory of test processes (default: the
                          repository root, or $TEST_PHP_SRCDIR)
    --context <n>         Lines of context in diffs (default: 3)
    --no-clean            Do not run --CLEAN-- sections
    -x                    Set SKIP_SLOW_TESTS
    --online, --offline   Clear/set SKIP_ONLINE_TESTS (default: offline)
    --color, --no-color   Force/disable colored result labels
    -h, --help            This help

Exit status: 0 if no test FAILed, was BORKed or timed out (and, with
--passing-list, every listed test passed); 1 otherwise; 2 on usage errors.
Environment: TEST_PHP_EXECUTABLE, TEST_PHP_SRCDIR, REPORT_EXIT_STATUS=0,
NO_PHPTEST_SUMMARY, NO_COLOR.
";

/// Parsed command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub php: Option<PathBuf>,
    pub jobs: Option<usize>,
    pub quiet: bool,
    pub show_diff: bool,
    pub keep_all: bool,
    pub timeout: Option<u64>,
    pub filter: Option<String>,
    pub json: Option<PathBuf>,
    pub passing_list: Option<PathBuf>,
    pub update_passing: Option<PathBuf>,
    pub summary_by_dir: bool,
    pub ini: Vec<String>,
    pub context: Option<usize>,
    pub color: Option<bool>,
    pub srcdir: Option<PathBuf>,
    pub no_clean: bool,
    pub skip_slow: bool,
    pub online: Option<bool>,
    pub paths: Vec<PathBuf>,
}

/// Result of argument parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Box<Options>),
}

fn parse_number<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("'{value}' is not a valid number for {name}"))
}

/// Parse command line arguments (without the program name).
pub fn parse_args<I>(args: I) -> Result<Parsed, String>
where
    I: IntoIterator<Item = String>,
{
    let mut o = Options::default();
    let mut it = args.into_iter();
    let mut only_paths = false;

    while let Some(arg) = it.next() {
        if only_paths || !arg.starts_with('-') || arg == "-" {
            o.paths.push(PathBuf::from(arg));
            continue;
        }
        // `--name=value` form.
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if arg.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .ok_or_else(|| format!("option {name} requires {what}")),
            }
        };

        match name.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--" => only_paths = true,
            "-p" => o.php = Some(PathBuf::from(value("a PHP binary")?)),
            "-q" | "--quiet" => o.quiet = true,
            "--show-diff" => o.show_diff = true,
            "--keep-all" => o.keep_all = true,
            "--timeout" | "--set-timeout" => {
                let secs: u64 = parse_number("--timeout", &value("seconds")?)?;
                if secs == 0 {
                    return Err("--timeout must be at least 1 second".to_string());
                }
                o.timeout = Some(secs);
            }
            "--filter" => o.filter = Some(value("a substring")?),
            "--json" => o.json = Some(PathBuf::from(value("a file")?)),
            "--passing-list" => o.passing_list = Some(PathBuf::from(value("a file")?)),
            "--update-passing" => o.update_passing = Some(PathBuf::from(value("a file")?)),
            "--summary-by-dir" => o.summary_by_dir = true,
            "-d" => o.ini.push(value("name=value")?),
            "--srcdir" => o.srcdir = Some(PathBuf::from(value("a directory")?)),
            "--context" => o.context = Some(parse_number("--context", &value("a number")?)?),
            "--color" => o.color = Some(true),
            "--no-color" => o.color = Some(false),
            "--no-clean" => o.no_clean = true,
            "-x" => o.skip_slow = true,
            "--online" => o.online = Some(true),
            "--offline" => o.online = Some(false),
            // Accepted for run-tests compatibility: php is always run with -n.
            "-n" => {}
            "--jobs" => o.jobs = Some(parse_jobs(&value("a number")?)?),
            _ if name.starts_with("-j") && !name.starts_with("--") => {
                let rest = &name[2..];
                let n = if rest.is_empty() {
                    it.next().ok_or("option -j requires a number")?
                } else {
                    rest.to_string()
                };
                o.jobs = Some(parse_jobs(&n)?);
            }
            _ => return Err(format!("unknown option '{arg}'")),
        }
    }
    Ok(Parsed::Run(Box::new(o)))
}

fn parse_jobs(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!(
            "'{text}' is not a valid number of workers, try e.g. -j8"
        )),
    }
}

/// Walk up from `start` to the directory whose `Cargo.toml` declares the workspace.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        manifest
            .lines()
            .any(|l| l.trim() == "[workspace]")
            .then(|| dir.to_path_buf())
    })
}

/// Recursively collect `*.phpt` files (like `find_files()`): hidden files and
/// `.git` / `.svn` directories are skipped.
pub fn find_tests(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    fn walk(
        dir: &Path,
        out: &mut Vec<PathBuf>,
        seen: &mut HashSet<PathBuf>,
    ) -> std::io::Result<()> {
        if let Ok(canon) = std::fs::canonicalize(dir)
            && !seen.insert(canon)
        {
            return Ok(()); // symlink loop
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let path = entry.path();
            // Follow symlinks to find out what the entry really is.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if name_str != ".git" && name_str != ".svn" {
                    walk(&path, out, seen)?;
                }
            } else if name_str.ends_with(".phpt") && !name_str.starts_with('.') {
                out.push(std::fs::canonicalize(&path).unwrap_or(path));
            }
        }
        Ok(())
    }
    walk(dir, out, &mut HashSet::new())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Locate the PHP binary: `-p`, `TEST_PHP_EXECUTABLE`, then cargo output dirs.
fn resolve_php(opt: Option<&Path>, root: &Path) -> Result<PathBuf, String> {
    let explicit = opt.map(Path::to_path_buf).or_else(|| {
        std::env::var_os("TEST_PHP_EXECUTABLE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    let candidate = match explicit {
        Some(p) => p,
        None => {
            let mut candidates = Vec::new();
            if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
                let target = PathBuf::from(target);
                let target = if target.is_absolute() {
                    target
                } else {
                    root.join(target)
                };
                candidates.push(target.join("release/php"));
                candidates.push(target.join("debug/php"));
            }
            candidates.push(root.join("target/release/php"));
            candidates.push(root.join("target/debug/php"));
            candidates
                .iter()
                .find(|c| c.is_file())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no PHP binary found (looked for {}); pass -p <php> or set TEST_PHP_EXECUTABLE",
                        candidates
                            .iter()
                            .map(|c| c.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
        }
    };
    let php = std::fs::canonicalize(&candidate).map_err(|e| {
        format!(
            "PHP executable '{}' does not exist ({e})",
            candidate.display()
        )
    })?;
    if !is_executable(&php) {
        return Err(format!("invalid PHP executable '{}'", php.display()));
    }
    Ok(php)
}

/// Removes the shared probe-cache directory when the run ends.
struct SharedCache(Option<PathBuf>);

impl SharedCache {
    fn create() -> Self {
        if std::env::var("TEST_PHP_SHARED_CACHE").is_ok_and(|v| v == "0") {
            return Self(None);
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = PathBuf::from(runner::temp_dir_string())
            .join(format!("php-run-tests-{}-{nanos:08x}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        Self(builder.create(&dir).ok().map(|()| dir))
    }
}

impl Drop for SharedCache {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn emit(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn emit_str(text: &str) {
    emit(text.as_bytes());
}

/// Run the selected tests on `jobs` threads, calling `on_result` (on the
/// calling thread) as each finishes.
fn run_all(
    runner: &Runner,
    tests: &[PathBuf],
    jobs: usize,
    mut on_result: impl FnMut(&TestResult),
) -> Vec<TestResult> {
    let conflicts = if jobs > 1 {
        collect_conflicts(tests)
    } else {
        vec![Vec::new(); tests.len()]
    };
    let sched = Scheduler::new(conflicts);
    let (tx, rx) = mpsc::channel::<(usize, TestResult)>();
    let mut results: Vec<(usize, TestResult)> = Vec::with_capacity(tests.len());

    thread::scope(|scope| {
        for _ in 0..jobs {
            let tx = tx.clone();
            let sched = &sched;
            scope.spawn(move || {
                while !exec::interrupted()
                    && let Some(ticket) = sched.next()
                {
                    let index = ticket.index;
                    let path = &tests[index];
                    let result =
                        std::panic::catch_unwind(AssertUnwindSafe(|| runner.run_test(path)))
                            .unwrap_or_else(|_| {
                                let mut r =
                                    TestResult::new(path, &runner.display_name(path), Status::Bork);
                                r.reason =
                                    "the runner panicked while executing this test".to_string();
                                r
                            });
                    sched.finish(ticket);
                    if exec::interrupted() {
                        // The result may be garbage (the child was killed).
                        sched.cancel();
                        break;
                    }
                    if tx.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (index, result) in rx {
            on_result(&result);
            results.push((index, result));
        }
    });

    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

fn read_list_file(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read passing list {}: {e}", path.display()))?;
    Ok(report::parse_list(&text))
}

/// Execute a run; returns the process exit code.
pub fn run(opts: &Options) -> Result<ExitCode, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("cannot determine the working directory: {e}"))?;
    let root = std::env::var_os("TEST_PHP_SRCDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .and_then(|p| std::fs::canonicalize(p).ok())
        .or_else(|| find_workspace_root(&cwd))
        .unwrap_or_else(|| cwd.clone());
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let srcdir = match &opts.srcdir {
        Some(dir) => std::fs::canonicalize(dir)
            .map_err(|e| format!("invalid --srcdir '{}': {e}", dir.display()))?,
        None => root.clone(),
    };

    // ---- select tests ------------------------------------------------------
    let mut tests: Vec<PathBuf> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    if let Some(list) = &opts.passing_list {
        if !opts.paths.is_empty() {
            eprintln!(
                "php-run-tests: ignoring test paths: --passing-list runs exactly the listed tests"
            );
        }
        for entry in read_list_file(list)? {
            match std::fs::canonicalize(root.join(&entry)) {
                Ok(path) if path.is_file() => tests.push(path),
                _ => missing.push(entry),
            }
        }
    } else {
        for arg in &opts.paths {
            let path = std::fs::canonicalize(arg)
                .map_err(|_| format!("Cannot find test file \"{}\".", arg.display()))?;
            if path.is_dir() {
                find_tests(&path, &mut tests)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            } else if path.extension().is_some_and(|e| e == "phpt") {
                tests.push(path);
            } else {
                return Err(format!("Cannot find test file \"{}\".", arg.display()));
            }
        }
        if opts.paths.is_empty() {
            return Err(
                "no tests specified (give .phpt files or directories, or --passing-list <file>)"
                    .to_string(),
            );
        }
    }
    tests.sort();
    tests.dedup();
    if let Some(filter) = &opts.filter {
        tests.retain(|t| {
            t.strip_prefix(&root)
                .unwrap_or(t)
                .to_string_lossy()
                .contains(filter.as_str())
        });
        missing.retain(|m| m.contains(filter.as_str()));
    }
    if tests.is_empty() && missing.is_empty() {
        println!("No tests found.");
        return Ok(ExitCode::SUCCESS);
    }

    // ---- configuration ------------------------------------------------------
    let php = resolve_php(opts.php.as_deref(), &root)?;
    let mut cfg = Config::new(php.clone(), root.clone());
    cfg.srcdir = srcdir;
    cfg.keep_all = opts.keep_all;
    cfg.no_clean = opts.no_clean;
    if let Some(secs) = opts.timeout {
        cfg.timeout = Duration::from_secs(secs);
        cfg.env.insert("TEST_TIMEOUT".to_string(), secs.to_string());
    }
    if let Some(n) = opts.context {
        cfg.context = n;
    }
    cfg.ini_overwrites.extend(opts.ini.iter().cloned());
    if opts.quiet {
        cfg.env
            .insert("NO_INTERACTION".to_string(), "1".to_string());
    }
    if opts.skip_slow {
        cfg.env
            .insert("SKIP_SLOW_TESTS".to_string(), "1".to_string());
    }
    if let Some(online) = opts.online {
        cfg.env.insert(
            "SKIP_ONLINE_TESTS".to_string(),
            if online { "0" } else { "1" }.to_string(),
        );
    }
    let shared_cache = SharedCache::create();
    if let Some(dir) = &shared_cache.0 {
        cfg.env.insert(
            "TEST_PHP_SHARED_CACHE_DIR".to_string(),
            dir.to_string_lossy().into_owned(),
        );
    }
    cfg.extensions = runner::probe_extensions(&cfg);
    cfg.apply_extension_overrides();

    let jobs = opts
        .jobs
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, usize::from))
        .clamp(1, tests.len().max(1));
    let color = opts.color.unwrap_or_else(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    });
    let show_diff = opts.show_diff || tests.len() + missing.len() == 1;

    emit_str(&format!(
        "\
=====================================================================
PHP         : {}
CWD         : {}
Extensions  : {}
Tests       : {} (workers: {jobs}, timeout: {}s)
=====================================================================
",
        php.display(),
        cfg.srcdir.display(),
        cfg.extensions.as_ref().map_or_else(
            || "unknown (php -m failed)".to_string(),
            |e| e.len().to_string()
        ),
        tests.len() + missing.len(),
        cfg.timeout.as_secs()
    ));

    // ---- run ---------------------------------------------------------------
    let runner = Runner::new(cfg);
    let start = Instant::now();
    let quiet = opts.quiet;
    let print_result = |r: &TestResult| {
        if quiet && matches!(r.status, Status::Pass | Status::Skip) {
            return;
        }
        emit_str(&format!("{}\n", report::result_line(r, color)));
        if show_diff
            && matches!(r.status, Status::Fail | Status::Timeout | Status::XFail)
            && let Some(diff) = &r.diff
        {
            emit(b"\n========DIFF========\n");
            let end = diff
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map_or(0, |p| p + 1);
            emit(&diff[..end]);
            emit(b"\n========DONE========\n");
        }
    };

    let mut results = Vec::with_capacity(tests.len() + missing.len());
    for entry in &missing {
        let mut r = TestResult::new(&root.join(entry), entry, Status::Fail);
        r.title = "listed test file not found".to_string();
        r.reason = "listed in the passing list but the file does not exist".to_string();
        print_result(&r);
        results.push(r);
    }
    exec::install_interrupt_handler();
    results.extend(run_all(&runner, &tests, jobs, print_result));
    let elapsed = start.elapsed();
    let was_interrupted = exec::interrupted();
    results.sort_by(|a, b| a.name.cmp(&b.name));

    // ---- report ------------------------------------------------------------
    emit_str("=====================================================================");
    emit_str(&report::summary_text(&results, elapsed));
    if std::env::var_os("NO_PHPTEST_SUMMARY").is_none() {
        emit_str(&report::failed_summary(&results));
    }
    if opts.summary_by_dir {
        emit_str(&format!("\n{}", report::dir_summary(&results)));
    }
    if let Some(path) = &opts.json {
        let json = report::json_report(&php.to_string_lossy(), &results, elapsed);
        std::fs::write(path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }

    if was_interrupted {
        emit_str(&format!(
            "\nInterrupted: only {} of {} tests were run; the passing list was not evaluated or updated.\n",
            results.len(),
            tests.len() + missing.len()
        ));
        drop(shared_cache);
        return Ok(ExitCode::from(130));
    }

    let mut failed = false;
    if opts.passing_list.is_some() {
        let regressions: Vec<&TestResult> = results
            .iter()
            .filter(|r| r.status != Status::Pass)
            .collect();
        if regressions.is_empty() {
            emit_str(&format!(
                "\nPassing list: all {} listed tests pass.\n",
                results.len()
            ));
        } else {
            emit_str(&format!(
                "\nPassing list REGRESSION: {} of {} listed tests do not pass:\n",
                regressions.len(),
                results.len()
            ));
            for r in regressions {
                emit_str(&format!("  {} {}\n", r.status.label(), r.name));
            }
            failed = true;
        }
    }
    if let Some(path) = &opts.update_passing {
        let existing = std::fs::read_to_string(path).ok();
        let update = report::update_list(existing.as_deref(), &results, |entry| {
            root.join(entry).is_file()
        });
        if existing.as_deref() != Some(update.text.as_str()) {
            std::fs::write(path, &update.text)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        emit_str(&format!(
            "\nPassing list {}: {} entries (+{} added, -{} removed).\n",
            path.display(),
            update.total,
            update.added.len(),
            update.removed.len()
        ));
        for entry in &update.removed {
            emit_str(&format!("  removed (no longer passes): {entry}\n"));
        }
    }

    let report_status = std::env::var("REPORT_EXIT_STATUS").ok();
    let ignore_failures = matches!(report_status.as_deref(), Some("0" | "no"));
    if !ignore_failures && results.iter().any(|r| r.status.is_failure()) {
        failed = true;
    }
    drop(shared_cache);
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Entry point of the `php-run-tests` binary.
pub fn main_entry() -> ExitCode {
    let args = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned());
    match parse_args(args) {
        Ok(Parsed::Help) => {
            emit_str(USAGE);
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run(opts)) => match run(&opts) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("ERROR: {e}");
                ExitCode::from(2)
            }
        },
        Err(e) => {
            eprintln!("php-run-tests: {e}\nTry 'php-run-tests --help'.");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(|s| (*s).to_string()))
    }

    fn opts(args: &[&str]) -> Options {
        match parse(args).unwrap() {
            Parsed::Run(o) => *o,
            Parsed::Help => panic!("unexpected help"),
        }
    }

    #[test]
    fn parses_spec_options() {
        let o = opts(&[
            "-p",
            "/bin/php",
            "-j4",
            "-q",
            "--show-diff",
            "--keep-all",
            "--timeout",
            "7",
            "--filter",
            "Zend",
            "--json",
            "out.json",
            "--passing-list",
            "p.txt",
            "--update-passing",
            "u.txt",
            "--summary-by-dir",
            "a.phpt",
            "dir",
        ]);
        assert_eq!(o.php, Some(PathBuf::from("/bin/php")));
        assert_eq!(o.jobs, Some(4));
        assert!(o.quiet && o.show_diff && o.keep_all && o.summary_by_dir);
        assert_eq!(o.timeout, Some(7));
        assert_eq!(o.filter.as_deref(), Some("Zend"));
        assert_eq!(o.json, Some(PathBuf::from("out.json")));
        assert_eq!(o.passing_list, Some(PathBuf::from("p.txt")));
        assert_eq!(o.update_passing, Some(PathBuf::from("u.txt")));
        assert_eq!(o.paths, vec![PathBuf::from("a.phpt"), PathBuf::from("dir")]);
    }

    #[test]
    fn accepts_equals_and_separate_forms() {
        let o = opts(&[
            "--timeout=9",
            "--filter=x",
            "-j",
            "3",
            "--jobs=2",
            "-d",
            "a=b",
            "-d",
            "c=d",
        ]);
        assert_eq!(o.timeout, Some(9));
        assert_eq!(o.filter.as_deref(), Some("x"));
        assert_eq!(o.jobs, Some(2));
        assert_eq!(o.ini, vec!["a=b", "c=d"]);
        assert_eq!(opts(&["-j", "5"]).jobs, Some(5));
    }

    #[test]
    fn double_dash_ends_options() {
        let o = opts(&["--", "-weird.phpt"]);
        assert_eq!(o.paths, vec![PathBuf::from("-weird.phpt")]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--bogus"]).unwrap_err().contains("unknown option"));
        assert!(parse(&["-j0"]).is_err());
        assert!(parse(&["-jx"]).is_err());
        assert!(parse(&["--timeout", "0"]).is_err());
        assert!(parse(&["--timeout", "abc"]).is_err());
        assert!(parse(&["--json"]).unwrap_err().contains("requires"));
        assert!(parse(&["-p"]).is_err());
    }

    #[test]
    fn help_wins() {
        assert_eq!(parse(&["a.phpt", "--help"]).unwrap(), Parsed::Help);
        assert_eq!(parse(&["-h"]).unwrap(), Parsed::Help);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("php-phpt-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn finds_tests_recursively_skipping_hidden_and_vcs() {
        let dir = scratch("find");
        for p in [
            "a.phpt",
            "sub/b.phpt",
            "sub/deep/c.phpt",
            ".hidden.phpt",
            ".git/x.phpt",
            "sub/not-a-test.php",
            "sub/d.phpt.out",
        ] {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let mut found = Vec::new();
        find_tests(&dir, &mut found).unwrap();
        found.sort();
        let rel: Vec<String> = found
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(rel, vec!["a.phpt", "sub/b.phpt", "sub/deep/c.phpt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loops_do_not_recurse_forever() {
        let dir = scratch("loop");
        std::fs::write(dir.join("a.phpt"), "x").unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
        let mut found = Vec::new();
        find_tests(&dir, &mut found).unwrap();
        assert_eq!(found.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_root_detection() {
        let dir = scratch("ws");
        std::fs::create_dir_all(dir.join("crates/x/src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        std::fs::write(dir.join("crates/x/Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(
            find_workspace_root(&dir.join("crates/x/src")),
            Some(dir.clone())
        );
        let lone = scratch("nows");
        assert_eq!(find_workspace_root(&lone.join("nothing")), None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&lone);
    }

    #[test]
    fn resolve_php_rejects_non_executables() {
        let dir = scratch("php");
        let file = dir.join("php");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            let e = resolve_php(Some(&file), &dir).unwrap_err();
            assert!(e.contains("invalid PHP executable"), "{e}");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                resolve_php(Some(&file), &dir).unwrap(),
                std::fs::canonicalize(&file).unwrap()
            );
        }
        let e = resolve_php(Some(&dir.join("missing")), &dir).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
