//! ext/standard/string.c — `similar_text()`.

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// `php_similar_str`: the first longest common substring of `a` and `b`
/// as `(pos_in_a, pos_in_b, len, count)`, where `count` is the number of
/// times the running maximum improved.
fn similar_str(a: &[u8], b: &[u8]) -> (usize, usize, usize, usize) {
    let (mut pos1, mut pos2, mut max, mut count) = (0, 0, 0, 0);
    for p in 0..a.len() {
        for q in 0..b.len() {
            let l = a[p..]
                .iter()
                .zip(&b[q..])
                .take_while(|(x, y)| x == y)
                .count();
            if l > max {
                max = l;
                count += 1;
                pos1 = p;
                pos2 = q;
            }
        }
    }
    (pos1, pos2, max, count)
}

/// `php_similar_char`: number of matching characters between `a` and `b`
/// (Oliver's algorithm, as implemented by php-src).
pub fn similar_char(a: &[u8], b: &[u8]) -> usize {
    let (pos1, pos2, max, count) = similar_str(a, b);
    let mut sum = max;
    if sum != 0 {
        if pos1 != 0 && pos2 != 0 && count > 1 {
            sum += similar_char(&a[..pos1], &b[..pos2]);
        }
        if pos1 + max < a.len() && pos2 + max < b.len() {
            sum += similar_char(&a[pos1 + max..], &b[pos2 + max..]);
        }
    }
    sum
}

fn similar_text(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let compute_percentage = args.len() >= 3;
    let (t1, t2) = (args.str(0), args.str(1));
    let total = t1.len() + t2.len();
    let sim = if total == 0 { 0 } else { similar_char(t1, t2) };
    if compute_percentage {
        let percent = if total == 0 {
            0.0
        } else {
            sim as f64 * 200.0 / total as f64
        };
        args.by_ref(2).set(Value::Float(percent));
    }
    Ok(Value::Int(sim as i64))
}

pub static SIMILAR_TEXT: NativeFunction = NativeFunction {
    sig: "similar_text(string $string1, string $string2, &$percent = null): int",
    handler: similar_text,
};

pub fn register(r: &mut Registry) {
    r.function(&SIMILAR_TEXT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;
    use php_runtime::PhpRef;

    fn sim(t: &mut TestVm, a: &str, b: &str) -> (i64, f64) {
        let r = PhpRef::new(Value::Int(0));
        let n = t
            .call(
                "similar_text",
                vec![Value::str(a), Value::str(b), Value::Ref(r.clone())],
            )
            .unwrap();
        let Value::Float(p) = r.get() else {
            panic!("percent not a float: {:?}", r.get())
        };
        (n.as_int().unwrap(), p)
    }

    #[test]
    fn basic() {
        let mut t = TestVm::new();
        // similar_text_basic.phpt
        assert_eq!(sim(&mut t, "abcdefgh", "efg"), (3, 54.54545454545455));
        assert_eq!(sim(&mut t, "abcdefgh", "mno"), (0, 0.0));
        assert_eq!(sim(&mut t, "abcdefghcc", "c"), (1, 18.181818181818183));
        assert_eq!(
            sim(&mut t, "abcdefghabcdef", "zzzzabcdefggg"),
            (7, 51.851851851851855)
        );
        assert_eq!(sim(&mut t, "", ""), (0, 0.0));
        assert_eq!(sim(&mut t, "World", "Word"), (4, 88.88888888888889));
        // Argument order matters (php.net example).
        assert_eq!(sim(&mut t, "bafoobar", "barfoo"), (5, 71.42857142857143));
        assert_eq!(sim(&mut t, "barfoo", "bafoobar"), (3, 42.857142857142854));
    }

    #[test]
    fn without_percent() {
        let mut t = TestVm::new();
        let n = t
            .call("similar_text", vec![Value::str("abc"), Value::str("abc")])
            .unwrap();
        assert_eq!(n.as_int(), Some(3));
    }
}
