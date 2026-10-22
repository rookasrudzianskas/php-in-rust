//! ext/standard/info.c and ext/standard/credits.c
//!
//! `phpinfo()` and `phpcredits()` render the CLI's plain-text format
//! (`sapi_module.phpinfo_as_text`).

use php_runtime::vm::version;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::basic::{BasicState, sys};

pub const PHP_INFO_GENERAL: i64 = 1 << 0;
pub const PHP_INFO_CREDITS: i64 = 1 << 1;
pub const PHP_INFO_CONFIGURATION: i64 = 1 << 2;
pub const PHP_INFO_MODULES: i64 = 1 << 3;
pub const PHP_INFO_ENVIRONMENT: i64 = 1 << 4;
pub const PHP_INFO_VARIABLES: i64 = 1 << 5;
pub const PHP_INFO_LICENSE: i64 = 1 << 6;
pub const PHP_INFO_ALL: i64 = 0xFFFF_FFFF;

pub const PHP_CREDITS_GROUP: i64 = 1 << 0;
pub const PHP_CREDITS_GENERAL: i64 = 1 << 1;
pub const PHP_CREDITS_SAPI: i64 = 1 << 2;
pub const PHP_CREDITS_MODULES: i64 = 1 << 3;
pub const PHP_CREDITS_DOCS: i64 = 1 << 4;
pub const PHP_CREDITS_FULLPAGE: i64 = 1 << 5;
pub const PHP_CREDITS_QA: i64 = 1 << 6;
pub const PHP_CREDITS_WEB: i64 = 1 << 7;
pub const PHP_CREDITS_ALL: i64 = 0xFFFF_FFFF;

/// `PHP_API_VERSION`, `ZEND_MODULE_API_NO` and `ZEND_EXTENSION_API_NO`.
const PHP_API_VERSION: i64 = 20260925;
const ZEND_EXTENSION_API_NO: i64 = 420260925;
const BUILD_ID_SUFFIX: &str = "NTS";

// ---------------------------------------------------------------------------
// Text-mode table primitives (`php_info_print_table_*`)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Text(Vec<u8>);

impl Text {
    fn put(&mut self, s: impl AsRef<[u8]>) {
        self.0.extend_from_slice(s.as_ref());
    }

    fn table_start(&mut self) {
        self.put("\n");
    }

    fn table_end(&mut self) {}

    fn box_start(&mut self, flag: bool) {
        self.table_start();
        if !flag {
            self.put("\n");
        }
    }

    fn box_end(&mut self) {
        self.table_end();
    }

    fn hr(&mut self) {
        self.put(
            "\n\n _______________________________________________________________________\n\n",
        );
    }

    fn colspan_header(&mut self, header: &str) {
        let spaces = 74 - header.len() as i64;
        let pad = " ".repeat((spaces / 2).unsigned_abs() as usize);
        self.put(format!("{pad}{header}{pad}\n"));
    }

    fn header(&mut self, cols: &[&[u8]]) {
        for (i, c) in cols.iter().enumerate() {
            let c: &[u8] = if c.is_empty() { b" " } else { c };
            self.put(c);
            self.put(if i + 1 < cols.len() { " => " } else { "\n" });
        }
    }

    fn row(&mut self, cols: &[&[u8]]) {
        for (i, c) in cols.iter().enumerate() {
            if c.is_empty() {
                self.put(" ");
            } else {
                self.put(c);
                if i + 1 < cols.len() {
                    self.put(" => ");
                }
            }
            if i + 1 == cols.len() {
                self.put("\n");
            }
        }
    }

    fn row2(&mut self, a: &str, b: &str) {
        self.row(&[a.as_bytes(), b.as_bytes()]);
    }

    /// `SECTION(name)`
    fn section(&mut self, name: &str) {
        self.table_start();
        self.header(&[name.as_bytes()]);
        self.table_end();
    }
}

// ---------------------------------------------------------------------------
// php_get_uname, version helpers
// ---------------------------------------------------------------------------

fn is_valid_uname_mode(c: u8) -> bool {
    matches!(c, b'a' | b'm' | b'n' | b'r' | b's' | b'v')
}

/// `php_get_uname()`
pub fn php_get_uname(mode: u8) -> Vec<u8> {
    let Some([sysname, nodename, release, version, machine]) = sys::uname() else {
        // PHP_UNAME
        let os = std::env::consts::OS;
        let mut c = os.chars();
        return c
            .next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
            .into_bytes();
    };
    match mode {
        b's' => sysname,
        b'r' => release,
        b'n' => nodename,
        b'v' => version,
        b'm' => machine,
        _ => {
            // assume mode == 'a'
            let mut out = sysname;
            for part in [nodename, release, version, machine] {
                out.push(b' ');
                out.extend_from_slice(&part);
            }
            out
        }
    }
}

/// `zend_get_module_version()`: every bundled module reports `PHP_VERSION`.
fn module_version(vm: &Vm, name: &[u8]) -> Option<&'static str> {
    vm.modules
        .iter()
        .any(|m| m.as_bytes().eq_ignore_ascii_case(name))
        .then_some(version::VERSION)
}

/// Directives displayed as `On`/`Off` (`zend_ini_boolean_displayer_cb`).
const BOOLEAN_DIRECTIVES: &[&str] = &[
    "allow_url_fopen",
    "allow_url_include",
    "assert.active",
    "assert.bail",
    "assert.exception",
    "assert.warning",
    "auto_detect_line_endings",
    "auto_globals_jit",
    "display_startup_errors",
    "enable_dl",
    "enable_post_data_reading",
    "error_include_args",
    "expose_php",
    "file_uploads",
    "html_errors",
    "ignore_repeated_errors",
    "ignore_repeated_source",
    "ignore_user_abort",
    "implicit_flush",
    "log_errors",
    "mail.add_x_header",
    "mail.mixed_lf_and_crlf",
    "register_argc_argv",
    "report_memleaks",
    "report_zend_debug",
    "short_open_tag",
    "xmlrpc_errors",
    "zend.detect_unicode",
    "zend.enable_gc",
    "zend.exception_ignore_args",
    "zend.multibyte",
    "mbstring.strict_detection",
    "mbstring.encoding_translation",
    "pcre.jit",
];

fn display_value(value: Option<&PhpStr>, boolean: bool) -> Vec<u8> {
    if boolean {
        let on = value.is_some_and(|v| php_runtime::ini::parse_bool(v));
        return if on { b"On".to_vec() } else { b"Off".to_vec() };
    }
    match value {
        Some(v) if !v.is_empty() => v.to_vec(),
        _ => b"no value".to_vec(),
    }
}

/// `display_ini_entries()`: the directives of one module (`None` = Core).
fn display_ini_entries(vm: &Vm, out: &mut Text, module: Option<&str>) {
    let module = module.unwrap_or("Core");
    let mut rows: Vec<(PhpStr, Option<PhpStr>, Option<Option<PhpStr>>)> = vm
        .ini
        .iter()
        .filter(|e| e.module.eq_ignore_ascii_case(module))
        .map(|e| (e.name.clone(), e.value.clone(), e.orig_value.clone()))
        .collect();
    if rows.is_empty() {
        return;
    }
    rows.sort_by(|a, b| {
        a.0.to_ascii_lowercase()
            .as_bytes()
            .cmp(b.0.to_ascii_lowercase().as_bytes())
    });
    out.table_start();
    out.header(&[b"Directive", b"Local Value", b"Master Value"]);
    for (name, value, orig) in rows {
        let n = name.to_string_lossy();
        let boolean = BOOLEAN_DIRECTIVES.contains(&n.as_ref());
        out.put(&name);
        out.put(" => ");
        out.put(display_value(value.as_ref(), boolean));
        out.put(" => ");
        let master = match &orig {
            Some(o) => o.clone(),
            None => value.clone(),
        };
        out.put(display_value(master.as_ref(), boolean));
        out.put("\n");
    }
    out.table_end();
}

/// `php_info_print_module()`
fn print_module(vm: &Vm, out: &mut Text, name: &str) {
    out.table_start();
    out.header(&[name.as_bytes()]);
    out.table_end();
    match name {
        "Core" => display_ini_entries(vm, out, None),
        "date" => {
            out.table_start();
            out.row2("date/time support", "enabled");
            out.row2("timelib version", php_timelib::TIMELIB_ASCII_VERSION);
            out.row2(
                "\"Olson\" Timezone Database Version",
                php_timelib::builtin_db().version(),
            );
            out.row2("Timezone Database", "internal");
            out.row2("Default timezone", &super::basic::default_timezone(vm));
            out.table_end();
            display_ini_entries(vm, out, Some(name));
        }
        "standard" => {
            out.table_start();
            out.row2("Dynamic Library Support", "enabled");
            let sendmail = vm.ini_get("sendmail_path").unwrap_or_default();
            out.row(&[b"Path to sendmail", sendmail.as_bytes()]);
            out.table_end();
            display_ini_entries(vm, out, Some(name));
        }
        _ => {
            out.table_start();
            out.row2("Version", version::VERSION);
            out.table_end();
            display_ini_entries(vm, out, Some(name));
        }
    }
}

const LICENSE_TEXT: &str = "\nLicense\n\nPHP is free software. You may redistribute it and/or modify it under the\nterms of the Modified BSD License (SPDX-License-Identifier: BSD-3-Clause).\n\nCopyright © The PHP Group and Contributors.\nCopyright © Zend Technologies Ltd., a subsidiary company of\n    Perforce Software, Inc.\n\nRedistribution and use in source and binary forms, with or without\nmodification, are permitted provided that the following conditions are met:\n\n1. Redistributions of source code must retain the above copyright notice, this\n   list of conditions and the following disclaimer.\n\n2. Redistributions in binary form must reproduce the above copyright notice,\n   this list of conditions and the following disclaimer in the documentation\n   and/or other materials provided with the distribution.\n\n3. Neither the name of the copyright holder nor the names of its\n   contributors may be used to endorse or promote products derived from\n   this software without specific prior written permission.\n\nTHIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\"\nAND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE\nIMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE\nDISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE\nFOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL\nDAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR\nSERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER\nCAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,\nOR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE\nOF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.\n";

/// `php_print_info()`
fn print_info(vm: &mut Vm, flag: i64) -> Vec<u8> {
    let mut out = Text::default();
    out.put("phpinfo()\n");

    if flag & PHP_INFO_GENERAL != 0 {
        let uname = php_get_uname(b'a');
        // (the box start is HTML-only: `if (!sapi_module.phpinfo_as_text)`)
        out.row2("PHP Version", version::VERSION);
        out.box_end();
        out.table_start();
        out.row(&[b"System", &uname]);
        let build_date = vm
            .constant("PHP_BUILD_DATE")
            .and_then(|v| v.as_str().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        out.row2("Build Date", &build_date);
        out.row2("Configure Command", "'php-in-rust'");
        out.row2("Server API", "Command Line Interface");
        out.row2("Virtual Directory Support", "disabled");
        out.row2("Configuration File (php.ini) Path", "/usr/local/etc/php");
        let (opened, scanned_path, scanned_files) = {
            let st = vm.ext_data::<BasicState>();
            (
                st.ini_opened_path.clone(),
                st.ini_scanned_path.clone(),
                st.ini_scanned_files.clone(),
            )
        };
        let none = PhpStr::from("(none)");
        out.row(&[
            b"Loaded Configuration File",
            opened.as_ref().unwrap_or(&none),
        ]);
        out.row(&[
            b"Scan this dir for additional .ini files",
            scanned_path.as_ref().unwrap_or(&none),
        ]);
        out.row(&[
            b"Additional .ini files parsed",
            scanned_files.as_ref().unwrap_or(&none),
        ]);
        out.row2("PHP API", &PHP_API_VERSION.to_string());
        out.row2("PHP Extension", &PHP_API_VERSION.to_string());
        out.row2("Zend Extension", &ZEND_EXTENSION_API_NO.to_string());
        out.row2(
            "Zend Extension Build",
            &format!("API{ZEND_EXTENSION_API_NO}{BUILD_ID_SUFFIX}"),
        );
        out.row2(
            "PHP Extension Build",
            &format!("API{PHP_API_VERSION}{BUILD_ID_SUFFIX}"),
        );
        out.row2("PHP Integer Size", "64 bits");
        out.row2("Debug Build", "no");
        out.row2("Thread Safety", "disabled");
        out.row2("Zend Signal Handling", "disabled");
        out.row2("Zend Memory Manager", "enabled");
        out.row2("Zend Multibyte Support", "disabled");
        out.row2("Zend Max Execution Timers", "disabled");
        out.row2("IPv6 Support", "enabled");
        out.row2("DTrace Support", "disabled");
        // `php_info_print_stream_hash()`: no trailing newline in text mode
        for (name, list) in [
            ("PHP Streams", "php, file, glob, data, http, ftp"),
            ("Stream Socket Transports", "tcp, udp, unix, udg"),
            (
                "Stream Filters",
                "string.rot13, string.toupper, string.tolower, convert.*, consumed, dechunk",
            ),
        ] {
            out.put(format!("\nRegistered {name} => {list}"));
        }
        out.table_end();

        // Zend Engine
        out.box_start(false);
        out.put("This program makes use of the Zend Scripting Language Engine:");
        out.put("\n");
        out.put(format!(
            "Zend Engine v{}, Copyright (c) Zend Technologies",
            version::ZEND_VERSION
        ));
        out.box_end();
    }

    if flag & PHP_INFO_CONFIGURATION != 0 {
        out.hr();
        out.section("Configuration");
        if flag & PHP_INFO_MODULES == 0 {
            out.section("PHP Core");
            display_ini_entries(vm, &mut out, None);
        }
    }

    if flag & PHP_INFO_MODULES != 0 {
        let mut names: Vec<&'static str> = vm.modules.clone();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        for name in &names {
            print_module(vm, &mut out, name);
        }
        out.section("Additional Modules");
        out.table_start();
        out.header(&[b"Module Name"]);
        out.table_end();
    }

    if flag & PHP_INFO_ENVIRONMENT != 0 {
        out.section("Environment");
        out.table_start();
        out.header(&[b"Variable", b"Value"]);
        for (k, v) in sys::env_vars() {
            if k.is_empty() {
                continue;
            }
            out.row(&[&k, &v]);
        }
        out.table_end();
    }

    if flag & PHP_INFO_VARIABLES != 0 {
        // TODO(interpreter): `$_SERVER`, `$_ENV`, ... live in the symbol
        // table, which the runtime does not expose to extensions yet.
        out.section("PHP Variables");
        out.table_start();
        out.header(&[b"Variable", b"Value"]);
        out.table_end();
    }

    if flag & PHP_INFO_CREDITS != 0 {
        out.hr();
        out.put(credits_text(PHP_CREDITS_ALL & !PHP_CREDITS_FULLPAGE));
    }

    if flag & PHP_INFO_LICENSE != 0 {
        out.put(LICENSE_TEXT);
    }
    out.0
}

// ---------------------------------------------------------------------------
// credits.c
// ---------------------------------------------------------------------------

const SAPI_CREDITS: &[(&str, &str)] = &[
    (
        "Apache 2 Handler",
        "Ian Holsman, Justin Erenkrantz (based on Apache 2 Filter code)",
    ),
    (
        "CGI / FastCGI",
        "Rasmus Lerdorf, Stig Bakken, Shane Caraveo, Dmitry Stogov",
    ),
    (
        "CLI",
        "Edin Kadribasic, Marcus Boerger, Johannes Schlueter, Moriyoshi Koizumi, Xinchen Hui",
    ),
    ("Embed", "Edin Kadribasic"),
    (
        "FastCGI Process Manager",
        "Andrei Nigmatulin, dreamcat4, Antony Dovgal, Jerome Loyet",
    ),
    ("litespeed", "George Wang"),
    ("phpdbg", "Felipe Pena, Joe Watkins, Bob Weinand"),
];

const EXT_CREDITS: &[(&str, &str)] = &[
    ("Bzip2", "Sterling Hughes"),
    (
        "Calendar",
        "Shane Caraveo, Colin Viebrock, Hartmut Holzgraefe, Wez Furlong",
    ),
    ("COM and .Net", "Wez Furlong"),
    ("ctype", "Hartmut Holzgraefe"),
    ("cURL", "Sterling Hughes"),
    ("Date/Time Support", "Derick Rethans"),
    (
        "DB-LIB (MS SQL, Sybase)",
        "Wez Furlong, Frank M. Kromann, Adam Baratz",
    ),
    ("DBA", "Sascha Schumann, Marcus Boerger"),
    (
        "DOM",
        "Christian Stocker, Rob Richards, Marcus Boerger, Nora Dossche",
    ),
    ("enchant", "Pierre-Alain Joye, Ilia Alshanetsky"),
    ("EXIF", "Rasmus Lerdorf, Marcus Boerger"),
    ("FFI", "Dmitry Stogov"),
    (
        "fileinfo",
        "Ilia Alshanetsky, Pierre Alain Joye, Scott MacVicar, Derick Rethans, Anatol Belski",
    ),
    ("Firebird driver for PDO", "Ard Biesheuvel"),
    ("FTP", "Stefan Esser, Andrew Skalski"),
    (
        "GD imaging",
        "Rasmus Lerdorf, Stig Bakken, Jim Winstead, Jouni Ahto, Ilia Alshanetsky, Pierre-Alain Joye, Marcus Boerger, Mark Randall",
    ),
    ("GetText", "Alex Plotnick"),
    ("GNU GMP support", "Stanislav Malyshev"),
    ("Iconv", "Rui Hirokawa, Stig Bakken, Moriyoshi Koizumi"),
    (
        "Input Filter",
        "Rasmus Lerdorf, Derick Rethans, Pierre-Alain Joye, Ilia Alshanetsky",
    ),
    (
        "Internationalization",
        "Ed Batutis, David Carlier, Vladimir Iordanov, Dmitry Lakhtyuk, Stanislav Malyshev, Vadim Savchuk, Kirti Velankar",
    ),
    ("JSON", "Jakub Zelenka, Omar Kilani, Scott MacVicar"),
    (
        "LDAP",
        "Amitay Isaacs, Eric Warnke, Rasmus Lerdorf, Gerrit Thomson, Stig Venaas",
    ),
    (
        "LIBXML",
        "Christian Stocker, Rob Richards, Marcus Boerger, Wez Furlong, Shane Caraveo",
    ),
    ("Multibyte String Functions", "Tsukada Takuya, Rui Hirokawa"),
    (
        "MySQL driver for PDO",
        "George Schlossnagle, Wez Furlong, Ilia Alshanetsky, Johannes Schlueter",
    ),
    (
        "MySQLi",
        "Zak Greant, Georg Richter, Andrey Hristov, Ulf Wendel",
    ),
    (
        "MySQLnd",
        "Andrey Hristov, Ulf Wendel, Georg Richter, Johannes Schlüter",
    ),
    ("ODBC driver for PDO", "Wez Furlong"),
    (
        "ODBC",
        "Stig Bakken, Andreas Karajannis, Frank M. Kromann, Daniel R. Kalowsky",
    ),
    (
        "Opcache",
        "Andi Gutmans, Zeev Suraski, Stanislav Malyshev, Dmitry Stogov, Xinchen Hui",
    ),
    (
        "OpenSSL",
        "Stig Venaas, Wez Furlong, Sascha Kettler, Scott MacVicar, Eliot Lear",
    ),
    ("pcntl", "Jason Greene, Arnaud Le Blanc"),
    ("Perl Compatible Regexps", "Andrei Zmievski"),
    ("PHP Archive", "Gregory Beaver, Marcus Boerger"),
    (
        "PHP Data Objects",
        "Wez Furlong, Marcus Boerger, Sterling Hughes, George Schlossnagle, Ilia Alshanetsky",
    ),
    (
        "PHP hash",
        "Sara Golemon, Rasmus Lerdorf, Stefan Esser, Michael Wallner, Scott MacVicar",
    ),
    ("Posix", "Kristian Koehntopp"),
    (
        "PostgreSQL driver for PDO",
        "Edin Kadribasic, Ilia Alshanetsky",
    ),
    (
        "PostgreSQL",
        "Jouni Ahto, Zeev Suraski, Yasuo Ohgaki, Chris Kings-Lynne",
    ),
    (
        "random",
        "Go Kudo, Tim Düsterhus, Guilliam Xavier, Christoph M. Becker, Jakub Zelenka, Bob Weinand, Máté Kocsis, and Original RNG implementers",
    ),
    ("Readline", "Thies C. Arntzen"),
    (
        "Reflection",
        "Marcus Boerger, Timm Friebe, George Schlossnagle, Andrei Zmievski, Johannes Schlueter",
    ),
    ("Sessions", "Sascha Schumann, Andrei Zmievski"),
    (
        "Shared Memory Operations",
        "Slava Poliakov, Ilia Alshanetsky",
    ),
    ("SimpleXML", "Sterling Hughes, Marcus Boerger, Rob Richards"),
    (
        "SNMP",
        "Rasmus Lerdorf, Harrie Hazewinkel, Mike Jackson, Steven Lawrance, Johann Hanne, Boris Lytochkin",
    ),
    ("SOAP", "Brad Lafountain, Shane Caraveo, Dmitry Stogov"),
    (
        "Sockets",
        "Chris Vandomelen, Sterling Hughes, Daniel Beulshausen, Jason Greene",
    ),
    ("Sodium", "Frank Denis"),
    ("SPL", "Marcus Boerger, Etienne Kneuss"),
    ("SQLite 3.x driver for PDO", "Wez Furlong"),
    ("SQLite3", "Scott MacVicar, Ilia Alshanetsky, Brad Dewar"),
    ("System V Message based IPC", "Wez Furlong"),
    ("System V Semaphores", "Tom May"),
    ("System V Shared Memory", "Christian Cartus"),
    ("tidy", "John Coggeshall, Ilia Alshanetsky"),
    ("tokenizer", "Andrei Zmievski, Johannes Schlueter"),
    (
        "uri",
        "Máté Kocsis, Tim Düsterhus, Ignace Nyamagana Butera, Arnaud Le Blanc, Dennis Snell, Nora Dossche, Nicolas Grekas",
    ),
    ("XML", "Stig Bakken, Thies C. Arntzen, Sterling Hughes"),
    ("XMLReader", "Rob Richards"),
    ("XMLWriter", "Rob Richards, Pierre-Alain Joye"),
    ("XSL", "Christian Stocker, Rob Richards"),
    ("Zip", "Pierre-Alain Joye, Remi Collet"),
    (
        "Zlib",
        "Rasmus Lerdorf, Stefan Roehrich, Zeev Suraski, Jade Nicoletti, Michael Wallner",
    ),
];

/// `php_print_credits()` in text mode.
fn credits_text(flag: i64) -> Vec<u8> {
    let mut o = Text::default();
    o.put("PHP Credits\n");

    if flag & PHP_CREDITS_GROUP != 0 {
        o.table_start();
        o.colspan_header("PHP Group");
        o.row(&[b"Thies C. Arntzen, Stig Bakken, Shane Caraveo, Andi Gutmans, Rasmus Lerdorf, Sam Ruby, Sascha Schumann, Zeev Suraski, Jim Winstead, Andrei Zmievski"]);
        o.table_end();
    }

    if flag & PHP_CREDITS_GENERAL != 0 {
        o.table_start();
        o.colspan_header("Language Design & Concept");
        o.row(&[b"Andi Gutmans, Rasmus Lerdorf, Zeev Suraski, Marcus Boerger"]);
        o.table_end();

        o.table_start();
        o.colspan_header("PHP Authors");
        o.header(&[b"Contribution", b"Authors"]);
        for (m, a) in [
            (
                "Zend Scripting Language Engine",
                "Andi Gutmans, Zeev Suraski, Stanislav Malyshev, Marcus Boerger, Dmitry Stogov, Xinchen Hui, Nikita Popov",
            ),
            (
                "Extension Module API",
                "Andi Gutmans, Zeev Suraski, Andrei Zmievski",
            ),
            (
                "UNIX Build and Modularization",
                "Stig Bakken, Sascha Schumann, Jani Taskinen, Peter Kokot",
            ),
            (
                "Windows Support",
                "Shane Caraveo, Zeev Suraski, Wez Furlong, Pierre-Alain Joye, Anatol Belski, Kalle Sommer Nielsen",
            ),
            (
                "Server API (SAPI) Abstraction Layer",
                "Andi Gutmans, Shane Caraveo, Zeev Suraski",
            ),
            ("Streams Abstraction Layer", "Wez Furlong, Sara Golemon"),
            (
                "PHP Data Objects Layer",
                "Wez Furlong, Marcus Boerger, Sterling Hughes, George Schlossnagle, Ilia Alshanetsky",
            ),
            (
                "Output Handler",
                "Zeev Suraski, Thies C. Arntzen, Marcus Boerger, Michael Wallner",
            ),
            (
                "Consistent 64 bit support",
                "Anthony Ferrara, Anatol Belski",
            ),
        ] {
            o.row2(m, a);
        }
        o.table_end();
    }

    if flag & PHP_CREDITS_SAPI != 0 {
        o.table_start();
        o.colspan_header("SAPI Modules");
        o.header(&[b"Contribution", b"Authors"]);
        for (m, a) in SAPI_CREDITS {
            o.row2(m, a);
        }
        o.table_end();
    }

    if flag & PHP_CREDITS_MODULES != 0 {
        o.table_start();
        o.colspan_header("Module Authors");
        o.header(&[b"Module", b"Authors"]);
        for (m, a) in EXT_CREDITS {
            o.row2(m, a);
        }
        o.table_end();
    }

    if flag & PHP_CREDITS_DOCS != 0 {
        o.table_start();
        o.colspan_header("PHP Documentation");
        o.row2("Authors", "Mehdi Achour, Friedhelm Betz, Antony Dovgal, Nuno Lopes, Hannes Magnusson, Philip Olson, Georg Richter, Damien Seguy, Jakub Vrana, Adam Harvey");
        o.row2("Editor", "Peter Cowburn");
        o.row2(
            "User Note Maintainers",
            "Daniel P. Brown, Thiago Henrique Pojda",
        );
        o.row2(
            "Other Contributors",
            "Previously active authors, editors and other contributors are listed in the manual.",
        );
        o.table_end();
    }

    if flag & PHP_CREDITS_QA != 0 {
        o.table_start();
        o.colspan_header("PHP Quality Assurance Team");
        o.row(&[b"Ilia Alshanetsky, Joerg Behrens, Antony Dovgal, Stefan Esser, Moriyoshi Koizumi, Magnus Maatta, Sebastian Nohn, Derick Rethans, Melvyn Sopacua, Pierre-Alain Joye, Dmitry Stogov, Felipe Pena, David Soria Parra, Stanislav Malyshev, Julien Pauli, Stephen Zarkos, Anatol Belski, Remi Collet, Ferenc Kovacs"]);
        o.table_end();
    }

    if flag & PHP_CREDITS_WEB != 0 {
        o.table_start();
        o.colspan_header("Websites and Infrastructure team");
        o.row2("PHP Websites Team", "Rasmus Lerdorf, Hannes Magnusson, Philip Olson, Lukas Kahwe Smith, Pierre-Alain Joye, Kalle Sommer Nielsen, Peter Cowburn, Adam Harvey, Ferenc Kovacs, Levi Morrison");
        o.row2("Event Maintainers", "Damien Seguy, Daniel P. Brown");
        o.row2("Network Infrastructure", "Daniel P. Brown");
        o.row2("Windows Infrastructure", "Alex Schoenmaker");
        o.table_end();
    }
    o.0
}

// ---------------------------------------------------------------------------
// The PHP functions
// ---------------------------------------------------------------------------

fn phpinfo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // `(int) flag`
    let flag = args.int(0) as i32 as i64;
    let out = print_info(vm, flag);
    vm.echo(&out)?;
    Ok(Value::Bool(true))
}

fn phpversion(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(ext) = args.str_opt(0) else {
        return Ok(Value::str(version::VERSION));
    };
    Ok(match module_version(_vm, ext) {
        Some(v) => Value::str(v),
        None => Value::Bool(false),
    })
}

fn phpcredits(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flag = args.int(0) as i32 as i64;
    vm.echo(&credits_text(flag))?;
    Ok(Value::Bool(true))
}

fn php_sapi_name(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::str("cli"))
}

fn php_uname(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.str(0).clone();
    if mode.len() != 1 {
        return Err(vm.argument_value_error(1, "must be a single character"));
    }
    if !is_valid_uname_mode(mode[0]) {
        return Err(vm.argument_value_error(
            1,
            "must be one of \"a\", \"m\", \"n\", \"r\", \"s\", or \"v\"",
        ));
    }
    Ok(Value::String(PhpStr::from_vec(php_get_uname(mode[0]))))
}

fn php_ini_scanned_files(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(
        match vm.ext_data::<BasicState>().ini_scanned_files.clone() {
            Some(f) => Value::String(f),
            None => Value::Bool(false),
        },
    )
}

fn php_ini_loaded_file(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match vm.ext_data::<BasicState>().ini_opened_path.clone() {
        Some(f) => Value::String(f),
        None => Value::Bool(false),
    })
}

pub static PHPINFO: NativeFunction = NativeFunction {
    sig: "phpinfo(int $flags = INFO_ALL): true",
    handler: phpinfo,
};
pub static PHPVERSION: NativeFunction = NativeFunction {
    sig: "phpversion(?string $extension = null): string|false",
    handler: phpversion,
};
pub static PHPCREDITS: NativeFunction = NativeFunction {
    sig: "phpcredits(int $flags = CREDITS_ALL): true",
    handler: phpcredits,
};
pub static PHP_SAPI_NAME: NativeFunction = NativeFunction {
    sig: "php_sapi_name(): string|false",
    handler: php_sapi_name,
};
pub static PHP_UNAME: NativeFunction = NativeFunction {
    sig: "php_uname(string $mode = \"a\"): string",
    handler: php_uname,
};
pub static PHP_INI_SCANNED_FILES: NativeFunction = NativeFunction {
    sig: "php_ini_scanned_files(): string|false",
    handler: php_ini_scanned_files,
};
pub static PHP_INI_LOADED_FILE: NativeFunction = NativeFunction {
    sig: "php_ini_loaded_file(): string|false",
    handler: php_ini_loaded_file,
};

pub fn register(r: &mut Registry) {
    for (name, v) in [
        ("INFO_GENERAL", PHP_INFO_GENERAL),
        ("INFO_CREDITS", PHP_INFO_CREDITS),
        ("INFO_CONFIGURATION", PHP_INFO_CONFIGURATION),
        ("INFO_MODULES", PHP_INFO_MODULES),
        ("INFO_ENVIRONMENT", PHP_INFO_ENVIRONMENT),
        ("INFO_VARIABLES", PHP_INFO_VARIABLES),
        ("INFO_LICENSE", PHP_INFO_LICENSE),
        ("INFO_ALL", PHP_INFO_ALL),
        ("CREDITS_GROUP", PHP_CREDITS_GROUP),
        ("CREDITS_GENERAL", PHP_CREDITS_GENERAL),
        ("CREDITS_SAPI", PHP_CREDITS_SAPI),
        ("CREDITS_MODULES", PHP_CREDITS_MODULES),
        ("CREDITS_DOCS", PHP_CREDITS_DOCS),
        ("CREDITS_FULLPAGE", PHP_CREDITS_FULLPAGE),
        ("CREDITS_QA", PHP_CREDITS_QA),
        ("CREDITS_ALL", PHP_CREDITS_ALL),
    ] {
        r.constant(name, v);
    }
    r.functions(&[
        &PHPINFO,
        &PHPVERSION,
        &PHPCREDITS,
        &PHP_SAPI_NAME,
        &PHP_UNAME,
        &PHP_INI_SCANNED_FILES,
        &PHP_INI_LOADED_FILE,
    ]);
}

#[cfg(test)]
mod tests;
