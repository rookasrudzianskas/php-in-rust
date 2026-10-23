//! Variable scopes (`zend_execute_data` CV slots + the lazily built symbol
//! table of `zend_rebuild_symbol_table`).
//!
//! An [`Env`] owns the variable slots of one function activation (or of the
//! global scope). Slots `0..n` belong to the compiled variables of the
//! function; variables created dynamically (`$$x`, `extract()`, included
//! files) are appended with their names recorded, so name lookups and
//! `get_defined_vars()` see insertion order like php-src's symbol table.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::class::{ClassRef, Function};
use crate::compile::ir::CvTable;
use crate::object::ObjRef;
use crate::value::{PhpArray, PhpRef, PhpStr, Value};

/// Variable names of an [`Env`] beyond the function's compiled variables.
#[derive(Default)]
struct Extra {
    names: Vec<PhpStr>,
    index: HashMap<PhpStr, u32>,
}

/// A variable scope.
pub struct Env {
    pub vars: RefCell<Vec<Value>>,
    base: Rc<CvTable>,
    extra: RefCell<Extra>,
    pub this: Option<ObjRef>,
    /// Class scope (visibility, `self::`).
    pub scope: Option<ClassRef>,
    /// Called scope (`static::`).
    pub called: Option<ClassRef>,
    /// The function being executed (`None` for the global scope).
    pub func: Option<Rc<Function>>,
    /// Number of arguments passed (`ZEND_CALL_NUM_ARGS`).
    pub num_args: Cell<u32>,
    /// Arguments beyond the declared parameters (`func_get_args`).
    pub extra_args: RefCell<Vec<Value>>,
    /// Number of declared (non-variadic) parameters.
    pub num_params: u32,
    /// Strict types of the code running in this scope.
    pub strict: bool,
    /// The (non-fake) closure being executed (`Closure::getCurrent()`).
    pub closure: Option<crate::object::WeakObjRef>,
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Env({} vars)", self.vars.borrow().len())
    }
}

impl Env {
    pub fn new(base: Rc<CvTable>) -> Env {
        let n = base.len();
        Env {
            vars: RefCell::new(vec![Value::Undef; n]),
            base,
            extra: RefCell::new(Extra::default()),
            this: None,
            scope: None,
            called: None,
            func: None,
            num_args: Cell::new(0),
            extra_args: RefCell::new(Vec::new()),
            num_params: 0,
            strict: false,
            closure: None,
        }
    }

    /// The global scope: no compiled variables, everything is "extra".
    pub fn global() -> Env {
        Env::new(Rc::new(CvTable::default()))
    }

    /// Slot of the named variable, if it exists.
    pub fn lookup(&self, name: &[u8]) -> Option<u32> {
        if let Some(s) = self.base.lookup(name) {
            return Some(s);
        }
        self.extra.borrow().index.get(name).copied()
    }

    /// Slot of the named variable, created (undefined) if missing.
    pub fn slot_for(&self, name: &[u8]) -> u32 {
        if let Some(s) = self.lookup(name) {
            return s;
        }
        let mut vars = self.vars.borrow_mut();
        let s = vars.len() as u32;
        vars.push(Value::Undef);
        let mut ex = self.extra.borrow_mut();
        let n = PhpStr::from_bytes(name);
        ex.names.push(n.clone());
        ex.index.insert(n, s);
        s
    }

    /// Name of slot `s`.
    pub fn name_of(&self, s: u32) -> PhpStr {
        let n = self.base.len() as u32;
        if s < n {
            self.base.names[s as usize].clone()
        } else {
            self.extra.borrow().names[(s - n) as usize].clone()
        }
    }

    /// Reads a slot (dereferenced; `Undef` if unset).
    #[inline]
    pub fn get(&self, s: u32) -> Value {
        match &self.vars.borrow()[s as usize] {
            Value::Ref(r) => r.get(),
            v => v.clone(),
        }
    }

    /// Whether the slot holds a value other than `Undef`.
    pub fn is_set(&self, s: u32) -> bool {
        match &self.vars.borrow()[s as usize] {
            Value::Undef => false,
            Value::Ref(r) => !r.borrow().is_undef(),
            _ => true,
        }
    }

    /// Assigns to a slot, writing through a reference.
    #[inline]
    pub fn set(&self, s: u32, v: Value) -> Value {
        let mut vars = self.vars.borrow_mut();
        let slot = &mut vars[s as usize];
        match slot {
            Value::Ref(r) => r.set(v),
            _ => std::mem::replace(slot, v),
        }
    }

    /// Binds a slot to a reference (`$a = &...`).
    pub fn bind_ref(&self, s: u32, r: PhpRef) -> Value {
        let mut vars = self.vars.borrow_mut();
        std::mem::replace(&mut vars[s as usize], Value::Ref(r))
    }

    /// Makes slot `s` a reference (creating it as `null` if unset) and returns it.
    pub fn make_ref(&self, s: u32) -> PhpRef {
        let mut vars = self.vars.borrow_mut();
        let slot = &mut vars[s as usize];
        match slot {
            Value::Ref(r) => r.clone(),
            other => {
                let v = std::mem::take(other);
                let r = PhpRef::new(if v.is_undef() { Value::Null } else { v });
                *other = Value::Ref(r.clone());
                r
            }
        }
    }

    /// `unset($x)`: returns the old value so it is dropped outside the borrow.
    pub fn unset(&self, s: u32) -> Value {
        let mut vars = self.vars.borrow_mut();
        std::mem::take(&mut vars[s as usize])
    }

    /// The symbol table (`get_defined_vars()` order), skipping unset variables.
    pub fn defined_vars(&self) -> Vec<(PhpStr, Value)> {
        let vars = self.vars.borrow();
        let mut out = Vec::with_capacity(vars.len());
        for (i, v) in vars.iter().enumerate() {
            let defined = match v {
                Value::Undef => false,
                Value::Ref(r) => !r.borrow().is_undef(),
                _ => true,
            };
            if defined {
                out.push((self.name_of(i as u32), v.clone()));
            }
        }
        out
    }

    /// The symbol table as a PHP array (values copied, references kept).
    pub fn to_array(&self) -> PhpArray {
        let mut a = PhpArray::new();
        for (k, v) in self.defined_vars() {
            a.insert(crate::value::ArrayKey::from_php_str(k), v);
        }
        a
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.vars.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The compiled-variable table of the function.
    pub fn base(&self) -> &Rc<CvTable> {
        &self.base
    }

    /// Arguments for backtraces (`debug_backtrace_get_args`): current values
    /// of the declared parameters, the variadic arguments expanded (named
    /// ones with their names), then extra arguments.
    pub fn trace_args(&self, func: Option<&Function>) -> Vec<(Option<PhpStr>, Value)> {
        let n = self.num_args.get() as usize;
        let np = self.num_params as usize;
        let mut out = Vec::with_capacity(n);
        let vars = self.vars.borrow();
        for v in vars.iter().take(np.min(n)) {
            out.push((None, v.unref().or_null()));
        }
        let variadic = func.is_some_and(|f| f.signature().variadic().is_some());
        if variadic {
            if let Some(Value::Array(a)) = vars.get(np).map(Value::unref) {
                for (k, v) in a.iter() {
                    match k {
                        crate::value::ArrayKey::Str(s) => out.push((Some(s), v.unref())),
                        crate::value::ArrayKey::Int(_) => out.push((None, v.unref())),
                    }
                }
            }
        }
        drop(vars);
        out.extend(self.extra_args.borrow().iter().map(|v| (None, v.unref())));
        out
    }

    /// Arguments as seen by `func_get_args()` / backtraces: the current values
    /// of the declared parameters followed by the extra arguments.
    pub fn args(&self) -> Vec<Value> {
        let n = self.num_args.get() as usize;
        let np = (self.num_params as usize).min(n);
        let mut out = Vec::with_capacity(n);
        {
            let vars = self.vars.borrow();
            for v in vars.iter().take(np) {
                out.push(match v {
                    Value::Ref(r) => r.get().or_null(),
                    v => v.clone().or_null(),
                });
            }
        }
        out.extend(self.extra_args.borrow().iter().map(Value::unref));
        out
    }
}
