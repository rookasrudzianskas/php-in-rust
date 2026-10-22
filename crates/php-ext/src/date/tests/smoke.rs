//! First end-to-end checks of the module.

use super::support::*;

#[test]
fn date_and_strtotime() {
    let mut p = P::tz("Europe/Amsterdam");
    let ts = p.call("strtotime", vec![s("2005-07-14 22:30:41")]);
    p.dump(&ts);
    let d = p.call("date", vec![s("D, d M Y H:i:s T"), ts]);
    p.dump(&d);
    p.expect("int(1121373041)\nstring(30) \"Thu, 14 Jul 2005 22:30:41 CEST\"");
}

#[test]
fn datetime_var_dump() {
    let mut p = P::new();
    let d = p.new_obj("DateTime", vec![s("2005-07-14 22:30:41")]);
    p.dump(&d);
    p.expectf(
        r#"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2005-07-14 22:30:41.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
    let r = p.try_new("DateTime", vec![s("foo")]);
    p.catch(r);
    p.expect("DateMalformedStringException: Failed to parse time string (foo) at position 0 (f): The timezone could not be found in the database");
}
