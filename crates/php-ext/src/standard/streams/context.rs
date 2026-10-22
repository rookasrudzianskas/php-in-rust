//! Stream contexts (`php_stream_context`): a `"stream-context"` resource
//! holding the `[wrapper => [option => value]]` array and the notification
//! callback.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::{PResult, PhpArray, PhpStr, ResRef, Value, Vm};

/// `php_stream_context`.
#[derive(Default)]
pub struct Context {
    pub options: PhpArray,
    /// `params["notification"]` (the notifier callback).
    pub notifier: Option<Value>,
}

pub type ContextHandle = Rc<RefCell<Context>>;

/// `php_stream_context_alloc`.
pub fn alloc() -> ResRef {
    ResRef::new(
        "stream-context",
        Box::new(Rc::new(RefCell::new(Context::default()))),
    )
}

/// The context behind a `"stream-context"` resource.
pub fn of(res: &ResRef) -> Option<ContextHandle> {
    if res.is_closed() || res.type_name() != "stream-context" {
        return None;
    }
    let data = res.resource().data.borrow();
    data.as_ref()?.downcast_ref::<ContextHandle>().cloned()
}

/// `FG(default_context)`, created on first use.
pub fn default_context(vm: &mut Vm) -> ResRef {
    let g = super::wrappers::globals(vm);
    if let Some(c) = &g.default_context {
        return c.clone();
    }
    let c = alloc();
    g.default_context = Some(c.clone());
    c
}

/// `php_stream_context_from_zval(zcontext, nocontext)`.
pub fn from_zval(vm: &mut Vm, v: &Value, no_default: bool) -> PResult<Option<ResRef>> {
    match &*v.deref() {
        Value::Null | Value::Undef => {
            if no_default {
                Ok(None)
            } else {
                Ok(Some(default_context(vm)))
            }
        }
        Value::Resource(r) => {
            if of(r).is_some() {
                Ok(Some(r.clone()))
            } else {
                let f = vm.current_function_display_name();
                Err(vm.throw_type_error(format!(
                    "{f}(): supplied resource is not a valid Stream-Context resource"
                )))
            }
        }
        _ => {
            let f = vm.current_function_display_name();
            Err(vm.throw_type_error(format!(
                "{f}(): supplied argument is not a valid Stream-Context resource"
            )))
        }
    }
}

/// `php_stream_context_get_option`.
pub fn get_option(ctx: &ResRef, wrapper: &[u8], option: &[u8]) -> Option<Value> {
    let c = of(ctx)?;
    let c = c.borrow();
    match c.options.get_str(wrapper)?.deref().clone() {
        Value::Array(a) => a.get_str(option).map(Value::unref),
        _ => None,
    }
}

/// `php_stream_context_set_option`.
pub fn set_option(ctx: &ResRef, wrapper: &[u8], option: &[u8], value: Value) {
    let Some(c) = of(ctx) else {
        return;
    };
    let mut c = c.borrow_mut();
    let key = php_runtime::ArrayKey::from_bytes(wrapper);
    let mut inner = match c.options.get(&key).map(Value::unref) {
        Some(Value::Array(a)) => (*a).clone(),
        _ => PhpArray::new(),
    };
    inner.insert(php_runtime::ArrayKey::from_bytes(option), value.unref());
    c.options.insert(key, Value::from(inner));
}

/// Option lookup through an optional context.
pub fn option(ctx: Option<&ResRef>, wrapper: &str, option: &str) -> Option<Value> {
    ctx.and_then(|c| get_option(c, wrapper.as_bytes(), option.as_bytes()))
}

pub fn label() -> PhpStr {
    PhpStr::from("stream-context")
}
