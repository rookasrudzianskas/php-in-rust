//! The per-format `php_handle_*` routines of `ext/standard/image.c`.
//!
//! Each handler continues reading from the position `php_getimagetype()` left
//! the stream at, exactly like its C counterpart, and returns the dimensions
//! (`struct php_gfxinfo`) or `None`.

use php_runtime::{PResult, PhpArray, PhpStr, Value, Vm};

use super::avifinfo;
use super::inflate::{self, Status};
use super::stream::Stream;

/// `struct php_gfxinfo` (all fields are `unsigned int` in C).
#[derive(Debug, Clone, Copy, Default)]
pub struct GfxInfo {
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    pub channels: u32,
}

/// Reports the notices the stream layer collected (`Read of N bytes failed ...`).
pub fn flush_notices(vm: &mut Vm, s: &mut Stream) -> PResult<()> {
    for n in s.take_notices() {
        vm.notice(n)?;
    }
    Ok(())
}

fn be32(a: &[u8]) -> u32 {
    ((a[0] as u32) << 24) + ((a[1] as u32) << 16) + ((a[2] as u32) << 8) + a[3] as u32
}

fn le32(a: &[u8]) -> u32 {
    ((a[3] as u32) << 24) + ((a[2] as u32) << 16) + ((a[1] as u32) << 8) + a[0] as u32
}

/// `php_handle_gif`.
pub fn handle_gif(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(3) != 0 {
        return None;
    }
    let dim = s.read_array::<5>()?;
    Some(GfxInfo {
        width: dim[0] as u32 | ((dim[1] as u32) << 8),
        height: dim[2] as u32 | ((dim[3] as u32) << 8),
        bits: if dim[4] & 0x80 != 0 {
            (dim[4] as u32 & 0x07) + 1
        } else {
            0
        },
        channels: 3,
    })
}

/// `php_handle_psd`.
pub fn handle_psd(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(11) != 0 {
        return None;
    }
    let dim = s.read_array::<8>()?;
    Some(GfxInfo {
        height: be32(&dim[0..4]),
        width: be32(&dim[4..8]),
        ..GfxInfo::default()
    })
}

/// `php_handle_bmp`.
pub fn handle_bmp(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(11) != 0 {
        return None;
    }
    let dim = s.read_array::<16>()?;
    let size = le32(&dim[0..4]) as i32;
    if size == 12 {
        Some(GfxInfo {
            width: ((dim[5] as u32) << 8) + dim[4] as u32,
            height: ((dim[7] as u32) << 8) + dim[6] as u32,
            bits: dim[11] as u32,
            channels: 0,
        })
    } else if size > 12 && (size <= 64 || size == 108 || size == 124) {
        Some(GfxInfo {
            width: le32(&dim[4..8]),
            height: (le32(&dim[8..12]) as i32).wrapping_abs() as u32,
            bits: ((dim[15] as u32) << 8) + dim[14] as u32,
            channels: 0,
        })
    } else {
        None
    }
}

/// `php_swf_get_bits`.
fn swf_get_bits(buffer: &[u8], pos: u32, count: u32) -> u64 {
    let mut result: u64 = 0;
    for lp in pos..pos + count {
        let bit = ((buffer[(lp / 8) as usize] >> (7 - (lp % 8))) & 0x01) as u64;
        result += bit << (count - (lp - pos) - 1);
    }
    result
}

fn swf_dimensions(a: &[u8]) -> GfxInfo {
    let bits = swf_get_bits(a, 0, 5) as u32;
    let width = swf_get_bits(a, 5 + bits, bits).wrapping_sub(swf_get_bits(a, 5, bits)) / 20;
    let height =
        swf_get_bits(a, 5 + 3 * bits, bits).wrapping_sub(swf_get_bits(a, 5 + 2 * bits, bits)) / 20;
    GfxInfo {
        width: width as u32,
        height: height as u32,
        bits: 0,
        channels: 0,
    }
}

/// `php_handle_swf`.
pub fn handle_swf(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(5) != 0 {
        return None;
    }
    let a = s.read_array::<32>()?;
    Some(swf_dimensions(&a))
}

/// `php_handle_swc`: compressed (`CWS`) Flash files.
pub fn handle_swc(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(5) != 0 {
        return None;
    }
    let a = s.read_array::<64>()?;

    // First try to inflate just the 64 bytes read (`uncompress(b, &len, a, 64)`).
    let first = inflate::zlib_inflate(&a, 64, 64);
    if first.status != Status::Ok {
        // Failed to decompress the 64 bytes: read the rest of the file and try
        // again with growing output limits (input length * 2 .. * 2^15).
        if s.seek_set(8) != 0 {
            return None;
        }
        let mut rest = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = s.read(&mut chunk);
            if n <= 0 {
                break;
            }
            rest.extend_from_slice(&chunk[..n as usize]);
        }
        let limit = (rest.len() as u64).saturating_mul(1 << 15);
        if inflate::zlib_inflate(&rest, limit, 0).status != Status::Ok {
            return None;
        }
    }
    // `b` is `len + 1` zeroed bytes holding the first `len` decompressed bytes.
    let mut b = vec![0u8; 65];
    b[..first.head.len()].copy_from_slice(&first.head);
    Some(swf_dimensions(&b))
}

/// `php_handle_png`.
pub fn handle_png(s: &mut Stream) -> Option<GfxInfo> {
    if s.seek_cur(8) != 0 {
        return None;
    }
    let dim = s.read_array::<9>()?;
    Some(GfxInfo {
        width: be32(&dim[0..4]),
        height: be32(&dim[4..8]),
        bits: dim[8] as u32,
        channels: 0,
    })
}

// JPEG block types.
const M_SOF0: u32 = 0xC0;
const M_SOF1: u32 = 0xC1;
const M_SOF2: u32 = 0xC2;
const M_SOF3: u32 = 0xC3;
const M_SOF5: u32 = 0xC5;
const M_SOF6: u32 = 0xC6;
const M_SOF7: u32 = 0xC7;
const M_SOF9: u32 = 0xC9;
const M_SOF10: u32 = 0xCA;
const M_SOF11: u32 = 0xCB;
const M_SOF13: u32 = 0xCD;
const M_SOF14: u32 = 0xCE;
const M_SOF15: u32 = 0xCF;
const M_EOI: u32 = 0xD9;
const M_SOS: u32 = 0xDA;
const M_APP0: u32 = 0xE0;
const M_APP15: u32 = 0xEF;

/// `php_read2`: 0 if the data could not be read.
fn read2(s: &mut Stream) -> u32 {
    match s.read_array::<2>() {
        Some(a) => ((a[0] as u32) << 8) + a[1] as u32,
        None => 0,
    }
}

/// `php_read4`: 0 if the data could not be read.
fn read4(s: &mut Stream) -> u32 {
    match s.read_array::<4>() {
        Some(a) => be32(&a),
        None => 0,
    }
}

/// `php_next_marker`: the next marker byte, swallowing padding.
fn next_marker(vm: &mut Vm, s: &mut Stream, ff_read: bool) -> PResult<u32> {
    if !ff_read {
        let mut extraneous: usize = 0;
        loop {
            let marker = s.getc();
            if marker == 0xff {
                break;
            }
            if marker < 0 {
                return Ok(M_EOI);
            }
            extraneous += 1;
        }
        if extraneous > 0 {
            flush_notices(vm, s)?;
            vm.warning(format!(
                "Corrupt JPEG data: {extraneous} extraneous bytes before marker"
            ))?;
        }
    }
    loop {
        let marker = s.getc();
        if marker < 0 {
            return Ok(M_EOI);
        }
        if marker != 0xff {
            return Ok(marker as u32);
        }
    }
}

/// `php_skip_variable`: skip a variable-length block.
fn skip_variable(s: &mut Stream) -> bool {
    let length = read2(s) as i64;
    if length < 2 {
        return false;
    }
    s.seek_cur(length - 2);
    true
}

/// `php_read_APP`: stores the first block of each APPn kind in `info`.
fn read_app(s: &mut Stream, marker: u32, info: &mut PhpArray) -> bool {
    let length = read2(s) as usize;
    if length < 2 {
        return false;
    }
    let length = length - 2;
    let mut buffer = vec![0u8; length];
    // `php_read_stream_all_chunks`: succeeds only if all `length` bytes arrive.
    if s.read(&mut buffer) != length as isize {
        return false;
    }
    let name = format!("APP{}", marker - M_APP0);
    // XXX we only catch the 1st tag of its kind!
    if info.get_str(name.as_bytes()).is_none() {
        info.insert_str(&name, Value::from(buffer));
    }
    true
}

/// `php_handle_jpeg`: main loop to parse the JPEG structure.
pub fn handle_jpeg(
    vm: &mut Vm,
    s: &mut Stream,
    mut info: Option<&mut PhpArray>,
) -> PResult<Option<GfxInfo>> {
    let mut result: Option<GfxInfo> = None;
    let mut ff_read = true;
    loop {
        let marker = next_marker(vm, s, ff_read)?;
        ff_read = false;
        match marker {
            M_SOF0 | M_SOF1 | M_SOF2 | M_SOF3 | M_SOF5 | M_SOF6 | M_SOF7 | M_SOF9 | M_SOF10
            | M_SOF11 | M_SOF13 | M_SOF14 | M_SOF15 => {
                if result.is_none() {
                    // Handle the SOFn block.
                    let length = read2(s);
                    let bits = s.getc() as u32;
                    let height = read2(s);
                    let width = read2(s);
                    let channels = s.getc() as u32;
                    result = Some(GfxInfo {
                        width,
                        height,
                        bits,
                        channels,
                    });
                    if info.is_none() || length < 8 {
                        // If we don't want an extended info -> return.
                        return Ok(result);
                    }
                    if s.seek_cur(length as i64 - 8) != 0 {
                        // File error after info.
                        return Ok(result);
                    }
                } else if !skip_variable(s) {
                    return Ok(result);
                }
            }
            M_APP0..=M_APP15 => {
                if let Some(info) = info.as_deref_mut() {
                    // Read all the app marks...
                    if !read_app(s, marker, info) {
                        return Ok(result);
                    }
                } else if !skip_variable(s) {
                    return Ok(result);
                }
            }
            // We're about to hit image data, or are at EOF. Stop processing.
            M_SOS | M_EOI => return Ok(result),
            // Anything else isn't interesting.
            _ => {
                if !skip_variable(s) {
                    return Ok(result);
                }
            }
        }
    }
}

const JPEG2000_MARKER_SIZ: i32 = 0x51;

/// `php_handle_jpc`: JPEG2000 raw codestream.
pub fn handle_jpc(vm: &mut Vm, s: &mut Stream) -> PResult<Option<GfxInfo>> {
    // JPEG 2000 components can be vastly different from one another; the
    // highest bit depth encountered is reported.

    // Get the single byte that remains after the file type identification.
    // (`unsigned char first_marker_id = php_stream_getc(stream)`)
    let first_marker_id = (s.getc() as u8) as i32;

    // Ensure that this marker is SIZ (as is mandated by the standard).
    if first_marker_id != JPEG2000_MARKER_SIZ {
        flush_notices(vm, s)?;
        vm.warning("JPEG2000 codestream corrupt(Expected SIZ marker not found after SOC)")?;
        return Ok(None);
    }

    read2(s); // Lsiz
    read2(s); // Rsiz
    let width = read4(s); // Xsiz
    let height = read4(s); // Ysiz

    if s.seek_cur(24) != 0 {
        return Ok(None);
    }

    let channels = read2(s); // Csiz
    if (channels == 0 && s.eof()) || channels > 256 {
        return Ok(None);
    }

    // Collect bit depth info.
    let mut highest_bit_depth: i32 = 0;
    for _ in 0..channels {
        let bit_depth = s.getc().wrapping_add(1); // Ssiz[i]
        if bit_depth > highest_bit_depth {
            highest_bit_depth = bit_depth;
        }
        s.getc(); // XRsiz[i]
        s.getc(); // YRsiz[i]
    }

    Ok(Some(GfxInfo {
        width,
        height,
        bits: highest_bit_depth as u32,
        channels,
    }))
}

/// `php_handle_jp2`: the JP2 wrapper format (a box structure).
pub fn handle_jp2(vm: &mut Vm, s: &mut Stream) -> PResult<Option<GfxInfo>> {
    let mut result = None;
    // It is valid for a JP2 file to contain multiple codestreams; the first one
    // at the root of the box structure is used.
    loop {
        let box_length = read4(s); // LBox
        let Some(box_type) = s.read_array::<4>() else {
            // Use this as a general "out of stream" error.
            break;
        };

        if box_length == 1 {
            // We won't handle XLBoxes.
            return Ok(None);
        }

        if &box_type == b"jp2c" {
            // Skip the first 3 bytes to emulate the file type examination.
            s.seek_cur(3);
            result = handle_jpc(vm, s)?;
            break;
        }

        // Stop if this was the last box.
        if (box_length as i32) <= 0 {
            break;
        }

        // Skip over LBox (which includes both TBox and LBox itself).
        if s.seek_cur(box_length.wrapping_sub(8) as i64) != 0 {
            break;
        }
    }

    if result.is_none() {
        flush_notices(vm, s)?;
        vm.warning("JP2 file has no codestreams at root level")?;
    }
    Ok(result)
}

fn ifd_get16u(b: &[u8], motorola_intel: bool) -> u32 {
    if motorola_intel {
        ((b[0] as u32) << 8) | b[1] as u32
    } else {
        ((b[1] as u32) << 8) | b[0] as u32
    }
}

fn ifd_get16s(b: &[u8], motorola_intel: bool) -> i16 {
    ifd_get16u(b, motorola_intel) as u16 as i16
}

fn ifd_get32s(b: &[u8], motorola_intel: bool) -> i32 {
    if motorola_intel {
        (((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | b[3] as u32) as i32
    } else {
        (((b[3] as u32) << 24) | ((b[2] as u32) << 16) | ((b[1] as u32) << 8) | b[0] as u32) as i32
    }
}

fn ifd_get32u(b: &[u8], motorola_intel: bool) -> u32 {
    ifd_get32s(b, motorola_intel) as u32
}

/// `php_handle_tiff`: main loop to parse the TIFF structure.
pub fn handle_tiff(s: &mut Stream, motorola_intel: bool) -> Option<GfxInfo> {
    const TAG_IMAGEWIDTH: u32 = 0x0100;
    const TAG_IMAGEHEIGHT: u32 = 0x0101;
    const TAG_COMP_IMAGEWIDTH: u32 = 0xA002;
    const TAG_COMP_IMAGEHEIGHT: u32 = 0xA003;

    let ifd_ptr = s.read_array::<4>()?;
    let ifd_addr = ifd_get32u(&ifd_ptr, motorola_intel) as u64;
    if s.seek_cur(ifd_addr.wrapping_sub(8) as i64) != 0 {
        return None;
    }
    let head = s.read_array::<2>()?;
    let num_entries = ifd_get16u(&head, motorola_intel) as usize;
    // 2 (num dir entries) + 12 * entries + 4 (offset to next ifd).
    let dir_size = 2 + 12 * num_entries + 4;
    let mut ifd_data = vec![0u8; dir_size];
    ifd_data[..2].copy_from_slice(&head);
    if s.read(&mut ifd_data[2..]) != (dir_size - 2) as isize {
        return None;
    }
    // Now we have the directory we can look how long it should be.
    let mut width: u64 = 0;
    let mut height: u64 = 0;
    for i in 0..num_entries {
        let dir_entry = &ifd_data[2 + i * 12..2 + (i + 1) * 12];
        let entry_tag = ifd_get16u(&dir_entry[0..], motorola_intel);
        let entry_type = ifd_get16u(&dir_entry[2..], motorola_intel);
        // Sign-extending conversions to size_t are intended (signed formats).
        let entry_value: u64 = match entry_type {
            1 | 6 => dir_entry[8] as u64,                            // BYTE, SBYTE
            3 => ifd_get16u(&dir_entry[8..], motorola_intel) as u64, // USHORT
            8 => ifd_get16s(&dir_entry[8..], motorola_intel) as i64 as u64, // SSHORT
            4 => ifd_get32u(&dir_entry[8..], motorola_intel) as u64, // ULONG
            9 => ifd_get32s(&dir_entry[8..], motorola_intel) as i64 as u64, // SLONG
            _ => continue,
        };
        match entry_tag {
            TAG_IMAGEWIDTH | TAG_COMP_IMAGEWIDTH => width = entry_value,
            TAG_IMAGEHEIGHT | TAG_COMP_IMAGEHEIGHT => height = entry_value,
            _ => {}
        }
    }
    if width != 0 && height != 0 {
        return Some(GfxInfo {
            width: width as u32,
            height: height as u32,
            bits: 0,
            channels: 0,
        });
    }
    None
}

/// `php_handle_iff`.
pub fn handle_iff(s: &mut Stream) -> Option<GfxInfo> {
    let mut a = [0u8; 10];
    if s.read(&mut a[..8]) != 8 {
        return None;
    }
    if &a[4..8] != b"ILBM" && &a[4..8] != b"PBM " {
        return None;
    }

    // Loop chunks to find the BMHD chunk.
    loop {
        if s.read(&mut a[..8]) != 8 {
            return None;
        }
        let chunk_id = ifd_get32s(&a[0..], true);
        let mut size = ifd_get32s(&a[4..], true);
        if size < 0 {
            return None;
        }
        if size & 1 == 1 {
            if size == i32::MAX {
                return None;
            }
            size += 1;
        }
        if chunk_id == 0x424d4844 {
            // BMHD chunk
            if size < 9 || s.read(&mut a[..9]) != 9 {
                return None;
            }
            let width = ifd_get16s(&a[0..], true);
            let height = ifd_get16s(&a[2..], true);
            let bits = a[8] as i16;
            if width > 0 && height > 0 && bits > 0 && bits < 33 {
                return Some(GfxInfo {
                    width: width as u32,
                    height: height as u32,
                    bits: bits as u32,
                    channels: 0,
                });
            }
        } else if s.seek_cur(size as i64) != 0 {
            return None;
        }
    }
}

/// `php_get_wbmp`: the WBMP dimensions, if the stream is a WBMP image.
pub fn get_wbmp(s: &mut Stream) -> Option<(u32, u32)> {
    if s.rewind() != 0 {
        return None;
    }

    // Get type.
    if s.getc() != 0 {
        return None;
    }

    // Skip header.
    loop {
        let i = s.getc();
        if i < 0 {
            return None;
        }
        if i & 0x80 == 0 {
            break;
        }
    }

    // Get width.
    let mut width: i32 = 0;
    loop {
        let i = s.getc();
        if i < 0 {
            return None;
        }
        width = (width << 7) | (i & 0x7f);
        // Maximum valid width for wbmp (although 127 may be a more accurate one).
        if width > 2048 {
            return None;
        }
        if i & 0x80 == 0 {
            break;
        }
    }

    // Get height.
    let mut height: i32 = 0;
    loop {
        let i = s.getc();
        if i < 0 {
            return None;
        }
        height = (height << 7) | (i & 0x7f);
        if height > 2048 {
            return None;
        }
        if i & 0x80 == 0 {
            break;
        }
    }

    if height == 0 || width == 0 {
        return None;
    }
    Some((width as u32, height as u32))
}

/// `php_handle_wbmp`.
pub fn handle_wbmp(s: &mut Stream) -> Option<GfxInfo> {
    get_wbmp(s).map(|(width, height)| GfxInfo {
        width,
        height,
        ..GfxInfo::default()
    })
}

/// C `isspace()` in the "C" locale.
fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `sscanf(line, "#define %s %d", name, &value) == 2`.
fn scan_define(line: &[u8]) -> Option<(&[u8], i32)> {
    let rest = line.strip_prefix(b"#define")?;
    let mut i = 0;
    while i < rest.len() && is_c_space(rest[i]) {
        i += 1;
    }
    let start = i;
    while i < rest.len() && !is_c_space(rest[i]) {
        i += 1;
    }
    if i == start {
        return None;
    }
    let name = &rest[start..i];
    while i < rest.len() && is_c_space(rest[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < rest.len() && (rest[i] == b'+' || rest[i] == b'-') {
        negative = rest[i] == b'-';
        i += 1;
    }
    let digits_start = i;
    let mut value: i64 = 0;
    while i < rest.len() && rest[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((rest[i] - b'0') as i64);
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    if negative {
        value = value.wrapping_neg();
    }
    // `%d` stores `(int) strtol(...)`.
    Some((name, value as i32))
}

/// `php_get_xbm`: the XBM dimensions, if the stream is an XBM image.
pub fn get_xbm(s: &mut Stream) -> Option<(u32, u32)> {
    if s.rewind() != 0 {
        return None;
    }
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    while let Some(fline) = s.gets() {
        // The line is a C string: sscanf stops at an embedded NUL.
        let line = match fline.iter().position(|&b| b == 0) {
            Some(n) => &fline[..n],
            None => &fline[..],
        };
        if let Some((iname, value)) = scan_define(line) {
            let kind = match iname.iter().rposition(|&c| c == b'_') {
                Some(i) => &iname[i + 1..],
                None => iname,
            };
            if kind == b"width" {
                width = value as u32;
                if height != 0 {
                    break;
                }
            }
            if kind == b"height" {
                height = value as u32;
                if width != 0 {
                    break;
                }
            }
        }
    }
    if width != 0 && height != 0 {
        Some((width, height))
    } else {
        None
    }
}

/// `php_handle_xbm`.
pub fn handle_xbm(s: &mut Stream) -> Option<GfxInfo> {
    get_xbm(s).map(|(width, height)| GfxInfo {
        width,
        height,
        ..GfxInfo::default()
    })
}

/// `php_handle_ico`.
pub fn handle_ico(s: &mut Stream) -> Option<GfxInfo> {
    let dim = s.read_array::<2>()?;
    let mut num_icons = ((dim[1] as u32) << 8) + dim[0] as u32;
    if !(1..=255).contains(&num_icons) {
        return None;
    }
    let mut result = GfxInfo::default();
    while num_icons > 0 {
        let Some(dim) = s.read_array::<16>() else {
            break;
        };
        let bits = ((dim[7] as u32) << 8) + dim[6] as u32;
        if bits >= result.bits {
            result.width = dim[0] as u32;
            result.height = dim[1] as u32;
            result.bits = bits;
        }
        num_icons -= 1;
    }
    if result.width == 0 {
        result.width = 256;
    }
    if result.height == 0 {
        result.height = 256;
    }
    Some(result)
}

/// `php_handle_webp`.
pub fn handle_webp(s: &mut Stream) -> Option<GfxInfo> {
    let buf = s.read_array::<18>()?;
    if &buf[..3] != b"VP8" {
        return None;
    }
    let format = match buf[3] {
        c @ (b' ' | b'L' | b'X') => c,
        _ => return None,
    };
    let b = |i: usize| buf[i] as u32;
    let (width, height) = match format {
        b' ' => (b(14) + ((b(15) & 0x3F) << 8), b(16) + ((b(17) & 0x3F) << 8)),
        b'L' => (
            b(9) + ((b(10) & 0x3F) << 8) + 1,
            (b(10) >> 6) + (b(11) << 2) + ((b(12) & 0xF) << 10) + 1,
        ),
        _ => (
            b(12) + (b(13) << 8) + (b(14) << 16) + 1,
            b(15) + (b(16) << 8) + (b(17) << 16) + 1,
        ),
    };
    Some(GfxInfo {
        width,
        height,
        bits: 8, // always 1 byte
        channels: 0,
    })
}

/// `php_handle_avif`: ISOBMFF (AVIF/HEIF) feature parsing with libavifinfo.
pub fn handle_avif(s: &mut Stream) -> Option<GfxInfo> {
    match avifinfo::get_features(s) {
        Ok(f) => Some(GfxInfo {
            width: f.width,
            height: f.height,
            bits: f.bit_depth,
            channels: f.num_channels,
        }),
        Err(_) => None,
    }
}

/// `php_is_image_avif`.
pub fn is_image_avif(s: &mut Stream) -> bool {
    avifinfo::identify(s) == avifinfo::AvifInfoStatus::Ok
}

/// The text `%s` prints for a PHP string: up to the first NUL byte.
pub fn c_string(s: &PhpStr) -> &[u8] {
    match s.iter().position(|&b| b == 0) {
        Some(n) => &s.as_bytes()[..n],
        None => s.as_bytes(),
    }
}
