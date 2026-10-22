//! Type variance and method compatibility checks used while linking classes
//! (`zend_inheritance.c`: `zend_perform_covariant_type_check`,
//! `zend_do_perform_implementation_check`, `unlinked_instanceof`, ...).
//!
//! Lookups see the class being linked (it is not registered yet) through the
//! [`Linking`] context pushed by `link_class`. Classes that cannot be found
//! make a check `Unresolved`; their names are collected like php-src's
//! `CG(delayed_autoloads)` for the "Could not check compatibility" error.

use std::cell::RefCell;
use std::rc::Rc;

use crate::class::{ClassRef, Function, class_flags};
use crate::types::{ClassType, TypeDecl, mask};
use crate::value::PhpStr;
use crate::vm::Vm;

/// `inheritance_status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inh {
    Success,
    Error,
    Warning,
    Unresolved,
}

/// The class currently being linked.
pub struct Linking {
    pub name: PhpStr,
    pub parent: Option<ClassRef>,
    /// Interfaces known so far (direct ones and their parents).
    pub interfaces: Vec<ClassRef>,
    pub flags: u32,
    /// Names of classes that were not available (`CG(delayed_autoloads)`).
    pub unresolved: Vec<PhpStr>,
    /// Checks delayed until the unresolved classes are loaded.
    pub obligations: Vec<Obligation>,
}

/// A delayed variance check (`variance_obligation`).
pub enum Obligation {
    Method {
        child: Rc<Function>,
        child_scope: PhpStr,
        parent: Rc<Function>,
        parent_scope: PhpStr,
    },
    Property {
        class: PhpStr,
        prop: PhpStr,
        child_ty: TypeDecl,
        parent_ty: TypeDecl,
        parent_class: PhpStr,
    },
    Constant {
        child_class: PhpStr,
        child_ty: Option<TypeDecl>,
        parent_class: PhpStr,
        parent_ty: TypeDecl,
        name: PhpStr,
    },
}

thread_local! {
    static STACK: RefCell<Vec<Linking>> = const { RefCell::new(Vec::new()) };
}

pub fn push(l: Linking) {
    STACK.with(|s| s.borrow_mut().push(l));
}

pub fn pop() -> Option<Linking> {
    STACK.with(|s| s.borrow_mut().pop())
}

/// Runs `f` on the innermost linking context.
pub fn with<R>(f: impl FnOnce(&mut Linking) -> R) -> Option<R> {
    STACK.with(|s| s.borrow_mut().last_mut().map(f))
}

pub fn set_parent(p: Option<ClassRef>) {
    with(|l| l.parent = p);
}

pub fn add_interface(i: &ClassRef) {
    with(|l| {
        if !l.interfaces.iter().any(|x| x.lc_name == i.lc_name) {
            l.interfaces.push(i.clone());
        }
    });
}

/// Takes the collected unresolved class names.
pub fn take_unresolved() -> Vec<PhpStr> {
    with(|l| std::mem::take(&mut l.unresolved)).unwrap_or_default()
}

/// The first unresolved class name.
pub fn first_unresolved() -> Option<PhpStr> {
    with(|l| l.unresolved.first().cloned()).flatten()
}

/// A class as seen by inheritance checks.
#[derive(Clone)]
enum Cls {
    Reg(ClassRef),
    /// The class being linked.
    Cur,
}

/// Normalised view of a type: a pure intersection lists its names as items.
struct View<'a> {
    mask: u32,
    is_intersection: bool,
    items: Vec<Item<'a>>,
}

enum Item<'a> {
    Name(&'a PhpStr),
    Inter(&'a [PhpStr]),
}

fn view(t: &TypeDecl) -> View<'_> {
    if t.mask == 0 && t.classes.len() == 1 {
        if let ClassType::Intersection(ns) = &t.classes[0] {
            return View {
                mask: 0,
                is_intersection: true,
                items: ns.iter().map(Item::Name).collect(),
            };
        }
    }
    View {
        mask: t.mask,
        is_intersection: false,
        items: t
            .classes
            .iter()
            .map(|c| match c {
                ClassType::Name(n) => Item::Name(n),
                ClassType::Intersection(ns) => Item::Inter(ns),
            })
            .collect(),
    }
}

fn inter_view(ns: &[PhpStr]) -> View<'_> {
    View {
        mask: 0,
        is_intersection: true,
        items: ns.iter().map(Item::Name).collect(),
    }
}

/// Checker bound to a VM and the current linking context.
pub struct Checker<'a> {
    pub vm: &'a Vm,
    cur_name: PhpStr,
    cur_parent: Option<ClassRef>,
    cur_interfaces: Vec<ClassRef>,
    cur_flags: u32,
    pub unresolved: Vec<PhpStr>,
}

impl<'a> Checker<'a> {
    /// A checker for the innermost linking context (or none).
    pub fn new(vm: &'a Vm) -> Checker<'a> {
        let (cur_name, cur_parent, cur_interfaces, cur_flags) =
            with(|l| (l.name.clone(), l.parent.clone(), l.interfaces.clone(), l.flags))
                .unwrap_or((PhpStr::new(), None, Vec::new(), 0));
        Checker {
            vm,
            cur_name,
            cur_parent,
            cur_interfaces,
            cur_flags,
            unresolved: Vec::new(),
        }
    }

    /// Records the unresolved names in the linking context.
    pub fn finish(self) {
        let names = self.unresolved;
        if names.is_empty() {
            return;
        }
        with(|l| {
            for n in names {
                if !l.unresolved.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    l.unresolved.push(n);
                }
            }
        });
    }

    fn is_cur(&self, name: &[u8]) -> bool {
        !self.cur_name.is_empty() && self.cur_name.eq_ignore_ascii_case(name)
    }

    fn scope_parent_name(&self, scope: &[u8]) -> Option<PhpStr> {
        if self.is_cur(scope) {
            return self.cur_parent.as_ref().map(|p| p.name.clone());
        }
        self.vm.find_class(scope).and_then(|c| c.parent.as_ref().map(|p| p.name.clone()))
    }

    /// `resolve_class_name` (zend_inheritance.c).
    pub fn resolve(&self, scope: &[u8], name: &PhpStr) -> PhpStr {
        if name.eq_ignore_ascii_case(b"parent") {
            if let Some(p) = self.scope_parent_name(scope) {
                return p;
            }
        } else if name.eq_ignore_ascii_case(b"self") && !scope.is_empty() {
            return PhpStr::from_bytes(scope);
        }
        name.clone()
    }

    fn lookup(&self, name: &[u8]) -> Option<Cls> {
        if self.is_cur(name) {
            return Some(Cls::Cur);
        }
        self.vm.find_class(name).map(Cls::Reg)
    }

    fn same(a: &Cls, b: &Cls) -> bool {
        match (a, b) {
            (Cls::Cur, Cls::Cur) => true,
            (Cls::Reg(x), Cls::Reg(y)) => Rc::ptr_eq(x, y) || x.lc_name == y.lc_name,
            _ => false,
        }
    }

    /// `unlinked_instanceof`.
    fn instanceof(&self, a: &Cls, b: &Cls) -> bool {
        if Self::same(a, b) {
            return true;
        }
        match (a, b) {
            (Cls::Reg(x), Cls::Reg(y)) => x.instance_of(y),
            (Cls::Reg(_), Cls::Cur) => false,
            (Cls::Cur, Cls::Reg(y)) => {
                self.cur_parent.as_ref().is_some_and(|p| p.instance_of(y))
                    || self.cur_interfaces.iter().any(|i| i.instance_of(y))
            }
            (Cls::Cur, Cls::Cur) => true,
        }
    }

    fn register_unresolved(&mut self, scope: &[u8], t: &TypeDecl) {
        let mut names: Vec<PhpStr> = Vec::new();
        for c in &t.classes {
            match c {
                ClassType::Name(n) => names.push(self.resolve(scope, n)),
                ClassType::Intersection(ns) => {
                    for n in ns {
                        names.push(self.resolve(scope, n));
                    }
                }
            }
        }
        for n in names {
            if self.lookup(&n).is_none() && !self.unresolved.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                self.unresolved.push(n);
            }
        }
    }

    /// `zend_type_permits_self`.
    fn permits_self(&self, t: &TypeDecl, scope: &[u8], self_scope: &[u8]) -> bool {
        if t.mask & mask::OBJECT != 0 {
            return true;
        }
        let v = view(t);
        let Some(self_cls) = self.lookup(self_scope) else { return false };
        for it in &v.items {
            if let Item::Name(n) = it {
                let name = self.resolve(scope, n);
                if let Some(ce) = self.lookup(&name) {
                    if self.instanceof(&self_cls, &ce) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn scope_is_final(&self, scope: &[u8]) -> bool {
        if self.is_cur(scope) {
            return self.cur_flags & class_flags::FINAL != 0;
        }
        self.vm.find_class(scope).is_some_and(|c| c.has_flag(class_flags::FINAL))
    }

    /// `zend_is_intersection_subtype_of_class`.
    fn inter_subtype_of_class(&self, fe_scope: &[u8], fe: &[PhpStr], proto_scope: &[u8], proto_name: &PhpStr) -> Inh {
        let _ = proto_scope;
        let mut have_unresolved = false;
        for n in fe {
            let fe_name = self.resolve(fe_scope, n);
            if fe_name.eq_ignore_ascii_case(proto_name) {
                return Inh::Success;
            }
            let proto_ce = self.lookup(proto_name);
            let fe_ce = self.lookup(&fe_name);
            let (Some(fe_ce), Some(proto_ce)) = (fe_ce, proto_ce) else {
                have_unresolved = true;
                continue;
            };
            if self.instanceof(&fe_ce, &proto_ce) {
                return Inh::Success;
            }
        }
        if have_unresolved { Inh::Unresolved } else { Inh::Error }
    }

    /// `zend_is_class_subtype_of_type`.
    fn class_subtype_of_type(&self, fe_scope: &[u8], fe_name: &PhpStr, proto_scope: &[u8], proto: &View<'_>) -> Inh {
        let mut fe_ce: Option<Cls> = None;
        let mut have_unresolved = false;
        if proto.mask & mask::OBJECT != 0 {
            if fe_ce.is_none() {
                fe_ce = self.lookup(fe_name);
            }
            if fe_ce.is_none() {
                have_unresolved = true;
            } else {
                return Inh::Success;
            }
        }
        if proto.mask & mask::CALLABLE != 0 {
            if fe_ce.is_none() {
                fe_ce = self.lookup(fe_name);
            }
            match &fe_ce {
                None => have_unresolved = true,
                Some(Cls::Reg(c)) if c.lc_name.as_bytes() == b"closure" => return Inh::Success,
                _ => {}
            }
        }
        if proto.mask & mask::STATIC != 0 && self.scope_is_final(fe_scope) {
            if fe_ce.is_none() {
                fe_ce = self.lookup(fe_name);
            }
            match &fe_ce {
                None => have_unresolved = true,
                Some(c) => {
                    if self.lookup(fe_scope).is_some_and(|s| Self::same(c, &s)) {
                        return Inh::Success;
                    }
                }
            }
        }
        let is_intersection = proto.is_intersection;
        for it in &proto.items {
            match it {
                Item::Inter(ns) => {
                    let sub = inter_view(ns);
                    match self.class_subtype_of_type(fe_scope, fe_name, proto_scope, &sub) {
                        Inh::Error => {
                            if is_intersection {
                                return Inh::Error;
                            }
                        }
                        Inh::Unresolved => have_unresolved = true,
                        Inh::Success => {
                            if !is_intersection {
                                return Inh::Success;
                            }
                        }
                        Inh::Warning => {}
                    }
                }
                Item::Name(n) => {
                    let proto_name = self.resolve(proto_scope, n);
                    if fe_name.eq_ignore_ascii_case(&proto_name) {
                        if !is_intersection {
                            return Inh::Success;
                        }
                        continue;
                    }
                    if fe_ce.is_none() {
                        fe_ce = self.lookup(fe_name);
                    }
                    let proto_ce = self.lookup(&proto_name);
                    let (Some(f), Some(p)) = (&fe_ce, &proto_ce) else {
                        have_unresolved = true;
                        continue;
                    };
                    if self.instanceof(f, p) {
                        if !is_intersection {
                            return Inh::Success;
                        }
                    } else if is_intersection {
                        return Inh::Error;
                    }
                }
            }
        }
        if have_unresolved {
            return Inh::Unresolved;
        }
        if is_intersection { Inh::Success } else { Inh::Error }
    }

    /// `zend_is_intersection_subtype_of_type`.
    fn inter_subtype_of_type(&self, fe_scope: &[u8], fe: &[PhpStr], proto_scope: &[u8], proto: &View<'_>) -> Inh {
        let mut have_unresolved = false;
        if proto.mask & mask::OBJECT != 0 {
            for n in fe {
                let fe_name = self.resolve(fe_scope, n);
                if self.lookup(&fe_name).is_some() {
                    return Inh::Success;
                }
                have_unresolved = true;
            }
        }
        let early_exit = if proto.is_intersection { Inh::Error } else { Inh::Success };
        for it in &proto.items {
            let status = match it {
                Item::Inter(ns) => self.inter_subtype_of_type(fe_scope, fe, proto_scope, &inter_view(ns)),
                Item::Name(n) => {
                    let proto_name = self.resolve(proto_scope, n);
                    self.inter_subtype_of_class(fe_scope, fe, proto_scope, &proto_name)
                }
            };
            if status == early_exit {
                return status;
            }
            if status == Inh::Unresolved {
                have_unresolved = true;
            }
        }
        if have_unresolved {
            return Inh::Unresolved;
        }
        if early_exit == Inh::Error { Inh::Success } else { Inh::Error }
    }

    /// `zend_perform_covariant_type_check`: is `fe` (declared in `fe_scope`)
    /// a subtype of `proto` (declared in `proto_scope`)?
    pub fn covariant(&mut self, fe_scope: &[u8], fe: &TypeDecl, proto_scope: &[u8], proto: &TypeDecl) -> Inh {
        if proto.mask == mask::ANY && fe.mask & mask::VOID == 0 {
            return Inh::Success;
        }
        let mut added = fe.mask & !proto.mask;
        if added != 0 {
            if added & mask::STATIC != 0 && self.permits_self(proto, proto_scope, fe_scope) {
                added &= !mask::STATIC;
            }
            if added == mask::NEVER {
                return Inh::Success;
            }
            if added != 0 {
                return Inh::Error;
            }
        }
        let fv = view(fe);
        let pv = view(proto);
        let mut have_unresolved = false;
        let early_exit;
        if fv.is_intersection {
            early_exit = if pv.is_intersection { Inh::Error } else { Inh::Success };
            let names: Vec<PhpStr> = fv
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Name(n) => Some((*n).clone()),
                    _ => None,
                })
                .collect();
            let status = self.inter_subtype_of_type(fe_scope, &names, proto_scope, &pv);
            if status == early_exit {
                return status;
            }
            if status == Inh::Unresolved {
                have_unresolved = true;
            }
        } else {
            early_exit = Inh::Error;
            for it in &fv.items {
                let status = match it {
                    Item::Inter(ns) => self.inter_subtype_of_type(fe_scope, ns, proto_scope, &pv),
                    Item::Name(n) => {
                        let fe_name = self.resolve(fe_scope, n);
                        self.class_subtype_of_type(fe_scope, &fe_name, proto_scope, &pv)
                    }
                };
                if status == early_exit {
                    return status;
                }
                if status == Inh::Unresolved {
                    have_unresolved = true;
                }
            }
        }
        if !have_unresolved {
            return if early_exit == Inh::Error { Inh::Success } else { Inh::Error };
        }
        self.register_unresolved(fe_scope, fe);
        self.register_unresolved(proto_scope, proto);
        Inh::Unresolved
    }

    /// `zend_do_perform_arg_type_hint_check`.
    fn arg_check(&mut self, fe_scope: &[u8], fe: Option<&TypeDecl>, proto_scope: &[u8], proto: Option<&TypeDecl>) -> Inh {
        let Some(ft) = fe else { return Inh::Success };
        if ft.mask == mask::ANY && ft.classes.is_empty() {
            return Inh::Success;
        }
        let Some(pt) = proto else { return Inh::Error };
        self.covariant(proto_scope, pt, fe_scope, ft)
    }

    /// `zend_do_perform_implementation_check`.
    pub fn implementation_check(&mut self, fe: &Function, fe_scope: &[u8], proto: &Function, proto_scope: &[u8]) -> Inh {
        let fs = fe.signature().clone();
        let ps = proto.signature().clone();
        if ps.required < fs.required {
            return Inh::Error;
        }
        if ps.returns_ref && !fs.returns_ref {
            return Inh::Error;
        }
        let pv = ps.variadic().is_some();
        let fv = fs.variadic().is_some();
        if pv && !fv {
            return Inh::Error;
        }
        let pn = ps.params.len();
        let fnn = fs.params.len();
        let n = pn.max(fnn);
        let mut status = Inh::Success;
        for i in 0..n {
            let pa = if i < pn {
                Some(&ps.params[i])
            } else if pv {
                ps.params.last()
            } else {
                None
            };
            let fa = if i < fnn {
                Some(&fs.params[i])
            } else if fv {
                fs.params.last()
            } else {
                None
            };
            let Some(pa) = pa else { continue };
            let Some(fa) = fa else { return Inh::Error };
            match self.arg_check(fe_scope, fa.ty.as_ref(), proto_scope, pa.ty.as_ref()) {
                Inh::Success => {}
                Inh::Error => return Inh::Error,
                _ => status = Inh::Unresolved,
            }
            if fa.by_ref != pa.by_ref {
                return Inh::Error;
            }
        }
        if let Some(prt) = &ps.return_type {
            let tentative = proto.is_native();
            let Some(frt) = &fs.return_type else {
                if !tentative {
                    return Inh::Error;
                }
                if status == Inh::Success {
                    return Inh::Warning;
                }
                return status;
            };
            let local = self.covariant(fe_scope, frt, proto_scope, prt);
            if local != Inh::Success {
                if local == Inh::Error && tentative {
                    return Inh::Warning;
                }
                return local;
            }
        }
        status
    }

    /// `zend_type_to_string_resolved` with `self`/`parent` resolved against `scope`.
    pub fn type_string(&self, t: &TypeDecl, scope: &[u8]) -> String {
        let mut r = t.clone();
        for c in &mut r.classes {
            match c {
                ClassType::Name(n) => *n = self.display_resolve(scope, n),
                ClassType::Intersection(ns) => {
                    for n in ns.iter_mut() {
                        *n = self.display_resolve(scope, n);
                    }
                }
            }
        }
        r.to_type_string()
    }

    fn display_resolve(&self, scope: &[u8], n: &PhpStr) -> PhpStr {
        let r = if scope.is_empty() { n.clone() } else { self.resolve(scope, n) };
        let end = r.iter().position(|&c| c == 0).unwrap_or(r.len());
        PhpStr::from_bytes(&r[..end])
    }
}
