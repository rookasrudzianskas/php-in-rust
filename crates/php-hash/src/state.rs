//! Context state export/import compatible with PHP's `HashContext::__serialize`.
//!
//! PHP serialises the raw C context struct of an algorithm according to a
//! "spec string" (see `php_hash_serialize_spec` in `ext/hash/hash.c`):
//! every 32-bit scalar becomes one integer, every 64-bit scalar becomes two
//! integers (low word first), and every run of two or more bytes becomes a
//! string.  This module reproduces that format without any `unsafe` byte
//! images: an algorithm describes its context by *visiting* its fields in C
//! struct order, and the visitor either records them ([`StateWriter`]) or
//! restores them from PHP's representation ([`StateReader`]).  The visitor also
//! tracks the C struct offset of every field so the error codes PHP reports
//! (`BYTE_OFFSET_POS_ERROR - pos`) are reproduced exactly.

use std::fmt;

/// The `magic` value PHP uses for contexts serialised with a spec string.
pub const SERIALIZE_MAGIC_SPEC: i64 = 2;

/// One element of the array PHP produces for a serialised context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateItem {
    /// A PHP integer (always within the `i32` range when produced by us).
    Int(i64),
    /// A PHP string holding a run of raw context bytes.
    Bytes(Vec<u8>),
}

/// Failure while exporting or importing a context (`hash_spec_result`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateError {
    /// `HASH_SPEC_FAILURE` (-1): generic failure, wrong magic or not serialisable.
    Failure,
    /// `WRONG_CONTEXT_SIZE` (-999).
    WrongContextSize,
    /// `BYTE_OFFSET_POS_ERROR - pos` (-1000 - pos): element at C offset `pos` is malformed.
    ByteOffset(usize),
    /// `CONTEXT_VALIDATION_FAILURE` (-2000): the decoded state is not usable.
    Validation,
}

impl StateError {
    /// The numeric code PHP prints in its exception message.
    pub fn code(self) -> i64 {
        match self {
            StateError::Failure => -1,
            StateError::WrongContextSize => -999,
            StateError::ByteOffset(pos) => -1000 - pos as i64,
            StateError::Validation => -2000,
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "hash context state error (code {})", self.code())
    }
}

impl std::error::Error for StateError {}

pub(crate) type StateResult = Result<(), StateError>;

fn align_to(pos: usize, alignment: usize) -> usize {
    let offset = pos & (alignment - 1);
    pos + if offset != 0 { alignment - offset } else { 0 }
}

/// Visits the fields of a context in C struct order.
pub(crate) trait Visitor {
    /// A scalar of `size` bytes (1, 2, 4 or 8).
    fn scalar(&mut self, size: usize, value: &mut u64) -> StateResult;
    /// A run of bytes; one byte is a scalar, two or more are a string.
    fn bytes(&mut self, data: &mut [u8]) -> StateResult;
    /// Bytes that PHP does not serialise (upper-case spec letters).
    fn skip(&mut self, size: usize, align: usize);

    fn u8(&mut self, x: &mut u8) -> StateResult {
        let mut t = u64::from(*x);
        self.scalar(1, &mut t)?;
        *x = t as u8;
        Ok(())
    }

    fn u32(&mut self, x: &mut u32) -> StateResult {
        let mut t = u64::from(*x);
        self.scalar(4, &mut t)?;
        *x = t as u32;
        Ok(())
    }

    fn u64(&mut self, x: &mut u64) -> StateResult {
        self.scalar(8, x)
    }

    fn u32s(&mut self, xs: &mut [u32]) -> StateResult {
        for x in xs {
            self.u32(x)?;
        }
        Ok(())
    }

    fn u64s(&mut self, xs: &mut [u64]) -> StateResult {
        for x in xs {
            self.u64(x)?;
        }
        Ok(())
    }

    /// A 64-bit counter stored as two 32-bit words `count[2]` (low word first).
    fn count32(&mut self, count: &mut u64) -> StateResult {
        let mut lo = *count as u32;
        let mut hi = (*count >> 32) as u32;
        self.u32(&mut lo)?;
        self.u32(&mut hi)?;
        *count = u64::from(lo) | (u64::from(hi) << 32);
        Ok(())
    }

    /// A 128-bit counter stored as two 64-bit words `count[2]` (low word first).
    fn count64(&mut self, count: &mut u128) -> StateResult {
        let mut lo = *count as u64;
        let mut hi = (*count >> 64) as u64;
        self.u64(&mut lo)?;
        self.u64(&mut hi)?;
        *count = u128::from(lo) | (u128::from(hi) << 64);
        Ok(())
    }
}

fn scalar_item(size: usize, value: u64, out: &mut Vec<StateItem>) {
    out.push(StateItem::Int(i64::from(value as u32 as i32)));
    if size == 8 {
        out.push(StateItem::Int(i64::from((value >> 32) as u32 as i32)));
    }
}

/// Records a context as PHP-compatible [`StateItem`]s.
#[derive(Default)]
pub(crate) struct StateWriter {
    items: Vec<StateItem>,
    pos: usize,
}

impl StateWriter {
    pub(crate) fn finish(self) -> Vec<StateItem> {
        self.items
    }
}

impl Visitor for StateWriter {
    fn scalar(&mut self, size: usize, value: &mut u64) -> StateResult {
        self.pos = align_to(self.pos, size);
        let masked = match size {
            1 => *value & 0xff,
            2 => *value & 0xffff,
            4 => *value & 0xffff_ffff,
            _ => *value,
        };
        scalar_item(size, masked, &mut self.items);
        self.pos += size;
        Ok(())
    }

    fn bytes(&mut self, data: &mut [u8]) -> StateResult {
        if data.len() == 1 {
            let mut t = u64::from(data[0]);
            return self.scalar(1, &mut t);
        }
        self.items.push(StateItem::Bytes(data.to_vec()));
        self.pos += data.len();
        Ok(())
    }

    fn skip(&mut self, size: usize, align: usize) {
        self.pos = align_to(self.pos, align) + size;
    }
}

/// Restores a context from PHP's [`StateItem`] representation.
pub(crate) struct StateReader<'a> {
    items: &'a [StateItem],
    next: usize,
    pos: usize,
}

impl<'a> StateReader<'a> {
    pub(crate) fn new(items: &'a [StateItem]) -> Self {
        Self {
            items,
            next: 0,
            pos: 0,
        }
    }

    fn int(&mut self) -> Result<i64, StateError> {
        match self.items.get(self.next) {
            Some(StateItem::Int(v)) => {
                self.next += 1;
                Ok(*v)
            }
            _ => Err(StateError::ByteOffset(self.pos)),
        }
    }
}

impl Visitor for StateReader<'_> {
    fn scalar(&mut self, size: usize, value: &mut u64) -> StateResult {
        self.pos = align_to(self.pos, size);
        let mut val = u64::from(self.int()? as u32);
        if size == 8 {
            val = val.wrapping_add((self.int()? as u64) << 32);
        }
        *value = match size {
            1 => val & 0xff,
            2 => val & 0xffff,
            4 => val & 0xffff_ffff,
            _ => val,
        };
        self.pos += size;
        Ok(())
    }

    fn bytes(&mut self, data: &mut [u8]) -> StateResult {
        if data.len() == 1 {
            let mut t = 0u64;
            self.scalar(1, &mut t)?;
            data[0] = t as u8;
            return Ok(());
        }
        match self.items.get(self.next) {
            Some(StateItem::Bytes(b)) if b.len() == data.len() => {
                data.copy_from_slice(b);
                self.next += 1;
                self.pos += data.len();
                Ok(())
            }
            _ => Err(StateError::ByteOffset(self.pos)),
        }
    }

    fn skip(&mut self, size: usize, align: usize) {
        self.pos = align_to(self.pos, align) + size;
    }
}
