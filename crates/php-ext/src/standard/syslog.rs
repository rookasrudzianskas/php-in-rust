//! ext/standard/syslog.c (and `main/php_syslog.c`)

use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use super::basic::sys::{self, SyslogIdent};

const LOG_USER: i64 = 1 << 3;

/// The numeric constants (identical on Linux and the BSDs/macOS).
const CONSTANTS: &[(&str, i64)] = &[
    ("LOG_EMERG", 0),
    ("LOG_ALERT", 1),
    ("LOG_CRIT", 2),
    ("LOG_ERR", 3),
    ("LOG_WARNING", 4),
    ("LOG_NOTICE", 5),
    ("LOG_INFO", 6),
    ("LOG_DEBUG", 7),
    ("LOG_KERN", 0),
    ("LOG_USER", LOG_USER),
    ("LOG_MAIL", 2 << 3),
    ("LOG_DAEMON", 3 << 3),
    ("LOG_AUTH", 4 << 3),
    ("LOG_SYSLOG", 5 << 3),
    ("LOG_LPR", 6 << 3),
    ("LOG_NEWS", 7 << 3),
    ("LOG_UUCP", 8 << 3),
    ("LOG_CRON", 9 << 3),
    ("LOG_AUTHPRIV", 10 << 3),
    ("LOG_LOCAL0", 16 << 3),
    ("LOG_LOCAL1", 17 << 3),
    ("LOG_LOCAL2", 18 << 3),
    ("LOG_LOCAL3", 19 << 3),
    ("LOG_LOCAL4", 20 << 3),
    ("LOG_LOCAL5", 21 << 3),
    ("LOG_LOCAL6", 22 << 3),
    ("LOG_LOCAL7", 23 << 3),
    ("LOG_PID", 0x01),
    ("LOG_CONS", 0x02),
    ("LOG_ODELAY", 0x04),
    ("LOG_NDELAY", 0x08),
    ("LOG_NOWAIT", 0x10),
    ("LOG_PERROR", 0x20),
];

/// `OnSetFacility`: the facility named by the `syslog.facility` directive.
pub(crate) fn parse_facility(name: &[u8]) -> Option<i64> {
    let table: &[(&str, &str, i64)] = &[
        ("LOG_AUTH", "auth", 4 << 3),
        ("LOG_AUTH", "security", 4 << 3),
        ("LOG_AUTHPRIV", "authpriv", 10 << 3),
        ("LOG_CRON", "cron", 9 << 3),
        ("LOG_DAEMON", "daemon", 3 << 3),
        ("LOG_INTERNAL_MARK", "mark", 24 << 3),
        ("LOG_KERN", "kern", 0),
        ("LOG_LPR", "lpr", 6 << 3),
        ("LOG_MAIL", "mail", 2 << 3),
        ("LOG_NEWS", "news", 7 << 3),
        ("LOG_SYSLOG", "syslog", 5 << 3),
        ("LOG_USER", "user", LOG_USER),
        ("LOG_UUCP", "uucp", 8 << 3),
        ("LOG_LOCAL0", "local0", 16 << 3),
        ("LOG_LOCAL1", "local1", 17 << 3),
        ("LOG_LOCAL2", "local2", 18 << 3),
        ("LOG_LOCAL3", "local3", 19 << 3),
        ("LOG_LOCAL4", "local4", 20 << 3),
        ("LOG_LOCAL5", "local5", 21 << 3),
        ("LOG_LOCAL6", "local6", 22 << 3),
        ("LOG_LOCAL7", "local7", 23 << 3),
    ];
    table
        .iter()
        .find(|(a, b, _)| name == a.as_bytes() || name == b.as_bytes())
        .map(|&(_, _, v)| v)
}

/// `PHP_SYSLOG_FILTER_*`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    NoCtrl,
    Ascii,
    Raw,
}

fn parse_filter(v: &[u8]) -> Option<Filter> {
    match v {
        b"all" => Some(Filter::All),
        b"no-ctrl" => Some(Filter::NoCtrl),
        b"ascii" => Some(Filter::Ascii),
        b"raw" => Some(Filter::Raw),
        _ => None,
    }
}

/// `BG(syslog_device)` and `PG(have_called_openlog)`.
#[derive(Default)]
struct SyslogState {
    device: Option<SyslogIdent>,
    have_called_openlog: bool,
}

/// `php_syslog_str()`: sends a message honouring `syslog.filter`; every
/// newline starts a new syslog record.
pub(crate) fn php_syslog_str(vm: &Vm, priority: i64, message: &[u8]) {
    let filter = vm
        .ini_get("syslog.filter")
        .and_then(|v| parse_filter(&v))
        .unwrap_or(Filter::NoCtrl);
    if filter == Filter::Raw {
        sys::syslog(priority, message);
        return;
    }
    let mut buf: Vec<u8> = Vec::new();
    for &c in message {
        if (0x20..=0x7e).contains(&c) {
            buf.push(c);
        } else if c == b'\n' {
            sys::syslog(priority, &buf);
            buf.clear();
        } else if (c >= 0x80 && filter != Filter::Ascii) || (c < 0x20 && filter == Filter::All) {
            buf.push(c);
        } else {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            buf.extend_from_slice(b"\\x");
            buf.push(HEX[(c >> 4) as usize]);
            buf.push(HEX[(c & 0xf) as usize]);
        }
    }
    sys::syslog(priority, &buf);
}

/// `php_syslog()`: opens the log with the configured ident and facility first
/// when `openlog()` was not called.
pub(crate) fn php_syslog(vm: &mut Vm, priority: i64, message: &[u8]) {
    if !vm.ext_data::<SyslogState>().have_called_openlog {
        let ident = vm.ini_get("syslog.ident").unwrap_or_default();
        let facility = vm
            .ini_get("syslog.facility")
            .and_then(|f| parse_facility(&f))
            .unwrap_or(LOG_USER);
        let id = SyslogIdent::new(&ident);
        id.open(0, facility);
        let st = vm.ext_data::<SyslogState>();
        st.device = Some(id);
        st.have_called_openlog = true;
    }
    php_syslog_str(vm, priority, message);
}

fn php_closelog(vm: &mut Vm) {
    sys::closelog();
    let st = vm.ext_data::<SyslogState>();
    st.have_called_openlog = false;
    st.device = None;
}

fn openlog(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ident = args.str(0).clone();
    if ident.contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    let (option, facility) = (args.int(1), args.int(2));
    let id = SyslogIdent::new(&ident);
    id.open(option, facility);
    let st = vm.ext_data::<SyslogState>();
    st.device = Some(id);
    st.have_called_openlog = true;
    Ok(Value::Bool(true))
}

fn closelog(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    php_closelog(vm);
    Ok(Value::Bool(true))
}

fn syslog(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_syslog_str(vm, args.int(0), args.str(1));
    Ok(Value::Bool(true))
}

pub static OPENLOG: NativeFunction = NativeFunction {
    sig: "openlog(string $prefix, int $flags, int $facility): true",
    handler: openlog,
};
pub static CLOSELOG: NativeFunction = NativeFunction {
    sig: "closelog(): true",
    handler: closelog,
};
pub static SYSLOG: NativeFunction = NativeFunction {
    sig: "syslog(int $priority, string $message): true",
    handler: syslog,
};

pub fn register(r: &mut Registry) {
    // `main/main.c` directives, owned by Core.
    for def in [
        IniDef::new("syslog.facility", Some("LOG_USER"), modifiable::SYSTEM)
            .validated(|v| parse_facility(v).is_some()),
        IniDef::new("syslog.ident", Some("php"), modifiable::SYSTEM),
        IniDef::new("syslog.filter", Some("no-ctrl"), modifiable::ALL)
            .validated(|v| parse_filter(v).is_some()),
    ] {
        r.vm.ini.register(&def, "Core");
    }
    super::basic::ini::snapshot_defaults(r.vm);
    for &(name, value) in CONSTANTS {
        r.constant(name, value);
    }
    r.functions(&[&OPENLOG, &CLOSELOG, &SYSLOG]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    #[test]
    fn constants_match_libc() {
        let t = TestVm::new();
        assert_eq!(t.vm.constant("LOG_ERR").unwrap().as_int(), Some(3));
        assert_eq!(t.vm.constant("LOG_USER").unwrap().as_int(), Some(8));
        assert_eq!(t.vm.constant("LOG_LOCAL7").unwrap().as_int(), Some(184));
        assert_eq!(t.vm.constant("LOG_PID").unwrap().as_int(), Some(1));
        assert_eq!(t.vm.constant("LOG_PERROR").unwrap().as_int(), Some(32));
    }

    #[test]
    fn facility_names() {
        assert_eq!(parse_facility(b"LOG_USER"), Some(8));
        assert_eq!(parse_facility(b"user"), Some(8));
        assert_eq!(parse_facility(b"local3"), Some(152));
        assert_eq!(parse_facility(b"security"), Some(32));
        assert_eq!(parse_facility(b"bogus"), None);
    }

    #[test]
    fn ini_directives_are_validated() {
        let mut t = TestVm::new();
        assert_eq!(t.vm.ini_get("syslog.filter").unwrap(), "no-ctrl");
        assert!(t.vm.ini_set("syslog.filter", "raw").is_some());
        assert!(t.vm.ini_set("syslog.filter", "weird").is_none());
        // syslog.facility is PHP_INI_SYSTEM
        assert!(t.vm.ini_set("syslog.facility", "LOG_MAIL").is_none());
    }

    #[test]
    fn openlog_rejects_null_bytes() {
        let mut t = TestVm::new();
        let r = t.call(
            "openlog",
            vec![Value::bytes(b"a\0b"), Value::Int(0), Value::Int(8)],
        );
        let (class, msg) = thrown(r);
        assert_eq!(class, "ValueError");
        assert_eq!(
            msg,
            "openlog(): Argument #1 ($prefix) must not contain any null bytes"
        );
    }

    #[test]
    fn open_log_close_return_true() {
        let mut t = TestVm::new();
        let r = t
            .call(
                "openlog",
                vec![
                    Value::str("php-in-rust-test"),
                    Value::Int(0),
                    Value::Int(8 << 3),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        assert!(matches!(
            t.call("closelog", vec![]).unwrap(),
            Value::Bool(true)
        ));
    }
}
