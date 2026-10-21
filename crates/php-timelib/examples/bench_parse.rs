//! Rough parse throughput check: `cargo run --release -p php-timelib --example bench_parse`.
use std::time::Instant;

fn main() {
    let inputs = [
        "2005-07-14 22:30:41",
        "next monday",
        "last day of next month",
        "Sat, 22 Oct 2005 20:30:00 +0200",
        "+1 week 2 days 4 hours 2 seconds",
        "@1234567890",
        "first monday of January 2024 noon",
    ];
    let start = Instant::now();
    let n = 20_000;
    let mut acc = 0i64;
    for _ in 0..n {
        for s in &inputs {
            let r = php_timelib::strtotime(s, &php_timelib::builtin_tzinfo);
            acc = acc.wrapping_add(r.time.y);
        }
    }
    let el = start.elapsed();
    println!(
        "{} parses in {:?} ({:.2} us/parse) [{acc}]",
        n * inputs.len(),
        el,
        el.as_secs_f64() * 1e6 / (n * inputs.len()) as f64
    );
}
