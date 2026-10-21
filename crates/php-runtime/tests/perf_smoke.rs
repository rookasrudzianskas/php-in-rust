//! Coarse performance smoke test (ignored by default): guards against
//! accidental quadratic behaviour in the hot data structures.

use std::time::Instant;

use php_runtime::{ArrayKey, PhpArray, PhpStr, Value};

#[test]
#[ignore]
fn array_and_string_hot_paths() {
    let n = 1_000_000i64;
    let t = Instant::now();
    let mut a = PhpArray::new();
    for i in 0..n {
        a.append(Value::Int(i)).unwrap();
    }
    let mut sum = 0i64;
    for (_, v) in a.iter() {
        sum += v.as_int().unwrap();
    }
    assert_eq!(sum, n * (n - 1) / 2);
    let mut h = PhpArray::new();
    for i in 0..n {
        h.insert(ArrayKey::Str(PhpStr::from(format!("k{i}"))), Value::Int(i));
    }
    for i in (0..n).step_by(2) {
        h.remove(&ArrayKey::Str(PhpStr::from(format!("k{i}"))));
    }
    assert_eq!(h.len() as i64, n / 2);
    assert!(h.get_str(b"k1").is_some());
    let mut q = PhpArray::from_values((0..10_000).map(Value::Int).collect());
    while q.shift().is_some() {}
    let mut s = PhpStr::new();
    for _ in 0..n {
        s.push_bytes(b"ab");
    }
    assert_eq!(s.len() as i64, 2 * n);
    eprintln!("elapsed: {:?}", t.elapsed());
    assert!(t.elapsed().as_secs() < 20);
}
