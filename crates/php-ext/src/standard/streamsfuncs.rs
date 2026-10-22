//! ext/standard/streamsfuncs.c

#![allow(clippy::collapsible_if)]

use std::net::{Shutdown, SocketAddr};
use std::rc::Rc;
use std::time::Duration;

use php_runtime::{
    Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpRef, PhpStr, Registry, ResRef, Value, Vm,
};

use super::streams::context::{self, Context};
use super::streams::filter::{self, FilterResource};
use super::streams::socket::{self, SocketOps};
use super::streams::stream::{self, CastAs, OptParam, Stream, opt};
use super::streams::wrappers;
use super::streams::{self as st, context_param, handle_of, stream_param, sys, with_stream};

fn bool_or(v: Option<Value>) -> Value {
    v.unwrap_or(Value::Bool(false))
}

/// `addslashes` (host names in "Unable to connect" warnings).
fn addslashes(s: &[u8]) -> String {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        match c {
            0 => out.extend_from_slice(b"\\0"),
            b'\'' | b'"' | b'\\' => {
                out.push(b'\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Converts a PHP timeout in seconds (`null` = default_socket_timeout) to
/// a duration; negative or huge values wait forever.
fn timeout_arg(vm: &mut Vm, t: Option<f64>, arg: usize) -> PResult<Option<Duration>> {
    let t = match t {
        None => vm.ini_int("default_socket_timeout") as f64,
        Some(t) if !t.is_finite() => {
            return Err(vm.argument_value_error(arg, "must be a finite value"));
        }
        Some(t) => t,
    };
    if t < 0.0 || t >= u64::MAX as f64 / 1_000_000.0 {
        return Ok(None);
    }
    Ok(Some(Duration::from_micros((t * 1_000_000.0) as u64)))
}

fn set_ref_opt(args: &Args, i: usize, v: Value) {
    if args.is_passed(i) {
        args.by_ref(i).set(v);
    }
}

// ---------------------------------------------------------------------------
// Socket functions
// ---------------------------------------------------------------------------

fn stream_socket_pair(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let domain = args.int(0);
    let ty = args.int(1);
    let _protocol = args.int(2);
    match socket::socket_pair(domain, ty) {
        Ok((a, b)) => {
            let _ctx = context_param(vm, args, 3, false)?;
            let mut arr = PhpArray::new();
            for s in [a, b] {
                let r = socket::alloc_socket(
                    vm,
                    s,
                    "generic_socket",
                    socket::default_timeout(vm),
                    false,
                );
                let _ = arr.append(Value::Resource(r));
            }
            Ok(Value::from(arr))
        }
        Err(n) => {
            vm.warning(format!(
                "Failed to create sockets: [{n}]: {}",
                sys::strerror(n)
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

fn stream_socket_client(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let host = args.str(0).clone();
    let timeout = timeout_arg(vm, args.float_opt(3), 4)?;
    let flags = args.int(4);
    let ctx = context_param(vm, args, 5, flags & 16 != 0)?;
    set_ref_opt(args, 1, Value::Int(0));
    set_ref_opt(args, 2, Value::empty_string());
    let key = (flags & 1 != 0).then(|| {
        let mut k = b"stream_socket_client__".to_vec();
        k.extend_from_slice(&host);
        k
    });
    match socket::xport_create(vm, &host, false, 0, timeout, ctx.as_ref(), key.as_deref())? {
        Ok(res) => Ok(Value::Resource(res)),
        Err((errno, msg)) => {
            vm.warning(format!(
                "Unable to connect to {} ({})",
                addslashes(&host),
                if msg.is_empty() {
                    "Unknown error"
                } else {
                    &msg
                }
            ))?;
            set_ref_opt(args, 1, Value::Int(errno as i64));
            if !msg.is_empty() {
                set_ref_opt(args, 2, Value::from(msg));
            }
            Ok(Value::Bool(false))
        }
    }
}

fn stream_socket_server(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let host = args.str(0).clone();
    let flags = args.int(3);
    let ctx = context_param(vm, args, 4, flags & 16 != 0)?;
    set_ref_opt(args, 1, Value::Int(0));
    set_ref_opt(args, 2, Value::empty_string());
    match socket::xport_create(vm, &host, true, flags, None, ctx.as_ref(), None)? {
        Ok(res) => Ok(Value::Resource(res)),
        Err((errno, msg)) => {
            vm.warning(format!(
                "Unable to connect to {} ({})",
                String::from_utf8_lossy(&host),
                if msg.is_empty() {
                    "Unknown error"
                } else {
                    &msg
                }
            ))?;
            set_ref_opt(args, 1, Value::Int(errno as i64));
            if !msg.is_empty() {
                set_ref_opt(args, 2, Value::from(msg));
            }
            Ok(Value::Bool(false))
        }
    }
}

fn with_socket<R>(
    vm: &mut Vm,
    res: &ResRef,
    f: impl FnOnce(&mut Vm, &mut SocketOps) -> PResult<R>,
) -> PResult<Option<R>> {
    Ok(with_stream(vm, res, |vm, s| match s.ops_as::<SocketOps>() {
        Some(o) => f(vm, o).map(Some),
        None => Ok(None),
    })?
    .flatten())
}

fn stream_socket_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let timeout = timeout_arg(vm, args.float_opt(1), 2)?;
    let r = with_socket(vm, &res, |_vm, o| Ok(o.accept(timeout)))?;
    match r {
        Some(Ok((sock, label, peer))) => {
            let client = socket::alloc_socket(vm, sock, label, socket::default_timeout(vm), false);
            if args.is_passed(2) {
                if let Some(p) = peer {
                    args.by_ref(2).set(Value::from(p));
                }
            }
            Ok(Value::Resource(client))
        }
        Some(Err(msg)) => {
            vm.warning(format!("Accept failed: {msg}"))?;
            Ok(Value::Bool(false))
        }
        None => {
            vm.warning("Accept failed: Unknown error")?;
            Ok(Value::Bool(false))
        }
    }
}

fn stream_socket_get_name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let want_peer = args.bool(1);
    let name = with_socket(vm, &res, |_vm, o| Ok(o.name(want_peer)))?.flatten();
    match name {
        Some(n) if !n.is_empty() && n[0] != 0 => Ok(Value::from(n)),
        _ => Ok(Value::Bool(false)),
    }
}

/// `php_network_parse_network_address_with_port`.
fn parse_network_address(s: &[u8]) -> Option<SocketAddr> {
    let text = std::str::from_utf8(s).ok()?;
    if let Ok(a) = text.parse::<SocketAddr>() {
        return Some(a);
    }
    let (host, port) = text.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    socket::resolve(host, port as i32).ok()?.into_iter().next()
}

fn stream_socket_sendto(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let data = args.str(1).clone();
    let target = args.str(3).clone();
    let addr = if target.is_empty() {
        None
    } else {
        match parse_network_address(&target) {
            Some(a) => Some(a),
            None => {
                vm.warning(format!(
                    "Failed to parse `{}' into a valid network address",
                    String::from_utf8_lossy(&target)
                ))?;
                return Ok(Value::Bool(false));
            }
        }
    };
    let r = with_socket(vm, &res, |vm, o| match o.sendto(&data, addr) {
        Ok(n) => Ok(n as i64),
        Err(e) => {
            vm.warning(sys::strerror(sys::errno_of(&e)))?;
            Ok(-1)
        }
    })?;
    Ok(Value::Int(r.unwrap_or(-1)))
}

fn stream_socket_recvfrom(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let to_read = args.int(1);
    let flags = args.int(2);
    set_ref_opt(args, 3, Value::Null);
    if to_read <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    let mut buf = vec![0u8; to_read as usize];
    let r = with_socket(vm, &res, |_vm, o| {
        Ok(o.recvfrom(&mut buf, flags & 2 != 0).ok())
    })?
    .flatten();
    match r {
        Some((n, addr)) => {
            if let Some(a) = addr {
                set_ref_opt(args, 3, Value::from(a));
            }
            buf.truncate(n);
            Ok(Value::from(buf))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn stream_socket_shutdown(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let how = args.int(1);
    let how = match how {
        0 => Shutdown::Read,
        1 => Shutdown::Write,
        2 => Shutdown::Both,
        _ => {
            return Err(vm.argument_value_error(
                2,
                "must be one of STREAM_SHUT_RD, STREAM_SHUT_WR, or STREAM_SHUT_RDWR",
            ));
        }
    };
    Ok(Value::Bool(
        with_socket(vm, &res, |_vm, o| Ok(o.shutdown(how)))?.unwrap_or(false),
    ))
}

fn stream_socket_enable_crypto(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let enable = args.bool(1);
    if !args.is_null(3) {
        st::stream_arg(vm, &args.get(3).clone(), 4)?;
    }
    if enable {
        if args.int_opt(2).is_none() {
            let has = handle_of(&res)
                .and_then(|h| h.borrow().st.context.clone())
                .and_then(|c| context::get_option(&c, b"ssl", b"crypto_method"))
                .is_some();
            if !has {
                return Err(
                    vm.argument_value_error(3, "must be specified when enabling encryption")
                );
            }
        }
        vm.warning("this stream does not support SSL/crypto")?;
        return Ok(Value::Bool(false));
    }
    vm.warning("this stream does not support SSL/crypto")?;
    Ok(Value::Bool(true))
}

fn stream_socket_get_crypto_status(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let _ = stream_param(vm, args, 0)?;
    Ok(Value::Int(0))
}

// ---------------------------------------------------------------------------
// Contents and metadata
// ---------------------------------------------------------------------------

fn stream_get_contents(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let maxlen = match args.int_opt(1) {
        None | Some(-1) => stream::COPY_ALL,
        Some(n) if n < 0 => {
            return Err(vm.argument_value_error(2, "must be greater than or equal to -1"));
        }
        Some(n) => n as usize,
    };
    let desiredpos = args.int(2);
    let r = with_stream(vm, &res, |vm, s| {
        if desiredpos >= 0 {
            let position = s.tell();
            let mut seek_res = 0;
            if position >= 0 && desiredpos > position {
                seek_res = s.seek(vm, desiredpos - position, 1)?;
            } else if desiredpos < position {
                seek_res = s.seek(vm, desiredpos, 0)?;
            }
            if seek_res != 0 {
                return Ok(None);
            }
        }
        Ok(Some(s.copy_to_mem(vm, maxlen)?.unwrap_or_default()))
    })?
    .flatten();
    match r {
        Some(d) => Ok(Value::from(d)),
        None => {
            vm.warning(format!(
                "Failed to seek to position {desiredpos} in the stream"
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

fn stream_copy_to_stream(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let src = stream_param(vm, args, 0)?;
    let dest = stream_param(vm, args, 1)?;
    let maxlen = args.int_opt(2).map_or(stream::COPY_ALL, |n| n as usize);
    let pos = args.int(3);
    let _ctx = context_param(vm, args, 4, false)?;
    let sh = handle_of(&src).expect("checked");
    let dh = handle_of(&dest).expect("checked");
    if pos > 0 {
        let r = with_stream(vm, &src, |vm, s| s.seek(vm, pos, 0))?.unwrap_or(-1);
        if r < 0 {
            vm.warning(format!("Failed to seek to position {pos} in the stream"))?;
            return Ok(Value::Bool(false));
        }
    }
    let (ok, len) = if Rc::ptr_eq(&sh, &dh) {
        let mut s = sh.borrow_mut();
        stream::copy_to_stream(vm, &mut s, None, maxlen)?
    } else {
        let mut s = sh.borrow_mut();
        let mut d = dh.borrow_mut();
        s.res = Some(src.clone());
        d.res = Some(dest.clone());
        let r = stream::copy_to_stream(vm, &mut s, Some(&mut d), maxlen);
        s.res = None;
        d.res = None;
        r?
    };
    Ok(if ok {
        Value::Int(len as i64)
    } else {
        Value::Bool(false)
    })
}

/// `stream_get_meta_data` for an already borrowed stream.
pub fn meta_data(vm: &mut Vm, s: &mut Stream) -> PResult<PhpArray> {
    let mut a = PhpArray::new();
    let populated = {
        let mut p = OptParam::Meta(&mut a);
        s.set_option(vm, opt::META_DATA_API, 0, &mut p)? == opt::RETURN_OK
    };
    if !populated {
        a.insert_str("timed_out", false);
        a.insert_str("blocked", true);
        let eof = s.eof(vm)?;
        a.insert_str("eof", eof);
    }
    if let Some(wd) = &s.wrapperdata {
        a.insert_str("wrapper_data", wd.clone());
    }
    if let Some(w) = &s.wrapper {
        a.insert_str("wrapper_type", w.label());
    }
    a.insert_str("stream_type", s.ops.label());
    a.insert_str("mode", Value::String(s.st.mode.clone()));
    a.insert_str("unread_bytes", s.buffered() as i64);
    a.insert_str(
        "seekable",
        s.ops.can_seek() && s.st.flags & stream::flags::NO_SEEK == 0,
    );
    if let Some(p) = &s.orig_path {
        a.insert_str("uri", Value::String(p.clone()));
    }
    Ok(a)
}

fn stream_get_meta_data(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let a = with_stream(vm, &res, meta_data)?.unwrap_or_default();
    Ok(Value::from(a))
}

fn stream_get_transports(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::from(PhpArray::from_values(
        socket::TRANSPORTS.iter().map(|t| Value::str(t)).collect(),
    )))
}

fn stream_get_wrappers(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let names = wrappers::wrapper_names(vm);
    Ok(Value::from(PhpArray::from_values(
        names.into_iter().map(Value::String).collect(),
    )))
}

fn stream_last_errors(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::empty_array())
}

fn stream_clear_errors(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// stream_select
// ---------------------------------------------------------------------------

struct SelEntry {
    key: ArrayKey,
    value: Value,
    fd: Option<i32>,
}

fn select_entries(arr: &PhpArray) -> Vec<SelEntry> {
    arr.iter()
        .filter_map(|(k, v)| {
            let v = v.unref();
            let Value::Resource(r) = &v else {
                return None;
            };
            let h = handle_of(r)?;
            let fd = h
                .try_borrow_mut()
                .ok()
                .and_then(|mut s| s.cast_fd(CastAs::FdForSelect));
            Some(SelEntry {
                key: k.clone(),
                value: v.clone(),
                fd,
            })
        })
        .collect()
}

fn stream_select(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let refs: Vec<PhpRef> = (0..3).map(|i| args.by_ref(i)).collect();
    let sec = args.int_opt(3);
    let usec = args.int_opt(4);
    let _ctx = context_param(vm, args, 5, false)?;
    let arrays: Vec<Option<PhpArray>> = refs
        .iter()
        .map(|r| match r.get() {
            Value::Array(a) => Some((*a).clone()),
            _ => None,
        })
        .collect();
    let sets: Vec<Vec<SelEntry>> = arrays
        .iter()
        .map(|a| a.as_ref().map(select_entries).unwrap_or_default())
        .collect();
    let have_sets = sets.iter().any(|s| s.iter().any(|e| e.fd.is_some()));
    if !have_sets {
        return Err(vm.throw_value_error("No stream arrays were passed"));
    }
    if sec.is_none() && usec.is_some_and(|u| u != 0) {
        return Err(vm.argument_value_error(5, "must be null when argument #4 ($seconds) is null"));
    }
    let timeout_ms: i32 = match sec {
        None => -1,
        Some(s) if s < 0 => {
            return Err(vm.argument_value_error(4, "must be greater than or equal to 0"));
        }
        Some(s) => {
            let u = usec.unwrap_or(0);
            if u < 0 {
                return Err(vm.argument_value_error(5, "must be greater than or equal to 0"));
            }
            let total_us = (s as i128) * 1_000_000 + u as i128;
            ((total_us + 999) / 1000).min(i32::MAX as i128) as i32
        }
    };
    // streams with buffered data count as readable right away
    if let Some(read) = &arrays[0] {
        let mut ready = PhpArray::new();
        for (k, v) in read.iter() {
            let vv = v.unref();
            if let Value::Resource(r) = &vv {
                if handle_of(r).is_some_and(|h| h.try_borrow().is_ok_and(|s| s.buffered() > 0)) {
                    ready.insert(k.clone(), vv.clone());
                }
            }
        }
        if !ready.is_empty() {
            let n = ready.len();
            refs[0].set(Value::from(ready));
            for r in refs.iter().skip(1) {
                if r.get().as_array().is_some() {
                    r.set(Value::empty_array());
                }
            }
            return Ok(Value::Int(n as i64));
        }
    }
    let events = [sys::POLLIN | sys::POLLHUP, sys::POLLOUT, sys::POLLPRI];
    let mut fds: Vec<(i32, i16)> = Vec::new();
    for (set, ev) in sets.iter().zip(events) {
        for e in set {
            if let Some(fd) = e.fd {
                fds.push((fd, ev));
            }
        }
    }
    let revents = match sys::sys_poll(&fds, timeout_ms) {
        Ok((_, r)) => r,
        Err(e) => {
            let n = sys::errno_of(&e);
            let max_fd = fds.iter().map(|f| f.0).max().unwrap_or(0);
            vm.warning(format!(
                "Unable to select [{n}]: {} (max_fd={max_fd})",
                sys::strerror(n)
            ))?;
            return Ok(Value::Bool(false));
        }
    };
    let mut total = 0i64;
    let mut idx = 0;
    for (k, set) in sets.iter().enumerate() {
        let mut out = PhpArray::new();
        for e in set {
            let Some(_) = e.fd else {
                continue;
            };
            let re = revents[idx];
            idx += 1;
            let hit = match k {
                0 => re & (sys::POLLIN | sys::POLLHUP | sys::POLLERR | sys::POLLNVAL) != 0,
                1 => re & (sys::POLLOUT | sys::POLLERR | sys::POLLHUP) != 0,
                _ => re & sys::POLLPRI != 0,
            };
            if hit {
                out.insert(e.key.clone(), e.value.clone());
                total += 1;
            }
        }
        if arrays[k].is_some() {
            refs[k].set(Value::from(out));
        }
    }
    Ok(Value::Int(total))
}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// `parse_context_options`.
fn parse_context_options(vm: &mut Vm, ctx: &ResRef, options: &PhpArray) -> PResult<()> {
    for (wkey, wval) in options.iter() {
        let wval = wval.unref();
        match (wkey, &wval) {
            (ArrayKey::Str(w), Value::Array(inner)) => {
                if !inner.is_packed() {
                    for (okey, oval) in inner.iter() {
                        if let ArrayKey::Str(o) = okey {
                            context::set_option(ctx, w.as_bytes(), o.as_bytes(), oval.unref());
                        }
                    }
                }
            }
            _ => {
                return Err(vm.throw_value_error(
                    "Options should have the form [\"wrappername\"][\"optionname\"] = $value",
                ));
            }
        }
    }
    Ok(())
}

/// `parse_context_params`.
fn parse_context_params(vm: &mut Vm, ctx: &ResRef, params: &PhpArray) -> PResult<()> {
    if let Some(n) = params.get_str(b"notification").map(Value::unref) {
        let host = vm.host.clone();
        if let Err(err) = host.check_callable(vm, &n) {
            return Err(vm.argument_type_error(
                1,
                &format!("must be an array with valid callbacks as values, {err}"),
            ));
        }
        if let Some(c) = context::of(ctx) {
            c.borrow_mut().notifier = Some(n);
        }
    }
    if let Some(o) = params.get_str(b"options").map(Value::unref) {
        match o {
            Value::Array(a) => parse_context_options(vm, ctx, &a)?,
            _ => return Err(vm.throw_type_error("Invalid stream/context parameter")),
        }
    }
    Ok(())
}

/// `decode_context_param`: a context resource, or a stream's context
/// (allocated on demand).
fn decode_context_param(v: &Value) -> Option<ResRef> {
    let Value::Resource(r) = &*v.deref() else {
        return None;
    };
    if context::of(r).is_some() {
        return Some(r.clone());
    }
    let h = handle_of(r)?;
    let mut s = h.try_borrow_mut().ok()?;
    if s.st.context.is_none() {
        s.st.context = Some(context::alloc());
    }
    s.st.context.clone()
}

fn resource_param(vm: &mut Vm, args: &Args, i: usize) -> PResult<Value> {
    let v = args.get(i).unref();
    if !matches!(v, Value::Resource(_)) {
        return Err(vm.argument_type_error(
            i + 1,
            &format!("must be of type resource, {} given", v.value_name()),
        ));
    }
    Ok(v)
}

fn context_of_param(vm: &mut Vm, args: &Args, i: usize) -> PResult<ResRef> {
    let v = resource_param(vm, args, i)?;
    match decode_context_param(&v) {
        Some(c) => Ok(c),
        None => Err(vm.argument_type_error(i + 1, "must be a valid stream/context")),
    }
}

fn stream_context_create(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let options = args.array_opt(0).cloned();
    let params = args.array_opt(1).cloned();
    let ctx = context::alloc();
    if let Some(o) = options {
        parse_context_options(vm, &ctx, &o)?;
    }
    if let Some(p) = params {
        parse_context_params(vm, &ctx, &p)?;
    }
    Ok(Value::Resource(ctx))
}

fn stream_context_set_params(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = context_of_param(vm, args, 0)?;
    let params = args.array(1).clone();
    parse_context_params(vm, &ctx, &params)?;
    Ok(Value::Bool(true))
}

fn stream_context_get_params(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = context_of_param(vm, args, 0)?;
    let mut a = PhpArray::new();
    if let Some(c) = context::of(&ctx) {
        let c: std::cell::Ref<'_, Context> = c.borrow();
        if let Some(n) = &c.notifier {
            a.insert_str("notification", n.clone());
        }
        a.insert_str("options", Value::from(c.options.clone()));
    }
    Ok(Value::from(a))
}

fn stream_context_set_option(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    if args.len() == 2 {
        vm.engine_deprecated(
            "Calling stream_context_set_option() with 2 arguments is deprecated, use stream_context_set_options() instead",
        )?;
    }
    let ctx = context_of_param(vm, args, 0)?;
    match args.get(1).unref() {
        Value::Array(options) => {
            if args.str_opt(2).is_some() {
                return Err(vm.argument_value_error(
                    3,
                    "must be null when argument #2 ($wrapper_or_options) is an array",
                ));
            }
            if args.is_passed(3) {
                return Err(vm.argument_value_error(
                    4,
                    "cannot be provided when argument #2 ($wrapper_or_options) is an array",
                ));
            }
            parse_context_options(vm, &ctx, &options)?;
            Ok(Value::Bool(true))
        }
        Value::String(wrapper) => {
            let Some(name) = args.str_opt(2).cloned() else {
                return Err(vm.argument_value_error(
                    3,
                    "cannot be null when argument #2 ($wrapper_or_options) is a string",
                ));
            };
            if !args.is_passed(3) {
                return Err(vm.argument_value_error(
                    4,
                    "must be provided when argument #2 ($wrapper_or_options) is a string",
                ));
            }
            context::set_option(&ctx, &wrapper, &name, args.get(3).unref());
            Ok(Value::Bool(true))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn stream_context_set_options(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = context_of_param(vm, args, 0)?;
    let options = args.array(1).clone();
    parse_context_options(vm, &ctx, &options)?;
    Ok(Value::Bool(true))
}

fn stream_context_get_options(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = context_of_param(vm, args, 0)?;
    let opts = context::of(&ctx)
        .map(|c| c.borrow().options.clone())
        .unwrap_or_default();
    Ok(Value::from(opts))
}

fn stream_context_get_default(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = context::default_context(vm);
    if let Some(o) = args.array_opt(0).cloned() {
        parse_context_options(vm, &ctx, &o)?;
    }
    Ok(Value::Resource(ctx))
}

fn stream_context_set_default(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let options = args.array(0).clone();
    let ctx = context::default_context(vm);
    if let Some(Value::Array(s)) = options.get_str(b"stream").map(Value::unref) {
        if s.get_str(b"error_mode").is_some()
            || s.get_str(b"error_store").is_some()
            || s.get_str(b"error_handler").is_some()
        {
            return Err(vm.throw_value_error(
                "Stream error handling options cannot be set on the default context",
            ));
        }
    }
    parse_context_options(vm, &ctx, &options)?;
    Ok(Value::Resource(ctx))
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

fn apply_filter_to_stream(vm: &mut Vm, args: &mut Args, append: bool) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let name = args.str(1).clone();
    let mut read_write = args.int(2);
    let params = if args.is_passed(3) {
        args.get(3).unref()
    } else {
        Value::Undef
    };
    let h = handle_of(&res).expect("checked");
    let (mode, persistent) = {
        let s = h.borrow();
        (s.st.mode.clone(), s.st.is_persistent)
    };
    if read_write & filter::FILTER_ALL == 0 {
        if mode.contains(&b'r') || mode.contains(&b'+') {
            read_write |= filter::FILTER_READ;
        }
        if mode.contains(&b'w') || mode.contains(&b'+') || mode.contains(&b'a') {
            read_write |= filter::FILTER_WRITE;
        }
    }
    let mut last: Option<u64> = None;
    for (bit, is_read) in [(filter::FILTER_READ, true), (filter::FILTER_WRITE, false)] {
        if read_write & bit == 0 {
            continue;
        }
        let Some(f) = filter::create_filter(vm, &name, &params, persistent)? else {
            return Ok(Value::Bool(false));
        };
        let id = f.id;
        let ok = with_stream(vm, &res, |vm, s| s.attach_filter(vm, f, is_read, append))?
            .unwrap_or(false);
        if !ok {
            return Ok(Value::Bool(false));
        }
        last = Some(id);
    }
    let Some(id) = last else {
        return Ok(Value::Bool(false));
    };
    let fres = ResRef::new(
        "stream filter",
        Box::new(FilterResource {
            stream: Rc::downgrade(&h),
            filter_id: id,
        }),
    );
    {
        let mut s = h.borrow_mut();
        let s = &mut *s;
        for f in s.readfilters.iter_mut().chain(s.writefilters.iter_mut()) {
            if f.id == id {
                f.res = Some(fres.clone());
            }
        }
    }
    Ok(Value::Resource(fres))
}

fn stream_filter_prepend(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    apply_filter_to_stream(vm, args, false)
}

fn stream_filter_append(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    apply_filter_to_stream(vm, args, true)
}

fn stream_filter_remove(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = resource_param(vm, args, 0)?;
    let Value::Resource(r) = &v else {
        unreachable!("checked");
    };
    let target = if !r.is_closed() && r.type_name() == "stream filter" {
        r.resource()
            .data
            .borrow()
            .as_ref()
            .and_then(|d| d.downcast_ref::<FilterResource>())
            .map(|f| (f.stream.upgrade(), f.filter_id))
    } else {
        None
    };
    let Some((Some(h), id)) = target else {
        return Err(vm.throw_type_error(
            "stream_filter_remove(): supplied resource is not a valid stream filter resource",
        ));
    };
    let Ok(mut s) = h.try_borrow_mut() else {
        return Ok(Value::Bool(false));
    };
    match s.remove_filter(vm, id, true)? {
        Some(true) => {
            r.close();
            Ok(Value::Bool(true))
        }
        Some(false) => {
            drop(s);
            vm.warning("Unable to flush filter, not removing")?;
            Ok(Value::Bool(false))
        }
        None => Err(vm.throw_type_error(
            "stream_filter_remove(): supplied resource is not a valid stream filter resource",
        )),
    }
}

// ---------------------------------------------------------------------------
// Misc stream functions
// ---------------------------------------------------------------------------

fn stream_get_line(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let mut max_length = args.int(1);
    let ending = args.str(2).clone();
    if max_length < 0 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
    }
    if max_length == 0 {
        max_length = stream::CHUNK_SIZE as i64;
    }
    Ok(bool_or(
        st::read_record(vm, &res, max_length as usize, &ending)?.map(Value::from),
    ))
}

fn set_option(
    vm: &mut Vm,
    res: &ResRef,
    option: i32,
    value: i32,
    mut p: OptParam<'_>,
) -> PResult<i32> {
    Ok(with_stream(vm, res, |vm, s| s.set_option(vm, option, value, &mut p))?.unwrap_or(-1))
}

fn stream_set_blocking(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let block = args.bool(1);
    Ok(Value::Bool(
        set_option(vm, &res, opt::BLOCKING, block as i32, OptParam::None)? != -1,
    ))
}

fn stream_set_timeout(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let mut sec = args.int(1);
    let mut usec = 0;
    if args.len() == 3 {
        let m = args.int(2);
        usec = m % 1_000_000;
        sec += m / 1_000_000;
    }
    Ok(Value::Bool(
        set_option(vm, &res, opt::READ_TIMEOUT, 0, OptParam::Timeout(sec, usec))? == opt::RETURN_OK,
    ))
}

fn socket_set_timeout(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.engine_deprecated(
        "Function socket_set_timeout() is deprecated since 8.5, use stream_set_timeout() instead",
    )?;
    stream_set_timeout(vm, args)
}

fn stream_set_write_buffer(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let buff = args.int(1);
    let ret = if buff == 0 {
        set_option(
            vm,
            &res,
            opt::WRITE_BUFFER,
            opt::BUFFER_NONE,
            OptParam::None,
        )?
    } else {
        set_option(
            vm,
            &res,
            opt::WRITE_BUFFER,
            opt::BUFFER_FULL,
            OptParam::Size(buff),
        )?
    };
    Ok(Value::Int(if ret == 0 { 0 } else { -1 }))
}

fn stream_set_read_buffer(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let buff = args.int(1);
    let ret = if buff == 0 {
        set_option(vm, &res, opt::READ_BUFFER, opt::BUFFER_NONE, OptParam::None)?
    } else {
        set_option(
            vm,
            &res,
            opt::READ_BUFFER,
            opt::BUFFER_FULL,
            OptParam::Size(buff),
        )?
    };
    Ok(Value::Int(if ret == 0 { 0 } else { -1 }))
}

fn stream_set_chunk_size(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let csize = args.int(1);
    if csize <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    if csize > i32::MAX as i64 {
        return Err(vm.argument_value_error(2, "is too large"));
    }
    let ret = set_option(vm, &res, opt::SET_CHUNK_SIZE, csize as i32, OptParam::None)?;
    Ok(Value::Int(if ret > 0 { ret as i64 } else { -1 }))
}

fn stream_resolve_include_path(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = st::path_param(vm, args, 0)?;
    Ok(bool_or(
        wrappers::resolve_path(vm, &filename).map(Value::String),
    ))
}

fn stream_is_local(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    let wrapper = match &v {
        Value::Resource(r) => {
            let Some(h) = handle_of(r) else {
                return Err(vm.throw_type_error(
                    "stream_is_local(): supplied resource is not a valid stream resource",
                ));
            };
            h.borrow().wrapper.clone()
        }
        other => {
            let s = vm.to_php_string(other)?;
            let _ctx = context_param(vm, args, 1, false)?;
            wrappers::locate_url_wrapper(vm, &s, 0)?.map(|l| l.wrapper)
        }
    };
    Ok(Value::Bool(wrapper.is_some_and(|w| !w.is_url())))
}

fn stream_supports_lock(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    Ok(Value::Bool(
        with_stream(vm, &res, |vm, s| s.supports_lock(vm))?.unwrap_or(false),
    ))
}

fn stream_isatty(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let fd = with_stream(vm, &res, |_vm, s| Ok(s.cast_fd(CastAs::FdForSelect)))?.flatten();
    Ok(Value::Bool(fd.is_some_and(sys::sys_isatty)))
}

/// Exposed for other modules: the `PhpStr` mode of a stream.
pub fn stream_mode(res: &ResRef) -> Option<PhpStr> {
    handle_of(res).map(|h| h.borrow().st.mode.clone())
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
        const ALL: &[&NativeFunction] = &[$(&$name),*];
    };
}

natives! {
    STREAM_SELECT: "stream_select(?array &$read, ?array &$write, ?array &$except, ?int $seconds, ?int $microseconds = null, $context = null): int|false" => stream_select;
    STREAM_CONTEXT_CREATE: "stream_context_create(?array $options = null, ?array $params = null)" => stream_context_create;
    STREAM_CONTEXT_SET_PARAMS: "stream_context_set_params($context, array $params): true" => stream_context_set_params;
    STREAM_CONTEXT_GET_PARAMS: "stream_context_get_params($context): array" => stream_context_get_params;
    STREAM_CONTEXT_SET_OPTION: "stream_context_set_option($context, array|string $wrapper_or_options, ?string $option_name = null, mixed $value = UNKNOWN): true" => stream_context_set_option;
    STREAM_CONTEXT_SET_OPTIONS: "stream_context_set_options($context, array $options): true" => stream_context_set_options;
    STREAM_CONTEXT_GET_OPTIONS: "stream_context_get_options($stream_or_context): array" => stream_context_get_options;
    STREAM_CONTEXT_GET_DEFAULT: "stream_context_get_default(?array $options = null)" => stream_context_get_default;
    STREAM_CONTEXT_SET_DEFAULT: "stream_context_set_default(array $options)" => stream_context_set_default;
    STREAM_FILTER_PREPEND: "stream_filter_prepend($stream, string $filter_name, int $mode = 0, mixed $params = UNKNOWN)" => stream_filter_prepend;
    STREAM_FILTER_APPEND: "stream_filter_append($stream, string $filter_name, int $mode = 0, mixed $params = UNKNOWN)" => stream_filter_append;
    STREAM_FILTER_REMOVE: "stream_filter_remove($stream_filter): bool" => stream_filter_remove;
    STREAM_SOCKET_CLIENT: "stream_socket_client(string $address, &$error_code = null, &$error_message = null, ?float $timeout = null, int $flags = STREAM_CLIENT_CONNECT, $context = null)" => stream_socket_client;
    STREAM_SOCKET_SERVER: "stream_socket_server(string $address, &$error_code = null, &$error_message = null, int $flags = STREAM_SERVER_BIND | STREAM_SERVER_LISTEN, $context = null)" => stream_socket_server;
    STREAM_SOCKET_ACCEPT: "stream_socket_accept($socket, ?float $timeout = null, &$peer_name = null)" => stream_socket_accept;
    STREAM_SOCKET_GET_NAME: "stream_socket_get_name($socket, bool $remote): string|false" => stream_socket_get_name;
    STREAM_SOCKET_RECVFROM: "stream_socket_recvfrom($socket, int $length, int $flags = 0, &$address = null): string|false" => stream_socket_recvfrom;
    STREAM_SOCKET_SENDTO: "stream_socket_sendto($socket, string $data, int $flags = 0, string $address = \"\"): int|false" => stream_socket_sendto;
    STREAM_SOCKET_ENABLE_CRYPTO: "stream_socket_enable_crypto($stream, bool $enable, ?int $crypto_method = null, $session_stream = null): int|bool" => stream_socket_enable_crypto;
    STREAM_SOCKET_GET_CRYPTO_STATUS: "stream_socket_get_crypto_status($stream): int" => stream_socket_get_crypto_status;
    STREAM_SOCKET_SHUTDOWN: "stream_socket_shutdown($stream, int $mode): bool" => stream_socket_shutdown;
    STREAM_SOCKET_PAIR: "stream_socket_pair(int $domain, int $type, int $protocol, $context = null): array|false" => stream_socket_pair;
    STREAM_COPY_TO_STREAM: "stream_copy_to_stream($from, $to, ?int $length = null, int $offset = 0, $context = null): int|false" => stream_copy_to_stream;
    STREAM_GET_CONTENTS: "stream_get_contents($stream, ?int $length = null, int $offset = -1): string|false" => stream_get_contents;
    STREAM_SUPPORTS_LOCK: "stream_supports_lock($stream): bool" => stream_supports_lock;
    STREAM_SET_WRITE_BUFFER: "stream_set_write_buffer($stream, int $size): int" => stream_set_write_buffer;
    SET_FILE_BUFFER: "set_file_buffer($stream, int $size): int" => stream_set_write_buffer;
    STREAM_SET_READ_BUFFER: "stream_set_read_buffer($stream, int $size): int" => stream_set_read_buffer;
    STREAM_SET_BLOCKING: "stream_set_blocking($stream, bool $enable): bool" => stream_set_blocking;
    SOCKET_SET_BLOCKING: "socket_set_blocking($stream, bool $enable): bool" => stream_set_blocking;
    STREAM_GET_META_DATA: "stream_get_meta_data($stream): array" => stream_get_meta_data;
    SOCKET_GET_STATUS: "socket_get_status($stream): array" => stream_get_meta_data;
    STREAM_GET_LINE: "stream_get_line($stream, int $length, string $ending = \"\"): string|false" => stream_get_line;
    STREAM_RESOLVE_INCLUDE_PATH: "stream_resolve_include_path(string $filename): string|false" => stream_resolve_include_path;
    STREAM_GET_WRAPPERS: "stream_get_wrappers(): array" => stream_get_wrappers;
    STREAM_LAST_ERRORS: "stream_last_errors(): array" => stream_last_errors;
    STREAM_CLEAR_ERRORS: "stream_clear_errors(): void" => stream_clear_errors;
    STREAM_GET_TRANSPORTS: "stream_get_transports(): array" => stream_get_transports;
    STREAM_IS_LOCAL: "stream_is_local($stream, $context = null): bool" => stream_is_local;
    STREAM_ISATTY: "stream_isatty($stream): bool" => stream_isatty;
    STREAM_SET_CHUNK_SIZE: "stream_set_chunk_size($stream, int $size): int" => stream_set_chunk_size;
    STREAM_SET_TIMEOUT: "stream_set_timeout($stream, int $seconds, int $microseconds = 0): bool" => stream_set_timeout;
    SOCKET_SET_TIMEOUT: "socket_set_timeout($stream, int $seconds, int $microseconds = 0): bool" => socket_set_timeout;
}

pub fn register(r: &mut Registry) {
    r.functions(ALL);
}

#[cfg(test)]
mod tests;
