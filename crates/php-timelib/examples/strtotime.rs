//! Debug helper: `cargo run -p php-timelib --example strtotime -- [--tz ZONE] [--now TS] STRING...`
//! Prints the parsed fields, errors and the resulting timestamp like PHP's
//! `date_parse()` + `strtotime()`.

use php_timelib::{NO_CLONE, Time, UNSET, ZoneType, builtin_tzinfo, format_timestamp, strtotime};

fn show(v: i64) -> String {
    if v == UNSET {
        "unset".into()
    } else {
        v.to_string()
    }
}

fn main() {
    let mut tz_name = "UTC".to_string();
    let mut now = 0i64;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tz" => tz_name = args.next().expect("zone"),
            "--now" => now = args.next().expect("ts").parse().expect("int"),
            _ => inputs.push(a),
        }
    }
    let tz = builtin_tzinfo(&tz_name).expect("valid zone");
    for input in inputs {
        let res = strtotime(&input, &builtin_tzinfo);
        let t = &res.time;
        println!("input: {input:?}");
        println!(
            "  y={} m={} d={} h={} i={} s={} us={} z={} dst={} zone_type={:?} abbr={:?} tz={:?}",
            show(t.y),
            show(t.m),
            show(t.d),
            show(t.h),
            show(t.i),
            show(t.s),
            show(t.us),
            t.z,
            t.dst,
            t.zone_type,
            t.tz_abbr,
            t.tz_name()
        );
        let r = &t.relative;
        println!(
            "  rel: {}y {}m {}d {}h {}i {}s {}us weekday={} behavior={} first_last={} special={:?} have_rel={} have_wd={} have_sp={}",
            r.y,
            r.m,
            r.d,
            r.h,
            r.i,
            r.s,
            r.us,
            r.weekday,
            r.weekday_behavior,
            r.first_last_day_of,
            r.special,
            t.have_relative,
            r.have_weekday_relative,
            r.have_special_relative
        );
        for e in &res.errors.warnings {
            println!(
                "  warning @{} ({:?}): {}",
                e.position, e.character as char, e.message
            );
        }
        for e in &res.errors.errors {
            println!(
                "  error @{} ({:?}): {}",
                e.position, e.character as char, e.message
            );
        }
        if res.errors.errors.is_empty() {
            let mut now_t = Time {
                tz_info: Some(tz.clone()),
                zone_type: ZoneType::Id,
                ..Time::new()
            };
            now_t.unixtime2local(now);
            let mut t = res.time.clone();
            t.fill_holes(&now_t, NO_CLONE);
            t.update_ts(Some(&tz));
            let s = format_timestamp(b"Y-m-d H:i:s T (U)", t.sse, Some(&tz));
            println!("  => {}", String::from_utf8_lossy(&s));
        }
    }
}
