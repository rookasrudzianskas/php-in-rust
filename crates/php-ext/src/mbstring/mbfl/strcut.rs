//! `mbfl_strcut` (`libmbfl/mbfl/mbfilter.c`): the byte-oriented substring
//! for encodings without a dedicated `cut` function, fixed width or
//! `mblen_table`. It decodes with the legacy filter of the encoding and
//! re-encodes, backing up filter state, to find the longest output of at
//! most `length` bytes starting at the character containing byte `from`.

use super::encoding::{Encoding, ILLEGAL_MODE_NONE, NoEncoding, enc};
use super::filter::{ConvertFilter, FilterOutput, convert_filter_new};

/// The memory device (`mbfl_memory_device_output`); the decoder has no
/// flush function.
struct DeviceOut<'a>(&'a mut Vec<u8>);

impl FilterOutput for DeviceOut<'_> {
    fn output(&mut self, c: i32) -> i32 {
        self.0.push(c as u8);
        0
    }
    fn flush(&mut self) -> i32 {
        0
    }
}

/// The encoder's output once connected to the decoder
/// (`output_function = decoder->filter_function`,
/// `flush_function = decoder->filter_flush`).
struct DecoderOut<'a> {
    decoder: &'a mut ConvertFilter,
    device: &'a mut Vec<u8>,
}

impl FilterOutput for DecoderOut<'_> {
    fn output(&mut self, c: i32) -> i32 {
        self.decoder.feed(c, &mut DeviceOut(self.device))
    }
    fn flush(&mut self) -> i32 {
        self.decoder.flush(&mut DeviceOut(self.device))
    }
}

/// `mbfl_filter_output_null`.
struct NullOut;

impl FilterOutput for NullOut {
    fn output(&mut self, _c: i32) -> i32 {
        0
    }
    fn flush(&mut self) -> i32 {
        0
    }
}

#[derive(Clone)]
struct Backup {
    encoder: ConvertFilter,
    decoder: ConvertFilter,
    p: usize,
    pos: usize,
}

struct State {
    encoder: ConvertFilter,
    decoder: ConvertFilter,
    device: Vec<u8>,
    p: usize,
}

impl State {
    fn backup(&self) -> Backup {
        Backup {
            encoder: self.encoder.clone(),
            decoder: self.decoder.clone(),
            p: self.p,
            pos: self.device.len(),
        }
    }

    fn restore(&mut self, bk: &Backup) {
        self.p = bk.p;
        self.device.truncate(bk.pos);
        self.decoder = bk.decoder.clone();
        self.encoder = bk.encoder.clone();
    }

    fn feed(&mut self, c: u8) -> i32 {
        let mut out = DecoderOut {
            decoder: &mut self.decoder,
            device: &mut self.device,
        };
        self.encoder.feed(c as i32, &mut out)
    }

    fn flush(&mut self) -> i32 {
        let mut out = DecoderOut {
            decoder: &mut self.decoder,
            device: &mut self.device,
        };
        self.encoder.flush(&mut out)
    }
}

/// `mbfl_strcut`: `None` when the encoding has no legacy filters.
pub fn mbfl_strcut(s: &[u8], encoding: &'static Encoding, from: usize, length: usize) -> Option<Vec<u8>> {
    let from = from.min(s.len());
    let wchar = enc(NoEncoding::Wchar);
    let decoder = convert_filter_new(wchar, encoding)?;
    let mut encoder = convert_filter_new(encoding, wchar)?;

    // Search the start position.
    for &c in &s[..from] {
        if encoder.feed(c as i32, &mut NullOut) < 0 {
            return None;
        }
    }

    let mut st = State {
        encoder,
        decoder,
        device: Vec::with_capacity(s.len().min(length) + 8),
        p: from,
    };
    let q = s.len();
    let mut length = length;

    let mut bk_prev = st.backup();
    if length > q - st.p {
        length = q - st.p;
    }

    let mut bk;
    if length >= 20 {
        // Output a little shorter than `length` (20 is php-src's heuristic).
        let r = st.p + length - 20;
        while st.p < r {
            if st.feed(s[st.p]) < 0 {
                return None;
            }
            st.p += 1;
        }
        if st.device.len() > length {
            st.restore(&bk_prev);
            bk = bk_prev.clone();
        } else {
            bk = st.backup();
            st.flush();
            if st.device.len() > length {
                st.restore(&bk_prev);
                bk = bk_prev.clone();
            } else {
                st.restore(&bk);
            }
        }
    } else {
        bk = bk_prev.clone();
    }

    // Detect the end position.
    while st.p < q {
        if st.feed(s[st.p]) < 0 {
            return None;
        }
        if st.device.len() > length {
            st.restore(&bk);
            break;
        }
        st.p += 1;

        bk_prev = st.backup();
        st.flush();
        if st.device.len() > length {
            st.restore(&bk);
            break;
        }
        st.restore(&bk_prev);
        bk = bk_prev.clone();
    }

    st.decoder.illegal_mode = ILLEGAL_MODE_NONE;
    st.flush();
    Some(st.device)
}
