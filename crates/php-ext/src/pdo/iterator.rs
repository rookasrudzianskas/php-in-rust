//! Statement iteration: `pdo_stmt_iter_get` and its iterator functions
//! (`struct php_pdo_iterator`), used by `foreach` over a statement (the
//! `get_iterator` handler) and wrapped in the runtime's `InternalIterator`
//! by `PDOStatement::getIterator()` (`zend_create_internal_iterator_zval`).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use php_runtime::{ObjRef, PResult, Registry, Value, Vm};

use super::consts::{FETCH_ORI_NEXT, FETCH_USE_DEFAULT};
use super::error::handle_stmt_err;
use super::finish;
use super::stmt::{do_fetch, stmt_of};
use crate::internal_iterator::{ObjectIterator, by_ref_error, create_internal_iterator, iterator_any};

/// `struct php_pdo_iterator`.
struct StmtIterator {
    stmt_obj: ObjRef,
    /// `I->key`; `None` is `(zend_ulong)-1`.
    key: Cell<Option<u64>>,
    fetch_ahead: RefCell<Option<Value>>,
}

impl StmtIterator {
    /// Advances to the next row (`pdo_stmt_iter_move_forwards`); the first
    /// call happens when the iterator is created (`pdo_stmt_iter_get`).
    fn fetch_next(&self, vm: &mut Vm) -> PResult<()> {
        self.fetch_ahead.replace(None);
        let Some(stmt) = stmt_of(&self.stmt_obj) else {
            return Ok(());
        };
        let Some(dbh) = stmt.dbh() else {
            return Ok(());
        };
        let r = (|| -> PResult<()> {
            match do_fetch(
                vm,
                &self.stmt_obj,
                &stmt,
                &dbh,
                FETCH_USE_DEFAULT,
                FETCH_ORI_NEXT,
                0,
                None,
            )? {
                Some(v) => {
                    self.fetch_ahead.replace(Some(v));
                    self.key.set(Some(self.key.get().map_or(0, |k| k.wrapping_add(1))));
                }
                None => {
                    handle_stmt_err(vm, &dbh, &stmt)?;
                    self.key.set(None);
                    self.fetch_ahead.replace(None);
                }
            }
            Ok(())
        })();
        finish(vm, Some(&dbh), r.map(|_| Value::Null)).map(|_| ())
    }
}

impl ObjectIterator for StmtIterator {
    fn valid(&self, _vm: &mut Vm) -> PResult<bool> {
        Ok(self.fetch_ahead.borrow().is_some())
    }

    fn current(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        Ok(self.fetch_ahead.borrow().clone())
    }

    fn key(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        Ok(Some(self.key.get().map_or(Value::Null, |k| Value::Int(k as i64))))
    }

    fn move_forward(&self, vm: &mut Vm) -> PResult<()> {
        self.fetch_next(vm)
    }

    /// PDO statement iterators have no rewind function.
    fn rewind(&self, _vm: &mut Vm) -> PResult<()> {
        Ok(())
    }

    fn can_rewind(&self) -> bool {
        false
    }
}

/// `pdo_stmt_iter_get`.
fn stmt_iterator(vm: &mut Vm, stmt_obj: &ObjRef) -> PResult<Rc<StmtIterator>> {
    if stmt_of(stmt_obj).filter(|s| s.dbh().is_some()).is_none() {
        return Err(vm.throw_error(
            "Error",
            format!("{} object is uninitialized", stmt_obj.class().name),
        ));
    }
    let it = Rc::new(StmtIterator {
        stmt_obj: stmt_obj.clone(),
        key: Cell::new(None),
        fetch_ahead: RefCell::new(None),
    });
    // The fetch-ahead starts with key -1 + 1 = 0.
    it.fetch_next(vm)?;
    Ok(it)
}

/// The `get_iterator` handler of `PDOStatement`.
pub fn get_iterator(vm: &mut Vm, stmt_obj: &ObjRef, by_ref: bool) -> PResult<Rc<dyn Any>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    let it: Rc<dyn ObjectIterator> = stmt_iterator(vm, stmt_obj)?;
    Ok(iterator_any(it))
}

/// `PDOStatement::getIterator()` (`zend_create_internal_iterator_zval`).
pub fn create(vm: &mut Vm, stmt_obj: &ObjRef) -> PResult<ObjRef> {
    let it: Rc<dyn ObjectIterator> = stmt_iterator(vm, stmt_obj)?;
    create_internal_iterator(vm, it)
}

/// Makes sure the runtime's `InternalIterator` class is registered.
pub fn register(r: &mut Registry) {
    crate::internal_iterator::register(r);
}
