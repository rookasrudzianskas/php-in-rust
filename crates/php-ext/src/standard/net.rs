//! ext/standard/net.c

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use super::basic::netfuncs::inet_ntop_ip;
use super::basic::sys::{self, SockAddr};

const IFF_UP: u32 = 0x1;
const IFF_BROADCAST: u32 = 0x2;
const IFF_POINTOPOINT: u32 = 0x10;

/// `php_inet_ntop()`: the numeric text of an IPv4/IPv6 socket address.
fn php_inet_ntop(addr: Option<&SockAddr>) -> Option<Value> {
    addr?.ip.as_ref().map(|ip| Value::from(inet_ntop_ip(ip)))
}

/// `iface_append_unicast()`
fn iface_append_unicast(
    unicast: &mut PhpArray,
    flags: u32,
    addr: Option<&SockAddr>,
    netmask: Option<&SockAddr>,
    broadcast: Option<&SockAddr>,
    ptp: Option<&SockAddr>,
) {
    let mut u = PhpArray::new();
    u.insert_str("flags", flags as i64);
    if let Some(a) = addr {
        u.insert_str("family", a.family);
        if let Some(host) = php_inet_ntop(Some(a)) {
            u.insert_str("address", host);
        }
    }
    if let Some(host) = php_inet_ntop(netmask) {
        u.insert_str("netmask", host);
    }
    if let Some(host) = php_inet_ntop(broadcast) {
        u.insert_str("broadcast", host);
    }
    if let Some(host) = php_inet_ntop(ptp) {
        u.insert_str("ptp", host);
    }
    let _ = unicast.append(Value::array(u));
}

fn net_get_interfaces(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let addrs = match sys::getifaddrs() {
        Ok(a) => a,
        Err(errno) => {
            let e = std::io::Error::from_raw_os_error(errno);
            vm.zend_error(
                php_runtime::errors::level::E_WARNING,
                format!(
                    "getifaddrs() failed {errno}: {}",
                    super::basic::os_error_text(&e)
                ),
            )?;
            return Ok(Value::Bool(false));
        }
    };

    let mut ret = PhpArray::new();
    for p in &addrs {
        let key = ArrayKey::from_bytes(&p.name);
        if ret.get(&key).is_none() {
            ret.insert(key.clone(), Value::array(PhpArray::new()));
        }
        let Some(Value::Array(iface)) = ret.get_mut(&key) else {
            continue;
        };
        let iface = std::rc::Rc::make_mut(iface);
        if iface.get_str(b"unicast").is_none() {
            iface.insert_str("unicast", Value::array(PhpArray::new()));
        }
        if let Some(Value::Array(unicast)) = iface.get_mut(&ArrayKey::from("unicast")) {
            let unicast = std::rc::Rc::make_mut(unicast);
            iface_append_unicast(
                unicast,
                p.flags,
                p.addr.as_ref(),
                p.netmask.as_ref(),
                if p.flags & IFF_BROADCAST != 0 {
                    p.dstaddr.as_ref()
                } else {
                    None
                },
                if p.flags & IFF_POINTOPOINT != 0 {
                    p.dstaddr.as_ref()
                } else {
                    None
                },
            );
        }
        if iface.get_str(b"up").is_none() {
            iface.insert_str("up", p.flags & IFF_UP != 0);
        }
    }
    Ok(Value::array(ret))
}

pub static NET_GET_INTERFACES: NativeFunction = NativeFunction {
    sig: "net_get_interfaces(): array|false",
    handler: net_get_interfaces,
};

pub fn register(r: &mut Registry) {
    r.function(&NET_GET_INTERFACES);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::Value;

    #[test]
    fn lists_interfaces_with_unicast_entries() {
        let mut t = TestVm::new();
        let r = t.call("net_get_interfaces", vec![]).unwrap();
        let Value::Array(a) = r else {
            panic!("array expected, got {r:?}")
        };
        assert!(!a.is_empty());
        for (_, iface) in a.iter() {
            let Value::Array(i) = iface.unref() else {
                panic!("interface must be an array")
            };
            assert!(matches!(i.get_str(b"unicast"), Some(Value::Array(_))));
            assert!(matches!(i.get_str(b"up"), Some(Value::Bool(_))));
            let Some(Value::Array(u)) = i.get_str(b"unicast") else {
                unreachable!()
            };
            for (_, entry) in u.iter() {
                let Value::Array(e) = entry.unref() else {
                    panic!("unicast entry must be an array")
                };
                assert!(matches!(e.get_str(b"flags"), Some(Value::Int(_))));
            }
        }
        // a loopback interface exists on every supported host
        let lo = a
            .iter()
            .find(|(k, _)| matches!(k.to_string().as_str(), "lo" | "lo0"))
            .expect("loopback interface")
            .1
            .unref();
        let Value::Array(lo) = lo else { panic!() };
        let Some(Value::Array(u)) = lo.get_str(b"unicast") else {
            panic!()
        };
        let has_v4 = u.iter().any(|(_, e)| {
            matches!(e.unref(), Value::Array(e)
                if e.get_str(b"address").and_then(Value::as_str).is_some_and(|s| s == "127.0.0.1")
                && e.get_str(b"netmask").and_then(Value::as_str).is_some_and(|s| s == "255.0.0.0"))
        });
        assert!(has_v4);
    }
}
