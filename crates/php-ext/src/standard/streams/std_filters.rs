//! ext/standard/filters.c — the standard stream filters: `string.rot13`,
//! `string.toupper`, `string.tolower`, `convert.*` (base64 and
//! quoted-printable through `php_conv`), `consumed` and `dechunk`.

use php_runtime::{PResult, PhpArray, PhpStr, Value, Vm};

use super::filter::{
    Brigade, Filter, FilterCtx, FilterFactory, FilterOps, FilterRegistry, FilterStatus,
    PSFS_FLAG_FLUSH_CLOSE, Seekable, bucket_make_writeable, new_bucket, parse_write_seek_mode,
};
use super::stream::handle_of;

/// Registers the standard filter factories (`PHP_MINIT(standard_filters)`),
/// keyed by each filter's `fops->label` in php-src order.
pub fn register_factories(reg: &mut FilterRegistry) {
    let factories: [(&str, FilterFactory); 6] = [
        ("string.rot13", strfilter_rot13_create),
        ("string.toupper", strfilter_toupper_create),
        ("string.tolower", strfilter_tolower_create),
        ("convert.*", strfilter_convert_create),
        ("consumed", consumed_filter_create),
        ("dechunk", chunked_filter_create),
    ];
    for (name, f) in factories {
        reg.register(name.as_bytes(), f);
    }
}

/// `php_stream_filter_alloc`.
fn alloc(
    ops: impl FilterOps + 'static,
    persistent: bool,
    read_seekable: Seekable,
    write_seekable: Seekable,
) -> Filter {
    let mut f = Filter::new(Box::new(ops), read_seekable, write_seekable);
    f.is_persistent = persistent;
    f
}

// ---------------------------------------------------------------------------
// string.rot13 / string.toupper / string.tolower
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StrFilter {
    Rot13,
    ToUpper,
    ToLower,
}

/// `strfilter_rot13_ops`, `strfilter_toupper_ops`, `strfilter_tolower_ops`
/// (`php_strtr` over ASCII letters only).
struct StrFilterOps(StrFilter);

fn rot13(buf: &mut [u8]) {
    for c in buf {
        *c = match *c {
            b'a'..=b'z' => (*c - b'a' + 13) % 26 + b'a',
            b'A'..=b'Z' => (*c - b'A' + 13) % 26 + b'A',
            other => other,
        };
    }
}

impl FilterOps for StrFilterOps {
    fn label(&self) -> PhpStr {
        PhpStr::from(match self.0 {
            StrFilter::Rot13 => "string.rot13",
            StrFilter::ToUpper => "string.toupper",
            StrFilter::ToLower => "string.tolower",
        })
    }

    fn filter(
        &mut self,
        _vm: &mut Vm,
        _ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        consumed: Option<&mut usize>,
        _flags: i32,
    ) -> PResult<FilterStatus> {
        let mut n = 0;
        while let Some(b) = inp.pop_front() {
            let b = bucket_make_writeable(b);
            {
                let mut bk = b.borrow_mut();
                match self.0 {
                    StrFilter::Rot13 => rot13(&mut bk.buf),
                    StrFilter::ToUpper => bk.buf.make_ascii_uppercase(),
                    StrFilter::ToLower => bk.buf.make_ascii_lowercase(),
                }
                n += bk.buf.len();
            }
            out.append(b);
        }
        if let Some(c) = consumed {
            *c = n;
        }
        Ok(FilterStatus::PassOn)
    }
}

fn strfilter_rot13_create(
    _vm: &mut Vm,
    _name: &[u8],
    _params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    Ok(Some(alloc(
        StrFilterOps(StrFilter::Rot13),
        persistent,
        Seekable::Always,
        Seekable::Always,
    )))
}

fn strfilter_toupper_create(
    _vm: &mut Vm,
    _name: &[u8],
    _params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    Ok(Some(alloc(
        StrFilterOps(StrFilter::ToUpper),
        persistent,
        Seekable::Always,
        Seekable::Always,
    )))
}

fn strfilter_tolower_create(
    _vm: &mut Vm,
    _name: &[u8],
    _params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    Ok(Some(alloc(
        StrFilterOps(StrFilter::ToLower),
        persistent,
        Seekable::Always,
        Seekable::Always,
    )))
}

// ---------------------------------------------------------------------------
// php_conv: base64 / quoted-printable converters
// ---------------------------------------------------------------------------

/// The `php_conv_err_t` values a converter can report. Converters write into
/// a growable buffer, so `PHP_CONV_ERR_TOO_BIG` (grow and retry) never
/// surfaces, and none of them ever returned `PHP_CONV_ERR_MORE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvErr {
    Unknown,
    InvalidSeq,
    UnexpectedEos,
}

type ConvResult = Result<(), ConvErr>;

const B64_TBL_ENC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `b64_tbl_enc` repeats the alphabet four times, so any byte indexes it.
fn b64_enc(i: u8) -> u8 {
    B64_TBL_ENC[(i & 63) as usize]
}

/// `b64_tbl_dec`: 0..63 for the alphabet, 128 for `=`, 64 for anything else.
fn b64_dec(c: u8) -> u32 {
    match c {
        b'A'..=b'Z' => (c - b'A') as u32,
        b'a'..=b'z' => (c - b'a') as u32 + 26,
        b'0'..=b'9' => (c - b'0') as u32 + 52,
        b'+' => 62,
        b'/' => 63,
        b'=' => 128,
        _ => 64,
    }
}

/// `php_conv_base64_encode`.
struct Base64Encode {
    lbchars: Option<Vec<u8>>,
    erem: [u8; 3],
    erem_len: usize,
    line_ccnt: u32,
    line_len: u32,
}

impl Base64Encode {
    fn new(line_len: u32, lbchars: Option<Vec<u8>>) -> Base64Encode {
        Base64Encode {
            lbchars,
            erem: [0; 3],
            erem_len: 0,
            line_ccnt: line_len,
            line_len,
        }
    }

    fn reset(&mut self) {
        self.erem_len = 0;
        self.line_ccnt = self.line_len;
    }

    /// Emits the line break when fewer than four columns are left.
    fn line_break(&self, line_ccnt: &mut u32, out: &mut Vec<u8>) {
        if let Some(lb) = &self.lbchars {
            if *line_ccnt < 4 {
                out.extend_from_slice(lb);
                *line_ccnt = self.line_len;
            }
        }
    }

    /// `php_conv_base64_encode_flush`: pads the 1 or 2 remaining bytes.
    fn flush(&mut self, out: &mut Vec<u8>) -> ConvResult {
        let mut line_ccnt = self.line_ccnt;
        let r = match self.erem_len {
            0 => Ok(()),
            1 => {
                self.line_break(&mut line_ccnt, out);
                let e = self.erem;
                out.extend_from_slice(&[b64_enc(e[0] >> 2), b64_enc(e[0] << 4), b'=', b'=']);
                self.erem_len = 0;
                line_ccnt = line_ccnt.wrapping_sub(4);
                Ok(())
            }
            2 => {
                self.line_break(&mut line_ccnt, out);
                let e = self.erem;
                out.extend_from_slice(&[
                    b64_enc(e[0] >> 2),
                    b64_enc((e[0] << 4) | (e[1] >> 4)),
                    b64_enc(e[1] << 2),
                    b'=',
                ]);
                self.erem_len = 0;
                line_ccnt = line_ccnt.wrapping_sub(4);
                Ok(())
            }
            _ => Err(ConvErr::Unknown),
        };
        self.line_ccnt = line_ccnt;
        r
    }

    /// `php_conv_base64_encode_convert`.
    fn convert(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> ConvResult {
        let Some(mut ps) = input else {
            return self.flush(out);
        };
        let mut line_ccnt = self.line_ccnt;
        // consume the remainder first
        match self.erem_len {
            1 if ps.len() >= 2 => {
                self.line_break(&mut line_ccnt, out);
                let e = self.erem[0];
                out.extend_from_slice(&[
                    b64_enc(e >> 2),
                    b64_enc((e << 4) | (ps[0] >> 4)),
                    b64_enc((ps[0] << 2) | (ps[1] >> 6)),
                    b64_enc(ps[1]),
                ]);
                ps = &ps[2..];
                self.erem_len = 0;
                line_ccnt = line_ccnt.wrapping_sub(4);
            }
            2 if !ps.is_empty() => {
                self.line_break(&mut line_ccnt, out);
                let e = self.erem;
                out.extend_from_slice(&[
                    b64_enc(e[0] >> 2),
                    b64_enc((e[0] << 4) | (e[1] >> 4)),
                    b64_enc((e[1] << 2) | (ps[0] >> 6)),
                    b64_enc(ps[0]),
                ]);
                ps = &ps[1..];
                self.erem_len = 0;
                line_ccnt = line_ccnt.wrapping_sub(4);
            }
            _ => {}
        }
        while ps.len() >= 3 {
            self.line_break(&mut line_ccnt, out);
            out.extend_from_slice(&[
                b64_enc(ps[0] >> 2),
                b64_enc((ps[0] << 4) | (ps[1] >> 4)),
                b64_enc((ps[1] << 2) | (ps[2] >> 6)),
                b64_enc(ps[2]),
            ]);
            ps = &ps[3..];
            line_ccnt = line_ccnt.wrapping_sub(4);
        }
        for &c in ps {
            self.erem[self.erem_len] = c;
            self.erem_len += 1;
        }
        self.line_ccnt = line_ccnt;
        Ok(())
    }
}

/// `php_conv_base64_decode`.
#[derive(Default)]
struct Base64Decode {
    urem: u32,
    urem_nbits: u32,
    ustat: u32,
    eos: bool,
}

/// `bmask(a)`: the low `a` bits.
fn bmask(a: u32) -> u32 {
    0xffff >> (16 - a)
}

impl Base64Decode {
    fn reset(&mut self) {
        *self = Base64Decode::default();
    }

    /// `php_conv_base64_decode_convert`, a bit-packing state machine that
    /// skips characters outside the alphabet and stops at `=` padding.
    fn convert(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> ConvResult {
        const NBITSOF_PACK: u32 = 8;
        let Some(input) = input else {
            if self.eos || self.urem_nbits == 0 {
                return Ok(());
            }
            return Err(ConvErr::UnexpectedEos);
        };
        let mut err = Ok(());
        let mut ps = input.iter();
        let mut urem = self.urem;
        let mut urem_nbits = self.urem_nbits;
        let mut ustat = self.ustat;
        let mut pack: u32 = 0;
        let mut pack_bcnt = NBITSOF_PACK;
        loop {
            if pack_bcnt >= urem_nbits {
                pack_bcnt -= urem_nbits;
                pack |= urem << pack_bcnt;
                urem_nbits = 0;
            } else {
                urem_nbits -= pack_bcnt;
                pack |= urem >> urem_nbits;
                urem &= bmask(urem_nbits);
                pack_bcnt = 0;
            }
            if pack_bcnt > 0 {
                let Some(&c) = ps.next() else {
                    break;
                };
                let i = b64_dec(c);
                ustat |= i & 0x80;
                if i & 0xc0 == 0 {
                    if ustat != 0 {
                        err = Err(ConvErr::InvalidSeq);
                        break;
                    }
                    if 6 <= pack_bcnt {
                        pack_bcnt -= 6;
                        pack |= i << pack_bcnt;
                        urem = 0;
                    } else {
                        urem_nbits = 6 - pack_bcnt;
                        pack |= i >> urem_nbits;
                        urem = i & bmask(urem_nbits);
                        pack_bcnt = 0;
                    }
                } else if ustat != 0 {
                    if pack_bcnt == 8 || pack_bcnt == 2 {
                        err = Err(ConvErr::InvalidSeq);
                        break;
                    }
                    self.eos = true;
                }
            }
            if (pack_bcnt | ustat) == 0 {
                out.push(pack as u8);
                pack = 0;
                pack_bcnt = NBITSOF_PACK;
            }
        }
        if urem_nbits >= pack_bcnt {
            urem |= pack << (urem_nbits - pack_bcnt);
        } else {
            urem |= pack >> (pack_bcnt - urem_nbits);
        }
        urem_nbits += NBITSOF_PACK - pack_bcnt;
        self.urem = urem;
        self.urem_nbits = urem_nbits;
        self.ustat = ustat;
        err
    }
}

const PHP_CONV_QPRINT_OPT_BINARY: u32 = 0x0000_0001;
const PHP_CONV_QPRINT_OPT_FORCE_ENCODE_FIRST: u32 = 0x0000_0002;

/// `lbchars[i]` of php-src's NUL-terminated copy.
fn lb_at(lb: &[u8], i: usize) -> u8 {
    lb.get(i).copied().unwrap_or(0)
}

/// `php_conv_qprint_encode`.
struct QprintEncode {
    lbchars: Option<Vec<u8>>,
    opts: u32,
    line_ccnt: u32,
    line_len: u32,
    lb_ptr: usize,
    lb_cnt: usize,
}

impl QprintEncode {
    fn new(line_len: u32, lbchars: Option<Vec<u8>>, opts: u32) -> QprintEncode {
        QprintEncode {
            lbchars,
            opts,
            line_ccnt: line_len,
            line_len,
            lb_ptr: 0,
            lb_cnt: 0,
        }
    }

    fn reset(&mut self) {
        self.line_ccnt = self.line_len;
        self.lb_ptr = 0;
        self.lb_cnt = 0;
    }

    /// Emits a soft line break (`=` + line-break chars) when fewer than
    /// `need` columns are left.
    fn soft_break(&self, need: u32, line_ccnt: &mut u32, out: &mut Vec<u8>) {
        if let Some(lb) = &self.lbchars {
            if *line_ccnt < need {
                out.push(b'=');
                out.extend_from_slice(lb);
                *line_ccnt = self.line_len;
            }
        }
    }

    /// `php_conv_qprint_encode_convert`.
    fn convert(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> ConvResult {
        const QP_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let Some(data) = input else {
            return Ok(());
        };
        let opts = self.opts;
        let binary = opts & PHP_CONV_QPRINT_OPT_BINARY != 0;
        let mut line_ccnt = self.line_ccnt;
        let mut lb_ptr = self.lb_ptr;
        let mut lb_cnt = self.lb_cnt;
        let mut ps = 0usize;
        let mut icnt = data.len();
        let mut trail_ws: u32 = 0;
        let lbchars = self.lbchars.clone();
        let lbchars = lbchars.as_deref();

        // CONSUME_CHAR
        let consume = |lb_ptr: &mut usize, lb_cnt: &mut usize, ps: &mut usize, icnt: &mut usize| {
            if *lb_ptr < *lb_cnt {
                *lb_ptr += 1;
            } else {
                *lb_cnt = 0;
                *lb_ptr = 0;
                *icnt -= 1;
                *ps += 1;
            }
        };

        loop {
            if !binary {
                if let Some(lb) = lbchars.filter(|lb| !lb.is_empty()) {
                    // look ahead for the line break chars to make a right
                    // decision how to consume incoming characters
                    if icnt > 0 && data[ps] == lb[lb_cnt] {
                        lb_cnt += 1;
                        if lb_cnt >= lb.len() {
                            out.extend_from_slice(&lb[..lb_cnt]);
                            line_ccnt = self.line_len;
                            lb_ptr = 0;
                            lb_cnt = 0;
                        }
                        ps += 1;
                        icnt -= 1;
                        continue;
                    }
                }
            }

            if lb_ptr >= lb_cnt && icnt == 0 {
                break;
            }

            // NEXT_CHAR
            let c = match lbchars {
                Some(lb) if lb_ptr < lb_cnt => lb_at(lb, lb_ptr),
                _ => data[ps],
            };

            if !binary && trail_ws == 0 && (c == b'\t' || c == b' ') {
                if line_ccnt < 2 && lbchars.is_some() {
                    self.soft_break(2, &mut line_ccnt, out);
                } else {
                    // Check to see if this is EOL whitespace.
                    if let Some(lb) = lbchars {
                        let mut lb_cnt2 = 0usize;
                        let mut ps2 = ps;
                        trail_ws = 1;
                        let mut j = icnt.saturating_sub(1);
                        while j > 0 {
                            let ch = data[ps2];
                            if ch == lb_at(lb, lb_cnt2) {
                                lb_cnt2 += 1;
                                if lb_cnt2 >= lb.len() {
                                    // Found trailing ws: back to the top of
                                    // the loop to encode it.
                                    break;
                                }
                            } else if lb_cnt2 != 0 || (ch != b'\t' && ch != b' ') {
                                // At least one non-EOL character following,
                                // so the whitespace need not be encoded.
                                trail_ws = 0;
                                break;
                            } else {
                                trail_ws += 1;
                            }
                            j -= 1;
                            ps2 += 1;
                        }
                    }
                    if trail_ws == 0 {
                        out.push(c);
                        line_ccnt = line_ccnt.wrapping_sub(1);
                        consume(&mut lb_ptr, &mut lb_cnt, &mut ps, &mut icnt);
                    }
                }
            } else if (opts & PHP_CONV_QPRINT_OPT_FORCE_ENCODE_FIRST == 0
                || line_ccnt < self.line_len)
                && ((33..=60).contains(&c) || (62..=126).contains(&c))
            {
                self.soft_break(2, &mut line_ccnt, out);
                out.push(c);
                line_ccnt = line_ccnt.wrapping_sub(1);
                consume(&mut lb_ptr, &mut lb_cnt, &mut ps, &mut icnt);
            } else {
                self.soft_break(4, &mut line_ccnt, out);
                out.extend_from_slice(&[
                    b'=',
                    QP_DIGITS[(c >> 4) as usize],
                    QP_DIGITS[(c & 0x0f) as usize],
                ]);
                line_ccnt = line_ccnt.wrapping_sub(3);
                trail_ws = trail_ws.saturating_sub(1);
                consume(&mut lb_ptr, &mut lb_cnt, &mut ps, &mut icnt);
            }
        }

        self.line_ccnt = line_ccnt;
        self.lb_ptr = lb_ptr;
        self.lb_cnt = lb_cnt;
        Ok(())
    }
}

/// `php_conv_qprint_decode`.
struct QprintDecode {
    lbchars: Option<Vec<u8>>,
    scan_stat: u32,
    next_char: u32,
    lb_ptr: usize,
    lb_cnt: usize,
}

impl QprintDecode {
    fn new(lbchars: Option<Vec<u8>>) -> QprintDecode {
        QprintDecode {
            lbchars,
            scan_stat: 0,
            next_char: 0,
            lb_ptr: 0,
            lb_cnt: 0,
        }
    }

    fn reset(&mut self) {
        self.scan_stat = 0;
        self.next_char = 0;
        self.lb_ptr = 0;
        self.lb_cnt = 0;
    }

    /// `php_conv_qprint_decode_convert`. Without `line-break-chars`, soft line
    /// breaks may be `\r\n`, `\r` or `\n`.
    fn convert(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> ConvResult {
        let Some(data) = input else {
            if self.scan_stat != 0 {
                return Err(ConvErr::UnexpectedEos);
            }
            return Ok(());
        };
        let lbchars = self.lbchars.as_deref();
        let has_lb = lbchars.is_some();
        let lb = lbchars.unwrap_or(&[]);
        let lbchars_len = lb.len();
        let mut lb_ptr = self.lb_ptr;
        let mut lb_cnt = self.lb_cnt;
        let mut scan_stat = self.scan_stat;
        let mut next_char = self.next_char;
        let mut ps = 0usize;
        let icnt = data.len();
        let mut err = Ok(());

        'out: loop {
            // Each `match` arm may hand over to the next state within the same
            // round, mirroring the C switch fallthroughs.
            let mut st = scan_stat;
            if st == 0 {
                if ps == icnt {
                    break 'out;
                }
                if data[ps] == b'=' {
                    scan_stat = 1;
                } else {
                    out.push(data[ps]);
                }
                ps += 1;
                continue;
            }
            if st == 1 {
                if ps == icnt {
                    break 'out;
                }
                let c = data[ps];
                if c == b' ' || c == b'\t' {
                    scan_stat = 4;
                    ps += 1;
                    continue;
                } else if !has_lb && lb_cnt == 0 && c == b'\r' {
                    // auto-detect line endings, looks like network line
                    // ending \r\n (could be mac \r)
                    lb_cnt += 1;
                    scan_stat = 5;
                    ps += 1;
                    continue;
                } else if !has_lb && lb_cnt == 0 && c == b'\n' {
                    // auto-detect line endings, looks like unix line
                    // endings, not to spec, but seen in the wild a lot
                    lb_cnt = 0;
                    lb_ptr = 0;
                    scan_stat = 0;
                    ps += 1;
                    continue;
                } else if lb_cnt < lbchars_len && c == lb[lb_cnt] {
                    lb_cnt += 1;
                    scan_stat = 5;
                    ps += 1;
                    continue;
                }
                st = 2;
            }
            if st == 2 {
                if ps == icnt {
                    break 'out;
                }
                let c = data[ps];
                if !c.is_ascii_hexdigit() {
                    err = Err(ConvErr::InvalidSeq);
                    break 'out;
                }
                let v = if c >= b'a' {
                    c - 0x57
                } else if c >= b'A' {
                    c - 0x37
                } else {
                    c - 0x30
                };
                next_char = (next_char << 4) | v as u32;
                scan_stat += 1;
                ps += 1;
                if scan_stat != 3 {
                    continue;
                }
                st = 3;
            }
            match st {
                3 => {
                    out.push(next_char as u8);
                    scan_stat = 0;
                }
                4 => {
                    if ps == icnt {
                        break 'out;
                    }
                    let c = data[ps];
                    if lb_cnt < lbchars_len && c == lb[lb_cnt] {
                        lb_cnt += 1;
                        scan_stat = 5;
                    } else if c != b'\t' && c != b' ' {
                        err = Err(ConvErr::InvalidSeq);
                        break 'out;
                    }
                    ps += 1;
                }
                5 => {
                    if ps == icnt {
                        break 'out;
                    }
                    if !has_lb && lb_cnt == 1 && data[ps] == b'\n' {
                        // auto-detect soft line breaks, found network line break
                        lb_cnt = 0;
                        lb_ptr = 0;
                        scan_stat = 0;
                        ps += 1;
                    } else if !has_lb && lb_cnt > 0 {
                        // auto-detect soft line breaks, found mac line break
                        lb_cnt = 0;
                        lb_ptr = 0;
                        scan_stat = 0;
                    } else if lb_cnt >= lbchars_len {
                        // soft line break
                        lb_cnt = 0;
                        lb_ptr = 0;
                        scan_stat = 0;
                    } else if data[ps] == lb[lb_cnt] {
                        lb_cnt += 1;
                        ps += 1;
                    } else {
                        // no break for short-cut
                        scan_stat = 6;
                    }
                }
                6 => {
                    if lb_ptr < lb_cnt {
                        out.push(lb[lb_ptr]);
                        lb_ptr += 1;
                    } else {
                        scan_stat = 0;
                        lb_cnt = 0;
                        lb_ptr = 0;
                    }
                }
                _ => unreachable!("qprint decode state {st}"),
            }
        }

        self.scan_stat = scan_stat;
        self.lb_ptr = lb_ptr;
        self.lb_cnt = lb_cnt;
        self.next_char = next_char;
        err
    }
}

/// `php_conv` instances.
enum Conv {
    Base64Encode(Base64Encode),
    Base64Decode(Base64Decode),
    QprintEncode(QprintEncode),
    QprintDecode(QprintDecode),
}

impl Conv {
    fn convert(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> ConvResult {
        match self {
            Conv::Base64Encode(c) => c.convert(input, out),
            Conv::Base64Decode(c) => c.convert(input, out),
            Conv::QprintEncode(c) => c.convert(input, out),
            Conv::QprintDecode(c) => c.convert(input, out),
        }
    }

    /// `php_conv_reset`: mutable state only, options are kept.
    fn reset(&mut self) {
        match self {
            Conv::Base64Encode(c) => c.reset(),
            Conv::Base64Decode(c) => c.reset(),
            Conv::QprintEncode(c) => c.reset(),
            Conv::QprintDecode(c) => c.reset(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvMode {
    Base64Encode,
    Base64Decode,
    QprintEncode,
    QprintDecode,
}

/// `php_conv_get_string_prop_ex` (`zval_get_tmp_string`).
fn get_str_prop(vm: &mut Vm, ht: &PhpArray, name: &[u8]) -> PResult<Option<Vec<u8>>> {
    match ht.get_str(name).map(Value::unref) {
        Some(v) => Ok(Some(vm.to_php_string(&v)?.to_vec())),
        None => Ok(None),
    }
}

/// `php_conv_get_uint_prop_ex`: `zval_get_long`, negatives become 0, then
/// truncated to `unsigned int`.
fn get_uint_prop(vm: &mut Vm, ht: &PhpArray, name: &[u8]) -> PResult<u32> {
    match ht.get_str(name).map(Value::unref) {
        Some(v) => Ok(vm.to_int(&v)?.max(0) as u64 as u32),
        None => Ok(0),
    }
}

/// `php_conv_get_bool_prop_ex` (`zend_is_true`).
fn get_bool_prop(vm: &mut Vm, ht: &PhpArray, name: &[u8]) -> PResult<bool> {
    match ht.get_str(name).map(Value::unref) {
        Some(v) => vm.to_bool(&v),
        None => Ok(false),
    }
}

/// `line-break-chars` / `line-length` handling shared by the encoders:
/// line breaks only when the line length is at least 4, `\r\n` by default.
fn encoder_line_options(
    vm: &mut Vm,
    options: Option<&PhpArray>,
) -> PResult<(u32, Option<Vec<u8>>)> {
    let Some(ht) = options else {
        return Ok((0, None));
    };
    let lbchars = get_str_prop(vm, ht, b"line-break-chars")?;
    let line_len = get_uint_prop(vm, ht, b"line-length")?;
    if line_len < 4 {
        Ok((0, None))
    } else {
        Ok((line_len, Some(lbchars.unwrap_or_else(|| b"\r\n".to_vec()))))
    }
}

/// `php_conv_open`.
fn php_conv_open(vm: &mut Vm, mode: ConvMode, options: Option<&PhpArray>) -> PResult<Conv> {
    Ok(match mode {
        ConvMode::Base64Encode => {
            let (line_len, lbchars) = encoder_line_options(vm, options)?;
            Conv::Base64Encode(Base64Encode::new(line_len, lbchars))
        }
        ConvMode::Base64Decode => Conv::Base64Decode(Base64Decode::default()),
        ConvMode::QprintEncode => {
            let mut opts = 0;
            let (line_len, lbchars) = match options {
                Some(ht) => {
                    let lbchars = get_str_prop(vm, ht, b"line-break-chars")?;
                    let line_len = get_uint_prop(vm, ht, b"line-length")?;
                    if get_bool_prop(vm, ht, b"binary")? {
                        opts |= PHP_CONV_QPRINT_OPT_BINARY;
                    }
                    if get_bool_prop(vm, ht, b"force-encode-first")? {
                        opts |= PHP_CONV_QPRINT_OPT_FORCE_ENCODE_FIRST;
                    }
                    if line_len < 4 {
                        (0, None)
                    } else {
                        (line_len, Some(lbchars.unwrap_or_else(|| b"\r\n".to_vec())))
                    }
                }
                None => (0, None),
            };
            Conv::QprintEncode(QprintEncode::new(line_len, lbchars, opts))
        }
        ConvMode::QprintDecode => {
            // Without line-break-chars the decoder detects \r, \n or \r\n.
            let lbchars = match options {
                Some(ht) => get_str_prop(vm, ht, b"line-break-chars")?,
                None => None,
            };
            Conv::QprintDecode(QprintDecode::new(lbchars))
        }
    })
}

/// `php_convert_filter` (`strfilter_convert_ops`, label `convert.*`).
struct ConvertFilter {
    cd: Conv,
    filtername: String,
}

impl ConvertFilter {
    /// `strfilter_convert_append_bucket`: converts `ps` (or flushes the
    /// converter when `None`) into one new output bucket. `Ok(false)` is
    /// FAILURE (the warning has been emitted).
    fn append_bucket(
        &mut self,
        vm: &mut Vm,
        persistent: bool,
        out: &mut Brigade,
        ps: Option<&[u8]>,
        consumed: &mut usize,
    ) -> PResult<bool> {
        let mut out_buf = Vec::with_capacity(ps.map_or(64, <[u8]>::len));
        match self.cd.convert(ps, &mut out_buf) {
            Err(ConvErr::InvalidSeq) => {
                vm.warning(format!(
                    "Stream filter ({}): invalid byte sequence",
                    self.filtername
                ))?;
                return Ok(false);
            }
            Err(ConvErr::Unknown) => {
                vm.warning(format!(
                    "Stream filter ({}): unknown error",
                    self.filtername
                ))?;
                return Ok(false);
            }
            // The flush's PHP_CONV_ERR_UNEXPECTED_EOS is not reported.
            Err(ConvErr::UnexpectedEos) | Ok(()) => {}
        }
        if !out_buf.is_empty() {
            let b = new_bucket(out_buf);
            b.borrow_mut().is_persistent = persistent;
            out.append(b);
        }
        *consumed += ps.map_or(0, <[u8]>::len);
        Ok(true)
    }
}

impl FilterOps for ConvertFilter {
    fn label(&self) -> PhpStr {
        PhpStr::from("convert.*")
    }

    /// `strfilter_convert_filter`.
    fn filter(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        bytes_consumed: Option<&mut usize>,
        flags: i32,
    ) -> PResult<FilterStatus> {
        let mut consumed = 0;
        while let Some(bucket) = inp.pop_front() {
            let data = std::mem::take(&mut bucket.borrow_mut().buf);
            if !self.append_bucket(vm, ctx.is_persistent, out, Some(&data), &mut consumed)? {
                return Ok(FilterStatus::ErrFatal);
            }
        }
        if flags & PSFS_FLAG_FLUSH_CLOSE != 0
            && !self.append_bucket(vm, ctx.is_persistent, out, None, &mut consumed)?
        {
            return Ok(FilterStatus::ErrFatal);
        }
        if let Some(c) = bytes_consumed {
            *c = consumed;
        }
        Ok(FilterStatus::PassOn)
    }

    /// `strfilter_convert_seek`: resets the converter state.
    fn seek(&mut self, _vm: &mut Vm, _offset: i64, _whence: i32) -> PResult<bool> {
        self.cd.reset();
        Ok(true)
    }
}

/// `strfilter_convert_create`.
fn strfilter_convert_create(
    vm: &mut Vm,
    name: &[u8],
    params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    let params = params.unref();
    let filtername = String::from_utf8_lossy(name).into_owned();
    // php-src passes no parameters as a NULL zval pointer; callers here pass
    // Null or Undef for that.
    let options = match &params {
        Value::Undef | Value::Null => None,
        Value::Array(a) => Some(a.clone()),
        _ => {
            vm.warning(format!(
                "Stream filter ({filtername}): invalid filter parameter"
            ))?;
            return Ok(None);
        }
    };
    let Some(dot) = name.iter().position(|&c| c == b'.') else {
        return Ok(None);
    };
    let Some(write_seekable) = parse_write_seek_mode(vm, &params)? else {
        return Ok(None);
    };
    let what = &name[dot + 1..];
    let mode = if what.eq_ignore_ascii_case(b"base64-encode") {
        ConvMode::Base64Encode
    } else if what.eq_ignore_ascii_case(b"base64-decode") {
        ConvMode::Base64Decode
    } else if what.eq_ignore_ascii_case(b"quoted-printable-encode") {
        ConvMode::QprintEncode
    } else if what.eq_ignore_ascii_case(b"quoted-printable-decode") {
        ConvMode::QprintDecode
    } else {
        return Ok(None);
    };
    let cd = php_conv_open(vm, mode, options.as_deref())?;
    Ok(Some(alloc(
        ConvertFilter { cd, filtername },
        persistent,
        Seekable::Start,
        write_seekable,
    )))
}

// ---------------------------------------------------------------------------
// consumed
// ---------------------------------------------------------------------------

/// `php_consumed_filter_data`: counts the bytes passing through and, on the
/// closing flush, seeks the stream to where the filter started plus that
/// count.
struct ConsumedFilter {
    consumed: usize,
    /// `~0` in php-src until the first call records `php_stream_tell`.
    offset: Option<i64>,
}

impl FilterOps for ConsumedFilter {
    fn label(&self) -> PhpStr {
        PhpStr::from("consumed")
    }

    /// `consumed_filter_filter`. The stream is borrowed by the filter chain
    /// while filters run, so its position is only reachable (and seekable)
    /// when the caller released it; otherwise both steps are skipped.
    fn filter(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        bytes_consumed: Option<&mut usize>,
        flags: i32,
    ) -> PResult<FilterStatus> {
        let stream = ctx.stream_res.and_then(handle_of);
        if self.offset.is_none() {
            self.offset = stream
                .as_ref()
                .and_then(|h| h.try_borrow().ok().map(|s| s.tell()));
        }
        let mut consumed = 0;
        while let Some(b) = inp.pop_front() {
            consumed += b.borrow().buf.len();
            out.append(b);
        }
        if let Some(c) = bytes_consumed {
            *c = consumed;
        }
        if flags & PSFS_FLAG_FLUSH_CLOSE != 0 {
            if let (Some(h), Some(offset)) = (&stream, self.offset) {
                if let Ok(mut s) = h.try_borrow_mut() {
                    s.seek(vm, offset + self.consumed as i64, 0)?;
                }
            }
        }
        self.consumed += consumed;
        Ok(FilterStatus::PassOn)
    }

    /// `consumed_filter_seek`.
    fn seek(&mut self, _vm: &mut Vm, _offset: i64, _whence: i32) -> PResult<bool> {
        self.consumed = 0;
        self.offset = None;
        Ok(true)
    }
}

/// `consumed_filter_create`.
fn consumed_filter_create(
    _vm: &mut Vm,
    name: &[u8],
    _params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    if !name.eq_ignore_ascii_case(b"consumed") {
        return Ok(None);
    }
    Ok(Some(alloc(
        ConsumedFilter {
            consumed: 0,
            offset: None,
        },
        persistent,
        Seekable::Start,
        Seekable::Always,
    )))
}

// ---------------------------------------------------------------------------
// dechunk
// ---------------------------------------------------------------------------

/// `php_chunked_filter_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkState {
    SizeStart,
    Size,
    SizeExt,
    SizeCr,
    SizeLf,
    Body,
    BodyCr,
    BodyLf,
    Trailer,
    Error,
}

/// `php_chunked_filter_data`.
struct ChunkedFilter {
    chunk_size: usize,
    state: ChunkState,
}

impl ChunkedFilter {
    /// `php_dechunk`: decodes `buf` in place and returns the decoded length.
    /// Once the input is malformed (including a chunk size overflowing
    /// `size_t`) the rest is passed through untouched.
    fn dechunk(&mut self, buf: &mut [u8]) -> usize {
        use ChunkState::*;
        let end = buf.len();
        let mut p = 0usize;
        let mut out_len = 0usize;
        while p < end {
            let mut st = self.state;
            if st == SizeStart || st == Size {
                if st == SizeStart {
                    self.chunk_size = 0;
                }
                while p < end {
                    let digit = match buf[p] {
                        c @ b'0'..=b'9' => (c - b'0') as usize,
                        c @ b'A'..=b'F' => (c - b'A') as usize + 10,
                        c @ b'a'..=b'f' => (c - b'a') as usize + 10,
                        _ => {
                            self.state = if self.state == SizeStart {
                                Error
                            } else {
                                SizeExt
                            };
                            break;
                        }
                    };
                    if self.chunk_size > usize::MAX / 16 {
                        self.state = Error;
                        break;
                    }
                    self.chunk_size = self.chunk_size * 16 + digit;
                    self.state = Size;
                    p += 1;
                }
                if self.state == Error {
                    continue;
                } else if p == end {
                    return out_len;
                }
                st = SizeExt;
            }
            if st == SizeExt {
                // skip extension
                while p < end && buf[p] != b'\r' && buf[p] != b'\n' {
                    p += 1;
                }
                if p == end {
                    return out_len;
                }
                st = SizeCr;
            }
            if st == SizeCr {
                if buf[p] == b'\r' {
                    p += 1;
                    if p == end {
                        self.state = SizeLf;
                        return out_len;
                    }
                }
                st = SizeLf;
            }
            if st == SizeLf {
                if buf[p] == b'\n' {
                    p += 1;
                    if self.chunk_size == 0 {
                        // last chunk
                        self.state = Trailer;
                        continue;
                    } else if p == end {
                        self.state = Body;
                        return out_len;
                    }
                } else {
                    self.state = Error;
                    continue;
                }
                st = Body;
            }
            if st == Body {
                if end - p >= self.chunk_size {
                    let n = self.chunk_size;
                    buf.copy_within(p..p + n, out_len);
                    out_len += n;
                    p += n;
                    if p == end {
                        self.state = BodyCr;
                        return out_len;
                    }
                } else {
                    buf.copy_within(p..end, out_len);
                    self.chunk_size -= end - p;
                    self.state = Body;
                    out_len += end - p;
                    return out_len;
                }
                st = BodyCr;
            }
            if st == BodyCr {
                if buf[p] == b'\r' {
                    p += 1;
                    if p == end {
                        self.state = BodyLf;
                        return out_len;
                    }
                }
                st = BodyLf;
            }
            match st {
                BodyLf => {
                    self.state = if buf[p] == b'\n' {
                        p += 1;
                        SizeStart
                    } else {
                        Error
                    };
                }
                Trailer => {
                    // ignore trailer
                    p = end;
                }
                Error => {
                    buf.copy_within(p..end, out_len);
                    out_len += end - p;
                    return out_len;
                }
                _ => unreachable!("dechunk state {st:?}"),
            }
        }
        out_len
    }
}

impl FilterOps for ChunkedFilter {
    fn label(&self) -> PhpStr {
        PhpStr::from("dechunk")
    }

    /// `php_chunked_filter`.
    fn filter(
        &mut self,
        _vm: &mut Vm,
        _ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        bytes_consumed: Option<&mut usize>,
        _flags: i32,
    ) -> PResult<FilterStatus> {
        let mut consumed = 0;
        while let Some(b) = inp.pop_front() {
            let b = bucket_make_writeable(b);
            {
                let mut bk = b.borrow_mut();
                consumed += bk.buf.len();
                let n = self.dechunk(&mut bk.buf);
                bk.buf.truncate(n);
            }
            out.append(b);
        }
        if let Some(c) = bytes_consumed {
            *c = consumed;
        }
        Ok(FilterStatus::PassOn)
    }

    /// `php_chunked_seek`: resets the decoder.
    fn seek(&mut self, _vm: &mut Vm, _offset: i64, _whence: i32) -> PResult<bool> {
        self.state = ChunkState::SizeStart;
        self.chunk_size = 0;
        Ok(true)
    }
}

/// `chunked_filter_create`.
fn chunked_filter_create(
    _vm: &mut Vm,
    name: &[u8],
    _params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    if !name.eq_ignore_ascii_case(b"dechunk") {
        return Ok(None);
    }
    Ok(Some(alloc(
        ChunkedFilter {
            chunk_size: 0,
            state: ChunkState::SizeStart,
        },
        persistent,
        Seekable::Start,
        Seekable::Always,
    )))
}

#[cfg(test)]
mod tests {
    //! Expectations from ext/standard/tests/filters/*.phpt.

    use super::*;
    use crate::standard::streams::{self, filter, with_stream};
    use crate::testing::TestVm;
    use php_runtime::ResRef;

    fn opts(pairs: &[(&str, Value)]) -> Value {
        let mut a = PhpArray::new();
        for (k, v) in pairs {
            a.insert_str(k, v.clone());
        }
        Value::from(a)
    }

    fn create(t: &mut TestVm, name: &str, params: &Value) -> Option<Filter> {
        filter::create_filter(&mut t.vm, name.as_bytes(), params, false).unwrap()
    }

    /// `stream_filter_append($res, $name, read ? READ : WRITE, $params)`.
    fn attach(t: &mut TestVm, res: &ResRef, name: &str, params: &Value, read: bool) -> u64 {
        let f = create(t, name, params).expect("filter created");
        let id = f.id;
        let ok = with_stream(&mut t.vm, res, |vm, s| s.attach_filter(vm, f, read, true))
            .unwrap()
            .unwrap();
        assert!(ok, "attach {name}");
        id
    }

    /// Reads `data` through the read filter `name`; the stream hands the
    /// filter `chunk` bytes per bucket.
    fn read_chunked(
        t: &mut TestVm,
        name: &str,
        params: &Value,
        data: &[u8],
        chunk: usize,
    ) -> Vec<u8> {
        let res = streams::open_memory(&mut t.vm, data);
        with_stream(&mut t.vm, &res, |_, s| {
            s.chunk_size = chunk;
            Ok(())
        })
        .unwrap();
        attach(t, &res, name, params, true);
        let out = streams::read_to_end(&mut t.vm, &res).unwrap().unwrap();
        close(t, &res);
        out
    }

    /// `fclose()`. Streams with write filters must be closed explicitly: the
    /// stream core defers their close to a thread-local that is gone at
    /// thread exit.
    fn close(t: &mut TestVm, res: &ResRef) {
        streams::close(&mut t.vm, res).unwrap();
    }

    fn read_through(t: &mut TestVm, name: &str, params: &Value, data: &[u8]) -> Vec<u8> {
        read_chunked(t, name, params, data, 8192)
    }

    /// Writes `chunks` through the write filter `name`, removes it (final
    /// flush) and returns what reached the stream.
    fn write_through(t: &mut TestVm, name: &str, params: &Value, chunks: &[&[u8]]) -> Vec<u8> {
        let res = streams::open_memory(&mut t.vm, b"");
        let id = attach(t, &res, name, params, false);
        for c in chunks {
            streams::write(&mut t.vm, &res, c).unwrap();
        }
        with_stream(&mut t.vm, &res, |vm, s| s.remove_filter(vm, id, true)).unwrap();
        streams::seek(&mut t.vm, &res, 0, 0).unwrap();
        let out = streams::read_to_end(&mut t.vm, &res).unwrap().unwrap();
        close(t, &res);
        out
    }

    /// Runs a converter over `chunks` and the final flush.
    fn conv(mode: ConvMode, options: &Value, chunks: &[&[u8]]) -> Result<Vec<u8>, ConvErr> {
        let mut t = TestVm::new();
        let ht = match options {
            Value::Array(a) => Some(a.clone()),
            _ => None,
        };
        let mut cd = php_conv_open(&mut t.vm, mode, ht.as_deref()).unwrap();
        let mut out = Vec::new();
        for c in chunks {
            cd.convert(Some(c), &mut out)?;
        }
        cd.convert(None, &mut out)?;
        Ok(out)
    }

    #[test]
    fn factories_registered_in_php_src_order() {
        let mut t = TestVm::new();
        let names: Vec<String> = filter::registry(&mut t.vm)
            .factories
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "string.rot13",
                "string.toupper",
                "string.tolower",
                "convert.*",
                "consumed",
                "dechunk"
            ]
        );
    }

    #[test]
    fn labels_and_seekability() {
        let mut t = TestVm::new();
        for (name, label, r, w) in [
            (
                "string.rot13",
                "string.rot13",
                Seekable::Always,
                Seekable::Always,
            ),
            (
                "string.toupper",
                "string.toupper",
                Seekable::Always,
                Seekable::Always,
            ),
            (
                "string.tolower",
                "string.tolower",
                Seekable::Always,
                Seekable::Always,
            ),
            (
                "convert.base64-encode",
                "convert.*",
                Seekable::Start,
                Seekable::Always,
            ),
            (
                "convert.quoted-printable-decode",
                "convert.*",
                Seekable::Start,
                Seekable::Always,
            ),
            ("consumed", "consumed", Seekable::Start, Seekable::Always),
            ("dechunk", "dechunk", Seekable::Start, Seekable::Always),
        ] {
            let f = create(&mut t, name, &Value::Null).expect(name);
            assert_eq!(f.label().to_string(), label);
            assert_eq!((f.read_seekable, f.write_seekable), (r, w), "{name}");
            assert!(!f.is_persistent);
        }
        let f = filter::create_filter(&mut t.vm, b"string.rot13", &Value::Null, true)
            .unwrap()
            .unwrap();
        assert!(f.is_persistent);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn string_filters_on_read() {
        // basic.phpt
        let mut t = TestVm::new();
        let text = b"Hello There!";
        let none = Value::Null;
        assert_eq!(
            read_through(&mut t, "string.rot13", &none, text),
            b"Uryyb Gurer!"
        );
        assert_eq!(
            read_through(&mut t, "string.toupper", &none, text),
            b"HELLO THERE!"
        );
        assert_eq!(
            read_through(&mut t, "string.tolower", &none, text),
            b"hello there!"
        );
        let res = streams::open_memory(&mut t.vm, text);
        attach(&mut t, &res, "string.rot13", &none, true);
        attach(&mut t, &res, "string.toupper", &none, true);
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"URYYB GURER!"
        );
        // Only ASCII letters change.
        assert_eq!(
            read_chunked(&mut t, "string.toupper", &none, b"\xe9t\xc9 z9", 2),
            b"\xe9T\xc9 Z9"
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn string_filters_on_write() {
        // bug22538.phpt: rot13 on the write chain
        let mut t = TestVm::new();
        let out = write_through(
            &mut t,
            "string.rot13",
            &Value::Null,
            &[b"abcdefghijklm", b"nopqrstuvwxyz\n", b"ABC"],
        );
        assert_eq!(out, b"nopqrstuvwxyzabcdefghijklm\nNOP");
        assert_eq!(
            write_through(&mut t, "string.tolower", &Value::Null, &[b"MiXeD"]),
            b"mixed"
        );
    }

    #[test]
    fn string_filters_are_fully_seekable() {
        // string_filters_seek.phpt
        let mut t = TestVm::new();
        let text = b"Hello World! This is a test for string filter seeking.";
        let res = streams::open_memory(&mut t.vm, text);
        attach(&mut t, &res, "string.rot13", &Value::Null, true);
        assert_eq!(
            streams::read(&mut t.vm, &res, 20).unwrap().unwrap(),
            b"Uryyb Jbeyq! Guvf vf"
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        assert_eq!(
            streams::read(&mut t.vm, &res, text.len()).unwrap().unwrap(),
            b"Uryyb Jbeyq! Guvf vf n grfg sbe fgevat svygre frrxvat."
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 13, 0).unwrap(), 0);
        assert_eq!(
            streams::read(&mut t.vm, &res, 10).unwrap().unwrap(),
            b"Guvf vf n "
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn base64_roundtrips() {
        // convert.phpt
        let mut t = TestVm::new();
        let none = Value::Null;
        for (plain, encoded) in [
            (&b"Hello World!"[..], &b"SGVsbG8gV29ybGQh"[..]),
            (b"ABC\xff", b"QUJD/w=="),
            (b"", b""),
            (b"A", b"QQ=="),
            (b"Test", b"VGVzdA=="),
        ] {
            assert_eq!(
                read_through(&mut t, "convert.base64-encode", &none, plain),
                encoded
            );
            assert_eq!(
                read_through(&mut t, "convert.base64-decode", &none, encoded),
                plain
            );
        }
        let o = opts(&[
            ("line-length", Value::Int(20)),
            ("line-break-chars", Value::from("\n")),
        ]);
        let plain = b"Long text that will be wrapped";
        let enc = read_through(&mut t, "convert.base64-encode", &o, plain);
        assert_eq!(enc, b"TG9uZyB0ZXh0IHRoYXQg\nd2lsbCBiZSB3cmFwcGVk");
        assert_eq!(
            read_through(&mut t, "convert.base64-decode", &o, &enc),
            plain
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn base64_state_survives_bucket_boundaries() {
        let mut t = TestVm::new();
        let none = Value::Null;
        let plain: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let encoded = read_through(&mut t, "convert.base64-encode", &none, &plain);
        assert_eq!(encoded.len(), 1336);
        for chunk in [1, 2, 3, 4, 5, 7, 64] {
            assert_eq!(
                read_chunked(&mut t, "convert.base64-encode", &none, &plain, chunk),
                encoded,
                "encode, chunk {chunk}"
            );
            assert_eq!(
                read_chunked(&mut t, "convert.base64-decode", &none, &encoded, chunk),
                plain,
                "decode, chunk {chunk}"
            );
        }
        // Line breaks are placed independently of the bucket layout.
        let o = opts(&[("line-length", Value::Int(8))]);
        let wrapped = read_chunked(&mut t, "convert.base64-encode", &o, b"abcdefghijklmnop", 5);
        assert_eq!(wrapped, b"YWJjZGVm\r\nZ2hpamts\r\nbW5vcA==");
        assert_eq!(
            read_chunked(&mut t, "convert.base64-decode", &none, &wrapped, 3),
            b"abcdefghijklmnop"
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn base64_encode_options() {
        // ghsa-88hq-2827-7pg6.phpt: NUL inside line-break-chars is kept.
        let o = opts(&[
            ("line-length", Value::Int(4)),
            ("line-break-chars", Value::from(&b"\0X"[..])),
        ]);
        assert_eq!(
            conv(ConvMode::Base64Encode, &o, &[b"AAAAAA"]).unwrap(),
            b"QUFB\0XQUFB"
        );
        // line-length below 4 disables line breaks.
        let o = opts(&[
            ("line-length", Value::Int(3)),
            ("line-break-chars", Value::from("|")),
        ]);
        assert_eq!(
            conv(ConvMode::Base64Encode, &o, &[b"AAAAAAAAA"]).unwrap(),
            b"QUFBQUFBQUFB"
        );
        // The default line break is CRLF; the length counts encoded columns.
        let o = opts(&[("line-length", Value::Int(6))]);
        assert_eq!(
            conv(ConvMode::Base64Encode, &o, &[b"abcdefgh"]).unwrap(),
            b"YWJj\r\nZGVm\r\nZ2g="
        );
        // Negative and numeric-string lengths.
        let o = opts(&[("line-length", Value::Int(-5))]);
        assert_eq!(
            conv(ConvMode::Base64Encode, &o, &[b"abcdefgh"]).unwrap(),
            b"YWJjZGVmZ2g="
        );
        let o = opts(&[
            ("line-length", Value::from("4")),
            ("line-break-chars", Value::from(".")),
        ]);
        assert_eq!(
            conv(ConvMode::Base64Encode, &o, &[b"abcdef"]).unwrap(),
            b"YWJj.ZGVm"
        );
    }

    #[test]
    fn base64_decode_details() {
        let none = Value::Null;
        // Characters outside the alphabet are skipped.
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"SGVs\r\nbG8=\n"]).unwrap(),
            b"Hello"
        );
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"S G V s"]).unwrap(),
            b"Hel"
        );
        // Missing padding at the end of the stream is not reported by the filter.
        let mut t = TestVm::new();
        assert_eq!(
            read_through(&mut t, "convert.base64-decode", &none, b"SGVsbG8"),
            b"Hello"
        );
        assert_eq!(t.output(), "");
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"SGVsbG8"]),
            Err(ConvErr::UnexpectedEos)
        );
        // Data after the padding, and padding in the wrong place, are invalid.
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"QQ==QQ=="]),
            Err(ConvErr::InvalidSeq)
        );
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"==="]),
            Err(ConvErr::InvalidSeq)
        );
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"Q==="]),
            Err(ConvErr::InvalidSeq)
        );
        assert_eq!(
            conv(ConvMode::Base64Decode, &none, &[b"QQ=", b"="]).unwrap(),
            b"A"
        );
    }

    #[test]
    fn base64_decode_errors() {
        // filter_errors_convert_base64_decode.phpt
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b".\r\n===");
        streams::read_record(&mut t.vm, &res, 8192, b"\r\n").unwrap();
        let f = create(&mut t, "convert.base64-decode", &Value::Null).unwrap();
        let ok = with_stream(&mut t.vm, &res, |vm, s| s.attach_filter(vm, f, true, true))
            .unwrap()
            .unwrap();
        assert!(!ok);
        assert_eq!(
            t.output(),
            "\nWarning: main(): Stream filter (convert.base64-decode): invalid byte sequence in /t.php on line 3\n\
             \nWarning: main(): Filter failed to process pre-buffered data in /t.php on line 3\n"
        );

        assert_eq!(
            read_through(&mut t, "convert.base64-decode", &Value::Null, b"==="),
            b""
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Stream filter (convert.base64-decode): invalid byte sequence in /t.php on line 3\n"
        );
    }

    #[test]
    fn base64_incremental_flush_keeps_state() {
        // gh22360.phpt: padding only on the closing flush.
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b"");
        let id = attach(&mut t, &res, "convert.base64-encode", &Value::Null, false);
        streams::write(&mut t.vm, &res, b"ab").unwrap();
        assert!(streams::flush(&mut t.vm, &res).unwrap());
        streams::write(&mut t.vm, &res, b"c").unwrap();
        assert!(streams::flush(&mut t.vm, &res).unwrap());
        with_stream(&mut t.vm, &res, |vm, s| s.remove_filter(vm, id, true)).unwrap();
        streams::seek(&mut t.vm, &res, 0, 0).unwrap();
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"YWJj"
        );
        close(&mut t, &res);
        assert_eq!(
            write_through(
                &mut t,
                "convert.base64-encode",
                &Value::Null,
                &[b"a", b"b", b"c", b"d"]
            ),
            b"YWJjZA=="
        );
    }

    #[test]
    fn convert_read_seeks_to_start_only() {
        // convert_filter_seek.phpt
        let mut t = TestVm::new();
        let none = Value::Null;
        let text = b"Hello World! This is a test for convert filter seeking functionality.";
        let encoded = conv(ConvMode::Base64Encode, &none, &[text]).unwrap();
        let res = streams::open_memory(&mut t.vm, &encoded);
        attach(&mut t, &res, "convert.base64-decode", &none, true);
        assert_eq!(
            streams::read(&mut t.vm, &res, 20).unwrap().unwrap(),
            b"Hello World! This is"
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        assert_eq!(
            streams::read(&mut t.vm, &res, text.len()).unwrap().unwrap(),
            text
        );
        assert_eq!(t.output(), "");
        assert_eq!(streams::seek(&mut t.vm, &res, 50, 0).unwrap(), -1);
        assert_eq!(
            t.output(),
            "\nWarning: main(): Stream filter convert.* is seekable only to start position in /t.php on line 3\n"
        );

        let text2 = b"Line1\r\nLine2\r\nLine3";
        let encoded = write_through(&mut t, "convert.quoted-printable-encode", &none, &[text2]);
        assert_eq!(encoded, b"Line1=0D=0ALine2=0D=0ALine3");
        let res = streams::open_memory(&mut t.vm, &encoded);
        attach(&mut t, &res, "convert.quoted-printable-decode", &none, true);
        assert_eq!(
            streams::read(&mut t.vm, &res, 10).unwrap().unwrap(),
            b"Line1\r\nLin"
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        assert_eq!(
            streams::read(&mut t.vm, &res, text2.len())
                .unwrap()
                .unwrap(),
            text2
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn convert_write_seek_modes() {
        // convert_filter_write_seek_modes.phpt
        for name in ["convert.base64-encode", "convert.quoted-printable-encode"] {
            let mut t = TestVm::new();
            let mode = |m: &str| opts(&[("write_seek_mode", Value::from(m))]);

            let res = streams::open_memory(&mut t.vm, b"");
            attach(&mut t, &res, name, &mode("preserve"), false);
            streams::write(&mut t.vm, &res, b"Hello").unwrap();
            assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
            assert_eq!(streams::seek(&mut t.vm, &res, 100, 0).unwrap(), 0);
            close(&mut t, &res);

            let res = streams::open_memory(&mut t.vm, b"");
            attach(&mut t, &res, name, &mode("reset"), false);
            streams::write(&mut t.vm, &res, b"Hello").unwrap();
            assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
            close(&mut t, &res);
            assert_eq!(t.output(), "");

            let res = streams::open_memory(&mut t.vm, b"");
            attach(&mut t, &res, name, &mode("strict"), false);
            streams::write(&mut t.vm, &res, b"Hello").unwrap();
            assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), -1);
            assert!(
                t.output()
                    .contains("Stream filter convert.* is never seekable")
            );
            close(&mut t, &res);

            let bad = opts(&[("write_seek_mode", Value::Int(42))]);
            assert!(create(&mut t, name, &bad).is_none());
            assert_eq!(
                t.output(),
                format!(
                    "\nWarning: main(): \"write_seek_mode\" filter parameter must be one of \"preserve\", \"reset\", or \"strict\" in /t.php on line 3\n\
                     \nWarning: main(): Unable to create or locate filter \"{name}\" in /t.php on line 3\n"
                )
            );
        }
    }

    #[test]
    fn convert_reset_on_write_seek() {
        // write_seek_mode=reset: seeking to the start drops the pending
        // remainder of the encoder.
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b"");
        let reset = opts(&[("write_seek_mode", Value::from("reset"))]);
        let id = attach(&mut t, &res, "convert.base64-encode", &reset, false);
        streams::write(&mut t.vm, &res, b"abcd").unwrap();
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        streams::write(&mut t.vm, &res, b"xyz").unwrap();
        with_stream(&mut t.vm, &res, |vm, s| s.remove_filter(vm, id, true)).unwrap();
        streams::seek(&mut t.vm, &res, 0, 0).unwrap();
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"eHl6"
        );
        close(&mut t, &res);
    }

    #[test]
    fn convert_invalid_parameters() {
        let mut t = TestVm::new();
        assert!(create(&mut t, "convert.base64-encode", &Value::from("x")).is_none());
        assert_eq!(
            t.output(),
            "\nWarning: main(): Stream filter (convert.base64-encode): invalid filter parameter in /t.php on line 3\n\
             \nWarning: main(): Unable to create or locate filter \"convert.base64-encode\" in /t.php on line 3\n"
        );
        assert!(create(&mut t, "convert.nothing", &Value::Null).is_none());
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unable to create or locate filter \"convert.nothing\" in /t.php on line 3\n"
        );
        assert!(create(&mut t, "convert.base64-encode.x", &Value::Null).is_none());
        assert!(
            t.output()
                .contains("Unable to create or locate filter \"convert.base64-encode.x\"")
        );
        assert!(create(&mut t, "nothing.here", &Value::Null).is_none());
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unable to locate filter \"nothing.here\" in /t.php on line 3\n"
        );
        // Mode names are case-insensitive.
        assert!(create(&mut t, "convert.BASE64-Encode", &Value::Null).is_some());
        assert_eq!(t.output(), "");
    }

    #[test]
    fn qprint_encode_basics() {
        // convert.phpt, bug77231.phpt
        let mut t = TestVm::new();
        let none = Value::Null;
        let enc = "convert.quoted-printable-encode";
        let dec = "convert.quoted-printable-decode";
        assert_eq!(
            read_through(&mut t, enc, &none, b"Hello World!"),
            b"Hello World!"
        );
        assert_eq!(
            read_through(&mut t, enc, &none, b"Line1\r\nLine2"),
            b"Line1=0D=0ALine2"
        );
        let bin = opts(&[("binary", Value::Bool(true))]);
        assert_eq!(
            read_through(&mut t, enc, &bin, b"Text\t\r\n"),
            b"Text=09=0D=0A"
        );
        let input =
            b"\xbfAAAAAAAAFAAAAAAAAAAAAAA\xff\xff\xff\xff\xff\xff\xff\xffAAAAAAAAAAAAAAAAAAAAAAAA";
        assert_eq!(
            read_through(&mut t, enc, &none, input),
            &b"=BFAAAAAAAAFAAAAAAAAAAAAAA=FF=FF=FF=FF=FF=FF=FF=FFAAAAAAAAAAAAAAAAAAAAAAAA"[..]
        );
        assert_eq!(read_through(&mut t, enc, &none, b"a=b"), b"a=3Db");
        for (plain, encoded) in [
            (&b"Line1\r\nLine2"[..], &b"Line1=0D=0ALine2"[..]),
            (b"Text\t\r\n", b"Text=09=0D=0A"),
        ] {
            assert_eq!(read_through(&mut t, dec, &none, encoded), plain);
        }
        assert_eq!(t.output(), "");
    }

    #[test]
    fn qprint_encode_line_options() {
        let q = ConvMode::QprintEncode;
        let o = opts(&[("line-length", Value::Int(10))]);
        assert_eq!(
            conv(q, &o, &[b"abcdefghijklmnop"]).unwrap(),
            b"abcdefghi=\r\njklmnop"
        );
        assert_eq!(
            conv(q, &o, &[b"\xff\xff\xff\xff"]).unwrap(),
            b"=FF=FF=FF=\r\n=FF"
        );
        // Line breaks reset the column; whitespace right before one is encoded.
        assert_eq!(conv(q, &o, &[b"a \r\nb"]).unwrap(), b"a=20\r\nb");
        assert_eq!(conv(q, &o, &[b"a  b\r\nc"]).unwrap(), b"a  b\r\nc");
        assert_eq!(
            conv(q, &o, &[b"abcdefgh\r\nabcdefgh"]).unwrap(),
            b"abcdefgh\r\nabcdefgh"
        );
        // Without line breaks whitespace is literal and CRLF is encoded.
        assert_eq!(conv(q, &Value::Null, &[b"a \r\nb"]).unwrap(), b"a =0D=0Ab");
        // binary encodes line breaks and whitespace too.
        let o = opts(&[
            ("line-length", Value::Int(10)),
            ("binary", Value::Bool(true)),
        ]);
        assert_eq!(conv(q, &o, &[b"a \r\nb"]).unwrap(), b"a=20=0D=\r\n=0Ab");
        let o = opts(&[("binary", Value::Bool(true))]);
        assert_eq!(conv(q, &o, &[b"a \r\nb"]).unwrap(), b"a=20=0D=0Ab");
        // force-encode-first encodes the first character of each line.
        let o = opts(&[
            ("line-length", Value::Int(10)),
            ("force-encode-first", Value::Bool(true)),
        ]);
        assert_eq!(conv(q, &o, &[b"ab\r\ncd"]).unwrap(), b"=61b\r\n=63d");
        // Custom line break chars.
        let o = opts(&[
            ("line-length", Value::Int(6)),
            ("line-break-chars", Value::from("\n")),
        ]);
        assert_eq!(conv(q, &o, &[b"abcdefgh\nij"]).unwrap(), b"abcde=\nfgh\nij");
    }

    #[test]
    fn qprint_decode_basics() {
        // qp_decode_lowercase.phpt
        let mut t = TestVm::new();
        let q = ConvMode::QprintDecode;
        let none = Value::Null;
        for (input, hex) in [
            ("=cd", "cd"),
            ("=0a", "0a"),
            ("=da", "da"),
            ("=CD", "cd"),
            ("=AB", "ab"),
            ("=ab", "ab"),
        ] {
            let out = read_through(
                &mut t,
                "convert.quoted-printable-decode",
                &none,
                input.as_bytes(),
            );
            let got: String = out.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, hex, "{input}");
        }
        // Soft line breaks: CRLF, LF and CR are detected without line-break-chars.
        for input in [&b"abc=\r\ndef"[..], b"abc=\ndef", b"abc=\rdef"] {
            assert_eq!(conv(q, &none, &[input]).unwrap(), b"abcdef");
        }
        // Whitespace after '=' needs the configured line break.
        let lb = opts(&[("line-break-chars", Value::from("\r\n"))]);
        assert_eq!(conv(q, &lb, &[b"abc= \t\r\ndef"]).unwrap(), b"abcdef");
        assert_eq!(conv(q, &none, &[b"abc= \r\ndef"]), Err(ConvErr::InvalidSeq));
        // A partial line break after '=' is passed through.
        assert_eq!(conv(q, &lb, &[b"a=\rb"]).unwrap(), b"a\rb");
        // Escapes split across buckets.
        assert_eq!(
            conv(q, &none, &[b"x=", b"4", b"1y=4", b"2"]).unwrap(),
            b"xAyB"
        );
        assert_eq!(t.output(), "");
        // An escape cut by the end of the stream.
        assert_eq!(conv(q, &none, &[b"x=4"]), Err(ConvErr::UnexpectedEos));
    }

    #[test]
    fn qprint_decode_on_write_chain() {
        // bug50363.phpt
        let mut t = TestVm::new();
        let mut foo = b"Sauvegarder=C3=A9ussi(e) n=C3=A3o N=C3=83O\n".to_vec();
        foo.extend_from_slice(b"Sauvegarder=c3=a9ussi(e) n=c3=a3o N=c3=83O\n");
        let line = "Sauvegarder\u{e9}ussi(e) n\u{e3}o N\u{c3}O\n";
        assert_eq!(
            write_through(
                &mut t,
                "convert.quoted-printable-decode",
                &Value::Null,
                &[&foo]
            ),
            format!("{line}{line}").as_bytes()
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn qprint_decode_invalid_sequences() {
        // bug74267.phpt
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b"");
        let lb = opts(&[("line-break-chars", Value::from("\r\n"))]);
        let id = attach(&mut t, &res, "convert.quoted-printable-decode", &lb, false);
        for line in [&b"\r\n"[..], b" -=()\r\n", b" -=\r\n", b"\r\n"] {
            streams::write(&mut t.vm, &res, line).unwrap();
        }
        with_stream(&mut t.vm, &res, |vm, s| s.remove_filter(vm, id, true)).unwrap();
        close(&mut t, &res);
        let w = "\nWarning: main(): Stream filter (convert.quoted-printable-decode): invalid byte sequence in /t.php on line 3\n";
        assert_eq!(t.output(), format!("{w}{w}"));

        // ghsa-r977-prxv-hc43.phpt: a dangling "=\r" at the end is dropped silently.
        let mut input = vec![b'A'; 8189];
        input.extend_from_slice(b"X=\r");
        let out = read_through(
            &mut t,
            "convert.quoted-printable-decode",
            &Value::Null,
            &input,
        );
        assert_eq!(out.len(), 8190);
        assert!(out.ends_with(b"AX"));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn dechunk_inputs() {
        // chunked_001.phpt
        let mut t = TestVm::new();
        let none = Value::Null;
        for (input, expected) in [
            (&b"0\r\n"[..], &b""[..]),
            (b"2\r\nte\r\n2\r\nst\r\n0\r\n", b"test"),
            (b"2\nte\n2\nst\n0\n", b"test"),
            (b"2;a=1\nte\n2;a=2;b=3\r\nst\n0\n", b"test"),
            (b"2\nte\n2\nst\n0\na=b\r\nc=d\n\r\n", b"test"),
            (
                b"1f\n0123456789abcdef0123456789abcde\n1\nf\n0\n",
                b"0123456789abcdef0123456789abcdef",
            ),
            (
                b"1E\n0123456789abcdef0123456789abcd\n2\nef\n0\n",
                b"0123456789abcdef0123456789abcdef",
            ),
        ] {
            assert_eq!(read_through(&mut t, "dechunk", &none, input), expected);
            for chunk in [1, 2, 3] {
                assert_eq!(
                    read_chunked(&mut t, "dechunk", &none, input, chunk),
                    expected,
                    "chunk {chunk}"
                );
            }
        }
        assert_eq!(t.output(), "");
    }

    #[test]
    fn dechunk_malformed_input_passes_through() {
        let mut t = TestVm::new();
        let none = Value::Null;
        assert_eq!(read_through(&mut t, "dechunk", &none, b"xyz"), b"xyz");
        assert_eq!(
            read_through(&mut t, "dechunk", &none, b"2\r\nteX\r\n0\r\n"),
            b"teX\r\n0\r\n"
        );
        assert_eq!(read_through(&mut t, "dechunk", &none, b"2\rte"), b"te");
        assert_eq!(read_through(&mut t, "dechunk", &none, b"2\r\rte"), b"\rte");
        // A body cut short by the end of the input.
        assert_eq!(read_through(&mut t, "dechunk", &none, b"9\r\nabc"), b"abc");
    }

    #[test]
    fn dechunk_size_overflow() {
        // dechunk_size_overflow.phpt: parsing stops at SIZE_MAX / 16.
        let mut t = TestVm::new();
        let none = Value::Null;
        assert_eq!(
            read_through(
                &mut t,
                "dechunk",
                &none,
                b"10000000000000000\nBODYDATA\n0\n"
            ),
            b"0\nBODYDATA\n0\n"
        );
        assert_eq!(
            read_through(
                &mut t,
                "dechunk",
                &none,
                b"fffffffffffffffff\nBODYDATA\n0\n"
            ),
            b"f\nBODYDATA\n0\n"
        );
        assert_eq!(
            read_through(&mut t, "dechunk", &none, b"5\nhello\n0\n"),
            b"hello"
        );
    }

    #[test]
    fn dechunk_write_state_survives_seek() {
        // chunked_002.phpt
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b"");
        attach(&mut t, &res, "dechunk", &Value::Null, false);
        let contents = |t: &mut TestVm| {
            streams::seek(&mut t.vm, &res, 0, 0).unwrap();
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap()
        };
        streams::write(&mut t.vm, &res, b"5\r\nHello\r\n").unwrap();
        assert_eq!(contents(&mut t), b"Hello");
        streams::write(&mut t.vm, &res, b"7\r\n, World\r\n").unwrap();
        assert_eq!(contents(&mut t), b"Hello, World");
        streams::write(&mut t.vm, &res, b"0\r\n\r\n").unwrap();
        assert_eq!(contents(&mut t), b"Hello, World");
        assert_eq!(streams::tell(&res), Some(12));
        close(&mut t, &res);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn dechunk_read_resets_on_seek_to_start() {
        let mut t = TestVm::new();
        let res = streams::open_memory(&mut t.vm, b"3\r\nabc\r\n3\r\ndef\r\n0\r\n");
        attach(&mut t, &res, "dechunk", &Value::Null, true);
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"abcdef"
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"abcdef"
        );
        assert_eq!(streams::seek(&mut t.vm, &res, 2, 0).unwrap(), -1);
        assert!(
            t.output()
                .contains("Stream filter dechunk is seekable only to start position")
        );
    }

    #[test]
    fn consumed_passes_data_through() {
        let mut t = TestVm::new();
        let none = Value::Null;
        assert_eq!(
            read_chunked(&mut t, "consumed", &none, b"some data", 4),
            b"some data"
        );
        assert_eq!(
            write_through(&mut t, "consumed", &none, &[b"ab", b"cd"]),
            b"abcd"
        );
        let mut f = ConsumedFilter {
            consumed: 0,
            offset: None,
        };
        let mut inp = Brigade::new();
        inp.append(new_bucket(b"abc".to_vec()));
        let mut out = Brigade::new();
        let mut n = 0;
        let ctx = FilterCtx {
            stream_res: None,
            is_persistent: false,
        };
        let st = f
            .filter(&mut t.vm, &ctx, &mut inp, &mut out, Some(&mut n), 0)
            .unwrap();
        assert_eq!(
            (st, n, f.consumed, out.total_len()),
            (FilterStatus::PassOn, 3, 3, 3)
        );
        assert!(f.seek(&mut t.vm, 0, 0).unwrap());
        assert_eq!(f.consumed, 0);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn filters_report_consumed_bytes() {
        let mut t = TestVm::new();
        let ctx = FilterCtx {
            stream_res: None,
            is_persistent: false,
        };
        for name in [
            "string.rot13",
            "convert.base64-encode",
            "convert.quoted-printable-encode",
            "dechunk",
            "consumed",
        ] {
            let mut f = create(&mut t, name, &Value::Null).unwrap();
            let mut inp = Brigade::new();
            inp.append(new_bucket(b"1\r\nab".to_vec()));
            inp.append(new_bucket(b"c".to_vec()));
            let mut out = Brigade::new();
            let mut n = 0;
            let st = f
                .ops
                .filter(&mut t.vm, &ctx, &mut inp, &mut out, Some(&mut n), 0)
                .unwrap();
            assert_eq!((st, n), (FilterStatus::PassOn, 6), "{name}");
            assert!(inp.is_empty());
        }
    }
}
