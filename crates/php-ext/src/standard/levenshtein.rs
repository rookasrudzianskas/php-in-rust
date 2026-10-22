//! ext/standard/levenshtein.c

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// Allocates a zeroed row of `len` cells; `None` when the allocation fails
/// (php-src's fallible `safe_femalloc`).
fn alloc_row(len: usize) -> Option<Vec<i64>> {
    let mut row = Vec::new();
    row.try_reserve_exact(len).ok()?;
    row.resize(len, 0);
    Some(row)
}

/// `reference_levdist`: the reference implementation, only optimized for
/// memory usage, not speed. `None` signals out of memory. Costs and distances
/// wrap around like the unsigned arithmetic of the C code.
pub fn reference_levdist(
    string1: &[u8],
    string2: &[u8],
    cost_ins: i64,
    cost_rep: i64,
    cost_del: i64,
) -> Option<i64> {
    if string1.is_empty() {
        return Some((string2.len() as i64).wrapping_mul(cost_ins));
    }
    if string2.is_empty() {
        return Some((string1.len() as i64).wrapping_mul(cost_del));
    }

    // When all costs are equal, levenshtein fulfills the requirements of a
    // metric, which means that the distance is symmetric. If string1 is
    // shorter than string 2 we can save memory (and CPU time) by having
    // shorter rows (p1 & p2).
    let (string1, string2) =
        if string1.len() < string2.len() && cost_ins == cost_rep && cost_rep == cost_del {
            (string2, string1)
        } else {
            (string1, string2)
        };

    let mut p1 = alloc_row(string2.len() + 1)?;
    let mut p2 = alloc_row(string2.len() + 1)?;

    for (i2, cell) in p1.iter_mut().enumerate() {
        *cell = (i2 as i64).wrapping_mul(cost_ins);
    }
    for &ch1 in string1 {
        p2[0] = p1[0].wrapping_add(cost_del);

        for (i2, &ch2) in string2.iter().enumerate() {
            let mut c0 = p1[i2].wrapping_add(if ch1 == ch2 { 0 } else { cost_rep });
            let c1 = p1[i2 + 1].wrapping_add(cost_del);
            if c1 < c0 {
                c0 = c1;
            }
            let c2 = p2[i2].wrapping_add(cost_ins);
            if c2 < c0 {
                c0 = c2;
            }
            p2[i2 + 1] = c0;
        }
        std::mem::swap(&mut p1, &mut p2);
    }
    Some(p1[string2.len()])
}

/// Parses a `memory_limit` value (`128M`, `-1`, ...) into bytes.
fn memory_limit_bytes(vm: &Vm) -> i64 {
    let Some(raw) = vm.ini_get("memory_limit") else {
        return 128 * 1024 * 1024;
    };
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let raw = raw.trim();
    let digits = raw.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let n: i64 = digits.parse().unwrap_or(128);
    match raw.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('g') => n.wrapping_mul(1 << 30),
        Some('m') => n.wrapping_mul(1 << 20),
        Some('k') => n.wrapping_mul(1 << 10),
        _ => n,
    }
}

/// Calculate Levenshtein distance between two strings.
fn levenshtein(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let string1 = args.str(0);
    let string2 = args.str(1);
    let (cost_ins, cost_rep, cost_del) = (args.int(2), args.int(3), args.int(4));

    match reference_levdist(string1, string2, cost_ins, cost_rep, cost_del) {
        Some(distance) => Ok(Value::Int(distance)),
        None => {
            // RETURN_OOM(): php-src reports the exhausted memory manager.
            // TODO(runtime): no memory_limit accounting; this only triggers
            // when the real allocation fails.
            let tried = (string1.len().min(string2.len()) + 1).saturating_mul(8);
            let limit = memory_limit_bytes(vm);
            Err(vm.fatal_error(format!(
                "Allowed memory size of {limit} bytes exhausted (tried to allocate {tried} bytes)"
            )))
        }
    }
}

pub static LEVENSHTEIN: NativeFunction = NativeFunction {
    sig: "levenshtein(string $string1, string $string2, int $insertion_cost = 1, int $replacement_cost = 1, int $deletion_cost = 1): int",
    handler: levenshtein,
};

pub fn register(r: &mut Registry) {
    r.function(&LEVENSHTEIN);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn lev(t: &mut TestVm, a: &[u8], b: &[u8], costs: &[i64]) -> i64 {
        let mut args = vec![Value::bytes(a), Value::bytes(b)];
        args.extend(costs.iter().map(|&c| Value::Int(c)));
        match t.call("levenshtein", args).unwrap() {
            Value::Int(i) => i,
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn levenshtein_phpt() {
        // levenshtein.phpt
        let mut t = TestVm::new();
        assert_eq!(lev(&mut t, b"12345", b"12345", &[]), 0);
        assert_eq!(lev(&mut t, b"", b"xyz", &[]), 3);
        assert_eq!(lev(&mut t, b"xyz", b"", &[]), 3);
        assert_eq!(lev(&mut t, b"", b"", &[]), 0);
        assert_eq!(lev(&mut t, b"", b"", &[10, 10, 10]), 0);
        assert_eq!(lev(&mut t, b"1", b"2", &[]), 1);
        assert_eq!(lev(&mut t, b"12", b"21", &[]), 2);
        assert_eq!(lev(&mut t, b"2121", b"11", &[2]), 2);
        assert_eq!(lev(&mut t, b"2121", b"11", &[2, 1, 5]), 10);
        assert_eq!(lev(&mut t, b"11", b"2121", &[]), 2);
        assert_eq!(lev(&mut t, b"11", b"2121", &[5]), 10);
        assert_eq!(lev(&mut t, b"111", b"121", &[2, 3, 2]), 3);
        assert_eq!(lev(&mut t, b"111", b"121", &[2, 9, 2]), 4);
    }

    #[test]
    fn levenshtein_bugs() {
        let mut t = TestVm::new();
        // levenshtein_bug_16473.phpt
        assert_eq!(lev(&mut t, b"a", b"bc", &[]), 2);
        assert_eq!(lev(&mut t, b"xa", b"xbc", &[]), 2);
        assert_eq!(lev(&mut t, b"xax", b"xbcx", &[]), 2);
        assert_eq!(lev(&mut t, b"ax", b"bcx", &[]), 2);
        // levenshtein_bug_6562.phpt
        assert_eq!(lev(&mut t, b"debugg", b"debug", &[]), 1);
        assert_eq!(lev(&mut t, b"ddebug", b"debug", &[]), 1);
        assert_eq!(lev(&mut t, b"debbbug", b"debug", &[]), 2);
        assert_eq!(lev(&mut t, b"debugging", b"debuging", &[]), 1);
        // levenshtein_bug_7368.phpt
        assert_eq!(lev(&mut t, b"13458", b"12345", &[]), 2);
        assert_eq!(lev(&mut t, b"1345", b"1234", &[]), 2);
    }

    #[test]
    fn levenshtein_no_length_limit() {
        // levenshtein_error_conditions.phpt: strings of 254 and 255+ bytes work.
        let mut t = TestVm::new();
        let base: Vec<u8> = (0..254).map(|i| b'a' + (i % 26) as u8).collect();
        let mut longer = base.clone();
        longer.push(b'x');
        assert_eq!(lev(&mut t, &base, &base, &[]), 0);
        assert_eq!(lev(&mut t, &base, &longer, &[]), 1);
        assert_eq!(lev(&mut t, b"A", &longer, &[]), 255);
        let big = vec![b'b'; 1000];
        assert_eq!(lev(&mut t, b"a", &big, &[]), 1000);
        assert_eq!(lev(&mut t, &big, b"a", &[]), 1000);
    }

    #[test]
    fn levenshtein_asymmetric_costs() {
        // With unequal costs the strings are not swapped.
        let mut t = TestVm::new();
        assert_eq!(lev(&mut t, b"ab", b"abcd", &[3, 1, 1]), 6);
        assert_eq!(lev(&mut t, b"abcd", b"ab", &[3, 1, 1]), 2);
        assert_eq!(lev(&mut t, b"ab", b"", &[3, 1, 7]), 14);
        assert_eq!(lev(&mut t, b"", b"ab", &[3, 1, 7]), 6);
    }

    #[test]
    fn levenshtein_signature() {
        // The removed custom-callback form must not exist.
        let mut t = TestVm::new();
        let r = t.call(
            "levenshtein",
            vec![
                Value::str("a"),
                Value::str("b"),
                Value::str("strlen"),
                Value::str("strlen"),
                Value::str("strlen"),
            ],
        );
        assert_eq!(thrown(r).0, "TypeError");
        let r = t.call("levenshtein", vec![Value::str("a")]);
        assert_eq!(thrown(r).0, "ArgumentCountError");
        let r = t.call(
            "levenshtein",
            vec![
                Value::str("a"),
                Value::str("b"),
                Value::Int(1),
                Value::Int(1),
                Value::Int(1),
                Value::Int(1),
            ],
        );
        assert_eq!(thrown(r).0, "ArgumentCountError");
    }

    #[test]
    fn reference_levdist_wraps() {
        assert_eq!(
            reference_levdist(b"", b"ab", i64::MAX, 1, 1),
            Some(2i64.wrapping_mul(i64::MAX))
        );
    }
}
