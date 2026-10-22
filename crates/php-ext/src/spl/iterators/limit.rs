//! `LimitIterator`: a window (`offset`, `count`) over its inner iterator.
//! Seeking uses the inner iterator's `seek()` when it is a
//! `SeekableIterator`, and `rewind()`/`next()` otherwise
//! (`spl_limit_it_seek`).

use php_runtime::{Args, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

use super::dual::{self, DitType, checked, with};

/// `spl_limit_it_relative_pos_for` (`pos - offset`, which cannot overflow:
/// both are non-negative).
fn relative_pos_for(offset: i64, pos: i64) -> i64 {
    pos.wrapping_sub(offset)
}

/// Whether the current position is inside the window.
fn in_window(obj: &ObjRef) -> bool {
    with(obj, |s| {
        s.limit_count == -1 || relative_pos_for(s.limit_offset, s.pos) < s.limit_count
    })
}

/// `spl_limit_it_valid`.
fn limit_valid(vm: &mut Vm, obj: &ObjRef) -> PResult<bool> {
    if !in_window(obj) {
        return Ok(false);
    }
    dual::valid(vm, obj)
}

/// `spl_limit_it_seek`.
fn seek(vm: &mut Vm, obj: &ObjRef, pos: i64) -> PResult<()> {
    dual::free(obj);
    let (offset, count, cur, inner_ce, inner) = with(obj, |s| {
        (
            s.limit_offset,
            s.limit_count,
            s.pos,
            s.inner_ce.clone(),
            s.inner_obj.clone(),
        )
    });
    if pos < offset {
        return Err(vm.throw_error(
            "OutOfBoundsException",
            format!("Cannot seek to {pos} which is below the offset {offset}"),
        ));
    }
    if relative_pos_for(offset, pos) >= count && count != -1 {
        return Err(vm.throw_error(
            "OutOfBoundsException",
            format!("Cannot seek to {pos} which is behind offset {offset} plus count {count}"),
        ));
    }
    let seekable = inner_ce
        .as_ref()
        .is_some_and(|ce| ce.instance_of_name(b"SeekableIterator"));
    if pos != cur && seekable {
        dual::free(obj);
        if let Some(inner) = inner {
            vm.call_method(&inner, b"seek", vec![Value::Int(pos)])?;
        }
        with(obj, |s| s.pos = pos);
        if limit_valid(vm, obj)? {
            dual::fetch(vm, obj, false)?;
        }
    } else {
        // Emulate the forward seek with next() calls; a backward seek
        // starts with a rewind().
        if pos < cur {
            dual::rewind(vm, obj)?;
        }
        while pos > with(obj, |s| s.pos) && dual::valid(vm, obj)? {
            dual::next(vm, obj, true)?;
        }
        if dual::valid(vm, obj)? {
            dual::fetch(vm, obj, true)?;
        }
    }
    Ok(())
}

fn li_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dual::construct(vm, args, "LimitIterator", "Iterator", DitType::Limit)?;
    Ok(Value::Null)
}

/// Rewinds the inner iterator and seeks to the offset.
fn li_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    dual::rewind(vm, &this)?;
    let offset = with(&this, |s| s.limit_offset);
    seek(vm, &this, offset)?;
    Ok(Value::Null)
}

fn li_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let has_data = with(&this, |s| s.data.is_some());
    Ok(Value::Bool(in_window(&this) && has_data))
}

fn li_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    dual::next(vm, &this, true)?;
    if in_window(&this) {
        dual::fetch(vm, &this, true)?;
    }
    Ok(Value::Null)
}

fn li_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pos = args.int(0);
    let this = checked(vm, args)?;
    seek(vm, &this, pos)?;
    Ok(Value::Int(with(&this, |s| s.pos)))
}

fn li_get_position(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Int(with(&this, |s| s.pos)))
}

static LI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator, int $offset = 0, int $limit = -1) {}",
    handler: li_construct,
};
static LI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: li_rewind,
};
static LI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: li_valid,
};
static LI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: li_next,
};
static LI_SEEK: NativeFunction = NativeFunction {
    sig: "public function seek(int $offset): int {}",
    handler: li_seek,
};
static LI_GET_POSITION: NativeFunction = NativeFunction {
    sig: "public function getPosition(): int {}",
    handler: li_get_position,
};

pub fn register(r: &mut Registry) {
    dual::subclass(
        r,
        "LimitIterator",
        0,
        "IteratorIterator",
        &[],
        &[
            &LI_CONSTRUCT,
            &LI_REWIND,
            &LI_VALID,
            &LI_NEXT,
            &LI_SEEK,
            &LI_GET_POSITION,
        ],
    );
}
