//! Smoke tests for the phpt helper layer.
mod common;
use common::*;

/// ext/date/tests/date_parse_001.phpt
#[test]
fn date_parse_001() {
    let mut out = String::new();
    for s in [
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
        out.push_str(&date_parse(s).var_dump());
    }
    assert_text(
        &out,
        r#"array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(12)
  ["hour"]=>
  int(10)
  ["minute"]=>
  int(0)
  ["second"]=>
  int(0)
  ["fraction"]=>
  float(0.5)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(12)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(1)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [7]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(-43200)
  ["is_dst"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(2)
  ["day"]=>
  int(30)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [11]=>
    string(27) "The parsed date was invalid"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(3)
  ["day"]=>
  int(4)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(3)
  ["day"]=>
  int(1)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(2)
  ["errors"]=>
  array(2) {
    [0]=>
    string(20) "Unexpected character"
    [1]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(-10800)
  ["is_dst"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(0)
  ["is_dst"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(12) "Empty string"
  }
  ["is_localtime"]=>
  bool(false)
}
"#,
    );
}

/// ext/date/tests/strtotime_basic.phpt
#[test]
fn strtotime_basic() {
    let f = |s: &str| date("Y-m-d", strtotime_abs(s, "UTC").unwrap(), "UTC");
    assert_eq!(f("1 Monday December 2008"), "2008-12-01");
    assert_eq!(f("2 Monday December 2008"), "2008-12-08");
    assert_eq!(f("3 Monday December 2008"), "2008-12-15");
    assert_eq!(f("first Monday December 2008"), "2008-12-08");
    assert_eq!(f("second Monday December 2008"), "2008-12-15");
    assert_eq!(f("third Monday December 2008"), "2008-12-22");
}
