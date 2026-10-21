//! Error display (`php_error_cb`), `@`, handlers and uncaught exceptions.

use super::*;
use crate::errors::level::*;

#[test]
fn engine_and_docref_formats() {
    let mut t = TestVm::new();
    t.vm.engine_warning("Undefined variable $x").unwrap();
    assert_eq!(
        t.output(),
        "\nWarning: Undefined variable $x in /t.php on line 3\n"
    );
    t.vm.engine_notice("n").unwrap();
    assert_eq!(t.output(), "\nNotice: n in /t.php on line 3\n");
    t.vm.engine_deprecated("d").unwrap();
    assert_eq!(t.output(), "\nDeprecated: d in /t.php on line 3\n");
    t.vm.zend_error(E_USER_WARNING, "u").unwrap();
    assert_eq!(t.output(), "\nWarning: u in /t.php on line 3\n");
    t.call("t_warn", vec![s("boom")]).unwrap();
    assert_eq!(
        t.output(),
        "\nWarning: t_warn(): boom in /t.php on line 3\n"
    );
    // php_error_docref at top level uses main().
    t.vm.warning("top").unwrap();
    assert_eq!(t.output(), "\nWarning: main(): top in /t.php on line 3\n");
}

#[test]
fn reporting_mask_silence_and_last_error() {
    let mut t = TestVm::new();
    t.vm.ini_set(b"error_reporting", (E_ALL & !E_NOTICE).to_string())
        .unwrap();
    t.vm.engine_notice("hidden").unwrap();
    assert_eq!(t.output(), "");
    assert_eq!(t.vm.errors.last_error.as_ref().unwrap().message, "hidden");
    t.vm.engine_warning("shown").unwrap();
    assert_eq!(t.output(), "\nWarning: shown in /t.php on line 3\n");

    let tok = t.vm.begin_silence();
    assert_eq!(t.vm.errors.reporting, E_ALL & !E_NOTICE & E_FATAL_ERRORS);
    t.vm.engine_warning("silenced").unwrap();
    t.vm.end_silence(tok);
    assert_eq!(t.output(), "");
    assert_eq!(t.vm.errors.reporting, E_ALL & !E_NOTICE);
    let last = t.vm.error_get_last().unwrap();
    assert_eq!(
        last.get_str(b"message").unwrap().as_str().unwrap(),
        "silenced"
    );
    assert!(matches!(last.get_str(b"type"), Some(Value::Int(2))));
    assert_eq!(last.get_str(b"file").unwrap().as_str().unwrap(), "/t.php");
}

#[test]
fn prepend_append_html_and_display_modes() {
    let mut t = TestVm::new();
    t.vm.ini_set(b"error_prepend_string", "<<").unwrap();
    t.vm.ini_set(b"error_append_string", ">>").unwrap();
    t.vm.engine_warning("w").unwrap();
    assert_eq!(t.output(), "<<\nWarning: w in /t.php on line 3\n>>");
    t.vm.ini_restore(b"error_prepend_string");
    t.vm.ini_restore(b"error_append_string");

    t.vm.ini_set(b"html_errors", "1").unwrap();
    t.vm.engine_warning("a<b").unwrap();
    assert_eq!(
        t.output(),
        "<br />\n<b>Warning</b>:  a<b in <b>/t.php</b> on line <b>3</b><br />\n"
    );
    t.call("t_warn", vec![s("x<y")]).unwrap();
    assert_eq!(
        t.output(),
        "<br />\n<b>Warning</b>:  t_warn(): x&lt;y in <b>/t.php</b> on line <b>3</b><br />\n"
    );
    t.vm.ini_set(b"html_errors", "0").unwrap();

    t.vm.ini_set(b"display_errors", "0").unwrap();
    t.vm.engine_warning("nothing").unwrap();
    assert_eq!(t.output(), "");
    t.vm.ini_set(b"display_errors", "stderr").unwrap();
    t.vm.engine_warning("to stderr").unwrap();
    assert_eq!(t.output(), "");
    assert_eq!(t.log.text(), "Warning: to stderr in /t.php on line 3\n");
    t.vm.ini_set(b"display_errors", "1").unwrap();

    t.vm.ini_set(b"log_errors", "1").unwrap();
    t.log.0.borrow_mut().clear();
    t.vm.engine_warning("logged").unwrap();
    assert_eq!(t.log.text(), "PHP Warning:  logged in /t.php on line 3\n");
    assert_eq!(t.output(), "\nWarning: logged in /t.php on line 3\n");
}

#[test]
fn fatal_errors_unwind() {
    let mut t = TestVm::new();
    t.vm.ini_set(b"fatal_error_backtraces", "0").unwrap();
    let f = t.vm.fatal_error("Allowed memory size exhausted");
    assert!(matches!(f, Flow::Fatal));
    assert_eq!(
        t.output(),
        "\nFatal error: Allowed memory size exhausted in /t.php on line 3\n"
    );
    assert_eq!(t.vm.exit_status, 255);

    let mut t = TestVm::new();
    let f = t.vm.fatal_error("with trace");
    assert!(matches!(f, Flow::Fatal));
    assert_eq!(
        t.output(),
        "\nFatal error: with trace in /t.php on line 3\nStack trace:\n#0 {main}\n"
    );
}

#[test]
fn uncaught_exception_report() {
    let mut t = TestVm::new();
    let ex = t.vm.create_exception("Exception", "boo", 0);
    t.vm.report_uncaught_exception(&ex).unwrap();
    assert_eq!(
        t.output(),
        "\nFatal error: Uncaught Exception: boo in /t.php:3\nStack trace:\n#0 {main}\n  thrown in /t.php on line 3\n"
    );
    assert_eq!(t.vm.exit_status, 255);

    let mut t = TestVm::new();
    let r = t.call("intdiv", vec![1.into(), 0.into()]);
    let Err(Flow::Exception(ex)) = r else {
        panic!()
    };
    t.vm.report_uncaught_exception(&ex).unwrap();
    assert_eq!(
        t.output(),
        "\nFatal error: Uncaught DivisionByZeroError: Division by zero in /t.php:3\nStack trace:\n#0 /t.php(3): intdiv(1, 0)\n#1 {main}\n  thrown in /t.php on line 3\n"
    );
}

#[test]
fn trace_argument_rendering() {
    let mut t = TestVm::new();
    let r = t.call("str_repeat", vec![s("a long string argument"), (-1).into()]);
    let Err(Flow::Exception(ex)) = r else {
        panic!()
    };
    let trace = crate::builtins::exceptions::trace_as_string(&t.vm, &ex);
    assert_eq!(
        trace,
        "#0 /t.php(3): str_repeat('a long string a...', -1)\n#1 {main}"
    );
    let r = t.call("t_int", vec![Value::from(1.0e100), Value::empty_array()]);
    let Err(Flow::Exception(ex)) = r else {
        panic!()
    };
    let trace = crate::builtins::exceptions::trace_as_string(&t.vm, &ex);
    assert_eq!(trace, "#0 /t.php(3): t_int(1.0E+100, Array)\n#1 {main}");
}

#[test]
fn user_error_handler() {
    let mut t = TestVm::new();
    let host = Rc::new(ClosureHost::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen2 = seen.clone();
    host.add("handler", move |vm, args| {
        let msg = args[1].as_str().unwrap().to_string();
        seen2
            .borrow_mut()
            .push((args[0].as_int().unwrap(), msg.clone(), vm.errors.reporting));
        Ok(Value::Bool(msg != "fallback"))
    });
    host.add("thrower", |vm, _args| {
        Err(vm.throw_error("Exception", "from handler"))
    });
    t.vm.set_host(host);
    assert!(t.vm.set_error_handler(Some(s("handler")), E_ALL).is_none());
    t.vm.engine_warning("handled").unwrap();
    assert_eq!(t.output(), "");
    t.vm.engine_notice("fallback").unwrap();
    assert_eq!(t.output(), "\nNotice: fallback in /t.php on line 3\n");
    // The handler runs even when silenced, seeing the masked error_reporting.
    let tok = t.vm.begin_silence();
    t.vm.engine_warning("silenced").unwrap();
    t.vm.end_silence(tok);
    {
        let seen = seen.borrow();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], (E_WARNING, "handled".to_string(), E_ALL));
        assert_eq!(seen[2].2, E_FATAL_ERRORS);
    }
    // Handler mask.
    t.vm.set_error_handler(Some(s("handler")), E_NOTICE);
    t.vm.engine_warning("not for handler").unwrap();
    assert_eq!(
        t.output(),
        "\nWarning: not for handler in /t.php on line 3\n"
    );
    t.vm.restore_error_handler();
    // A throwing handler propagates.
    t.vm.set_error_handler(Some(s("thrower")), E_ALL);
    let r = t.vm.engine_warning("x");
    assert!(matches!(r, Err(Flow::Exception(_))));
    t.vm.restore_error_handler();
    t.vm.restore_error_handler();
    assert!(t.vm.errors.user_handler.is_none());
}

#[test]
fn ini_mirrors() {
    let mut t = TestVm::new();
    assert_eq!(t.vm.ini_get("precision").unwrap(), "14");
    assert_eq!(t.vm.ini_get("serialize_precision").unwrap(), "-1");
    assert_eq!(t.vm.ini_get("display_errors").unwrap(), "1");
    assert_eq!(t.vm.ini_get("html_errors").unwrap(), "0");
    assert_eq!(t.vm.ini_get("memory_limit").unwrap(), "128M");
    assert_eq!(t.vm.ini_get("date.timezone").unwrap(), "UTC");
    assert_eq!(t.vm.ini_get("error_reporting").unwrap(), "");
    assert_eq!(t.vm.ini_get("zend.assertions").unwrap(), "1");
    assert!(t.vm.ini_get("no.such.thing").is_none());
    assert_eq!(t.vm.ini_set(b"precision", "17").unwrap(), "14");
    assert_eq!(t.vm.precision, 17);
    assert_eq!(
        t.vm.to_php_string(&Value::Float(0.1)).unwrap(),
        "0.10000000000000001"
    );
    assert!(t.vm.ini_set(b"precision", "-5").is_none());
    assert!(t.vm.ini_set(b"enable_dl", "0").is_none());
    assert!(t.vm.ini_set(b"nope", "0").is_none());
    t.vm.ini_restore(b"precision");
    assert_eq!(t.vm.precision, 14);
    assert_eq!(t.vm.ini_int("memory_limit"), 128 * 1024 * 1024);
}

#[test]
fn eh_throw_mode() {
    let mut t = TestVm::new();
    t.vm.errors.mode = crate::errors::ErrorHandlingMode::Throw(PhpStr::from("ErrorException"));
    let r = t.vm.engine_warning("as exception");
    match r {
        Err(Flow::Exception(e)) => {
            assert_eq!(e.class().name, "ErrorException");
            assert!(matches!(e.get_prop(b"severity"), Some(Value::Int(2))));
        }
        other => panic!("{other:?}"),
    }
    // Notices are not converted.
    t.vm.engine_notice("still a notice").unwrap();
    assert!(t.output().contains("Notice: still a notice"));
}

#[test]
fn include_args_and_recorded_errors() {
    let mut t = TestVm::new();
    t.vm.ini_set("error_include_args", "1").unwrap();
    t.call("t_warn", vec![s("w")]).unwrap();
    assert_eq!(
        t.output(),
        "\nWarning: t_warn('w'): w in /t.php on line 3\n"
    );
    t.vm.ini_set("error_include_args", "0").unwrap();

    t.vm.ini_set("fatal_error_backtraces", "0").unwrap();
    t.vm.errors.recorded = Some(Vec::new());
    t.vm.engine_warning("delayed").unwrap();
    assert_eq!(t.output(), "");
    let f = t.vm.fatal_error("fatal");
    assert!(matches!(f, Flow::Fatal));
    assert_eq!(
        t.output(),
        "\nWarning: delayed in /t.php on line 3\n\nFatal error: fatal in /t.php on line 3\n"
    );
}
