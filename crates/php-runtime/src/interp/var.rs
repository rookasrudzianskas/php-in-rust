//! `var_dump`, `print_r`, `var_export`, `debug_zval_dump`
//! (`ext/standard/var.c`, `zend_print_zval_r` in `Zend/zend.c`).

use std::rc::Rc;

use crate::class::{ClassRef, PropPurpose};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::types::TypeDecl;
use crate::value::array::ArrayKey;
use crate::value::float_fmt::append_double;
use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{PResult, Vm};

/// One entry of an object's debug property listing.
pub struct PropEntry {
    /// Mangled key (`\0A\0x`, `\0*\0x`, `x`) or an integer key.
    pub key: ArrayKey,
    /// `None` for an uninitialised typed property.
    pub value: Option<Value>,
    pub ty: Option<TypeDecl>,
}

/// The properties shown by `var_dump`/`print_r` (`ZEND_PROP_PURPOSE_DEBUG`):
/// `__debugInfo()` / the class's debug handler, or the declared slots
/// (uninitialised typed ones included) followed by dynamic properties.
pub fn debug_properties(vm: &mut Vm, obj: &ObjRef, purpose: PropPurpose, with_uninit: bool) -> PResult<Vec<PropEntry>> {
    let class = obj.class().clone();
    let custom = if purpose == PropPurpose::Debug {
        if let Some(f) = class.handlers.get_debug_info {
            f(vm, obj, purpose)?
        } else if class.magic.debug_info.is_some() {
            let v = vm.call_method(obj, b"__debuginfo", Vec::new())?;
            match v.into_unref() {
                Value::Array(a) => Some((*a).clone()),
                Value::Null => Some(PhpArray::new()),
                _ => {
                    return Err(vm.fatal_error("__debuginfo() must return an array"));
                }
            }
        } else if let Some(f) = class.handlers.get_properties_for {
            f(vm, obj, purpose)?
        } else if super::closure::closure_data(obj).is_some() {
            Some(closure_debug_info(vm, obj))
        } else {
            None
        }
    } else if let Some(f) = class.handlers.get_properties_for {
        f(vm, obj, purpose)?
    } else {
        None
    };
    if let Some(t) = custom {
        return Ok(t
            .iter()
            .map(|(k, v)| PropEntry {
                key: k,
                value: Some(v.clone()),
                ty: None,
            })
            .collect());
    }
    let mut out = Vec::new();
    // Declared slots in slot order (inherited ones first).
    let mut infos: Vec<(usize, PhpStr, Option<TypeDecl>)> = Vec::new();
    let mut ce: Option<ClassRef> = Some(class.clone());
    while let Some(c) = ce {
        let own = std::rc::Rc::ptr_eq(&c, &class);
        for info in c.properties.values() {
            if info.is_static() || infos.iter().any(|(s, _, _)| *s == info.slot) {
                continue;
            }
            // Parents contribute only their private properties (rebuild_object_properties).
            if !own && !(info.flags & crate::class::member_flags::PRIVATE != 0 && info.declaring_class.eq_ignore_ascii_case(&c.name)) {
                continue;
            }
            if info.flags & crate::class::member_flags::VIRTUAL != 0 {
                continue;
            }
            infos.push((info.slot, super::objects::mangled_name(info), info.ty.clone()));
        }
        ce = c.parent.clone();
    }
    infos.sort_by_key(|(s, _, _)| *s);
    {
        let props = obj.obj().props.borrow();
        for (slot, key, ty) in infos {
            let v = props.get(slot).cloned().unwrap_or_default();
            if v.is_undef() {
                if with_uninit && ty.is_some() {
                    out.push(PropEntry {
                        key: ArrayKey::Str(key),
                        value: None,
                        ty,
                    });
                }
                continue;
            }
            out.push(PropEntry {
                key: ArrayKey::Str(key),
                value: Some(v),
                ty,
            });
        }
    }
    if let Some(d) = obj.obj().dyn_props.borrow().as_ref() {
        for (k, v) in d.iter() {
            out.push(PropEntry {
                key: k,
                value: Some(v.clone()),
                ty: None,
            });
        }
    }
    Ok(out)
}

/// `zend_closure_get_debug_info`.
fn closure_debug_info(vm: &mut Vm, obj: &ObjRef) -> PhpArray {
    let _ = vm;
    let c = super::closure::closure_data(obj).expect("closure");
    let mut t = PhpArray::new();
    let user = super::user_fn(&c.func);
    if c.from_callable || user.is_none() {
        let name = match &c.func.scope {
            Some(s) => format!("{}::{}", s, c.func.name),
            None => c.func.name.to_string(),
        };
        t.insert_str("function", name);
    } else {
        let u = user.unwrap();
        t.insert_str("name", Value::String(c.func.name.clone()));
        t.insert_str("file", Value::String(u.code.file.clone()));
        t.insert_str("line", Value::Int(u.code.start_line as i64));
    }
    if user.is_some() {
        let mut statics = PhpArray::new();
        for (name, v) in super::closure::closure_static_vars(&c) {
            statics.insert(ArrayKey::Str(name), v);
        }
        if !statics.is_empty() {
            t.insert_str("static", Value::array(statics));
        }
    }
    if let Some(this) = &c.this {
        t.insert_str("this", Value::Object(this.clone()));
    }
    let sig = c.func.signature().clone();
    if !sig.params.is_empty() {
        let mut params = PhpArray::new();
        for (i, p) in sig.params.iter().enumerate() {
            let name = format!("{}${}", if p.by_ref { "&" } else { "" }, p.name);
            params.insert(
                ArrayKey::from(name.as_str()),
                Value::str(if i >= sig.required { "<optional>" } else { "<required>" }),
            );
        }
        t.insert_str("parameter", Value::array(params));
    }
    t
}

fn spaces(out: &mut Vec<u8>, n: usize) {
    out.extend(std::iter::repeat_n(b' ', n));
}

fn unmangle(key: &[u8]) -> (Option<&[u8]>, &[u8]) {
    if key.first() == Some(&0) {
        if let Some(end) = key[1..].iter().position(|&c| c == 0) {
            return (Some(&key[1..1 + end]), &key[2 + end..]);
        }
    }
    (None, key)
}

// ---------------------------------------------------------------------------
// var_dump
// ---------------------------------------------------------------------------

struct Dumper {
    arrays: Vec<*const PhpArray>,
    objects: Vec<u32>,
    debug_zval: bool,
}

impl Dumper {
    fn dump(&mut self, vm: &mut Vm, v: &Value, level: usize, out: &mut Vec<u8>) -> PResult<()> {
        if level > 1 {
            spaces(out, level - 1);
        }
        let (v, is_ref) = match v {
            Value::Ref(r) => {
                if self.debug_zval {
                    out.extend_from_slice(format!("reference refcount({}) {{\n", r.refcount()).as_bytes());
                    let inner = r.get();
                    self.dump(vm, &inner, level + 2, out)?;
                    if level > 1 {
                        spaces(out, level - 1);
                    }
                    out.extend_from_slice(b"}\n");
                    return Ok(());
                }
                (r.get(), r.refcount() > 1)
            }
            v => (v.clone(), false),
        };
        let amp: &[u8] = if is_ref { b"&" } else { b"" };
        match &v {
            Value::Bool(false) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(b"bool(false)\n");
            }
            Value::Bool(true) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(b"bool(true)\n");
            }
            Value::Null | Value::Undef => {
                out.extend_from_slice(amp);
                out.extend_from_slice(b"NULL\n");
            }
            Value::Int(i) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(format!("int({i})\n").as_bytes());
            }
            Value::Float(d) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(b"float(");
                append_double(out, *d, vm.serialize_precision, false);
                out.extend_from_slice(b")\n");
            }
            Value::String(s) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
                out.extend_from_slice(s);
                if self.debug_zval {
                    out.extend_from_slice(b"\" interned\n");
                } else {
                    out.extend_from_slice(b"\"\n");
                }
            }
            Value::Array(a) => {
                let ptr = Rc::as_ptr(a);
                if self.arrays.contains(&ptr) {
                    out.extend_from_slice(b"*RECURSION*\n");
                    return Ok(());
                }
                self.arrays.push(ptr);
                out.extend_from_slice(amp);
                if self.debug_zval {
                    let packed = if a.is_packed() { "packed " } else { "" };
                    out.extend_from_slice(format!("array({}) {}refcount({}){{\n", a.len(), packed, Rc::strong_count(a).saturating_sub(1).max(1)).as_bytes());
                } else {
                    out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
                }
                let entries: Vec<(ArrayKey, Value)> = a.iter().map(|(k, v)| (k, v.clone())).collect();
                for (k, ev) in entries {
                    spaces(out, level + 1);
                    match &k {
                        ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                        ArrayKey::Str(s) => {
                            out.extend_from_slice(b"[\"");
                            out.extend_from_slice(s);
                            out.extend_from_slice(b"\"]=>\n");
                        }
                    }
                    self.dump(vm, &ev, level + 2, out)?;
                }
                self.arrays.pop();
                if level > 1 {
                    spaces(out, level - 1);
                }
                out.extend_from_slice(b"}\n");
            }
            Value::Object(o) => {
                let class = o.class().clone();
                if class.is_enum() && class.magic.debug_info.is_none() && !self.debug_zval {
                    let case = crate::enums::enum_case_name(o).unwrap_or_default();
                    out.extend_from_slice(amp);
                    out.extend_from_slice(format!("enum({}::{})\n", class.name, case).as_bytes());
                    return Ok(());
                }
                if self.objects.contains(&o.id()) {
                    out.extend_from_slice(b"*RECURSION*\n");
                    return Ok(());
                }
                self.objects.push(o.id());
                let props = debug_properties(vm, o, PropPurpose::Debug, true);
                let props = match props {
                    Ok(p) => p,
                    Err(e) => {
                        self.objects.pop();
                        return Err(e);
                    }
                };
                let count = props.iter().filter(|p| p.value.is_some()).count();
                out.extend_from_slice(amp);
                let shown_name = super::decl::display_class_name(&class.name).into_bytes();
                if class.is_enum() && !self.debug_zval {
                    // An enum with __debugInfo().
                    let case = crate::enums::enum_case_name(o).unwrap_or_default();
                    out.extend_from_slice(format!("enum({}::{}) ({}) {{\n", class.name, case, count).as_bytes());
                } else {
                out.extend_from_slice(b"object(");
                out.extend_from_slice(&shown_name);
                if self.debug_zval {
                    out.extend_from_slice(format!(")#{} ({}) refcount({}){{\n", o.id(), count, o.refcount().saturating_sub(1).max(1)).as_bytes());
                } else {
                    out.extend_from_slice(format!(")#{} ({}) {{\n", o.id(), count).as_bytes());
                }
                }
                for p in props {
                    spaces(out, level + 1);
                    match &p.key {
                        ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                        ArrayKey::Str(s) => {
                            let (cls, name) = unmangle(s);
                            out.push(b'[');
                            match cls {
                                Some(b"*") => {
                                    out.push(b'"');
                                    out.extend_from_slice(name);
                                    out.extend_from_slice(b"\":protected");
                                }
                                Some(c) => {
                                    out.push(b'"');
                                    out.extend_from_slice(name);
                                    out.extend_from_slice(b"\":\"");
                                    out.extend_from_slice(c);
                                    out.extend_from_slice(b"\":private");
                                }
                                None => {
                                    out.push(b'"');
                                    out.extend_from_slice(s);
                                    out.push(b'"');
                                }
                            }
                            out.extend_from_slice(b"]=>\n");
                        }
                    }
                    match &p.value {
                        Some(v) => self.dump(vm, v, level + 2, out)?,
                        None => {
                            spaces(out, level + 1);
                            let t = p.ty.as_ref().map(TypeDecl::to_type_string).unwrap_or_default();
                            out.extend_from_slice(format!("uninitialized({t})\n").as_bytes());
                        }
                    }
                }
                self.objects.pop();
                if level > 1 {
                    spaces(out, level - 1);
                }
                out.extend_from_slice(b"}\n");
            }
            Value::Resource(r) => {
                out.extend_from_slice(amp);
                out.extend_from_slice(format!("resource({}) of type ({})\n", r.id(), r.type_name()).as_bytes());
            }
            Value::Ref(_) => unreachable!(),
        }
        Ok(())
    }
}

/// `php_var_dump` into a buffer.
pub fn var_dump_to(vm: &mut Vm, v: &Value, out: &mut Vec<u8>) -> PResult<()> {
    let mut d = Dumper {
        arrays: Vec::new(),
        objects: Vec::new(),
        debug_zval: false,
    };
    d.dump(vm, v, 1, out)
}

fn var_dump(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let vals: Vec<Value> = a.values().to_vec();
    for v in &vals {
        let mut out = Vec::new();
        let r = var_dump_to(vm, v, &mut out);
        vm.echo(&out)?;
        r?;
    }
    Ok(Value::Null)
}

fn debug_zval_dump(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let vals: Vec<Value> = a.values().to_vec();
    for v in &vals {
        let mut out = Vec::new();
        let mut d = Dumper {
            arrays: Vec::new(),
            objects: Vec::new(),
            debug_zval: true,
        };
        d.dump(vm, v, 1, &mut out)?;
        vm.echo(&out)?;
    }
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// print_r
// ---------------------------------------------------------------------------

struct Printer {
    arrays: Vec<*const PhpArray>,
    objects: Vec<u32>,
}

impl Printer {
    fn print(&mut self, vm: &mut Vm, v: &Value, indent: usize, out: &mut Vec<u8>) -> PResult<()> {
        match &*v.deref() {
            Value::Array(a) => {
                out.extend_from_slice(b"Array\n");
                let ptr = Rc::as_ptr(a);
                if self.arrays.contains(&ptr) {
                    out.extend_from_slice(b" *RECURSION*");
                    return Ok(());
                }
                self.arrays.push(ptr);
                let entries: Vec<(ArrayKey, Value)> = a.iter().map(|(k, v)| (k, v.clone())).collect();
                let r = self.hash(vm, entries.into_iter().map(|(k, v)| (k, Some(v))).collect(), indent, false, out);
                self.arrays.pop();
                r?;
            }
            Value::Object(o) => {
                let class = o.class().clone();
                out.extend_from_slice(super::decl::display_class_name(&class.name).as_bytes());
                if class.is_enum() {
                    out.extend_from_slice(b" Enum");
                    match class.enum_backing {
                        Some(crate::class::EnumBacking::Int) => out.extend_from_slice(b":int"),
                        Some(crate::class::EnumBacking::String) => out.extend_from_slice(b":string"),
                        _ => {}
                    }
                    out.push(b'\n');
                } else {
                    out.extend_from_slice(b" Object\n");
                }
                if self.objects.contains(&o.id()) {
                    out.extend_from_slice(b" *RECURSION*");
                    return Ok(());
                }
                self.objects.push(o.id());
                let props = match debug_properties(vm, o, PropPurpose::Debug, false) {
                    Ok(p) => p,
                    Err(e) => {
                        self.objects.pop();
                        return Err(e);
                    }
                };
                let r = self.hash(vm, props.into_iter().map(|p| (p.key, p.value)).collect(), indent, true, out);
                self.objects.pop();
                r?;
            }
            Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            Value::String(s) => out.extend_from_slice(s),
            other => {
                let s = vm.to_php_string(other)?;
                out.extend_from_slice(&s);
            }
        }
        Ok(())
    }

    fn hash(&mut self, vm: &mut Vm, entries: Vec<(ArrayKey, Option<Value>)>, indent: usize, is_object: bool, out: &mut Vec<u8>) -> PResult<()> {
        spaces(out, indent);
        out.extend_from_slice(b"(\n");
        let inner = indent + 4;
        for (k, v) in entries {
            let Some(v) = v else { continue };
            spaces(out, inner);
            out.push(b'[');
            match &k {
                ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                ArrayKey::Str(s) => {
                    if is_object {
                        let (cls, name) = unmangle(s);
                        out.extend_from_slice(name);
                        match cls {
                            Some(b"*") => out.extend_from_slice(b":protected"),
                            Some(c) => {
                                out.push(b':');
                                out.extend_from_slice(c);
                                out.extend_from_slice(b":private");
                            }
                            None => {}
                        }
                    } else {
                        out.extend_from_slice(s);
                    }
                }
            }
            out.extend_from_slice(b"] => ");
            self.print(vm, &v, inner + 4, out)?;
            out.push(b'\n');
        }
        spaces(out, indent);
        out.extend_from_slice(b")\n");
        Ok(())
    }
}

/// `zend_print_zval_r` into a buffer.
pub fn print_r_to(vm: &mut Vm, v: &Value, out: &mut Vec<u8>) -> PResult<()> {
    let mut p = Printer {
        arrays: Vec::new(),
        objects: Vec::new(),
    };
    p.print(vm, v, 0, out)
}

fn print_r(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let ret = a.bool(1);
    if ret {
        // Output buffering is used for the return mode in php-src.
        let mut out = Vec::new();
        print_r_to(vm, &v, &mut out)?;
        return Ok(Value::String(PhpStr::from_vec(out)));
    }
    let mut out = Vec::new();
    let r = print_r_to(vm, &v, &mut out);
    vm.echo(&out)?;
    r?;
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// var_export
// ---------------------------------------------------------------------------

/// `'` and `\` escaped, NUL bytes as `' . "\0" . '`.
fn export_string(out: &mut Vec<u8>, s: &[u8]) {
    for &c in s {
        match c {
            b'\'' | b'\\' => {
                out.push(b'\\');
                out.push(c);
            }
            0 => out.extend_from_slice(b"' . \"\\0\" . '"),
            c => out.push(c),
        }
    }
}

struct Exporter {
    arrays: Vec<*const PhpArray>,
    objects: Vec<u32>,
}

impl Exporter {
    fn export(&mut self, vm: &mut Vm, v: &Value, level: usize, out: &mut Vec<u8>) -> PResult<()> {
        let v = v.unref();
        match &v {
            Value::Bool(false) => out.extend_from_slice(b"false"),
            Value::Bool(true) => out.extend_from_slice(b"true"),
            Value::Null | Value::Undef => out.extend_from_slice(b"NULL"),
            Value::Int(i) => {
                if *i == i64::MIN {
                    out.extend_from_slice(format!("{}-1", i64::MIN + 1).as_bytes());
                } else {
                    out.extend_from_slice(i.to_string().as_bytes());
                }
            }
            Value::Float(d) => append_double(out, *d, vm.serialize_precision, true),
            Value::String(s) => {
                out.push(b'\'');
                export_string(out, s);
                out.push(b'\'');
            }
            Value::Array(a) => {
                let ptr = Rc::as_ptr(a);
                if self.arrays.contains(&ptr) {
                    out.extend_from_slice(b"NULL");
                    vm.engine_warning("var_export does not handle circular references")?;
                    return Ok(());
                }
                self.arrays.push(ptr);
                if level > 1 {
                    out.push(b'\n');
                    spaces(out, level - 1);
                }
                out.extend_from_slice(b"array (\n");
                let entries: Vec<(ArrayKey, Value)> = a.iter().map(|(k, v)| (k, v.clone())).collect();
                for (k, ev) in entries {
                    spaces(out, level + 1);
                    match &k {
                        ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                        ArrayKey::Str(s) => {
                            out.push(b'\'');
                            export_string(out, s);
                            out.push(b'\'');
                        }
                    }
                    out.extend_from_slice(b" => ");
                    let r = self.export(vm, &ev, level + 2, out);
                    if r.is_err() {
                        self.arrays.pop();
                        return r;
                    }
                    out.extend_from_slice(b",\n");
                }
                self.arrays.pop();
                if level > 1 {
                    spaces(out, level - 1);
                }
                out.push(b')');
            }
            Value::Object(o) => {
                if self.objects.contains(&o.id()) {
                    out.extend_from_slice(b"NULL");
                    vm.engine_warning("var_export does not handle circular references")?;
                    return Ok(());
                }
                self.objects.push(o.id());
                let r = self.export_object(vm, o, level, out);
                self.objects.pop();
                r?;
            }
            Value::Resource(_) => out.extend_from_slice(b"NULL"),
            Value::Ref(_) => unreachable!(),
        }
        Ok(())
    }

    fn export_object(&mut self, vm: &mut Vm, o: &ObjRef, level: usize, out: &mut Vec<u8>) -> PResult<()> {
        let class = o.class().clone();
        let props = debug_properties(vm, o, PropPurpose::VarExport, false)?;
        if level > 1 {
            out.push(b'\n');
            spaces(out, level - 1);
        }
        let is_enum = class.is_enum();
        let is_std = class.lc_name.as_bytes() == b"stdclass";
        if is_std {
            out.extend_from_slice(b"(object) array(\n");
        } else {
            out.push(b'\\');
            out.extend_from_slice(&class.name);
            if is_enum {
                out.extend_from_slice(b"::");
                out.extend_from_slice(&crate::enums::enum_case_name(o).unwrap_or_default());
            } else {
                out.extend_from_slice(b"::__set_state(array(\n");
            }
        }
        if !is_enum {
            for p in props {
                let Some(v) = p.value else { continue };
                spaces(out, level + 2);
                match &p.key {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(s) => {
                        let (_, name) = unmangle(s);
                        out.push(b'\'');
                        export_string(out, name);
                        out.push(b'\'');
                    }
                }
                out.extend_from_slice(b" => ");
                self.export(vm, &v, level + 2, out)?;
                out.extend_from_slice(b",\n");
            }
        }
        if level > 1 && !is_enum {
            spaces(out, level - 1);
        }
        if is_std {
            out.push(b')');
        } else if !is_enum {
            out.extend_from_slice(b"))");
        }
        Ok(())
    }
}

/// `php_var_export_ex` into a buffer.
pub fn var_export_to(vm: &mut Vm, v: &Value, out: &mut Vec<u8>) -> PResult<()> {
    let mut e = Exporter {
        arrays: Vec::new(),
        objects: Vec::new(),
    };
    e.export(vm, v, 1, out)
}

fn var_export(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let ret = a.bool(1);
    let mut out = Vec::new();
    var_export_to(vm, &v, &mut out)?;
    if ret {
        return Ok(Value::String(PhpStr::from_vec(out)));
    }
    vm.echo(&out)?;
    Ok(Value::Null)
}

pub static VAR_DUMP: NativeFunction = NativeFunction {
    sig: "function var_dump(mixed $value, mixed ...$values): void {}",
    handler: var_dump,
};
pub static DEBUG_ZVAL_DUMP: NativeFunction = NativeFunction {
    sig: "function debug_zval_dump(mixed $value, mixed ...$values): void {}",
    handler: debug_zval_dump,
};
pub static PRINT_R: NativeFunction = NativeFunction {
    sig: "function print_r(mixed $value, bool $return = false): string|true {}",
    handler: print_r,
};
pub static VAR_EXPORT: NativeFunction = NativeFunction {
    sig: "function var_export(mixed $value, bool $return = false): ?string {}",
    handler: var_export,
};

/// Registers the dumping functions under `standard`.
pub fn register(r: &mut Registry<'_>) {
    r.functions(&[&VAR_DUMP, &DEBUG_ZVAL_DUMP, &PRINT_R, &VAR_EXPORT]);
}
