//! Output buffering (`main/output.c` semantics).

use super::*;
use crate::output::ob;

fn ob(t: &mut TestVm, f: &str, args: Vec<Value>) -> Value {
    t.call(f, args).unwrap()
}

#[test]
fn unbuffered_and_basic_buffering() {
    let mut t = TestVm::new();
    t.vm.echo(b"a").unwrap();
    assert_eq!(t.output(), "a");
    assert!(matches!(ob(&mut t, "ob_start", vec![]), Value::Bool(true)));
    t.vm.echo(b"hello").unwrap();
    assert_eq!(t.output(), "");
    assert_eq!(
        ob(&mut t, "ob_get_contents", vec![]).as_str().unwrap(),
        "hello"
    );
    assert!(matches!(ob(&mut t, "ob_get_length", vec![]), Value::Int(5)));
    assert!(matches!(ob(&mut t, "ob_get_level", vec![]), Value::Int(1)));
    assert!(matches!(
        ob(&mut t, "ob_end_flush", vec![]),
        Value::Bool(true)
    ));
    assert_eq!(t.output(), "hello");
    assert!(matches!(ob(&mut t, "ob_get_level", vec![]), Value::Int(0)));
    assert!(matches!(
        ob(&mut t, "ob_get_contents", vec![]),
        Value::Bool(false)
    ));
    assert!(matches!(
        ob(&mut t, "ob_get_clean", vec![]),
        Value::Bool(false)
    ));
}

#[test]
fn nested_buffers() {
    let mut t = TestVm::new();
    ob(&mut t, "ob_start", vec![]);
    t.vm.echo(b"a").unwrap();
    ob(&mut t, "ob_start", vec![]);
    t.vm.echo(b"b").unwrap();
    assert!(matches!(ob(&mut t, "ob_get_level", vec![]), Value::Int(2)));
    ob(&mut t, "ob_end_clean", vec![]);
    t.vm.echo(b"c").unwrap();
    ob(&mut t, "ob_start", vec![]);
    t.vm.echo(b"d").unwrap();
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(ob(&mut t, "ob_get_clean", vec![]).as_str().unwrap(), "acd");
    assert_eq!(t.output(), "");
    let handlers = ob(&mut t, "ob_list_handlers", vec![]);
    assert!(handlers.as_array().unwrap().is_empty());
}

#[test]
fn flush_and_clean() {
    let mut t = TestVm::new();
    ob(&mut t, "ob_start", vec![]);
    t.vm.echo(b"x").unwrap();
    ob(&mut t, "ob_flush", vec![]);
    assert_eq!(t.output(), "x");
    t.vm.echo(b"y").unwrap();
    ob(&mut t, "ob_clean", vec![]);
    t.vm.echo(b"z").unwrap();
    assert_eq!(ob(&mut t, "ob_get_contents", vec![]).as_str().unwrap(), "z");
    ob(&mut t, "ob_end_clean", vec![]);
    assert_eq!(t.output(), "");
}

#[test]
fn chunk_size_flushes_automatically() {
    let mut t = TestVm::new();
    ob(&mut t, "ob_start", vec![Value::Null, 4.into()]);
    t.vm.echo(b"ab").unwrap();
    assert_eq!(t.output(), "");
    t.vm.echo(b"cd").unwrap();
    assert_eq!(t.output(), "abcd");
    t.vm.echo(b"e").unwrap();
    assert_eq!(t.output(), "");
    let st = ob(&mut t, "ob_get_status", vec![]);
    let st = st.as_array().unwrap();
    assert_eq!(
        st.get_str(b"name").unwrap().as_str().unwrap(),
        "default output handler"
    );
    assert!(matches!(st.get_str(b"chunk_size"), Some(Value::Int(4))));
    assert!(matches!(st.get_str(b"buffer_used"), Some(Value::Int(1))));
    assert!(matches!(st.get_str(b"buffer_size"), Some(Value::Int(4096))));
    assert!(matches!(st.get_str(b"level"), Some(Value::Int(0))));
    assert!(matches!(st.get_str(b"type"), Some(Value::Int(0))));
    let flags = st.get_str(b"flags").unwrap().as_int().unwrap();
    assert_eq!(flags, ob::STDFLAGS | ob::STARTED | ob::PROCESSED);
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(t.output(), "e");
}

#[test]
fn failure_notices() {
    let mut t = TestVm::new();
    assert!(matches!(ob(&mut t, "ob_flush", vec![]), Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nNotice: ob_flush(): Failed to flush buffer. No buffer to flush in /t.php on line 3\n"
    );
    ob(&mut t, "ob_end_clean", vec![]);
    assert_eq!(
        t.output(),
        "\nNotice: ob_end_clean(): Failed to delete buffer. No buffer to delete in /t.php on line 3\n"
    );
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(
        t.output(),
        "\nNotice: ob_end_flush(): Failed to delete and flush buffer. No buffer to delete or flush in /t.php on line 3\n"
    );
    ob(&mut t, "ob_clean", vec![]);
    assert_eq!(
        t.output(),
        "\nNotice: ob_clean(): Failed to delete buffer. No buffer to delete in /t.php on line 3\n"
    );

    // A buffer without PHP_OUTPUT_HANDLER_REMOVABLE / CLEANABLE.
    ob(&mut t, "ob_start", vec![Value::Null, 0.into(), 0.into()]);
    t.vm.echo(b"kept").unwrap();
    assert!(matches!(
        ob(&mut t, "ob_end_clean", vec![]),
        Value::Bool(false)
    ));
    assert!(matches!(ob(&mut t, "ob_clean", vec![]), Value::Bool(false)));
    assert!(matches!(
        ob(&mut t, "ob_get_clean", vec![]),
        Value::String(_)
    ));
    let contents = ob(&mut t, "ob_get_contents", vec![]);
    let text = contents.as_str().unwrap().to_string();
    assert!(text.starts_with("kept"));
    assert!(text.contains(
        "Notice: ob_end_clean(): Failed to discard buffer of default output handler (0)"
    ));
    assert!(
        text.contains("Notice: ob_clean(): Failed to delete buffer of default output handler (0)")
    );
    assert!(text.contains(
        "Notice: ob_get_clean(): Failed to discard buffer of default output handler (0)"
    ));
    assert!(
        text.contains(
            "Notice: ob_get_clean(): Failed to delete buffer of default output handler (0)"
        )
    );
    t.vm.ob_end_all().unwrap();
    assert!(t.output().starts_with("kept"));
}

#[test]
fn user_handlers() {
    let mut t = TestVm::new();
    let host = Rc::new(ClosureHost::default());
    let modes = Rc::new(RefCell::new(Vec::new()));
    let m2 = modes.clone();
    host.add("upper", move |_vm, args| {
        m2.borrow_mut().push(args[1].as_int().unwrap());
        Ok(Value::from(
            args[0].as_str().unwrap().to_ascii_uppercase().to_vec(),
        ))
    });
    host.add("reject", |_vm, _args| Ok(Value::Bool(false)));
    host.add("swallow", |_vm, _args| Ok(Value::Bool(true)));
    host.add("noisy", |vm, args| {
        vm.echo(b"[noise]")?;
        Ok(args[0].clone())
    });
    host.add("throws", |vm, _args| {
        Err(vm.throw_error("Exception", "in handler"))
    });
    t.vm.set_host(host);

    ob(&mut t, "ob_start", vec![s("upper")]);
    t.vm.echo(b"abc").unwrap();
    ob(&mut t, "ob_flush", vec![]);
    t.vm.echo(b"def").unwrap();
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(t.output(), "ABCDEF");
    assert_eq!(*modes.borrow(), vec![ob::START | ob::FLUSH, ob::FINAL]);

    ob(&mut t, "ob_start", vec![s("reject")]);
    t.vm.echo(b"raw").unwrap();
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(t.output(), "raw");

    ob(&mut t, "ob_start", vec![s("swallow")]);
    t.vm.echo(b"gone").unwrap();
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(t.output(), "");

    ob(&mut t, "ob_start", vec![s("noisy")]);
    t.vm.echo(b"data").unwrap();
    ob(&mut t, "ob_end_flush", vec![]);
    assert_eq!(
        t.output(),
        "\nDeprecated: ob_end_flush(): Producing output from user output handler noisy is deprecated in /t.php on line 3\ndata"
    );

    ob(&mut t, "ob_start", vec![s("throws")]);
    t.vm.echo(b"passes through").unwrap();
    let r = t.call("ob_end_flush", vec![]);
    assert!(matches!(r, Err(Flow::Exception(_))));
    assert_eq!(t.output(), "passes through");
    assert_eq!(t.vm.ob_get_level(), 0);

    assert!(matches!(
        ob(&mut t, "ob_start", vec![s("missing_fn")]),
        Value::Bool(false)
    ));
    assert_eq!(
        t.output(),
        "\nWarning: ob_start(): function \"missing_fn\" not found or invalid function name in /t.php on line 3\n\nNotice: ob_start(): Failed to create buffer in /t.php on line 3\n"
    );
}

#[test]
fn errors_are_buffered_and_lock_errors_are_fatal() {
    let mut t = TestVm::new();
    ob(&mut t, "ob_start", vec![]);
    t.vm.engine_warning("inside").unwrap();
    let got = ob(&mut t, "ob_get_clean", vec![]);
    assert_eq!(
        got.as_str().unwrap(),
        "\nWarning: inside in /t.php on line 3\n"
    );

    let host = Rc::new(ClosureHost::default());
    host.add("nested", |vm, args| {
        vm.call_native("ob_start", vec![])?;
        Ok(args[0].clone())
    });
    t.vm.set_host(host);
    t.vm.ini_set(b"fatal_error_backtraces", "0").unwrap();
    ob(&mut t, "ob_start", vec![]);
    t.vm.echo(b"discarded").unwrap();
    ob(&mut t, "ob_start", vec![s("nested")]);
    t.vm.echo(b"x").unwrap();
    let r = t.call("ob_end_flush", vec![]);
    assert!(matches!(r, Err(Flow::Fatal)));
    // php_output_deactivate dropped all buffers; the fatal error went straight out.
    assert_eq!(
        t.output(),
        "\nFatal error: ob_start(): Cannot use output buffering in output buffering display handlers in /t.php on line 3\n"
    );
    assert_eq!(t.vm.ob_get_level(), 0);
    t.vm.echo(b"direct").unwrap();
    assert_eq!(t.output(), "direct");
}

#[test]
fn concat_assign_appends_in_place() {
    let mut t = TestVm::new();
    let mut v = Value::str("");
    for _ in 0..1000 {
        t.vm.concat_assign(&mut v, &Value::str("ab")).unwrap();
    }
    assert_eq!(v.as_str().unwrap().len(), 2000);
    let before = v.as_str().unwrap().as_bytes().as_ptr();
    let mut v2 = v.clone();
    t.vm.concat_assign(&mut v2, &Value::Int(7)).unwrap();
    assert_eq!(v.as_str().unwrap().len(), 2000);
    assert_eq!(v2.as_str().unwrap().len(), 2001);
    assert_eq!(v.as_str().unwrap().as_bytes().as_ptr(), before);
}
