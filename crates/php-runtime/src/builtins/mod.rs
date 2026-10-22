//! The `Core` module: engine constants, core ini directives and the internal
//! classes the runtime itself needs (`stdClass`, core interfaces and the
//! `Throwable` hierarchy).

pub mod exceptions;

use crate::class::{ClassEntry, class_flags};
use crate::errors::level::*;
use crate::ini::{IniDef, modifiable, validate_precision};
use crate::native::Registry;
use crate::output::ob;
use crate::vm::version;

/// `PHP_OS` for the build target.
pub const PHP_OS: &str = if cfg!(target_os = "macos") {
    "Darwin"
} else if cfg!(target_os = "linux") {
    "Linux"
} else if cfg!(target_os = "windows") {
    "WINNT"
} else if cfg!(target_os = "freebsd") {
    "FreeBSD"
} else if cfg!(target_os = "openbsd") {
    "OpenBSD"
} else if cfg!(target_os = "netbsd") {
    "NetBSD"
} else {
    "Unknown"
};

/// `PHP_OS_FAMILY` for the build target.
pub const PHP_OS_FAMILY: &str = if cfg!(target_os = "macos") {
    "Darwin"
} else if cfg!(target_os = "linux") {
    "Linux"
} else if cfg!(target_os = "windows") {
    "Windows"
} else if cfg!(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)) {
    "BSD"
} else if cfg!(target_os = "solaris") {
    "Solaris"
} else {
    "Unknown"
};

const ALL: u32 = modifiable::ALL;
const SYS: u32 = modifiable::SYSTEM;
const SYS_PERDIR: u32 = modifiable::SYSTEM | modifiable::PERDIR;

/// Core ini directives (`main/main.c`, `Zend/zend.c`) with php-src defaults.
pub const CORE_INI: &[IniDef] = &[
    IniDef::new("precision", Some("14"), ALL).validated(validate_precision),
    IniDef::new("serialize_precision", Some("-1"), ALL).validated(validate_precision),
    IniDef::new("error_reporting", None, ALL),
    IniDef::new("display_errors", Some("1"), ALL),
    IniDef::new("display_startup_errors", Some("1"), ALL),
    IniDef::new("html_errors", Some("1"), ALL),
    IniDef::new("log_errors", Some("0"), ALL),
    IniDef::new("error_log", None, ALL),
    IniDef::new("error_log_mode", Some("0644"), ALL),
    IniDef::new("error_prepend_string", None, ALL),
    IniDef::new("error_append_string", None, ALL),
    IniDef::new("error_include_args", Some("0"), ALL),
    IniDef::new("ignore_repeated_errors", Some("0"), ALL),
    IniDef::new("ignore_repeated_source", Some("0"), ALL),
    IniDef::new("report_memleaks", Some("1"), ALL),
    IniDef::new("docref_root", Some(""), ALL),
    IniDef::new("docref_ext", Some(""), ALL),
    IniDef::new("xmlrpc_errors", Some("0"), SYS),
    IniDef::new("xmlrpc_error_number", Some("0"), ALL),
    IniDef::new("implicit_flush", Some("0"), ALL),
    IniDef::new("output_buffering", Some("0"), SYS_PERDIR),
    IniDef::new("output_handler", None, SYS_PERDIR),
    IniDef::new("memory_limit", Some("128M"), ALL),
    IniDef::new("max_memory_limit", Some("-1"), SYS),
    IniDef::new("max_execution_time", Some("30"), ALL),
    IniDef::new("max_input_time", Some("-1"), SYS_PERDIR),
    IniDef::new("max_input_nesting_level", Some("64"), SYS_PERDIR),
    IniDef::new("max_input_vars", Some("1000"), SYS_PERDIR),
    IniDef::new("default_charset", Some("UTF-8"), ALL),
    IniDef::new("default_mimetype", Some("text/html"), ALL),
    IniDef::new("internal_encoding", None, ALL),
    IniDef::new("input_encoding", None, ALL),
    IniDef::new("output_encoding", None, ALL),
    IniDef::new("include_path", Some(".:/usr/share/php"), ALL),
    IniDef::new("open_basedir", None, ALL),
    IniDef::new("disable_functions", Some(""), SYS),
    IniDef::new("auto_prepend_file", None, SYS_PERDIR),
    IniDef::new("auto_append_file", None, SYS_PERDIR),
    IniDef::new("auto_globals_jit", Some("1"), SYS_PERDIR),
    IniDef::new("register_argc_argv", Some("0"), SYS_PERDIR),
    IniDef::new("variables_order", Some("EGPCS"), SYS_PERDIR),
    IniDef::new("request_order", None, SYS_PERDIR),
    IniDef::new("arg_separator.output", Some("&"), ALL),
    IniDef::new("arg_separator.input", Some("&"), SYS_PERDIR),
    IniDef::new("short_open_tag", Some("1"), SYS_PERDIR),
    IniDef::new("enable_dl", Some("1"), SYS),
    IniDef::new("expose_php", Some("1"), SYS),
    IniDef::new("file_uploads", Some("1"), SYS),
    IniDef::new("upload_max_filesize", Some("2M"), SYS_PERDIR),
    IniDef::new("post_max_size", Some("8M"), SYS_PERDIR),
    IniDef::new("upload_tmp_dir", None, SYS),
    IniDef::new("max_file_uploads", Some("20"), SYS_PERDIR),
    IniDef::new("allow_url_fopen", Some("1"), SYS),
    IniDef::new("allow_url_include", Some("0"), SYS),
    IniDef::new("enable_post_data_reading", Some("1"), SYS_PERDIR),
    IniDef::new("sys_temp_dir", None, SYS),
    IniDef::new("user_dir", None, SYS),
    IniDef::new("doc_root", None, SYS),
    IniDef::new("extension_dir", Some("/usr/local/lib/php/extensions"), SYS),
    IniDef::new("unserialize_callback_func", None, ALL),
    IniDef::new("realpath_cache_size", Some("4096K"), SYS),
    IniDef::new("realpath_cache_ttl", Some("120"), SYS),
    IniDef::new("hard_timeout", Some("2"), SYS),
    IniDef::new("ignore_user_abort", Some("0"), ALL),
    IniDef::new("fatal_error_backtraces", Some("1"), ALL),
    IniDef::new("zend.assertions", Some("1"), ALL),
    IniDef::new("zend.enable_gc", Some("1"), ALL),
    IniDef::new("zend.multibyte", Some("0"), modifiable::PERDIR),
    IniDef::new("zend.script_encoding", None, ALL),
    IniDef::new("zend.detect_unicode", Some("1"), ALL),
    IniDef::new("zend.exception_ignore_args", Some("0"), ALL),
    IniDef::new("zend.exception_string_param_max_len", Some("15"), ALL),
    IniDef::new("fiber.stack_size", None, ALL),
    IniDef::new("zend.max_allowed_stack_size", Some("0"), SYS),
    IniDef::new("zend.reserved_stack_size", Some("0"), SYS),
    IniDef::new("date.timezone", Some("UTC"), ALL),
    IniDef::new("assert.active", Some("1"), ALL),
    IniDef::new("assert.exception", Some("1"), ALL),
    IniDef::new("assert.warning", Some("1"), ALL),
    IniDef::new("assert.bail", Some("0"), ALL),
    IniDef::new("assert.callback", None, ALL),
    IniDef::new("default_socket_timeout", Some("60"), ALL),
    IniDef::new("user_agent", None, ALL),
    IniDef::new("auto_detect_line_endings", Some("0"), ALL),
    IniDef::new("from", None, ALL),
];

/// Registers the `Core` module.
pub fn register_core(r: &mut Registry<'_>) {
    r.ini_entries(CORE_INI);

    for (name, v) in [
        ("E_ERROR", E_ERROR),
        ("E_RECOVERABLE_ERROR", E_RECOVERABLE_ERROR),
        ("E_WARNING", E_WARNING),
        ("E_PARSE", E_PARSE),
        ("E_NOTICE", E_NOTICE),
        ("E_DEPRECATED", E_DEPRECATED),
        ("E_CORE_ERROR", E_CORE_ERROR),
        ("E_CORE_WARNING", E_CORE_WARNING),
        ("E_COMPILE_ERROR", E_COMPILE_ERROR),
        ("E_COMPILE_WARNING", E_COMPILE_WARNING),
        ("E_USER_ERROR", E_USER_ERROR),
        ("E_USER_WARNING", E_USER_WARNING),
        ("E_USER_NOTICE", E_USER_NOTICE),
        ("E_USER_DEPRECATED", E_USER_DEPRECATED),
        ("E_ALL", E_ALL),
    ] {
        r.constant(name, v);
    }
    r.deprecated_constant(
        "E_STRICT",
        E_STRICT,
        "since 8.4, the error level was removed",
    );
    r.constant("DEBUG_BACKTRACE_PROVIDE_OBJECT", 1i64);
    r.constant("DEBUG_BACKTRACE_IGNORE_ARGS", 2i64);
    r.constant("ZEND_THREAD_SAFE", false);
    r.constant("ZEND_DEBUG_BUILD", false);
    r.constant("ZEND_VM_KIND", "HYBRID");
    r.constant("TRUE", true);
    r.constant("FALSE", false);
    r.constant("NULL", crate::value::Value::Null);

    r.constant("PHP_VERSION", version::VERSION);
    r.constant("PHP_MAJOR_VERSION", version::MAJOR);
    r.constant("PHP_MINOR_VERSION", version::MINOR);
    r.constant("PHP_RELEASE_VERSION", version::RELEASE);
    r.constant("PHP_EXTRA_VERSION", version::EXTRA);
    r.constant("PHP_VERSION_ID", version::VERSION_ID);
    r.constant("PHP_BUILD_DATE", "Jan  1 2026 00:00:00");
    r.constant("PHP_ZTS", 0i64);
    r.constant("PHP_DEBUG", 0i64);
    r.constant("PHP_OS", PHP_OS);
    r.constant("PHP_OS_FAMILY", PHP_OS_FAMILY);
    r.constant("DEFAULT_INCLUDE_PATH", ".:/usr/share/php");
    r.constant("PEAR_INSTALL_DIR", "/usr/share/php");
    r.constant("PEAR_EXTENSION_DIR", "/usr/local/lib/php/extensions");
    r.constant("PHP_EXTENSION_DIR", "/usr/local/lib/php/extensions");
    r.constant("PHP_PREFIX", "/usr/local");
    r.constant("PHP_BINDIR", "/usr/local/bin");
    r.constant("PHP_SBINDIR", "/usr/local/sbin");
    r.constant("PHP_MANDIR", "/usr/local/share/man");
    r.constant("PHP_LIBDIR", "/usr/local/lib/php");
    r.constant("PHP_DATADIR", "/usr/local/share/php");
    r.constant("PHP_SYSCONFDIR", "/usr/local/etc");
    r.constant("PHP_LOCALSTATEDIR", "/usr/local/var");
    r.constant("PHP_CONFIG_FILE_PATH", "/usr/local/etc/php");
    r.constant("PHP_CONFIG_FILE_SCAN_DIR", "");
    r.constant(
        "PHP_SHLIB_SUFFIX",
        if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        },
    );
    r.constant(
        "PHP_EOL",
        if cfg!(target_os = "windows") {
            "\r\n"
        } else {
            "\n"
        },
    );
    r.constant(
        "PHP_MAXPATHLEN",
        if cfg!(target_os = "linux") {
            4096i64
        } else {
            1024i64
        },
    );
    r.constant("PHP_INT_MAX", i64::MAX);
    r.constant("PHP_INT_MIN", i64::MIN);
    r.constant("PHP_INT_SIZE", 8i64);
    r.constant("PHP_FD_SETSIZE", 1024i64);
    r.constant("PHP_FLOAT_DIG", 15i64);
    r.constant("PHP_FLOAT_EPSILON", f64::EPSILON);
    r.constant("PHP_FLOAT_MAX", f64::MAX);
    r.constant("PHP_FLOAT_MIN", f64::MIN_POSITIVE);
    r.constant("PHP_SAPI", "cli");
    r.constant("PHP_BINARY", "");

    r.constant("PHP_OUTPUT_HANDLER_START", ob::START);
    r.constant("PHP_OUTPUT_HANDLER_WRITE", ob::WRITE);
    r.constant("PHP_OUTPUT_HANDLER_FLUSH", ob::FLUSH);
    r.constant("PHP_OUTPUT_HANDLER_CLEAN", ob::CLEAN);
    r.constant("PHP_OUTPUT_HANDLER_FINAL", ob::FINAL);
    r.constant("PHP_OUTPUT_HANDLER_CONT", ob::CONT);
    r.constant("PHP_OUTPUT_HANDLER_END", ob::END);
    r.constant("PHP_OUTPUT_HANDLER_CLEANABLE", ob::CLEANABLE);
    r.constant("PHP_OUTPUT_HANDLER_FLUSHABLE", ob::FLUSHABLE);
    r.constant("PHP_OUTPUT_HANDLER_REMOVABLE", ob::REMOVABLE);
    r.constant("PHP_OUTPUT_HANDLER_STDFLAGS", ob::STDFLAGS);
    r.constant("PHP_OUTPUT_HANDLER_STARTED", ob::STARTED);
    r.constant("PHP_OUTPUT_HANDLER_DISABLED", ob::DISABLED);
    r.constant("PHP_OUTPUT_HANDLER_PROCESSED", ob::PROCESSED);

    for (i, name) in [
        "UPLOAD_ERR_OK",
        "UPLOAD_ERR_INI_SIZE",
        "UPLOAD_ERR_FORM_SIZE",
        "UPLOAD_ERR_PARTIAL",
        "UPLOAD_ERR_NO_FILE",
        "",
        "UPLOAD_ERR_NO_TMP_DIR",
        "UPLOAD_ERR_CANT_WRITE",
        "UPLOAD_ERR_EXTENSION",
    ]
    .iter()
    .enumerate()
    {
        if !name.is_empty() {
            r.constant(name, i as i64);
        }
    }

    r.class(ClassEntry::new(
        "stdClass",
        class_flags::ALLOW_DYNAMIC_PROPERTIES,
    ));
    let traversable = r.class(ClassEntry::new("Traversable", class_flags::INTERFACE));
    let mut iterator = ClassEntry::new("Iterator", class_flags::INTERFACE);
    iterator.interfaces.push(traversable.clone());
    r.class(iterator);
    let mut aggregate = ClassEntry::new("IteratorAggregate", class_flags::INTERFACE);
    aggregate.interfaces.push(traversable);
    r.class(aggregate);
    r.class(ClassEntry::new("ArrayAccess", class_flags::INTERFACE));
    r.class(ClassEntry::new("Countable", class_flags::INTERFACE));
    r.class(ClassEntry::new("Serializable", class_flags::INTERFACE));
    r.class(ClassEntry::new("Stringable", class_flags::INTERFACE));
    crate::enums::register_interfaces(r);

    exceptions::register(r);
}
