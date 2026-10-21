//! Panic smoke test: feeds pseudo-random inputs through all parsers and calculations.
//! `cargo run -p php-timelib --example fuzz_smoke -- [iterations]`
use php_timelib::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() % v.len() as u64) as usize]
    }
}

const TOKENS: &[&str] = &[
    "2005",
    "-",
    "+",
    "07",
    "14",
    " ",
    "T",
    "22:30:41",
    ".5",
    "UTC",
    "EDT",
    "Europe/Amsterdam",
    "next",
    "last",
    "monday",
    "week",
    "day",
    "of",
    "first",
    "ago",
    "@",
    "1234567890",
    "99999999999999999999999",
    "noon",
    "tomorrow",
    "am",
    "pm",
    "/",
    ",",
    ":",
    "W",
    "P",
    "1Y",
    "R5",
    "Z",
    "00",
    "9",
    "jan",
    "XII",
    "(",
    ")",
    "GMT",
    "+0200",
    "\u{a0}",
    "\u{202f}",
    "weekday",
    "µs",
    "sec",
    "back of",
    "front of",
    "\0",
    "\n",
    "x",
    "Sept",
    "31",
    "60",
    "24",
    "-9223372036854775808",
    "9223372036854775807",
    "fortnight",
    "this",
];
const FORMATS: &[&str] = &[
    "Y-m-d",
    "!d/m/Y",
    "U",
    "Y-m-d H:i:s.u e",
    "D, d M Y H:i:s O",
    "z Y",
    "Y z",
    "|",
    "+",
    "X-m-d",
    "G:i A",
    "\\Y",
    "*",
    "#",
    "?",
    "v",
    "T",
    "p",
    "P",
];

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20000);
    let seed: u64 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
    let tz = builtin_tzinfo("America/New_York").unwrap();
    for _ in 0..n {
        let k = (rng.next() % 10) as usize + 1;
        let s: Vec<u8> = if rng.next().is_multiple_of(4) {
            (0..k * 3).map(|_| (rng.next() % 256) as u8).collect()
        } else {
            (0..k)
                .map(|_| rng.pick(TOKENS))
                .collect::<String>()
                .into_bytes()
        };
        let r = strtotime(&s, &builtin_tzinfo);
        let mut now = Time {
            tz_info: Some(tz.clone()),
            zone_type: ZoneType::Id,
            ..Time::new()
        };
        now.unixtime2local(1_121_380_241);
        let mut t = r.time.clone();
        t.fill_holes(&now, NO_CLONE);
        t.update_ts(Some(&tz));
        t.update_from_sse();
        let _ = date_format(b"Y-m-d H:i:s.u T e P c r U B W o N z t L", &t, true);
        let d = diff(&t, &now);
        let _ = add(&now, &d);
        let _ = sub_wall(&now, &d);
        let _ = add_wall(&t, &d);
        let f = rng.pick(FORMATS);
        let _ = parse_from_format(f, &s, &builtin_tzinfo);
        let _ = strtointerval(&s);
        let _ = php::date_modify(&mut now.clone(), &s);
    }
    println!("ok {n}");
}
