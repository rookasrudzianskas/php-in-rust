//! INI setting bookkeeping, the port of `settings2array`, `settings2params`
//! and `settings2arguments` from run-tests.php.

use crate::shell::{addslashes, trim_php};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    values: Vec<String>,
}

/// An ordered set of `-d name=value` settings.
///
/// Like the PHP associative array it models, assigning an existing name
/// replaces the value in place (keeping its original position), except for
/// `extension` / `zend_extension` which accumulate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IniSettings {
    entries: Vec<Entry>,
}

impl IniSettings {
    pub fn new() -> Self {
        Self::default()
    }

    /// `settings2array` for one `name=value` line; lines without `=` are ignored.
    pub fn apply_line(&mut self, setting: &str) {
        if let Some((name, value)) = setting.split_once('=') {
            self.set(trim_php(name), trim_php(value));
        }
    }

    /// `settings2array` for many lines.
    pub fn apply_lines<I, S>(&mut self, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for line in lines {
            self.apply_line(line.as_ref());
        }
    }

    /// Assign a setting (accumulating for `extension` / `zend_extension`).
    pub fn set(&mut self, name: &str, value: &str) {
        let accumulate = name == "extension" || name == "zend_extension";
        match self.entries.iter_mut().find(|e| e.name == name) {
            Some(entry) if accumulate => entry.values.push(value.to_string()),
            Some(entry) => entry.values = vec![value.to_string()],
            None => self.entries.push(Entry {
                name: name.to_string(),
                values: vec![value.to_string()],
            }),
        }
    }

    /// Current (last) value of a setting.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .and_then(|e| e.values.last())
            .map(String::as_str)
    }

    /// `settings2arguments`: `["-d", "name=value", ...]`.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for entry in &self.entries {
            for value in &entry.values {
                args.push("-d".to_string());
                args.push(format!("{}={}", entry.name, value));
            }
        }
        args
    }

    /// `settings2params`: ` -d "name=value"` fragments for `TEST_PHP_EXTRA_ARGS`.
    pub fn to_params(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            for value in &entry.values {
                out.push_str(&format!(" -d \"{}={}\"", entry.name, addslashes(value)));
            }
        }
        out
    }
}

/// The `$ini_overwrites` defaults of run-tests.php (kept verbatim and in order).
pub fn default_ini_overwrites() -> Vec<String> {
    [
        "output_handler=",
        "open_basedir=",
        "disable_functions=",
        "output_buffering=Off",
        "error_reporting=32767",
        "fatal_error_backtraces=Off",
        "display_errors=1",
        "display_startup_errors=1",
        "error_include_args=0",
        "log_errors=0",
        "html_errors=0",
        "track_errors=0",
        "report_zend_debug=0",
        "docref_root=",
        "docref_ext=.html",
        "error_prepend_string=",
        "error_append_string=",
        "auto_prepend_file=",
        "auto_append_file=",
        "ignore_repeated_errors=0",
        "precision=14",
        "serialize_precision=-1",
        "memory_limit=128M",
        "opcache.fast_shutdown=0",
        "opcache.file_update_protection=0",
        "opcache.revalidate_freq=0",
        "opcache.jit_hot_loop=1",
        "opcache.jit_hot_func=1",
        "opcache.jit_hot_return=1",
        "opcache.jit_hot_side_exit=1",
        "opcache.jit_max_root_traces=100000",
        "opcache.jit_max_side_traces=100000",
        "opcache.jit_max_exit_counters=100000",
        "opcache.protect_memory=1",
        "zend.assertions=1",
        "zend.exception_ignore_args=0",
        "zend.exception_string_param_max_len=15",
        "short_open_tag=0",
        "date.timezone=UTC",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Extension-specific overrides run-tests applies when the extension is loaded.
pub fn extension_ini_overrides(ext: &str) -> &'static [&'static str] {
    match ext {
        "session" => &["session.auto_start=0"],
        "tidy" => &["tidy.clean_output=0"],
        "zlib" => &["zlib.output_compression=Off"],
        "xdebug" => &["xdebug.mode=off"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_assignment_replaces_in_place() {
        let mut ini = IniSettings::new();
        ini.apply_lines(["a=1", "b=2", " a = 3 "]);
        assert_eq!(ini.to_args(), vec!["-d", "a=3", "-d", "b=2"]);
        assert_eq!(ini.get("a"), Some("3"));
    }

    #[test]
    fn lines_without_equals_are_ignored() {
        let mut ini = IniSettings::new();
        ini.apply_lines(["", "just text", "x=y"]);
        assert_eq!(ini.to_args(), vec!["-d", "x=y"]);
    }

    #[test]
    fn value_keeps_further_equals_signs() {
        let mut ini = IniSettings::new();
        ini.apply_line("sendmail_path=tee a=b >/dev/null");
        assert_eq!(ini.get("sendmail_path"), Some("tee a=b >/dev/null"));
    }

    #[test]
    fn extensions_accumulate() {
        let mut ini = IniSettings::new();
        ini.apply_lines(["extension=a.so", "extension=b.so", "zend_extension=c.so"]);
        assert_eq!(
            ini.to_args(),
            vec![
                "-d",
                "extension=a.so",
                "-d",
                "extension=b.so",
                "-d",
                "zend_extension=c.so"
            ]
        );
    }

    #[test]
    fn params_use_addslashes() {
        let mut ini = IniSettings::new();
        ini.apply_line(r#"error_prepend_string=it's "x""#);
        assert_eq!(ini.to_params(), r#" -d "error_prepend_string=it\'s \"x\"""#);
    }

    #[test]
    fn defaults_match_run_tests() {
        let d = default_ini_overwrites();
        assert_eq!(d.first().map(String::as_str), Some("output_handler="));
        assert!(d.contains(&"error_reporting=32767".to_string()));
        assert!(d.contains(&"date.timezone=UTC".to_string()));
        assert_eq!(d.len(), 39);
    }
}
