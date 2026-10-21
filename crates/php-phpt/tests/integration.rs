//! End-to-end tests: run the real `php-run-tests` binary against a fake
//! `php` shell script (see `tests/fixtures/fake-php.sh`).

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A scratch directory containing the fake php and test files.
struct Sandbox {
    dir: PathBuf,
    php: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("php-phpt-it-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dir = fs::canonicalize(&dir).unwrap();
        let php = dir.join("fake-php");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-php.sh");
        fs::copy(&fixture, &php).unwrap();
        fs::set_permissions(&php, fs::Permissions::from_mode(0o755)).unwrap();
        Sandbox { dir, php }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    /// Write a `.phpt` file (creating parent directories).
    fn test(&self, rel: &str, content: &str) -> PathBuf {
        self.file(rel, content.as_bytes())
    }

    fn file(&self, rel: &str, content: &[u8]) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_php-run-tests"));
        cmd.current_dir(&self.dir)
            .arg("-p")
            .arg(&self.php)
            .env("NO_COLOR", "1")
            .env_remove("TEST_PHP_SRCDIR")
            .env_remove("TEST_PHP_EXECUTABLE")
            .env_remove("REPORT_EXIT_STATUS")
            .env_remove("NO_PHPTEST_SUMMARY");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// Run with `--json` and return (output, parsed statuses).
    fn run_json(&self, args: &[&str]) -> (Output, Results) {
        let json = self.path("results.json");
        let _ = fs::remove_file(&json);
        let mut all: Vec<&str> = vec!["--json", json.to_str().unwrap()];
        all.extend_from_slice(args);
        let out = self.run(&all);
        let text = fs::read_to_string(&json)
            .unwrap_or_else(|e| panic!("no json written ({e}); stdout:\n{}", stdout(&out)));
        (out, Results(text))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

/// Minimal reader for the runner's JSON report.
struct Results(String);

impl Results {
    fn field(&self, test: &str, field: &str) -> String {
        let marker = format!("{{\"path\": \"{test}\", ");
        let start = self
            .0
            .find(&marker)
            .unwrap_or_else(|| panic!("{test} not in report:\n{}", self.0));
        let record = &self.0[start..];
        let record = &record[..record.find('}').unwrap()];
        let key = format!("\"{field}\": ");
        let value = &record[record.find(&key).unwrap() + key.len()..];
        if let Some(rest) = value.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => out.push(chars.next().unwrap()),
                    c => out.push(c),
                }
            }
            out
        } else {
            value.split([',', '}']).next().unwrap().trim().to_string()
        }
    }

    fn status(&self, test: &str) -> String {
        self.field(test, "status")
    }

    fn reason(&self, test: &str) -> String {
        self.field(test, "reason")
    }
}

#[test]
fn mixed_results_end_to_end() {
    let sb = Sandbox::new("mixed");
    sb.test(
        "pass.phpt",
        "--TEST--\nplain pass\n--FILE--\nhello\nworld\n--EXPECT--\nhello\nworld\n",
    );
    sb.test(
        "fail.phpt",
        "--TEST--\nplain fail\n--FILE--\nhello\n--EXPECT--\ngoodbye\n",
    );
    sb.test(
        "expectf.phpt",
        "--TEST--\nexpectf\n--FILE--\nWarning: foo() in /tmp/x/test.php on line 12\nvalue 0.5e3\n\
         --EXPECTF--\nWarning: foo() in %s on line %d\nvalue %f\n",
    );
    sb.test(
        "regex.phpt",
        "--TEST--\nexpectregex\n--FILE--\nabc123\n--EXPECTREGEX--\n[a-c]+\\d{3}\n",
    );
    sb.test(
        "skip.phpt",
        "--TEST--\nskipped\n--SKIPIF--\nskip not on this machine\n--FILE--\nnever\n--EXPECT--\nnever\n",
    );
    sb.test("bork.phpt", "--TEST--\nborked\n--EXPECT--\nx\n");
    sb.test(
        "xfail.phpt",
        "--TEST--\nxfail\n--XFAIL--\nknown bug\n--FILE--\nactual\n--EXPECT--\nwanted\n",
    );
    sb.test(
        "xpass.phpt",
        "--TEST--\nxpass\n--XFAIL--\nfixed already\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    sb.test(
        "cgi.phpt",
        "--TEST--\ncgi only\n--GET--\nfoo=bar\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "redirect.phpt",
        "--TEST--\nredirect\n--REDIRECTTEST--\nreturn [];\n",
    );

    let (out, res) = sb.run_json(&["-j2", "."]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert_eq!(res.status("pass.phpt"), "PASS", "{text}");
    assert_eq!(res.status("fail.phpt"), "FAIL");
    assert!(
        res.reason("fail.phpt")
            .contains("expected `goodbye`, got `hello`")
    );
    assert_eq!(res.status("expectf.phpt"), "PASS", "{text}");
    assert_eq!(res.status("regex.phpt"), "PASS", "{text}");
    assert_eq!(res.status("skip.phpt"), "SKIP");
    assert_eq!(res.reason("skip.phpt"), "not on this machine");
    assert_eq!(res.status("bork.phpt"), "BORK");
    assert_eq!(res.reason("bork.phpt"), "missing section --FILE--");
    assert_eq!(res.status("xfail.phpt"), "XFAIL");
    assert_eq!(res.status("xpass.phpt"), "XPASS");
    assert_eq!(res.status("cgi.phpt"), "SKIP");
    assert_eq!(res.reason("cgi.phpt"), "CGI not available");
    assert_eq!(res.status("redirect.phpt"), "SKIP");

    assert!(text.contains("PASS plain pass [pass.phpt]"), "{text}");
    assert!(
        text.contains("SKIP skipped [skip.phpt] reason: not on this machine"),
        "{text}"
    );
    assert!(
        text.contains("XFAIL xfail [xfail.phpt]   XFAIL REASON: known bug"),
        "{text}"
    );
    assert!(text.contains("FAILED TEST SUMMARY"), "{text}");
    assert!(text.contains("BORKED TEST SUMMARY"), "{text}");
    assert!(text.contains("Tests passed    :     3"), "{text}");
    assert!(text.contains("Number of tests :    10"), "{text}");

    // Artifacts: failures keep .php/.out/.exp/.diff, passes clean up.
    for ext in ["php", "out", "exp", "diff", "log"] {
        assert!(
            sb.path(&format!("fail.{ext}")).exists(),
            "fail.{ext} missing"
        );
    }
    assert_eq!(fs::read_to_string(sb.path("fail.exp")).unwrap(), "goodbye");
    assert_eq!(fs::read_to_string(sb.path("fail.out")).unwrap(), "hello");
    let diff = fs::read_to_string(sb.path("fail.diff")).unwrap();
    assert!(
        diff.contains("001- goodbye") && diff.contains("001+ hello"),
        "{diff}"
    );
    assert!(
        fs::metadata(sb.path("fail.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111
            != 0
    );
    for stem in ["pass", "expectf", "regex", "skip", "xpass"] {
        assert!(
            !sb.path(&format!("{stem}.php")).exists(),
            "{stem}.php left behind"
        );
        assert!(!sb.path(&format!("{stem}.out")).exists());
        assert!(!sb.path(&format!("{stem}.skip.php")).exists());
    }
}

#[test]
fn all_passing_run_exits_zero() {
    let sb = Sandbox::new("allpass");
    for i in 0..30 {
        sb.test(
            &format!("t{i:02}.phpt"),
            &format!("--TEST--\ntest {i}\n--FILE--\nvalue {i}\n--EXPECT--\nvalue {i}\n"),
        );
    }
    let (out, res) = sb.run_json(&["-j8", "."]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    for i in 0..30 {
        assert_eq!(res.status(&format!("t{i:02}.phpt")), "PASS");
    }
    assert!(stdout(&out).contains("workers: 8"), "{}", stdout(&out));
}

#[test]
fn quiet_hides_pass_and_skip_lines() {
    let sb = Sandbox::new("quiet");
    sb.test("a.phpt", "--TEST--\nok\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test(
        "b.phpt",
        "--TEST--\nsk\n--SKIPIF--\nskip no\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test("c.phpt", "--TEST--\nbad\n--FILE--\nx\n--EXPECT--\ny\n");
    let out = sb.run(&["-q", "."]);
    let text = stdout(&out);
    assert!(!text.contains("PASS ok"), "{text}");
    assert!(!text.contains("SKIP sk"), "{text}");
    assert!(text.contains("FAIL bad [c.phpt]"), "{text}");
}

#[test]
fn single_test_shows_its_diff_automatically() {
    let sb = Sandbox::new("showdiff");
    let t = sb.test("one.phpt", "--TEST--\nbad\n--FILE--\nx\n--EXPECT--\ny\n");
    let out = sb.run(&[t.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(text.contains("========DIFF========"), "{text}");
    assert!(text.contains("001- y"), "{text}");
    assert!(text.contains("========DONE========"), "{text}");
}

#[test]
fn show_diff_flag_for_many_tests() {
    let sb = Sandbox::new("showdiff2");
    sb.test("a.phpt", "--TEST--\nbad\n--FILE--\nx\n--EXPECT--\ny\n");
    sb.test("b.phpt", "--TEST--\nok\n--FILE--\nx\n--EXPECT--\nx\n");
    let without = stdout(&sb.run(&["."]));
    assert!(!without.contains("========DIFF"), "{without}");
    let with = stdout(&sb.run(&["--show-diff", "."]));
    assert!(with.contains("========DIFF========"), "{with}");
}

#[test]
fn timeouts_do_not_hang_the_runner() {
    let sb = Sandbox::new("timeout");
    sb.test(
        "hang.phpt",
        "--TEST--\nhangs\n--FILE--\nbefore\n@sleep 30\nafter\n--EXPECT--\nbefore\nafter\n",
    );
    sb.test(
        "fast.phpt",
        "--TEST--\nfast\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    let started = Instant::now();
    let (out, res) = sb.run_json(&["--timeout", "1", "-j2", "."]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(res.status("hang.phpt"), "TIMEOUT", "{}", stdout(&out));
    assert!(res.reason("hang.phpt").contains("timed out after 1s"));
    assert_eq!(res.status("fast.phpt"), "PASS");
    assert_eq!(code(&out), 1);
    let produced = fs::read_to_string(sb.path("hang.out")).unwrap();
    assert!(
        produced.contains("before") && produced.contains("process timed out"),
        "{produced}"
    );
}

#[test]
fn timeout_applies_to_skipif_too() {
    let sb = Sandbox::new("skiptimeout");
    sb.test(
        "t.phpt",
        "--TEST--\nslow skipif\n--SKIPIF--\n@sleep 30\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let started = Instant::now();
    let (_, res) = sb.run_json(&["--timeout", "1", "."]);
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(res.status("t.phpt"), "BORK");
    assert!(res.reason("t.phpt").contains("invalid output from SKIPIF"));
}

#[test]
fn crashing_binary_is_reported_with_termsig() {
    let sb = Sandbox::new("crash");
    sb.test(
        "crash.phpt",
        "--TEST--\ncrash\n--FILE--\nbefore\n@kill\n--EXPECT--\nbefore\n",
    );
    sb.test(
        "expected.phpt",
        "--TEST--\nexpected crash\n--FILE--\nbefore\n@kill\n--EXPECT--\nbefore\n\nTermsig=11\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("crash.phpt"), "FAIL");
    assert_eq!(res.status("expected.phpt"), "PASS");
    let out = fs::read_to_string(sb.path("crash.out")).unwrap();
    assert_eq!(out, "before\n\nTermsig=11");
}

#[test]
fn nonexistent_php_binary_is_an_error() {
    let sb = Sandbox::new("nophp");
    sb.test("a.phpt", "--TEST--\nok\n--FILE--\nx\n--EXPECT--\nx\n");
    let out = Command::new(env!("CARGO_BIN_EXE_php-run-tests"))
        .current_dir(&sb.dir)
        .args(["-p", "/nonexistent/php", "."])
        .output()
        .unwrap();
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not exist"));
}

#[test]
fn unexecutable_php_binary_borks_instead_of_crashing() {
    let sb = Sandbox::new("badphp");
    // Executable, but not a valid program (no shebang, not a binary).
    let bad = sb.file("bad-php", b"\x00\x01\x02 not a program");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
    sb.test("a.phpt", "--TEST--\nok\n--FILE--\nx\n--EXPECT--\nx\n");
    let out = Command::new(env!("CARGO_BIN_EXE_php-run-tests"))
        .current_dir(&sb.dir)
        .args(["-p", bad.to_str().unwrap(), "--json", "r.json", "."])
        .output()
        .unwrap();
    // Probing `-m` fails; the test itself cannot be executed.  Either way the
    // runner must finish and report a non-passing status.
    let json = fs::read_to_string(sb.path("r.json")).unwrap();
    assert!(!json.contains("\"PASS\": 1"), "{json}");
    assert_eq!(code(&out), 1, "{}", stdout(&out));
}

#[test]
fn stdin_args_env_and_ini_sections() {
    let sb = Sandbox::new("sections");
    sb.test(
        "stdin.phpt",
        "--TEST--\nstdin\n--FILE--\n@stdin\n--STDIN--\nfoo\nbar\n--EXPECT--\nfoo\nbar\n",
    );
    sb.test(
        "args.phpt",
        "--TEST--\nargs\n--FILE--\n@args\n--ARGS--\n--first \"second arg\" 'third'\n--EXPECT--\narg:--first\narg:second arg\narg:third\n",
    );
    sb.test(
        "env.phpt",
        "--TEST--\nenv\n--FILE--\n@env MY_VAR\n@env PWD_VAR\n@env TEST_PHP_EXECUTABLE\n\
         --ENV--\nMY_VAR=hello world\nPWD_VAR={PWD}/x\n--EXPECTF--\nMY_VAR=hello world\nPWD_VAR=%s/x\nTEST_PHP_EXECUTABLE=%s/fake-php\n",
    );
    sb.test(
        "ini.phpt",
        "--TEST--\nini\n--FILE--\n@ini precision\n@ini custom.setting\n@ini date.timezone\n@ini my.pwd\n\
         --INI--\nprecision=10\ncustom.setting = spaced value \nmy.pwd={PWD}\n--EXPECTF--\nprecision=10\ncustom.setting=spaced value\ndate.timezone=UTC\nmy.pwd=%s\n",
    );
    sb.test(
        "defaults.phpt",
        "--TEST--\nrun-tests ini defaults\n--FILE--\n@ini display_errors\n@ini error_reporting\n@ini serialize_precision\n@ini memory_limit\n\
         --EXPECT--\ndisplay_errors=1\nerror_reporting=32767\nserialize_precision=-1\nmemory_limit=128M\n",
    );
    sb.test(
        "cgienv.phpt",
        "--TEST--\nrequest env\n--FILE--\n@env REQUEST_METHOD\n@env REDIRECT_STATUS\n@env QUERY_STRING\n@env TEST_PHP_EVALUATING_SKIPIF\n\
         --EXPECT--\nREQUEST_METHOD=GET\nREDIRECT_STATUS=1\nQUERY_STRING=<unset>\nTEST_PHP_EVALUATING_SKIPIF=<unset>\n",
    );
    sb.test(
        "extra.phpt",
        "--TEST--\nextra args env\n--FILE--\n@env TEST_PHP_EXTRA_ARGS\n--ENV--\nIGNORED=1\n\
         --EXPECTF--\nTEST_PHP_EXTRA_ARGS='-n'  -d \"output_handler=\" %a\n",
    );
    let (out, res) = sb.run_json(&["-j4", "."]);
    let text = stdout(&out);
    for t in ["stdin", "args", "env", "ini", "defaults", "cgienv", "extra"] {
        assert_eq!(res.status(&format!("{t}.phpt")), "PASS", "{t}:\n{text}");
    }
}

#[test]
fn skipif_environment_marker_and_request_vars() {
    let sb = Sandbox::new("skipifenv");
    sb.test(
        "marker.phpt",
        "--TEST--\nskipif marker\n--SKIPIF--\n@env TEST_PHP_EVALUATING_SKIPIF\n@env REQUEST_METHOD\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let (_, res) = sb.run_json(&["."]);
    // The SKIPIF output is not a skip/info/warn/xfail directive: BORK, and the
    // reason shows what the script saw.
    assert_eq!(res.status("marker.phpt"), "BORK");
    let reason = res.reason("marker.phpt");
    assert!(reason.contains("TEST_PHP_EVALUATING_SKIPIF=1"), "{reason}");
    assert!(reason.contains("REQUEST_METHOD=<unset>"), "{reason}");
}

#[test]
fn skipif_directives() {
    let sb = Sandbox::new("skipifdir");
    sb.test(
        "info.phpt",
        "--TEST--\ninfo\n--SKIPIF--\ninfo just so you know\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "warn.phpt",
        "--TEST--\nwarn\n--SKIPIF--\nwarn careful\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "warnfail.phpt",
        "--TEST--\nwarnfail\n--SKIPIF--\nwarn careful\n--FILE--\nx\n--EXPECT--\ny\n",
    );
    sb.test("xfail.phpt", "--TEST--\nxfail via skipif\n--SKIPIF--\nxfail not implemented\n--FILE--\nx\n--EXPECT--\ny\n");
    sb.test(
        "junk.phpt",
        "--TEST--\njunk\n--SKIPIF--\nsomething unexpected\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "empty.phpt",
        "--TEST--\nempty skipif\n--SKIPIF--\n\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "nocache.phpt",
        "--TEST--\nnocache\n--SKIPIF--\nnocache because dynamic\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "upper.phpt",
        "--TEST--\nuppercase\n--SKIPIF--\nSKIP Because\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let (out, res) = sb.run_json(&["."]);
    assert_eq!(res.status("info.phpt"), "PASS");
    assert_eq!(res.status("warn.phpt"), "PASS");
    assert_eq!(res.status("warnfail.phpt"), "FAIL");
    assert!(stdout(&out).contains("(warn: careful)"), "{}", stdout(&out));
    assert_eq!(res.status("xfail.phpt"), "XFAIL");
    assert_eq!(res.status("junk.phpt"), "BORK");
    assert_eq!(res.status("empty.phpt"), "PASS");
    assert_eq!(res.status("nocache.phpt"), "PASS");
    assert_eq!(res.status("upper.phpt"), "SKIP");
    assert_eq!(res.reason("upper.phpt"), "Because");
}

#[test]
fn extensions_section() {
    let sb = Sandbox::new("exts");
    sb.test(
        "have.phpt",
        "--TEST--\nhave\n--EXTENSIONS--\njson\nStandard\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    sb.test(
        "opcache.phpt",
        "--TEST--\nzend opcache is mapped\n--EXTENSIONS--\nopcache\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    sb.test(
        "missing.phpt",
        "--TEST--\nmissing\n--EXTENSIONS--\njson\nbogus\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    sb.test(
        "missing2.phpt",
        "--TEST--\nmissing two\n--EXTENSIONS--\nbogus\nother\n--FILE--\nok\n--EXPECT--\nok\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("have.phpt"), "PASS");
    assert_eq!(res.status("opcache.phpt"), "PASS");
    assert_eq!(res.status("missing.phpt"), "SKIP");
    assert_eq!(
        res.reason("missing.phpt"),
        "Required extension missing: bogus"
    );
    assert_eq!(
        res.reason("missing2.phpt"),
        "Required extensions missing: bogus, other"
    );
}

#[test]
fn clean_section_runs_and_validates() {
    let sb = Sandbox::new("clean");
    let marker = sb.path("cleaned.marker");
    sb.test(
        "cleans.phpt",
        &format!(
            "--TEST--\nclean runs\n--FILE--\nok\n--CLEAN--\n@touch {}\n--EXPECT--\nok\n",
            marker.display()
        ),
    );
    sb.test(
        "noisy.phpt",
        "--TEST--\nclean prints\n--FILE--\nok\n--CLEAN--\nunexpected clean output\n--EXPECT--\nok\n",
    );
    let marker2 = sb.path("cleaned-after-fail.marker");
    sb.test(
        "failing.phpt",
        &format!(
            "--TEST--\nclean runs after failure\n--FILE--\nok\n--CLEAN--\n@touch {}\n--EXPECT--\nnot ok\n",
            marker2.display()
        ),
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("cleans.phpt"), "PASS");
    assert!(marker.exists(), "CLEAN did not run");
    assert_eq!(res.status("noisy.phpt"), "BORK");
    assert!(
        res.reason("noisy.phpt")
            .contains("invalid output from CLEAN")
    );
    assert_eq!(res.status("failing.phpt"), "FAIL");
    assert!(marker2.exists(), "CLEAN must run even when the test fails");
    assert!(!sb.path("cleans.clean.php").exists());

    // --no-clean skips it.
    let _ = fs::remove_file(&marker);
    let (_, res) = sb.run_json(&["--no-clean", "cleans.phpt"]);
    assert_eq!(res.status("cleans.phpt"), "PASS");
    assert!(!marker.exists(), "--no-clean must not run CLEAN");
}

#[test]
fn keep_all_retains_generated_files() {
    let sb = Sandbox::new("keepall");
    sb.test(
        "k.phpt",
        "--TEST--\nkeep\n--SKIPIF--\n\n--FILE--\nok\n--CLEAN--\n@touch /dev/null\n--EXPECT--\nok\n",
    );
    let out = sb.run(&["--keep-all", "k.phpt"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    for f in ["k.php", "k.skip.php", "k.clean.php", "k.out", "k.exp"] {
        assert!(sb.path(f).exists(), "{f} should be kept");
    }
}

#[test]
fn output_is_compared_as_bytes() {
    let sb = Sandbox::new("bytes");
    sb.file(
        "bin.phpt",
        b"--TEST--\nnon-utf8 output\n--FILE--\n@bytes\n--EXPECT--\na\xffb\n",
    );
    sb.file(
        "binf.phpt",
        b"--TEST--\nnon-utf8 output with EXPECTF\n--FILE--\n@bytes\n--EXPECTF--\na%cb\n",
    );
    sb.file(
        "binfail.phpt",
        b"--TEST--\nnon-utf8 mismatch\n--FILE--\n@bytes\n--EXPECT--\na\xfeb\n",
    );
    sb.test(
        "crlf.phpt",
        "--TEST--\ncrlf is normalised\n--FILE--\n@crlf\n--EXPECT--\none\ntwo\n",
    );
    sb.file(
        "crlf-expect.phpt",
        b"--TEST--\r\ncrlf in expectation\r\n--FILE--\r\none\r\ntwo\r\n--EXPECT--\r\none\r\ntwo\r\n",
    );
    sb.test(
        "trim.phpt",
        "--TEST--\nsurrounding whitespace is trimmed\n--FILE--\n\n\n  padded  \n\n\n--EXPECT--\n  padded\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("bin.phpt"), "PASS");
    assert_eq!(res.status("binf.phpt"), "PASS");
    assert_eq!(res.status("binfail.phpt"), "FAIL");
    assert_eq!(res.status("crlf.phpt"), "PASS");
    assert_eq!(res.status("crlf-expect.phpt"), "PASS");
    assert_eq!(res.status("trim.phpt"), "PASS");
    assert_eq!(fs::read(sb.path("binfail.out")).unwrap(), b"a\xffb");
}

#[test]
fn stderr_handling_and_capture_stdio() {
    let sb = Sandbox::new("stderr");
    sb.test(
        "merged.phpt",
        "--TEST--\nstderr is merged by default\n--FILE--\nout1\n@stderr err1\nout2\n--EXPECT--\nout1\nerr1\nout2\n",
    );
    sb.test(
        "stdout-only.phpt",
        "--TEST--\nCAPTURE_STDIO STDOUT\n--FILE--\nout1\n@stderr err1\nout2\n--CAPTURE_STDIO--\nSTDOUT\n--EXPECT--\nout1\nout2\n",
    );
    sb.test(
        "stderr-only.phpt",
        "--TEST--\nCAPTURE_STDIO STDERR\n--FILE--\nout1\n@stderr err1\nout2\n--CAPTURE_STDIO--\nSTDERR\n--EXPECT--\nerr1\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("merged.phpt"), "PASS");
    assert_eq!(res.status("stdout-only.phpt"), "PASS");
    assert_eq!(res.status("stderr-only.phpt"), "PASS");
}

#[test]
fn flaky_tests_are_retried_once() {
    let sb = Sandbox::new("flaky");
    let counter = sb.path("counter");
    sb.test(
        "flaky.phpt",
        &format!(
            "--TEST--\nflaky passes on retry\n--FLAKY--\nwhy\n--FILE--\n@count {}\n--EXPECT--\nrun 2\n",
            counter.display()
        ),
    );
    let counter2 = sb.path("counter2");
    sb.test(
        "notflaky.phpt",
        &format!(
            "--TEST--\nnot flaky never retried\n--FILE--\n@count {}\n--EXPECT--\nrun 2\n",
            counter2.display()
        ),
    );
    let (out, res) = sb.run_json(&["."]);
    assert_eq!(res.status("flaky.phpt"), "WARN", "{}", stdout(&out));
    assert!(stdout(&out).contains("Test passed on retry attempt"));
    assert_eq!(res.status("notflaky.phpt"), "FAIL");
    assert_eq!(fs::read_to_string(&counter2).unwrap().trim(), "1");
    assert_eq!(code(&out), 1);
}

#[test]
fn working_directory_is_the_source_dir() {
    let sb = Sandbox::new("cwd");
    sb.test(
        "sub/cwd.phpt",
        &format!(
            "--TEST--\ncwd\n--FILE--\n@pwd\n--EXPECT--\n{}\n",
            sb.dir.display()
        ),
    );
    let (_, res) = sb.run_json(&["sub"]);
    assert_eq!(res.status("sub/cwd.phpt"), "PASS");

    let other = sb.path("other");
    fs::create_dir_all(&other).unwrap();
    sb.test(
        "sub/cwd2.phpt",
        &format!(
            "--TEST--\ncwd2\n--FILE--\n@pwd\n--EXPECT--\n{}\n",
            other.display()
        ),
    );
    let (_, res) = sb.run_json(&["--srcdir", "other", "sub/cwd2.phpt"]);
    assert_eq!(res.status("sub/cwd2.phpt"), "PASS");
}

#[test]
fn external_sections() {
    let sb = Sandbox::new("external");
    sb.file("expected.txt", b"hello\n");
    sb.file("code.inc", b"hello\n");
    sb.test(
        "ext.phpt",
        "--TEST--\nexternal sections\n--FILE_EXTERNAL--\ncode.inc\n--EXPECT_EXTERNAL--\nexpected.txt\n",
    );
    sb.test(
        "bad.phpt",
        "--TEST--\nmissing external\n--FILE--\nx\n--EXPECT_EXTERNAL--\nnope.txt\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert_eq!(res.status("ext.phpt"), "PASS");
    assert_eq!(res.status("bad.phpt"), "BORK");
}

#[test]
fn filter_and_summary_by_dir() {
    let sb = Sandbox::new("filter");
    sb.test(
        "Zend/tests/a.phpt",
        "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    sb.test(
        "Zend/tests/b.phpt",
        "--TEST--\nb\n--FILE--\nx\n--EXPECT--\ny\n",
    );
    sb.test(
        "ext/json/tests/c.phpt",
        "--TEST--\nc\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let out = sb.run(&["--summary-by-dir", "."]);
    let text = stdout(&out);
    let zend = text
        .lines()
        .find(|l| l.starts_with("Zend/tests"))
        .unwrap_or_else(|| panic!("{text}"));
    let cols: Vec<&str> = zend.split_whitespace().collect();
    assert_eq!(&cols[1..4], &["2", "1", "1"], "{text}");
    assert!(
        text.lines().any(|l| l.starts_with("ext/json/tests")),
        "{text}"
    );

    let (_, res) = sb.run_json(&["--filter", "json", "."]);
    assert_eq!(res.status("ext/json/tests/c.phpt"), "PASS");
    assert!(!res.0.contains("Zend/tests"), "{}", res.0);
}

#[test]
fn directories_and_files_can_be_mixed_and_are_deduplicated() {
    let sb = Sandbox::new("mixed-args");
    sb.test("d/a.phpt", "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test("d/b.phpt", "--TEST--\nb\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test("c.phpt", "--TEST--\nc\n--FILE--\nx\n--EXPECT--\nx\n");
    let (_, res) = sb.run_json(&["d", "d/a.phpt", "c.phpt"]);
    assert!(res.0.contains("\"total\": 3"), "{}", res.0);
}

#[test]
fn usage_errors_exit_with_two() {
    let sb = Sandbox::new("usage");
    for args in [
        vec!["--bogus"],
        vec!["does-not-exist.phpt"],
        vec!["-j0", "x.phpt"],
        vec![],
    ] {
        let out = sb.run(&args);
        assert_eq!(
            code(&out),
            2,
            "args {args:?}: {}{}",
            stdout(&out),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let help = sb.run(&["--help"]);
    assert_eq!(code(&help), 0);
    assert!(stdout(&help).contains("--passing-list"));
}

#[test]
fn report_exit_status_env_is_honoured() {
    let sb = Sandbox::new("exitstatus");
    sb.test("bad.phpt", "--TEST--\nbad\n--FILE--\nx\n--EXPECT--\ny\n");
    assert_eq!(code(&sb.run(&["."])), 1);
    let out = sb
        .command()
        .env("REPORT_EXIT_STATUS", "0")
        .arg(".")
        .output()
        .unwrap();
    assert_eq!(code(&out), 0);
}

// ---------------------------------------------------------------------------
// Passing lists
// ---------------------------------------------------------------------------

#[test]
fn passing_list_regression_mode() {
    let sb = Sandbox::new("passlist");
    sb.test("a.phpt", "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test("sub/b.phpt", "--TEST--\nb\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test(
        "c.phpt",
        "--TEST--\nc (not in list, fails)\n--FILE--\nx\n--EXPECT--\ny\n",
    );
    sb.file(
        "PASSING",
        b"# header comment\n\na.phpt\nsub/b.phpt  # inline\n",
    );

    // Only the listed tests run; c.phpt (failing) is not part of the run.
    let (out, res) = sb.run_json(&["--passing-list", "PASSING"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "{text}");
    assert!(res.0.contains("\"total\": 2"), "{}", res.0);
    assert!(text.contains("all 2 listed tests pass"), "{text}");

    // Break a listed test: regression -> non-zero exit and a clear message.
    sb.test(
        "sub/b.phpt",
        "--TEST--\nb\n--FILE--\nx\n--EXPECT--\nbroken\n",
    );
    let out = sb.run(&["--passing-list", "PASSING"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(
        text.contains("REGRESSION: 1 of 2 listed tests do not pass"),
        "{text}"
    );
    assert!(text.contains("FAIL sub/b.phpt"), "{text}");

    // A skipped listed test is also "not PASS".
    sb.test(
        "sub/b.phpt",
        "--TEST--\nb\n--SKIPIF--\nskip gone\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let out = sb.run(&["--passing-list", "PASSING"]);
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).contains("SKIP sub/b.phpt"), "{}", stdout(&out));
}

#[test]
fn passing_list_with_missing_file_fails() {
    let sb = Sandbox::new("passmissing");
    sb.test("a.phpt", "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.file("PASSING", b"a.phpt\ndeleted.phpt\n");
    let out = sb.run(&["--passing-list", "PASSING"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(text.contains("deleted.phpt"), "{text}");
}

#[test]
fn update_passing_creates_and_grows_the_list() {
    let sb = Sandbox::new("update");
    sb.test("z.phpt", "--TEST--\nz\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test("a.phpt", "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test("bad.phpt", "--TEST--\nbad\n--FILE--\nx\n--EXPECT--\ny\n");
    sb.test(
        "skip.phpt",
        "--TEST--\nskip\n--SKIPIF--\nskip x\n--FILE--\nx\n--EXPECT--\nx\n",
    );

    // Creating a new list: sorted, only passing tests, with a header.
    let out = sb.run(&["--update-passing", "PASSING", "."]);
    assert_eq!(code(&out), 1, "bad.phpt fails so the run fails");
    let list = fs::read_to_string(sb.path("PASSING")).unwrap();
    let entries: Vec<&str> = list.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(entries, vec!["a.phpt", "z.phpt"], "{list}");
    assert!(list.starts_with('#'));
    assert!(
        stdout(&out).contains("2 entries (+2 added, -0 removed)"),
        "{}",
        stdout(&out)
    );

    // A later partial run keeps entries that were not part of it, adds new
    // passes and drops entries that ran and regressed.
    sb.file(
        "PASSING",
        b"# my header\n\nz.phpt\nuntouched/other.phpt\nbad.phpt\n",
    );
    sb.test(
        "untouched/other.phpt",
        "--TEST--\nother\n--FILE--\nx\n--EXPECT--\nx\n",
    );
    let out = sb.run(&["--update-passing", "PASSING", "a.phpt", "bad.phpt"]);
    assert_eq!(code(&out), 1);
    let list = fs::read_to_string(sb.path("PASSING")).unwrap();
    assert_eq!(
        list, "# my header\n\na.phpt\nuntouched/other.phpt\nz.phpt\n",
        "bad.phpt ran and failed, so it is dropped; z.phpt was not run, so it stays"
    );
    assert!(
        stdout(&out).contains("removed (no longer passes): bad.phpt"),
        "{}",
        stdout(&out)
    );

    // Entries whose file vanished and which were not run are dropped.
    fs::remove_file(sb.path("untouched/other.phpt")).unwrap();
    let _ = sb.run(&["--update-passing", "PASSING", "a.phpt"]);
    let list = fs::read_to_string(sb.path("PASSING")).unwrap();
    assert_eq!(list, "# my header\n\na.phpt\nz.phpt\n");
}

#[test]
fn update_passing_round_trips_with_passing_list() {
    let sb = Sandbox::new("roundtrip");
    for i in 0..5 {
        sb.test(
            &format!("t{i}.phpt"),
            &format!("--TEST--\nt{i}\n--FILE--\nv{i}\n--EXPECT--\nv{i}\n"),
        );
    }
    let out = sb.run(&["--update-passing", "PASSING", "."]);
    assert_eq!(code(&out), 0);
    let out = sb.run(&["--passing-list", "PASSING", "--update-passing", "PASSING"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    let list = fs::read_to_string(sb.path("PASSING")).unwrap();
    assert_eq!(list.lines().filter(|l| l.ends_with(".phpt")).count(), 5);
}

#[test]
fn json_report_contains_all_fields() {
    let sb = Sandbox::new("json");
    sb.test("a.phpt", "--TEST--\na\n--FILE--\nx\n--EXPECT--\nx\n");
    sb.test(
        "b.phpt",
        "--TEST--\nb \"quoted\"\n--FILE--\nx\n--EXPECT--\ny\n",
    );
    let (_, res) = sb.run_json(&["."]);
    assert!(res.0.contains("\"summary\": {"), "{}", res.0);
    assert!(
        res.0.contains("\"PASS\": 1") && res.0.contains("\"FAIL\": 1"),
        "{}",
        res.0
    );
    assert!(res.0.contains("\"duration_ms\":"));
    assert!(res.0.contains("\"php\":"));
    assert!(res.field("a.phpt", "duration_ms").parse::<u64>().is_ok());
}

#[test]
fn conflicting_tests_are_serialised() {
    // Every test takes the same directory-based lock while it runs.  Tests
    // sharing a CONFLICTS key must never overlap, so no test may print
    // VIOLATION even with many workers.
    let sb = Sandbox::new("conflicts");
    let lock = sb.path("lockdir");
    for i in 0..8 {
        sb.test(
            &format!("c{i}.phpt"),
            &format!(
                "--TEST--\nconflict {i}\n--CONFLICTS--\nshared\n--FILE--\n@lock {lock}\ninside\n@sleep 0.2\n@unlock {lock}\n--EXPECT--\ninside\n",
                lock = lock.display()
            ),
        );
    }
    let (out, res) = sb.run_json(&["-j8", "."]);
    for i in 0..8 {
        assert_eq!(
            res.status(&format!("c{i}.phpt")),
            "PASS",
            "{}",
            stdout(&out)
        );
    }

    // Control experiment: without the CONFLICTS key the same tests do collide,
    // which proves the check above is able to detect overlap.
    let sb2 = Sandbox::new("conflicts-control");
    let lock2 = sb2.path("lockdir");
    for i in 0..8 {
        sb2.test(
            &format!("c{i}.phpt"),
            &format!(
                "--TEST--\nno conflict {i}\n--FILE--\n@lock {lock}\ninside\n@sleep 0.2\n@unlock {lock}\n--EXPECT--\ninside\n",
                lock = lock2.display()
            ),
        );
    }
    let (_, res2) = sb2.run_json(&["-j8", "."]);
    let collided = (0..8).any(|i| res2.status(&format!("c{i}.phpt")) == "FAIL");
    assert!(
        collided,
        "expected overlapping tests to be detected without CONFLICTS"
    );
}

#[test]
fn directory_level_conflicts_file_is_honoured() {
    let sb = Sandbox::new("dirconflicts");
    let lock = sb.path("lockdir");
    sb.file("d/CONFLICTS", b"# serialise this directory\nshared\n");
    for i in 0..6 {
        sb.test(
            &format!("d/c{i}.phpt"),
            &format!(
                "--TEST--\ndir conflict {i}\n--FILE--\n@lock {lock}\ninside\n@sleep 0.2\n@unlock {lock}\n--EXPECT--\ninside\n",
                lock = lock.display()
            ),
        );
    }
    let (out, res) = sb.run_json(&["-j6", "d"]);
    for i in 0..6 {
        assert_eq!(
            res.status(&format!("d/c{i}.phpt")),
            "PASS",
            "{}",
            stdout(&out)
        );
    }
}

#[test]
fn interrupt_kills_children_and_exits_130() {
    let sb = Sandbox::new("interrupt");
    let started = sb.path("started.marker");
    sb.test(
        "hang.phpt",
        &format!(
            "--TEST--\nhangs until interrupted\n--FILE--\n@touch {}\n@sleep 1234.5\n--EXPECT--\nx\n",
            started.display()
        ),
    );
    for i in 0..20 {
        sb.test(
            &format!("later{i:02}.phpt"),
            "--TEST--\nnever reached\n--FILE--\n@sleep 1234.5\n--EXPECT--\nx\n",
        );
    }
    let mut child = sb
        .command()
        .args(["-j1", "--timeout", "600", "hang.phpt", "later00.phpt"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the test never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = child.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );

    let t = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "runner did not exit after SIGINT"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));

    // No orphaned `sleep 1234.5` may survive (give the kernel a moment).
    std::thread::sleep(Duration::from_millis(300));
    if let Ok(pgrep) = Command::new("pgrep").args(["-f", "sleep 1234.5"]).output() {
        assert!(
            pgrep.stdout.is_empty(),
            "orphaned child processes: {}",
            String::from_utf8_lossy(&pgrep.stdout)
        );
    }
    assert!(
        !sb.path("hang.php").exists(),
        "interrupted test left its .php behind"
    );
}

#[test]
fn default_php_lookup_order() {
    let sb = Sandbox::new("defaultphp");
    sb.file("Cargo.toml", b"[workspace]\nmembers = []\n");
    sb.test("a.phpt", "--TEST--\nok\n--FILE--\nx\n--EXPECT--\nx\n");
    // Run from a nested directory: the workspace root must be found by
    // walking up to the Cargo.toml that declares [workspace].
    let nested = sb.path("tests/deep");
    fs::create_dir_all(&nested).unwrap();

    let run = |env: &[(&str, &str)]| -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_php-run-tests"));
        cmd.current_dir(&nested)
            .arg(sb.path("a.phpt"))
            .env("NO_COLOR", "1")
            .env_remove("TEST_PHP_EXECUTABLE")
            .env_remove("TEST_PHP_SRCDIR")
            .env_remove("CARGO_TARGET_DIR");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        (
            code(&out),
            stdout(&out),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let install = |rel: &str| {
        let dst = sb.path(rel);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::copy(&sb.php, &dst).unwrap();
        fs::set_permissions(&dst, fs::Permissions::from_mode(0o755)).unwrap();
        dst
    };

    // Nothing built yet.
    let (status, _, err) = run(&[]);
    assert_eq!(status, 2);
    assert!(err.contains("no PHP binary found"), "{err}");
    assert!(
        err.contains("target/release/php") && err.contains("target/debug/php"),
        "{err}"
    );

    // Only a debug build.
    let debug = install("target/debug/php");
    let (status, out, _) = run(&[]);
    assert_eq!(status, 0, "{out}");
    assert!(
        out.contains(&format!("PHP         : {}", debug.display())),
        "{out}"
    );

    // A release build wins over debug.
    let release = install("target/release/php");
    let (_, out, _) = run(&[]);
    assert!(
        out.contains(&format!("PHP         : {}", release.display())),
        "{out}"
    );

    // CARGO_TARGET_DIR is consulted first.
    let custom = install("custom-target/debug/php");
    let (_, out, _) = run(&[(
        "CARGO_TARGET_DIR",
        sb.path("custom-target").to_str().unwrap(),
    )]);
    assert!(
        out.contains(&format!("PHP         : {}", custom.display())),
        "{out}"
    );

    // TEST_PHP_EXECUTABLE beats everything but -p.
    let (_, out, _) = run(&[("TEST_PHP_EXECUTABLE", sb.php.to_str().unwrap())]);
    assert!(
        out.contains(&format!("PHP         : {}", sb.php.display())),
        "{out}"
    );
}
