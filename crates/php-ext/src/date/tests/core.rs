//! Representative phpt files covering each class.

use super::support::*;

#[test]
fn date_interval_serialize_001() {
    let mut p = P::phpt("DateInterval_serialize-001");
    p.call("date_default_timezone_set", vec![s("Europe/London")]);
    let d = p.new_obj("DateInterval", vec![s("P2Y4DT6H8M")]);
    p.print("Original object:\n");
    p.dump(&d);
    p.print("\n\nSerialised object:\n");
    let ser = p.call("serialize", vec![d.clone()]);
    p.dump(&ser);
    p.print("\n\nUnserialised object:\n");
    let e = p.call("unserialize", vec![ser]);
    p.dump(&e);
    p.print("\n\nCalling __serialize manually:\n");
    let a = p.m(&d, "__serialize", vec![]);
    p.dump(&a);
    p.print("\n\nUsed serialised interval:\n");
    let now = p.new_obj("DateTimeImmutable", vec![s("2022-04-22 16:25:11 BST")]);
    let x = p.m(&now, "add", vec![e.clone()]);
    p.dump(&x);
    drop(x);
    let x = p.m(&now, "sub", vec![e.clone()]);
    p.dump(&x);
    p.finish();
}

#[test]
fn date_period_serialize_001() {
    let mut p = P::phpt("DatePeriod_serialize-001");
    p.call("date_default_timezone_set", vec![s("Europe/London")]);
    let d = p.sm(
        "DatePeriod",
        "createFromISO8601String",
        vec![s("R4/2012-07-01T00:00:00Z/P7D")],
    );
    p.print("Original object:\n");
    p.dump(&d);
    p.print("\n\nSerialised object:\n");
    let ser = p.call("serialize", vec![d.clone()]);
    p.dump(&ser);
    p.print("\n\nUnserialised object:\n");
    let e = p.call("unserialize", vec![ser]);
    p.dump(&e);
    p.print("\n\nCalling __serialize manually:\n");
    let a = p.m(&d, "__serialize", vec![]);
    p.dump(&a);
    p.finish();
}

#[test]
fn date_period_iterator_aggregate() {
    let mut p = P::phpt("DatePeriod_IteratorAggregate");
    let period = p.new_obj("DatePeriod", vec![s("R2/2012-07-01T00:00:00Z/P7D")]);
    for _ in 0..4 {
        let iter = p.m(&period, "getIterator", vec![]);
        p.m(&iter, "rewind", vec![]);
        loop {
            if !p.m(&iter, "valid", vec![]).as_bool().unwrap() {
                break;
            }
            let k = p.m(&iter, "key", vec![]);
            let date = p.m(&iter, "current", vec![]);
            let f = p.m(&date, "format", vec![s("Y-m-d")]);
            p.echo(&[k, s(": "), f, s("\n")]);
            p.m(&iter, "next", vec![]);
        }
        p.print("\n");
    }
    p.subclass("MyDatePeriod1", "DatePeriod");
    let period = p.new_obj("MyDatePeriod1", vec![s("R2/2012-07-01T00:00:00Z/P7D")]);
    let iter = p.m(&period, "getIterator", vec![]);
    while p.m(&iter, "valid", vec![]).as_bool().unwrap() {
        let k = p.m(&iter, "key", vec![]);
        let date = p.m(&iter, "current", vec![]);
        let f = p.m(&date, "format", vec![s("Y-m-d")]);
        p.echo(&[k, s(": "), f, s("\n")]);
        p.m(&iter, "next", vec![]);
    }
    let out = p.out();
    assert_eq!(
        out.matches("0: 2012-07-01\n1: 2012-07-08\n2: 2012-07-15\n").count(),
        5,
        "{out}"
    );
    assert_eq!(out.matches("Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead").count(), 2);
}

#[test]
fn multiple_calls_date_period_iterator_current() {
    let mut p = P::phpt("multiple_calls_date_period_iterator_current");
    let start = p.new_obj("DateTime", vec![s("2018-12-31 00:00:00")]);
    let interval = p.new_obj("DateInterval", vec![s("P1M")]);
    let period = p.new_obj("DatePeriod", vec![start, interval, i(1)]);
    let iter = p.m(&period, "getIterator", vec![]);
    let c = p.m(&iter, "current", vec![]);
    p.dump(&c);
    let c = p.m(&iter, "current", vec![]);
    p.dump(&c);
    let c = p.m(&iter, "current", vec![]);
    p.m(&c, "setTimestamp", vec![i(0)]);
    let c = p.m(&iter, "current", vec![]);
    p.dump(&c);
    p.finish();
}

#[test]
fn date_parse_001() {
    let mut p = P::phpt("date_parse_001");
    p.call("date_default_timezone_set", vec![s("UTC")]);
    for d in [
        "2006-12-12 10:00:00.5",
        "2006-12-12",
        "2006-12--12",
        "2006-02-30",
        "2006-03-04",
        "2006-03",
        "03-03",
        "0-0",
        "",
    ] {
        let r = p.call("date_parse", vec![s(d)]);
        p.dump(&r);
    }
    p.print("Done\n");
    p.finish();
}

#[test]
fn create_from_timestamp() {
    let mut p = P::phpt("createFromTimestamp");
    p.subclass("MyDateTime", "DateTime");
    p.subclass("MyDateTimeImmutable", "DateTimeImmutable");
    let timestamps = [
        i(1696883232),
        i(-1696883232),
        f(1696883232.013981),
        f(-1696883232.013981),
        f(0.123456),
        f(-0.123456),
        i(0),
        f(0.0),
        f(-0.0),
        f(i64::MAX as f64 + 1024.0),
        f(i64::MIN as f64 - 1025.0),
        f(f64::NAN),
        f(f64::INFINITY),
        f(f64::NEG_INFINITY),
        f(1599828571.235628),
        f(-1599828571.235628),
    ];
    for ts in timestamps {
        for class in ["DateTime", "DateTimeImmutable"] {
            let exported = p.export_str(&ts);
            p.print(&format!("{class}::createFromTimestamp("));
            p.write(&exported);
            p.print("): ");
            let r = p.try_sm(class, "createFromTimestamp", vec![ts.clone()]);
            if let Some(v) = p.catch(r) {
                p.dump(&v);
            }
        }
    }
    for class in ["MyDateTime", "MyDateTimeImmutable"] {
        p.print(&format!("{class}::createFromTimestamp(0): "));
        let r = p.try_sm(class, "createFromTimestamp", vec![i(0)]);
        if let Some(v) = p.catch(r) {
            p.dump(&v);
        }
    }
    p.finish();
}
