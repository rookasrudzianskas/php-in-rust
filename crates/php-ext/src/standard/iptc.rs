//! ext/standard/iptc.c: `iptcparse()` and `iptcembed()`.
//!
//! Functions to parse & compose IPTC data. PhotoShop >= 3.0 can read and
//! write textual data to JPEG files.

use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::rc::Rc;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::image::path_from_bytes;

// Some defines for the different JPEG block types.
const M_EOI: i32 = 0xD9; // End Of Image (end of datastream)
const M_SOS: i32 = 0xDA; // Start Of Scan (begins compressed data)
const M_APP0: i32 = 0xe0;
const M_APP1: i32 = 0xe1;
const M_APP13: i32 = 0xed;

const EOF: i32 = -1;

/// `sizeof(psheader)`: the 28 header bytes plus the C string's terminator.
const PSHEADER_SIZEOF: usize = 29;

/// `php_iptc_*`: copies a JPEG (read from `fp`) to the output, to a buffer,
/// or both, inserting the new APP13 block.
struct Embedder<'v, R: Read> {
    vm: &'v mut Vm,
    fp: BufReader<R>,
    /// The `$spool` argument (`> 0`: write to the output as well).
    spool: i64,
    /// `spoolbuf` (only when `$spool < 2`) and the size it was allocated with.
    buf: Option<Vec<u8>>,
    limit: usize,
    /// Output waiting to be echoed (only batched when no output buffer can
    /// observe how the bytes are chunked).
    pending: Vec<u8>,
}

impl<R: Read> Embedder<'_, R> {
    /// `fgetc(fp)`.
    fn getc(&mut self) -> i32 {
        let mut b = [0u8; 1];
        loop {
            return match self.fp.read(&mut b) {
                Ok(1) => b[0] as i32,
                Ok(_) => EOF,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => EOF,
            };
        }
    }

    /// `PUTC(c)`: one byte through the output layer.
    fn putc(&mut self, c: u8) -> PResult<()> {
        if self.vm.output.level() == 0 {
            self.pending.push(c);
            Ok(())
        } else {
            // With output buffering active every byte is a separate write
            // (a handler with a chunk size sees them one by one).
            self.vm.echo(&[c])
        }
    }

    fn flush_pending(&mut self) -> PResult<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.pending);
        self.vm.echo(&data)
    }

    /// Appends to the spool buffer; `false` when it is full.
    fn store(&mut self, c: u8) -> bool {
        if let Some(buf) = &mut self.buf {
            if buf.len() >= self.limit {
                return false;
            }
            buf.push(c);
        }
        true
    }

    /// `php_iptc_put1` (always called with the function's spool settings).
    fn put1(&mut self, c: u8) -> PResult<i32> {
        if self.spool > 0 {
            self.putc(c)?;
        }
        if !self.store(c) {
            return Ok(EOF);
        }
        Ok(c as i32)
    }

    /// `php_iptc_get1`; `normal` is false for the calls that pass `spool = 0`
    /// and no spool buffer.
    fn get1(&mut self, normal: bool) -> PResult<i32> {
        let c = self.getc();
        if c == EOF {
            return Ok(EOF);
        }
        if normal {
            if self.spool > 0 {
                self.putc(c as u8)?;
            }
            if !self.store(c as u8) {
                return Ok(EOF);
            }
        }
        Ok(c)
    }

    /// `php_iptc_read_remaining`.
    fn read_remaining(&mut self, normal: bool) -> PResult<i32> {
        while self.get1(normal)? != EOF {}
        Ok(M_EOI)
    }

    /// `php_iptc_skip_variable`.
    fn skip_variable(&mut self, normal: bool) -> PResult<i32> {
        let c1 = self.get1(normal)?;
        if c1 == EOF {
            return Ok(M_EOI);
        }
        let c2 = self.get1(normal)?;
        if c2 == EOF {
            return Ok(M_EOI);
        }
        // `unsigned int length`: the subtraction wraps for lengths below 2.
        let mut length = (((c1 as u8) as u32) << 8) + (c2 as u8) as u32;
        length = length.wrapping_sub(2);
        while length != 0 {
            length -= 1;
            if self.get1(normal)? == EOF {
                return Ok(M_EOI);
            }
        }
        Ok(0)
    }

    /// `php_iptc_next_marker`.
    fn next_marker(&mut self) -> PResult<i32> {
        // Skip unimportant stuff.
        let mut c = self.get1(true)?;
        if c == EOF {
            return Ok(M_EOI);
        }
        while c != 0xff {
            c = self.get1(true)?;
            if c == EOF {
                return Ok(M_EOI); // we hit EOF
            }
        }
        // Get the marker byte, swallowing possible padding.
        loop {
            c = self.get1(false)?;
            if c == EOF {
                return Ok(M_EOI); // we hit EOF
            } else if c == 0xff {
                self.put1(c as u8)?;
            }
            if c != 0xff {
                break;
            }
        }
        Ok(c)
    }

    /// The body of `iptcembed()` after the file was opened. `None` is `false`.
    fn embed(&mut self, iptcdata: &[u8]) -> PResult<Option<()>> {
        let mut psheader: [u8; 28] = *b"\xFF\xED\0\0Photoshop 3.0\08BIM\x04\x04\0\0\0\0";
        let mut iptcdata_len = iptcdata.len();
        let mut done = false;
        let mut written = false;

        if self.get1(true)? != 0xFF {
            return Ok(None);
        }
        if self.get1(true)? != 0xD8 {
            return Ok(None);
        }

        while !done {
            let marker = self.next_marker()?;

            if marker == M_EOI {
                // EOF
                break;
            } else if marker != M_APP13 && self.put1(marker as u8)? < 0 {
                return Ok(None);
            }

            match marker {
                M_APP13 => {
                    // We are going to write a new APP13 marker, so don't output the old one.
                    self.skip_variable(false)?;
                    self.getc(); // skip already copied 0xFF byte
                    self.read_remaining(true)?;
                    done = true;
                }
                // APP0 is in each and every JPEG, so when we hit APP0 we insert our new APP13!
                M_APP0 | M_APP1 => {
                    if written {
                        // Don't try to write the data twice.
                        continue;
                    }
                    written = true;

                    self.skip_variable(true)?;

                    if iptcdata_len & 1 != 0 {
                        iptcdata_len += 1; // make the length even
                    }

                    psheader[2] = ((iptcdata_len + 28) >> 8) as u8;
                    psheader[3] = ((iptcdata_len + 28) & 0xff) as u8;

                    for &b in &psheader {
                        if self.put1(b)? < 0 {
                            return Ok(None);
                        }
                    }

                    if self.put1((iptcdata_len >> 8) as u8)? < 0 {
                        return Ok(None);
                    }
                    if self.put1((iptcdata_len & 0xff) as u8)? < 0 {
                        return Ok(None);
                    }

                    for inx in 0..iptcdata_len {
                        // The padding byte is the string's NUL terminator.
                        let b = iptcdata.get(inx).copied().unwrap_or(0);
                        if self.put1(b)? < 0 {
                            return Ok(None);
                        }
                    }
                }
                M_SOS => {
                    // We hit data, no more marker-inserting can be done!
                    self.read_remaining(true)?;
                    done = true;
                }
                _ => {
                    self.skip_variable(true)?;
                }
            }
        }
        Ok(Some(()))
    }
}

/// `iptcembed()` on an already opened JPEG stream; `st_size` is the file size
/// the spool buffer is dimensioned with (0 for pipes).
fn embed_stream<R: Read>(
    vm: &mut Vm,
    reader: R,
    iptcdata: &[u8],
    spool: i64,
    st_size: u64,
) -> PResult<Value> {
    let (buf, limit) = if spool < 2 {
        // zend_string_safe_alloc(1, iptcdata_len + sizeof(psheader) + 1024 + 1, st_size, 0)
        let limit = iptcdata
            .len()
            .saturating_add(PSHEADER_SIZEOF + 1024 + 1)
            .saturating_add(usize::try_from(st_size).unwrap_or(usize::MAX));
        (Some(Vec::new()), limit)
    } else {
        (None, 0)
    };
    let mut e = Embedder {
        vm,
        fp: BufReader::new(reader),
        spool,
        buf,
        limit,
        pending: Vec::new(),
    };
    let result = e.embed(iptcdata);
    let flushed = e.flush_pending();
    let ok = result?.is_some();
    flushed?;
    if !ok {
        return Ok(Value::Bool(false));
    }
    match e.buf {
        // spool < 2: the assembled JPEG.
        Some(buf) => Ok(Value::String(PhpStr::from_vec(buf))),
        None => Ok(Value::Bool(true)),
    }
}

/// Embed binary IPTC data into a JPEG image.
fn iptcembed(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let iptcdata = args.str(0).clone();
    let jpeg_file = args.str(1).clone();
    let spool = args.int(2);

    // Z_PARAM_PATH
    if jpeg_file.as_bytes().contains(&0) {
        return Err(vm.argument_value_error(2, "must not contain any null bytes"));
    }

    // TODO(open_basedir): php_check_open_basedir() is not available in the runtime yet.

    let Ok(file) = File::open(path_from_bytes(jpeg_file.as_bytes())) else {
        let mut msg = b"Unable to open ".to_vec();
        msg.extend_from_slice(jpeg_file.as_bytes());
        vm.warning(PhpStr::from_vec(msg))?;
        return Ok(Value::Bool(false));
    };

    let mut st_size = 0;
    if spool < 2 {
        match file.metadata() {
            Ok(m) => st_size = m.len(),
            Err(_) => return Ok(Value::Bool(false)),
        }
    }

    embed_stream(vm, file, iptcdata.as_bytes(), spool, st_size)
}

/// Parse binary IPTC-data into associative array
fn iptcparse(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let buffer = args.str(0).as_bytes();
    let str_len = buffer.len();
    // The C code reads one byte past the end (the string's NUL terminator).
    let at = |i: usize| buffer.get(i).copied().unwrap_or(0);
    let mut inx = 0usize;
    let mut result: Option<PhpArray> = None;

    while inx < str_len {
        // find 1st tag
        if at(inx) == 0x1c && (at(inx + 1) == 0x01 || at(inx + 1) == 0x02) {
            break;
        }
        inx += 1;
    }

    while inx < str_len {
        let marker = at(inx);
        inx += 1;
        if marker != 0x1c {
            break; // we ran against some data which does not conform to IPTC - stop parsing!
        }

        if inx + 4 >= str_len {
            break;
        }

        let dataset = at(inx);
        inx += 1;
        let recnum = at(inx);
        inx += 1;

        let len: usize;
        if at(inx) & 0x80 != 0 {
            // long tag
            if inx + 6 >= str_len {
                break;
            }
            len = ((at(inx + 2) as usize) << 24)
                + ((at(inx + 3) as usize) << 16)
                + ((at(inx + 4) as usize) << 8)
                + at(inx + 5) as usize;
            inx += 6;
        } else {
            // short tag
            len = ((at(inx) as usize) << 8) | at(inx + 1) as usize;
            inx += 2;
        }

        if len > str_len || inx + len > str_len {
            break;
        }

        let key = format!("{}#{:03}", dataset, recnum);

        // The first tag found initialises the return array.
        let arr = result.get_or_insert_with(PhpArray::new);
        let element = arr.entry_mut(ArrayKey::from(key.as_str()));
        if element.is_null() {
            *element = Value::array(PhpArray::new());
        }
        if let Value::Array(list) = element {
            let _ = Rc::make_mut(list).append(Value::from(&buffer[inx..inx + len]));
        }
        inx += len;
    }

    Ok(match result {
        Some(arr) => Value::array(arr),
        None => Value::Bool(false),
    })
}

pub static IPTCEMBED: NativeFunction = NativeFunction {
    sig: "iptcembed(string $iptc_data, string $filename, int $spool = 0): string|bool",
    handler: iptcembed,
};

pub static IPTCPARSE: NativeFunction = NativeFunction {
    sig: "iptcparse(string $iptc_block): array|false",
    handler: iptcparse,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&IPTCEMBED, &IPTCPARSE]);
}

#[cfg(test)]
mod tests;
