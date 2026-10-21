//! The `php` command-line binary.

// The parser is finished before the engine that consumes it is wired in, so
// parts of its API have no caller yet.
#[allow(dead_code)]
mod args;

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let argv0 = argv.first().map_or_else(
        || "php".to_owned(),
        |arg| arg.to_string_lossy().into_owned(),
    );

    // Write errors (a closed pipe, say) are deliberately ignored, as php's own
    // output layer does.
    match args::parse(argv) {
        Ok(config) if config.mode == args::Mode::Help => {
            let _ = std::io::stdout().write_all(args::usage(&argv0).as_bytes());
            ExitCode::SUCCESS
        }
        Ok(config) => {
            let _ = writeln!(std::io::stdout(), "{config:?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let (stdout, stderr) = error.render(&argv0);
            let _ = std::io::stderr().write_all(stderr.as_bytes());
            let _ = std::io::stdout().write_all(stdout.as_bytes());
            ExitCode::from(u8::try_from(error.exit_code).unwrap_or(1))
        }
    }
}
