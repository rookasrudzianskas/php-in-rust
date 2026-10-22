use super::*;
use crate::standard::basic::set_ini_files;
use crate::testing::{TestVm, thrown};

fn run(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let r = t.call(name, args).unwrap();
    assert!(matches!(r, Value::Bool(true)), "{r:?}");
    t.output()
}

#[test]
fn constants() {
    let t = TestVm::new();
    for (n, v) in [
        ("INFO_GENERAL", 1),
        ("INFO_CREDITS", 2),
        ("INFO_CONFIGURATION", 4),
        ("INFO_MODULES", 8),
        ("INFO_ENVIRONMENT", 16),
        ("INFO_VARIABLES", 32),
        ("INFO_LICENSE", 64),
        ("INFO_ALL", 0xFFFF_FFFF),
        ("CREDITS_GROUP", 1),
        ("CREDITS_GENERAL", 2),
        ("CREDITS_SAPI", 4),
        ("CREDITS_MODULES", 8),
        ("CREDITS_DOCS", 16),
        ("CREDITS_FULLPAGE", 32),
        ("CREDITS_QA", 64),
        ("CREDITS_ALL", 0xFFFF_FFFF),
    ] {
        assert_eq!(t.vm.constant(n).unwrap().as_int(), Some(v), "{n}");
    }
    // there is no CREDITS_WEB constant in the stub
    assert!(t.vm.constant("CREDITS_WEB").is_none());
}

#[test]
fn phpinfo_zero_prints_only_the_title() {
    // phpinfo.phpt: `phpinfo(0)`
    let mut t = TestVm::new();
    assert_eq!(run(&mut t, "phpinfo", vec![Value::Int(0)]), "phpinfo()\n");
}

#[test]
fn phpinfo_general_matches_phpt_layout() {
    let mut t = TestVm::new();
    set_ini_files(
        &mut t.vm,
        Some("/etc/php.ini".into()),
        None,
        Some("/etc/conf.d/a.ini,\n/etc/conf.d/b.ini".into()),
    );
    let out = run(&mut t, "phpinfo", vec![Value::Int(PHP_INFO_GENERAL)]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "phpinfo()");
    assert_eq!(lines[1], format!("PHP Version => {}", version::VERSION));
    assert_eq!(lines[2], "");
    for prefix in [
        "System => ",
        "Build Date => ",
        "Configure Command => ",
        "Server API => Command Line Interface",
        "Virtual Directory Support => ",
        "Configuration File (php.ini) Path => ",
        "Loaded Configuration File => /etc/php.ini",
        "Scan this dir for additional .ini files => (none)",
        "Additional .ini files parsed => /etc/conf.d/a.ini,",
        "PHP API => 20260925",
        "PHP Extension => ",
        "Zend Extension => ",
        "Zend Extension Build => API",
        "PHP Extension Build => API",
        "PHP Integer Size => 64 bits",
        "Debug Build => no",
        "Thread Safety => disabled",
        "Zend Signal Handling => ",
        "Zend Memory Manager => ",
        "Zend Multibyte Support => ",
        "Zend Max Execution Timers => ",
        "IPv6 Support => ",
        "DTrace Support => ",
        "Registered PHP Streams => ",
        "Registered Stream Socket Transports => ",
        "Registered Stream Filters => ",
        "This program makes use of the Zend Scripting Language Engine:",
        "Zend Engine v",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(prefix)),
            "missing {prefix:?} in\n{out}"
        );
    }
    // the Zend box follows the stream lists after a blank line
    let i = lines
        .iter()
        .position(|l| l.starts_with("Registered Stream Filters"))
        .unwrap();
    assert_eq!(lines[i + 1], "");
    assert_eq!(
        lines[i + 2],
        "This program makes use of the Zend Scripting Language Engine:"
    );
}

#[test]
fn phpinfo_sections() {
    let mut t = TestVm::new();
    let out = run(&mut t, "phpinfo", vec![Value::Int(PHP_INFO_ALL)]);
    // the section order of phpinfo.phpt
    let mut at = 0;
    for marker in [
        "phpinfo()\n",
        "\n _______________________________________________________________________\n\n\nConfiguration\n",
        "\nCore\n",
        "\nAdditional Modules\n",
        "\nEnvironment\n",
        "\nPHP Variables\n",
        "PHP Credits\n",
        "\nLicense\n",
    ] {
        let found = out[at..].find(marker);
        assert!(found.is_some(), "missing {marker:?} after offset {at}");
        at += found.unwrap() + marker.len();
    }
    assert!(out.contains("Directive => Local Value => Master Value\n"));
    assert!(out.contains("precision => 14 => 14\n"));
    assert!(out.contains("display_errors => 1 => 1\n"));
    assert!(out.contains("Variable => Value\n"));
}

#[test]
fn phpinfo_configuration_without_modules_lists_core() {
    let mut t = TestVm::new();
    let out = run(&mut t, "phpinfo", vec![Value::Int(PHP_INFO_CONFIGURATION)]);
    assert!(out.contains("\nConfiguration\n"));
    assert!(out.contains("\nPHP Core\n"));
    assert!(out.contains("memory_limit => 128M => 128M\n"));
    // booleans show as On/Off, empty values as "no value"
    assert!(out.contains("log_errors => Off => Off\n"));
    assert!(out.contains("error_log => no value => no value\n"));
}

#[test]
fn phpinfo_modules_are_sorted() {
    let mut t = TestVm::new();
    let out = run(&mut t, "phpinfo", vec![Value::Int(PHP_INFO_MODULES)]);
    let pos = |m: &str| {
        out.find(&format!("\n{m}\n"))
            .unwrap_or_else(|| panic!("{m}"))
    };
    assert!(pos("Core") < pos("date"));
    assert!(pos("date") < pos("hash"));
    assert!(pos("hash") < pos("standard"));
    assert!(out.contains("date/time support => enabled\n"));
    assert!(out.contains("Default timezone => UTC\n"));
    assert!(out.contains("Dynamic Library Support => enabled\n"));
    assert!(out.contains("Path to sendmail => /usr/sbin/sendmail -t -i\n"));
    assert!(out.contains("assert.active => On => On\n"));
}

#[test]
fn phpinfo_flag_is_an_int() {
    // `phpinfo(-1)` is INFO_ALL as a C int
    let mut t = TestVm::new();
    let all = run(&mut t, "phpinfo", vec![Value::Int(-1)]);
    assert!(all.contains("\nLicense\n"));
}

#[test]
fn credits_match_phpcredits_phpt() {
    let mut t = TestVm::new();
    let all = run(&mut t, "phpcredits", vec![]);
    assert!(all.starts_with("PHP Credits\n\n"));
    let mut at = 0;
    for h in [
        "PHP Group",
        "Language Design & Concept",
        "PHP Authors",
        "SAPI Modules",
        "Module Authors",
        "PHP Documentation",
        "PHP Quality Assurance Team",
        "Websites and Infrastructure team",
    ] {
        let found = all[at..].find(h);
        assert!(found.is_some(), "missing {h}");
        at += found.unwrap() + h.len();
    }
    assert_eq!(
        run(&mut t, "phpcredits", vec![Value::Int(0)]),
        "PHP Credits\n"
    );
    let group = run(&mut t, "phpcredits", vec![Value::Int(PHP_CREDITS_GROUP)]);
    assert!(group.starts_with("PHP Credits\n\n"));
    assert!(group.contains("PHP Group"));
    assert!(!group.contains("PHP Authors"));
    // headers are centred in a 74 column field
    assert!(group.contains(&format!("{}PHP Group{}\n", " ".repeat(32), " ".repeat(32))));
    // rows
    let general = run(&mut t, "phpcredits", vec![Value::Int(PHP_CREDITS_GENERAL)]);
    assert!(general.contains("Contribution => Authors\n"));
    assert!(general.contains("Streams Abstraction Layer => Wez Furlong, Sara Golemon\n"));
    let modules = run(&mut t, "phpcredits", vec![Value::Int(PHP_CREDITS_MODULES)]);
    assert!(modules.contains("JSON => Jakub Zelenka, Omar Kilani, Scott MacVicar\n"));
}

#[test]
fn phpversion_of_extensions() {
    let mut t = TestVm::new();
    assert_eq!(
        t.call("phpversion", vec![]).unwrap().as_str().unwrap(),
        version::VERSION
    );
    assert_eq!(
        t.call("phpversion", vec![Value::str("standard")])
            .unwrap()
            .as_str()
            .unwrap(),
        version::VERSION
    );
    // the lookup is case-insensitive
    assert_eq!(
        t.call("phpversion", vec![Value::str("DATE")])
            .unwrap()
            .as_str()
            .unwrap(),
        version::VERSION
    );
    assert!(matches!(
        t.call("phpversion", vec![Value::str("nosuchext")]).unwrap(),
        Value::Bool(false)
    ));
}

#[test]
fn sapi_name_and_uname() {
    let mut t = TestVm::new();
    assert_eq!(
        t.call("php_sapi_name", vec![]).unwrap().as_str().unwrap(),
        "cli"
    );

    let a = t
        .call("php_uname", vec![])
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let parts: Vec<String> = ["s", "n", "r", "v", "m"]
        .iter()
        .map(|m| {
            t.call("php_uname", vec![Value::str(m)])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(a, parts.join(" "));
    assert_eq!(
        t.call("php_uname", vec![Value::str("a")])
            .unwrap()
            .as_str()
            .unwrap(),
        a.as_str()
    );
    assert_eq!(
        t.call("php_uname", vec![Value::str("s")])
            .unwrap()
            .as_str()
            .unwrap(),
        String::from_utf8_lossy(&crate::standard::basic::sys::uname().unwrap()[0]).as_ref()
    );

    let (c, m) = thrown(t.call("php_uname", vec![Value::str("")]));
    assert_eq!(c, "ValueError");
    assert_eq!(
        m,
        "php_uname(): Argument #1 ($mode) must be a single character"
    );
    let (_, m) = thrown(t.call("php_uname", vec![Value::str("x")]));
    assert_eq!(
        m,
        "php_uname(): Argument #1 ($mode) must be one of \"a\", \"m\", \"n\", \"r\", \"s\", or \"v\""
    );
    let (_, m) = thrown(t.call("php_uname", vec![Value::str("as")]));
    assert_eq!(
        m,
        "php_uname(): Argument #1 ($mode) must be a single character"
    );
}

#[test]
fn ini_file_functions() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("php_ini_loaded_file", vec![]).unwrap(),
        Value::Bool(false)
    ));
    assert!(matches!(
        t.call("php_ini_scanned_files", vec![]).unwrap(),
        Value::Bool(false)
    ));
    set_ini_files(
        &mut t.vm,
        Some("/x/loaded.ini".into()),
        Some("/x/scandir".into()),
        Some("/x/scandir/0.ini\n".into()),
    );
    assert_eq!(
        t.call("php_ini_loaded_file", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "/x/loaded.ini"
    );
    assert_eq!(
        t.call("php_ini_scanned_files", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "/x/scandir/0.ini\n"
    );
}
