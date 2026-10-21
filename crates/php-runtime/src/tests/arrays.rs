//! `PhpArray` semantics (`zend_hash.c`).

use std::rc::Rc;

use super::*;
use crate::value::array::{KeyCast, NextElementOccupied, cast_key, make_element_ref};
use crate::value::convert::OffsetKind;
use crate::value::{ArrayKey, PhpArray, PhpRef};

fn keys(a: &PhpArray) -> Vec<String> {
    a.keys().map(|k| format!("{k:?}")).collect()
}

fn ints(a: &PhpArray) -> Vec<i64> {
    a.values().map(|v| v.unref().as_int().unwrap()).collect()
}

#[test]
fn key_normalisation() {
    let cases: &[(&[u8], Option<i64>)] = &[
        (b"8", Some(8)),
        (b"0", Some(0)),
        (b"-1", Some(-1)),
        (b"9223372036854775807", Some(i64::MAX)),
        (b"-9223372036854775808", Some(i64::MIN)),
        (b"9223372036854775808", None),
        (b"-9223372036854775809", None),
        (b"08", None),
        (b"-0", None),
        (b" 8", None),
        (b"8 ", None),
        (b"+8", None),
        (b"1.5", None),
        (b"", None),
        (b"-", None),
        (b"0x1A", None),
        (b"1e3", None),
        (b"00", None),
    ];
    for (s, want) in cases {
        assert_eq!(
            ArrayKey::parse_numeric(s),
            *want,
            "{:?}",
            String::from_utf8_lossy(s)
        );
    }
    assert!(matches!(
        cast_key(&Value::Bool(true)),
        KeyCast::Key(ArrayKey::Int(1))
    ));
    assert!(matches!(
        cast_key(&Value::Float(2.0)),
        KeyCast::Key(ArrayKey::Int(2))
    ));
    assert!(matches!(
        cast_key(&Value::Float(2.5)),
        KeyCast::FloatFractional(2, _)
    ));
    assert!(matches!(
        cast_key(&Value::Float(f64::NAN)),
        KeyCast::FloatNotRepresentable(0, _)
    ));
    assert!(matches!(cast_key(&Value::Null), KeyCast::Null));
    assert!(matches!(cast_key(&Value::empty_array()), KeyCast::Illegal));
}

#[test]
fn offset_conversion_diagnostics() {
    let mut t = TestVm::new();
    let k = t.vm.to_array_key(&Value::Null, OffsetKind::Read).unwrap();
    assert_eq!(k, ArrayKey::Str(PhpStr::new()));
    assert_eq!(
        t.output(),
        "\nDeprecated: Using null as an array offset is deprecated, use an empty string instead in /t.php on line 3\n"
    );
    assert_eq!(
        t.vm.to_array_key(&Value::Float(1.7), OffsetKind::Read)
            .unwrap(),
        ArrayKey::Int(1)
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float 1.7 to int loses precision in /t.php on line 3\n"
    );
    assert_eq!(
        t.vm.to_array_key(&Value::Float(f64::INFINITY), OffsetKind::Read)
            .unwrap(),
        ArrayKey::Int(0)
    );
    assert_eq!(
        t.output(),
        "\nWarning: The float INF is not representable as an int, cast occurred in /t.php on line 3\n"
    );
    assert_eq!(
        t.vm.to_array_key(&s("12"), OffsetKind::Read).unwrap(),
        ArrayKey::Int(12)
    );
    assert_eq!(
        t.vm.to_array_key(&s("012"), OffsetKind::Read).unwrap(),
        ArrayKey::Str("012".into())
    );
    let res = crate::value::ResRef::new("stream", Box::new(()));
    let id = res.id();
    assert_eq!(
        t.vm.to_array_key(&Value::Resource(res), OffsetKind::Read)
            .unwrap(),
        ArrayKey::Int(id)
    );
    assert_eq!(
        t.output(),
        format!(
            "\nWarning: Resource ID#{id} used as offset, casting to integer ({id}) in /t.php on line 3\n"
        )
    );
    let r = t.vm.to_array_key(&Value::empty_array(), OffsetKind::Read);
    assert_eq!(
        thrown(r.map(|_| Value::Null)).1,
        "Cannot access offset of type array on array"
    );
    let r = t.vm.to_array_key(&Value::empty_array(), OffsetKind::IsSet);
    assert_eq!(
        thrown(r.map(|_| Value::Null)).1,
        "Cannot access offset of type array in isset or empty"
    );
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    let r = t.vm.to_array_key(&Value::Object(o), OffsetKind::Unset);
    assert_eq!(
        thrown(r.map(|_| Value::Null)).1,
        "Cannot unset offset of type stdClass on array"
    );
}

#[test]
fn insertion_order_and_overwrite() {
    let mut a = PhpArray::new();
    a.insert("b".into(), 1.into());
    a.insert("a".into(), 2.into());
    a.insert(5.into(), 3.into());
    a.insert("b".into(), 4.into());
    assert_eq!(keys(&a), ["\"b\"", "\"a\"", "5"]);
    assert_eq!(ints(&a), [4, 2, 3]);
    a.remove(&"b".into());
    a.insert("b".into(), 5.into());
    assert_eq!(keys(&a), ["\"a\"", "5", "\"b\""]);
    assert_eq!(a.len(), 3);
    assert!(a.get_sym(b"5").is_some());
    assert!(a.get_str(b"5").is_none());
}

#[test]
fn next_free_element() {
    let mut a = PhpArray::new();
    assert_eq!(a.append(Value::Null), Ok(0));
    assert_eq!(a.append(Value::Null), Ok(1));
    a.insert(10.into(), Value::Null);
    assert_eq!(a.append(Value::Null), Ok(11));
    a.insert((-20).into(), Value::Null);
    assert_eq!(a.append(Value::Null), Ok(12));
    a.remove(&12.into());
    assert_eq!(a.append(Value::Null), Ok(13));

    // PHP >= 8.3: an array starting with a negative key continues from it.
    let mut n = PhpArray::new();
    n.insert((-5).into(), Value::Null);
    assert_eq!(n.append(Value::Null), Ok(-4));

    let mut s = PhpArray::new();
    s.insert("x".into(), Value::Null);
    assert_eq!(s.append(Value::Null), Ok(0));

    let mut m = PhpArray::new();
    m.insert(i64::MAX.into(), Value::Null);
    assert_eq!(m.append(Value::Null), Err(NextElementOccupied));
    assert_eq!(
        NextElementOccupied.to_string(),
        "Cannot add element to the array as the next element is already occupied"
    );
}

#[test]
fn packed_and_hash_modes() {
    let mut a = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    assert!(a.is_packed());
    assert!(a.is_list());
    a.remove(&1.into());
    assert!(a.is_packed());
    assert!(!a.is_list());
    assert_eq!(keys(&a), ["0", "2"]);
    a.insert(1.into(), 9.into());
    assert!(!a.is_packed());
    assert_eq!(keys(&a), ["0", "2", "1"]);
    assert_eq!(a.append(4), Ok(3));

    let mut b = PhpArray::new();
    b.insert(3.into(), 1.into());
    assert_eq!(keys(&b), ["3"]);
    assert!(!b.is_list());
    let mut c = PhpArray::from_values(vec![1.into()]);
    c.insert("k".into(), 2.into());
    assert!(!c.is_packed());
    assert_eq!(keys(&c), ["0", "\"k\""]);
    assert!(PhpArray::new().is_list());
}

#[test]
fn positions_survive_deletion_and_compaction() {
    let mut a = PhpArray::new();
    for i in 0..100 {
        a.insert(ArrayKey::Str(PhpStr::from(format!("k{i}"))), Value::Int(i));
    }
    let p50 = a.pos_of(&"k50".into()).unwrap();
    for i in 0..100 {
        if i != 50 && i != 99 {
            a.remove(&ArrayKey::Str(PhpStr::from(format!("k{i}"))));
        }
    }
    assert_eq!(a.len(), 2);
    assert_eq!(a.entry_at(p50).unwrap().1.as_int(), Some(50));
    let next = a.valid_pos(a.pos_after(p50)).unwrap();
    assert_eq!(a.entry_at(next).unwrap().1.as_int(), Some(99));
    // A position of a deleted element resolves to the next live one.
    let p_deleted = p50 - 10;
    assert_eq!(a.valid_pos(p_deleted), Some(p50));
}

#[test]
fn foreach_by_reference_sees_appends() {
    // foreach ($a as &$v) { if ($v == 2) $a[] = 99; }
    let mut a = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    let mut seen = Vec::new();
    let mut pos = a.first_pos().unwrap();
    while let Some(p) = a.valid_pos(pos) {
        let v = a.entry_at(p).unwrap().1.as_int().unwrap();
        seen.push(v);
        pos = a.pos_after(p);
        if v == 2 {
            a.append(99).unwrap();
        }
        if v == 3 {
            a.insert("s".into(), 100.into());
        }
    }
    assert_eq!(seen, [1, 2, 3, 99, 100]);

    // Deleting the element after the current one skips it.
    let mut b = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    let mut seen = Vec::new();
    let mut pos = b.first_pos().unwrap();
    while let Some(p) = b.valid_pos(pos) {
        let v = b.entry_at(p).unwrap().1.as_int().unwrap();
        seen.push(v);
        pos = b.pos_after(p);
        if v == 1 {
            b.remove(&1.into());
        }
    }
    assert_eq!(seen, [1, 3]);
}

#[test]
fn positions_follow_cow_copies() {
    // php-src: Zend/tests/array_dup_iterator_past_end.phpt
    let mut values = Rc::new(PhpArray::from_pairs([
        ("a", 10),
        ("b", 11),
        ("c", 12),
        ("d", 13),
    ]));
    Rc::make_mut(&mut values).remove(&"a".into());
    Rc::make_mut(&mut values).remove(&"b".into());
    let mut seen = Vec::new();
    let mut copy = None;
    let mut pos = values.first_pos().unwrap();
    while let Some(p) = values.valid_pos(pos) {
        let (k, v) = values.entry_at(p).map(|(k, v)| (k, v.clone())).unwrap();
        seen.push(format!("{k}=>{}", v.as_int().unwrap()));
        pos = values.pos_after(p);
        if k == ArrayKey::from("d") {
            copy = Some(values.clone());
            Rc::make_mut(&mut values).insert("e".into(), 14.into());
        }
    }
    assert_eq!(seen, ["c=>12", "d=>13", "e=>14"]);
    assert_eq!(keys(copy.as_ref().unwrap()), ["\"c\"", "\"d\""]);
}

#[test]
fn internal_pointer() {
    let mut a = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    assert_eq!(a.current().unwrap().as_int(), Some(1));
    assert!(a.move_forward());
    assert_eq!(a.current().unwrap().as_int(), Some(2));
    assert_eq!(a.current_key(), Some(ArrayKey::Int(1)));
    assert!(a.move_forward());
    assert!(a.move_forward());
    assert!(a.current().is_none());
    assert!(a.current_key().is_none());
    assert!(!a.move_forward());
    assert!(!a.move_backward());
    a.append(4).unwrap();
    assert_eq!(a.current().unwrap().as_int(), Some(4));
    a.reset();
    assert_eq!(a.current().unwrap().as_int(), Some(1));
    assert!(a.move_backward());
    assert!(a.current().is_none());
    a.end();
    assert_eq!(a.current().unwrap().as_int(), Some(4));
    assert!(a.move_backward());
    assert_eq!(a.current().unwrap().as_int(), Some(3));
    // Deleting the current element moves current() to the next one.
    a.remove(&2.into());
    assert_eq!(a.current().unwrap().as_int(), Some(4));

    // A past-the-end pointer is reset by duplication (zend_array_dup).
    let mut b = PhpArray::from_values(vec![1.into(), 2.into()]);
    b.end();
    b.move_forward();
    assert!(b.current().is_none());
    let c = b.clone();
    assert_eq!(c.current().unwrap().as_int(), Some(1));
    // A pointer on a valid element is kept.
    let mut d = PhpArray::from_values(vec![1.into(), 2.into()]);
    d.move_forward();
    assert_eq!(d.clone().current().unwrap().as_int(), Some(2));
    let mut e = PhpArray::new();
    assert!(!e.move_forward());
    assert!(e.current().is_none());
    e.reset();
    e.append(1).unwrap();
    assert_eq!(e.current().unwrap().as_int(), Some(1));
}

#[test]
fn cow_with_references() {
    // $a = [1, 2]; $r = &$a[0]; $b = $a; → $b[0] stays a reference (refcount 2)
    let mut a = PhpArray::from_values(vec![1.into(), 2.into()]);
    let r = make_element_ref(&mut a, 0.into());
    let b = a.clone();
    assert!(b.get_int(0).unwrap().is_ref());
    r.set(Value::Int(10));
    assert_eq!(b.get_int(0).unwrap().unref().as_int(), Some(10));
    // Once the outside reference is gone, the refcount-1 reference is copied as a value.
    drop(r);
    drop(b);
    let c = a.clone();
    assert!(!c.get_int(0).unwrap().is_ref());
    assert_eq!(c.get_int(0).unwrap().as_int(), Some(10));

    // A reference to the array itself is preserved.
    let holder = PhpRef::new(Value::Null);
    let mut arr = Rc::new(PhpArray::new());
    Rc::make_mut(&mut arr).insert(0.into(), Value::Ref(holder.clone()));
    holder.set(Value::Array(arr.clone()));
    drop(holder);

    // Rc::make_mut separates only when shared.
    let mut x = Rc::new(PhpArray::from_values(vec![1.into()]));
    let y = x.clone();
    Rc::make_mut(&mut x).append(2).unwrap();
    assert_eq!(x.len(), 2);
    assert_eq!(y.len(), 1);
}

#[test]
fn pop_shift_unshift() {
    let mut a = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    assert_eq!(a.pop().unwrap().as_int(), Some(3));
    assert_eq!(a.append(9), Ok(2));
    assert_eq!(ints(&a), [1, 2, 9]);

    let mut h = PhpArray::from_pairs([(5, 1), (9, 2)]);
    h.pop();
    assert_eq!(h.append(0), Ok(9));

    let mut s = PhpArray::new();
    s.insert(3.into(), 'a'.to_string().into());
    s.insert("x".into(), "b".into());
    s.insert(7.into(), "c".into());
    s.insert(1.into(), "d".into());
    assert_eq!(s.shift().unwrap().as_str().unwrap(), "a");
    assert_eq!(keys(&s), ["\"x\"", "0", "1"]);
    assert_eq!(s.append(Value::Null), Ok(2));

    let mut q = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    q.shift();
    assert_eq!(keys(&q), ["0", "1"]);
    assert_eq!(ints(&q), [2, 3]);
    assert!(q.is_packed());
    q.unshift(vec![7.into(), 8.into()]);
    assert_eq!(ints(&q), [7, 8, 2, 3]);
    assert_eq!(keys(&q), ["0", "1", "2", "3"]);
    assert_eq!(q.current().unwrap().as_int(), Some(7));

    let mut u = PhpArray::from_pairs([("k", 1), ("5", 2)]);
    u.unshift(vec![0.into()]);
    assert_eq!(keys(&u), ["0", "\"k\"", "1"]);
    assert_eq!(u.append(Value::Null), Ok(2));

    // Iterator positions survive shift/unshift of other elements.
    let mut w = PhpArray::from_values(vec![1.into(), 2.into(), 3.into()]);
    let p2 = w.pos_of(&1.into()).unwrap();
    w.shift();
    assert_eq!(w.entry_at(p2).unwrap().1.as_int(), Some(2));
    w.unshift(vec![0.into()]);
    assert_eq!(w.entry_at(p2).unwrap().1.as_int(), Some(2));
}

#[test]
fn stable_sort() {
    let mut a = PhpArray::from_pairs([("a", 3), ("b", 1), ("c", 3), ("d", 2)]);
    a.sort_by(false, |x, y| Ok::<_, ()>(x.1.as_int().cmp(&y.1.as_int())))
        .unwrap();
    assert_eq!(keys(&a), ["\"b\"", "\"d\"", "\"a\"", "\"c\""]);
    a.sort_by(true, |x, y| Ok::<_, ()>(y.1.as_int().cmp(&x.1.as_int())))
        .unwrap();
    assert_eq!(keys(&a), ["0", "1", "2", "3"]);
    assert_eq!(ints(&a), [3, 3, 2, 1]);
    assert!(a.is_packed());
    assert_eq!(a.append(0), Ok(4));
    let r = a.sort_by(true, |_, _| Err::<std::cmp::Ordering, _>("boom"));
    assert_eq!(r, Err("boom"));
    assert_eq!(a.len(), 5);
}

#[test]
fn array_union_operator() {
    let mut t = TestVm::new();
    let a = Value::array(PhpArray::from_values(vec![1.into(), 2.into()]));
    let b = Value::array(PhpArray::from_values(vec![3.into(), 4.into(), 5.into()]));
    let r = t.vm.add(&a, &b).unwrap();
    assert_eq!(ints(r.as_array().unwrap()), [1, 2, 5]);
    let c = Value::array(PhpArray::from_pairs([("x", 1)]));
    let r = t.vm.add(&c, &a).unwrap();
    assert_eq!(keys(r.as_array().unwrap()), ["\"x\"", "0", "1"]);
}

#[test]
fn value_size_and_debug() {
    assert_eq!(std::mem::size_of::<Value>(), 16);
    let a = PhpArray::from_pairs([("a", 1)]);
    assert_eq!(format!("{a:?}"), "{\"a\": Int(1)}");
}

#[test]
fn iteration_from_position() {
    let a = PhpArray::from_pairs([("a", 1), ("b", 2), ("c", 3)]);
    let pb = a.pos_of(&"b".into()).unwrap();
    let got: Vec<i64> = a
        .iter_from(pb)
        .map(|(_, _, v)| v.as_int().unwrap())
        .collect();
    assert_eq!(got, [2, 3]);
    assert!(a.contains(&"c".into()));
    assert_eq!(a.iter_from(a.end_pos()).count(), 0);
}
