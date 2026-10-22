use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::resolver::{HErrno, Resolver, dn_expand, encode_name};
use super::*;
use crate::testing::{TestVm, thrown};
use php_runtime::PhpRef;

/// One resource record of a canned answer.
struct Rr {
    name: &'static str,
    ty: u16,
    ttl: u32,
    rdata: Vec<u8>,
}

#[derive(Default)]
struct Reply {
    rcode: u8,
    answers: Vec<Rr>,
    authority: Vec<Rr>,
    additional: Vec<Rr>,
}

fn name_wire(n: &str) -> Vec<u8> {
    encode_name(n).unwrap()
}

fn rr(name: &'static str, ty: u16, rdata: Vec<u8>) -> Rr {
    Rr {
        name,
        ty,
        ttl: 300,
        rdata,
    }
}

fn pack_rr(out: &mut Vec<u8>, r: &Rr) {
    out.extend_from_slice(&name_wire(r.name));
    out.extend_from_slice(&r.ty.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&r.ttl.to_be_bytes());
    out.extend_from_slice(&(r.rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(&r.rdata);
}

fn build_response(query: &[u8], reply: &Reply) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&query[..2]);
    out.push(0x81);
    out.push(0x80 | reply.rcode);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(reply.answers.len() as u16).to_be_bytes());
    out.extend_from_slice(&(reply.authority.len() as u16).to_be_bytes());
    out.extend_from_slice(&(reply.additional.len() as u16).to_be_bytes());
    // the question, copied
    out.extend_from_slice(&query[12..]);
    for r in reply
        .answers
        .iter()
        .chain(&reply.authority)
        .chain(&reply.additional)
    {
        pack_rr(&mut out, r);
    }
    out
}

/// A UDP name server on localhost answering from a closure.
struct Mock {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Mock {
    fn start(handler: impl Fn(&str, u16) -> Reply + Send + 'static) -> Mock {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let addr = sock.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            while !flag.load(Ordering::Relaxed) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else {
                    continue;
                };
                let query = &buf[..n];
                let Some((name, next)) = dn_expand(query, 12) else {
                    continue;
                };
                let qtype = u16::from_be_bytes([query[next], query[next + 1]]);
                let reply = handler(&String::from_utf8_lossy(&name), qtype);
                let _ = sock.send_to(&build_response(query, &reply), from);
            }
        });
        Mock {
            addr,
            stop,
            handle: Some(handle),
        }
    }

    fn resolver(&self) -> Resolver {
        Resolver {
            servers: vec![self.addr],
            search: Vec::new(),
            ndots: 1,
            timeout: Duration::from_secs(2),
            attempts: 1,
        }
    }

    fn vm(&self) -> TestVm {
        let mut t = TestVm::new();
        set_resolver(&mut t.vm, self.resolver());
        t
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn records(v: Value) -> Vec<PhpArray> {
    let Value::Array(a) = v else {
        panic!("array expected, got {v:?}")
    };
    a.values()
        .map(|r| match r.unref() {
            Value::Array(r) => (*r).clone(),
            other => panic!("record expected, got {other:?}"),
        })
        .collect()
}

fn text(a: &PhpArray, k: &str) -> String {
    a.get_str(k.as_bytes())
        .unwrap_or_else(|| panic!("missing {k}"))
        .as_str()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn int(a: &PhpArray, k: &str) -> i64 {
    a.get_str(k.as_bytes()).unwrap().as_int().unwrap()
}

fn keys(a: &PhpArray) -> Vec<String> {
    a.keys().map(|k| k.to_string()).collect()
}

#[test]
fn constants_match_php_dns_h() {
    let t = TestVm::new();
    for (n, v) in [
        ("DNS_A", 1),
        ("DNS_NS", 2),
        ("DNS_CNAME", 16),
        ("DNS_SOA", 32),
        ("DNS_PTR", 2048),
        ("DNS_HINFO", 4096),
        ("DNS_CAA", 8192),
        ("DNS_MX", 16384),
        ("DNS_TXT", 32768),
        ("DNS_A6", 16777216),
        ("DNS_SRV", 33554432),
        ("DNS_NAPTR", 67108864),
        ("DNS_AAAA", 134217728),
        ("DNS_ANY", 268435456),
        ("DNS_ALL", 251_721_779),
    ] {
        assert_eq!(t.vm.constant(n).unwrap().as_int(), Some(v), "{n}");
    }
}

#[test]
fn a_and_aaaa_records() {
    let mock = Mock::start(|name, qtype| {
        assert_eq!(name, "host.test");
        match qtype {
            1 => Reply {
                answers: vec![rr("host.test", 1, vec![192, 0, 2, 7])],
                ..Reply::default()
            },
            28 => Reply {
                answers: vec![
                    rr(
                        "host.test",
                        28,
                        vec![0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                    ),
                    rr("host.test", 28, vec![0; 16]),
                    rr(
                        "host.test",
                        28,
                        vec![0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 3, 0, 0, 0, 4],
                    ),
                ],
                ..Reply::default()
            },
            _ => Reply {
                rcode: 3,
                ..Reply::default()
            },
        }
    });
    let mut t = mock.vm();
    let recs = records(
        t.call(
            "dns_get_record",
            vec![Value::str("host.test"), Value::Int(DNS_A)],
        )
        .unwrap(),
    );
    assert_eq!(recs.len(), 1);
    assert_eq!(keys(&recs[0]), ["host", "class", "ttl", "type", "ip"]);
    assert_eq!(text(&recs[0], "host"), "host.test");
    assert_eq!(text(&recs[0], "class"), "IN");
    assert_eq!(int(&recs[0], "ttl"), 300);
    assert_eq!(text(&recs[0], "type"), "A");
    assert_eq!(text(&recs[0], "ip"), "192.0.2.7");

    let recs = records(
        t.call(
            "dns_get_record",
            vec![Value::str("host.test"), Value::Int(DNS_AAAA)],
        )
        .unwrap(),
    );
    assert_eq!(recs.len(), 3);
    assert_eq!(text(&recs[0], "ipv6"), "2001:db8::1");
    assert_eq!(text(&recs[1], "ipv6"), "::");
    // later zero runs print as ":0", unlike inet_ntop
    assert_eq!(text(&recs[2], "ipv6"), "1::2:0:0:3:0:4");
}

#[test]
fn name_server_mail_exchange_and_cname_records() {
    let mock = Mock::start(|_, qtype| {
        let target = |n: &str| name_wire(n);
        match qtype {
            15 => {
                let mut mx = 10u16.to_be_bytes().to_vec();
                mx.extend(target("mail.host.test"));
                let mut mx2 = 20u16.to_be_bytes().to_vec();
                mx2.extend(target("mail2.host.test"));
                Reply {
                    answers: vec![rr("host.test", 15, mx), rr("host.test", 15, mx2)],
                    ..Reply::default()
                }
            }
            2 => Reply {
                answers: vec![rr("host.test", 2, target("ns1.host.test"))],
                ..Reply::default()
            },
            5 => Reply {
                answers: vec![rr("www.host.test", 5, target("host.test"))],
                ..Reply::default()
            },
            12 => Reply {
                answers: vec![rr("host.test", 12, target("ptr.host.test"))],
                ..Reply::default()
            },
            _ => Reply {
                rcode: 3,
                ..Reply::default()
            },
        }
    });
    let mut t = mock.vm();
    let recs = records(
        t.call(
            "dns_get_record",
            vec![Value::str("host.test"), Value::Int(DNS_MX)],
        )
        .unwrap(),
    );
    assert_eq!(recs.len(), 2);
    assert_eq!(
        keys(&recs[0]),
        ["host", "class", "ttl", "type", "pri", "target"]
    );
    assert_eq!(int(&recs[0], "pri"), 10);
    assert_eq!(text(&recs[0], "target"), "mail.host.test");
    assert_eq!(int(&recs[1], "pri"), 20);

    // a mask queries every requested type
    let recs = records(
        t.call(
            "dns_get_record",
            vec![
                Value::str("host.test"),
                Value::Int(DNS_NS | DNS_CNAME | DNS_PTR),
            ],
        )
        .unwrap(),
    );
    let types: Vec<String> = recs.iter().map(|r| text(r, "type")).collect();
    assert_eq!(types, ["NS", "CNAME", "PTR"]);
    assert_eq!(text(&recs[0], "target"), "ns1.host.test");

    // dns_get_mx / getmxrr
    let hosts = PhpRef::new(Value::Null);
    let weights = PhpRef::new(Value::Null);
    let r = t
        .call(
            "getmxrr",
            vec![
                Value::str("host.test"),
                Value::Ref(hosts.clone()),
                Value::Ref(weights.clone()),
            ],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(true)));
    let h = hosts.get();
    let w = weights.get();
    let (Value::Array(h), Value::Array(w)) = (h, w) else {
        panic!("arrays expected")
    };
    assert_eq!(h.get_int(0).unwrap().as_str().unwrap(), "mail.host.test");
    assert_eq!(h.get_int(1).unwrap().as_str().unwrap(), "mail2.host.test");
    assert_eq!(w.get_int(1).unwrap().as_int(), Some(20));
}

#[test]
fn txt_soa_srv_hinfo_caa_naptr() {
    let mock = Mock::start(|_, qtype| match qtype {
        16 => {
            let mut d = vec![5];
            d.extend_from_slice(b"hello");
            d.push(6);
            d.extend_from_slice(b" world");
            Reply {
                answers: vec![rr("t.test", 16, d)],
                ..Reply::default()
            }
        }
        6 => {
            let mut d = name_wire("ns.t.test");
            d.extend(name_wire("hostmaster.t.test"));
            for v in [2024u32, 3600, 600, 86400, 60] {
                d.extend_from_slice(&v.to_be_bytes());
            }
            Reply {
                answers: vec![rr("t.test", 6, d)],
                ..Reply::default()
            }
        }
        33 => {
            let mut d = Vec::new();
            for v in [5u16, 10, 443] {
                d.extend_from_slice(&v.to_be_bytes());
            }
            d.extend(name_wire("sip.t.test"));
            Reply {
                answers: vec![rr("_sip._tcp.t.test", 33, d)],
                ..Reply::default()
            }
        }
        13 => {
            let mut d = vec![3];
            d.extend_from_slice(b"cpu");
            d.push(2);
            d.extend_from_slice(b"os");
            Reply {
                answers: vec![rr("t.test", 13, d)],
                ..Reply::default()
            }
        }
        257 => {
            let mut d = vec![128, 5];
            d.extend_from_slice(b"issue");
            d.extend_from_slice(b"ca.example");
            Reply {
                answers: vec![rr("t.test", 257, d)],
                ..Reply::default()
            }
        }
        35 => {
            let mut d = Vec::new();
            d.extend_from_slice(&100u16.to_be_bytes());
            d.extend_from_slice(&50u16.to_be_bytes());
            for part in ["S", "SIP+D2U", "!^.*$!sip:x@t.test!"] {
                d.push(part.len() as u8);
                d.extend_from_slice(part.as_bytes());
            }
            d.extend(name_wire("_sip._udp.t.test"));
            Reply {
                answers: vec![rr("t.test", 35, d)],
                ..Reply::default()
            }
        }
        _ => Reply {
            rcode: 3,
            ..Reply::default()
        },
    });
    let mut t = mock.vm();
    let get = |t: &mut TestVm, ty: i64, host: &str| {
        records(
            t.call("dns_get_record", vec![Value::str(host), Value::Int(ty)])
                .unwrap(),
        )
    };

    let txt = &get(&mut t, DNS_TXT, "t.test")[0];
    assert_eq!(
        keys(txt),
        ["host", "class", "ttl", "type", "txt", "entries"]
    );
    assert_eq!(text(txt, "txt"), "hello world");
    let Some(Value::Array(entries)) = txt.get_str(b"entries") else {
        panic!()
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries.get_int(1).unwrap().as_str().unwrap(), " world");

    let soa = &get(&mut t, DNS_SOA, "t.test")[0];
    assert_eq!(
        keys(soa)[3..],
        [
            "type",
            "mname",
            "rname",
            "serial",
            "refresh",
            "retry",
            "expire",
            "minimum-ttl"
        ]
    );
    assert_eq!(text(soa, "mname"), "ns.t.test");
    assert_eq!(text(soa, "rname"), "hostmaster.t.test");
    assert_eq!(int(soa, "serial"), 2024);
    assert_eq!(int(soa, "minimum-ttl"), 60);

    let srv = &get(&mut t, DNS_SRV, "_sip._tcp.t.test")[0];
    assert_eq!(keys(srv)[3..], ["type", "pri", "weight", "port", "target"]);
    assert_eq!(int(srv, "port"), 443);
    assert_eq!(text(srv, "target"), "sip.t.test");

    let hinfo = &get(&mut t, DNS_HINFO, "t.test")[0];
    assert_eq!(text(hinfo, "cpu"), "cpu");
    assert_eq!(text(hinfo, "os"), "os");

    let caa = &get(&mut t, DNS_CAA, "t.test")[0];
    assert_eq!(keys(caa)[3..], ["type", "flags", "tag", "value"]);
    assert_eq!(int(caa, "flags"), 128);
    assert_eq!(text(caa, "tag"), "issue");
    assert_eq!(text(caa, "value"), "ca.example");

    let naptr = &get(&mut t, DNS_NAPTR, "t.test")[0];
    assert_eq!(
        keys(naptr)[3..],
        [
            "type",
            "order",
            "pref",
            "flags",
            "services",
            "regex",
            "replacement"
        ]
    );
    assert_eq!(text(naptr, "services"), "SIP+D2U");
    assert_eq!(text(naptr, "replacement"), "_sip._udp.t.test");
}

#[test]
fn raw_mode_returns_rdata() {
    let mock = Mock::start(|_, qtype| Reply {
        answers: vec![rr("r.test", qtype, vec![1, 2, 3, 4])],
        ..Reply::default()
    });
    let mut t = mock.vm();
    let r = t
        .call(
            "dns_get_record",
            vec![
                Value::str("r.test"),
                Value::Int(1),
                Value::Null,
                Value::Null,
                Value::Bool(true),
            ],
        )
        .unwrap();
    let recs = records(r);
    assert_eq!(recs.len(), 1);
    assert_eq!(keys(&recs[0]), ["host", "class", "ttl", "type", "data"]);
    assert_eq!(int(&recs[0], "type"), 1);
    assert_eq!(
        recs[0]
            .get_str(b"data")
            .unwrap()
            .as_str()
            .unwrap()
            .as_bytes(),
        [1, 2, 3, 4]
    );

    let (c, m) = thrown(t.call(
        "dns_get_record",
        vec![
            Value::str("r.test"),
            Value::Int(0),
            Value::Null,
            Value::Null,
            Value::Bool(true),
        ],
    ));
    assert_eq!(c, "ValueError");
    assert_eq!(
        m,
        "dns_get_record(): Argument #2 ($type) must be between 1 and 65535 when argument #5 ($raw) is true"
    );
}

#[test]
fn authoritative_and_additional_records() {
    let mock = Mock::start(|_, qtype| {
        if qtype == 255 {
            Reply {
                answers: vec![rr("a.test", 1, vec![10, 0, 0, 1])],
                authority: vec![rr("a.test", 2, name_wire("ns.a.test"))],
                additional: vec![rr("ns.a.test", 1, vec![10, 0, 0, 53])],
                ..Reply::default()
            }
        } else {
            Reply {
                rcode: 3,
                ..Reply::default()
            }
        }
    });
    let mut t = mock.vm();
    let (authns, addtl) = (PhpRef::new(Value::Null), PhpRef::new(Value::Null));
    let r = t
        .call(
            "dns_get_record",
            vec![
                Value::str("a.test"),
                Value::Int(DNS_ANY),
                Value::Ref(authns.clone()),
                Value::Ref(addtl.clone()),
            ],
        )
        .unwrap();
    let recs = records(r);
    assert_eq!(recs.len(), 1);
    assert_eq!(text(&recs[0], "ip"), "10.0.0.1");
    let ns = records(authns.get());
    assert_eq!(ns.len(), 1);
    assert_eq!(text(&ns[0], "type"), "NS");
    assert_eq!(text(&ns[0], "target"), "ns.a.test");
    let ar = records(addtl.get());
    assert_eq!(text(&ar[0], "ip"), "10.0.0.53");
}

#[test]
fn check_dns_record() {
    let mock = Mock::start(|name, qtype| match (name, qtype) {
        ("mx.test", 15) => {
            let mut mx = 10u16.to_be_bytes().to_vec();
            mx.extend(name_wire("mail.mx.test"));
            Reply {
                answers: vec![rr("mx.test", 15, mx)],
                ..Reply::default()
            }
        }
        ("a.test", 1) => Reply {
            answers: vec![rr("a.test", 1, vec![1, 1, 1, 1])],
            ..Reply::default()
        },
        _ => Reply {
            rcode: 3,
            ..Reply::default()
        },
    });
    let mut t = mock.vm();
    let b = |t: &mut TestVm, args: Vec<Value>| {
        matches!(t.call("checkdnsrr", args).unwrap(), Value::Bool(true))
    };
    assert!(b(&mut t, vec![Value::str("mx.test")]));
    assert!(b(&mut t, vec![Value::str("a.test"), Value::str("a")]));
    assert!(!b(&mut t, vec![Value::str("a.test")]));
    assert!(!b(
        &mut t,
        vec![Value::str("nothing.test"), Value::str("A")]
    ));
    assert!(matches!(
        t.call(
            "dns_check_record",
            vec![Value::str("a.test"), Value::str("A")]
        )
        .unwrap(),
        Value::Bool(true)
    ));
    let (c, m) = thrown(t.call(
        "checkdnsrr",
        vec![Value::str("a.test"), Value::str("BOGUS")],
    ));
    assert_eq!(c, "ValueError");
    assert_eq!(
        m,
        "checkdnsrr(): Argument #2 ($type) must be a valid DNS record type"
    );
    let (_, m) = thrown(t.call("checkdnsrr", vec![Value::str("")]));
    assert_eq!(m, "checkdnsrr(): Argument #1 ($hostname) cannot be empty");
}

#[test]
fn invalid_type_masks_and_failures() {
    let mock = Mock::start(|_, _| Reply {
        rcode: 2,
        ..Reply::default()
    });
    let mut t = mock.vm();
    let (c, m) = thrown(t.call("dns_get_record", vec![Value::str("x.test"), Value::Int(4)]));
    assert_eq!(c, "ValueError");
    assert_eq!(
        m,
        "dns_get_record(): Argument #2 ($type) must be a DNS_* constant"
    );
    // SERVFAIL: a temporary error
    let r = t
        .call(
            "dns_get_record",
            vec![Value::str("x.test"), Value::Int(DNS_A)],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: dns_get_record(): A temporary server error occurred. in /t.php on line 3\n"
    );
    // by-ref arrays are initialised even on failure
    let hosts = PhpRef::new(Value::Null);
    let r = t
        .call(
            "dns_get_mx",
            vec![Value::str("x.test"), Value::Ref(hosts.clone())],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(hosts.get().as_array().unwrap().len(), 0);
}

#[test]
fn resolver_config_and_search_list() {
    let r = Resolver::from_resolv_conf(
        "# comment\nnameserver 10.1.1.1\nnameserver ::1\nsearch a.example b.example\noptions ndots:2 timeout:3 attempts:4\n",
        "box.local.example",
    );
    assert_eq!(r.servers.len(), 2);
    assert_eq!(r.servers[0].to_string(), "10.1.1.1:53");
    assert_eq!(r.search, ["a.example", "b.example"]);
    assert_eq!((r.ndots, r.timeout.as_secs(), r.attempts), (2, 3, 4));
    // no search directive: the host's domain
    let r = Resolver::from_resolv_conf("nameserver 1.2.3.4\n", "box.local.example");
    assert_eq!(r.search, ["local.example"]);
    // no nameserver: localhost
    let r = Resolver::from_resolv_conf("", "box");
    assert_eq!(r.servers[0].to_string(), "127.0.0.1:53");
    assert!(r.search.is_empty());

    // the search list is tried when the name has too few dots
    let mock = Mock::start(|name, qtype| {
        if name == "www.example.test" && qtype == 1 {
            Reply {
                answers: vec![rr("www.example.test", 1, vec![9, 9, 9, 9])],
                ..Reply::default()
            }
        } else {
            Reply {
                rcode: 3,
                ..Reply::default()
            }
        }
    });
    let mut res = mock.resolver();
    res.search = vec!["other.test".into(), "example.test".into()];
    let answer = res.search("www", 1).unwrap();
    assert_eq!(answer[6..8], [0, 1]);
    // a trailing dot means absolute
    assert_eq!(res.search("www.", 1), Err(HErrno::HostNotFound));
}

#[test]
fn wire_helpers() {
    assert_eq!(name_wire("a.bc"), [1, b'a', 2, b'b', b'c', 0]);
    assert_eq!(name_wire(""), [0]);
    assert!(encode_name(&"x".repeat(64)).is_none());
    // compression pointers
    let msg = [3, b'w', b'w', b'w', 0xc0, 7, 0, 3, b'c', b'o', b'm', 0];
    let (n, next) = dn_expand(&msg, 0).unwrap();
    assert_eq!((n.as_slice(), next), (&b"www.com"[..], 6));
    assert_eq!(super::resolver::dn_skipname(&msg, 0), Some(6));
    // a pointer loop is rejected
    assert!(dn_expand(&[0xc0, 0], 0).is_none());
    // special characters are escaped
    let (n, _) = dn_expand(&[3, b'a', b'.', b'b', 0], 0).unwrap();
    assert_eq!(n, b"a\\.b");
}

#[test]
fn host_lookup_functions() {
    let mut t = TestVm::new();
    // numeric addresses resolve to themselves
    let r = t
        .call("gethostbyname", vec![Value::str("127.0.0.1")])
        .unwrap();
    assert_eq!(r.as_str().unwrap(), "127.0.0.1");
    let r = t
        .call("gethostbynamel", vec![Value::str("127.0.0.1")])
        .unwrap();
    assert_eq!(
        r.as_array().unwrap().get_int(0).unwrap().as_str().unwrap(),
        "127.0.0.1"
    );
    // an overlong name warns and is returned unchanged
    let long = "a".repeat(300);
    let r = t.call("gethostbyname", vec![Value::str(&long)]).unwrap();
    assert_eq!(r.as_str().unwrap().len(), 300);
    assert_eq!(
        t.output(),
        "\nWarning: gethostbyname(): Host name cannot be longer than 255 characters in /t.php on line 3\n"
    );
    let r = t.call("gethostbynamel", vec![Value::str(&long)]).unwrap();
    assert!(matches!(r, Value::Bool(false)));
    let _ = t.output();
    // invalid addresses
    let r = t
        .call("gethostbyaddr", vec![Value::str("not an ip")])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: gethostbyaddr(): Address is not a valid IPv4 or IPv6 address in /t.php on line 3\n"
    );
    // a valid address always yields a string (the name or the address itself)
    let r = t
        .call("gethostbyaddr", vec![Value::str("127.0.0.1")])
        .unwrap();
    assert!(r.as_str().is_some());
    // gethostname
    let r = t.call("gethostname", vec![]).unwrap();
    assert!(!r.as_str().unwrap().is_empty());
    let (c, _) = thrown(t.call("gethostbyname", vec![Value::bytes(b"a\0b")]));
    assert_eq!(c, "ValueError");
}
