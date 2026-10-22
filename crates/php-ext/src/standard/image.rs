//! ext/standard/image.c: `getimagesize()`, `getimagesizefromstring()`,
//! `image_type_to_mime_type()`, `image_type_to_extension()` and the
//! `IMAGETYPE_*` constants.
//!
//! The format detection and per-format parsers are ported statement by
//! statement from php-src. They read through [`stream::Stream`], a faithful
//! model of the slice of `php_stream` image.c depends on (sequential reads,
//! seeks, `eof`, `gets`), over a lazily read local file or an in-memory string.

mod avifinfo;
mod formats;
mod inflate;
mod stream;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::rc::Rc;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use self::formats::{GfxInfo, c_string, flush_notices};
use self::stream::{Stream, strerror};

/// `enum image_filetype` (php_image.h).
mod filetype {
    pub const UNKNOWN: i32 = 0;
    pub const GIF: i32 = 1;
    pub const JPEG: i32 = 2;
    pub const PNG: i32 = 3;
    pub const SWF: i32 = 4;
    pub const PSD: i32 = 5;
    pub const BMP: i32 = 6;
    pub const TIFF_II: i32 = 7;
    pub const TIFF_MM: i32 = 8;
    pub const JPC: i32 = 9;
    pub const JP2: i32 = 10;
    pub const JPX: i32 = 11;
    pub const JB2: i32 = 12;
    pub const SWC: i32 = 13;
    pub const IFF: i32 = 14;
    pub const WBMP: i32 = 15;
    pub const XBM: i32 = 16;
    pub const ICO: i32 = 17;
    pub const WEBP: i32 = 18;
    pub const AVIF: i32 = 19;
    pub const HEIF: i32 = 20;
    pub const FIXED_COUNT: i32 = 21;
}

// File type markers.
const SIG_PNG: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const SIG_JP2: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a,
];

/// `php_image_type_to_mime_type`.
fn image_type_to_mime(image_type: i32) -> &'static str {
    use filetype::*;
    match image_type {
        GIF => "image/gif",
        JPEG => "image/jpeg",
        PNG => "image/png",
        SWF | SWC => "application/x-shockwave-flash",
        PSD => "image/psd",
        BMP => "image/bmp",
        TIFF_II | TIFF_MM => "image/tiff",
        IFF => "image/iff",
        WBMP => "image/vnd.wap.wbmp",
        JPC => "application/octet-stream",
        JP2 => "image/jp2",
        XBM => "image/xbm",
        ICO => "image/vnd.microsoft.icon",
        WEBP => "image/webp",
        AVIF => "image/avif",
        HEIF => "image/heif",
        // IMAGE_FILETYPE_UNKNOWN and anything else: suppose binary format.
        _ => "application/octet-stream",
    }
}

/// Get Mime-Type for image-type returned by getimagesize, exif_read_data, exif_thumbnail, exif_imagetype
fn image_type_to_mime_type(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // `php_image_type_to_mime_type(int)` takes a C int: the zend_long is truncated.
    Ok(Value::str(image_type_to_mime(args.int(0) as i32)))
}

/// Get file extension for image-type returned by getimagesize, exif_read_data, exif_thumbnail, exif_imagetype
fn image_type_to_extension(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    use filetype::*;
    let image_type = args.int(0);
    let include_dot = args.bool(1);
    let ext = match i32::try_from(image_type) {
        Ok(t) => match t {
            GIF => ".gif",
            JPEG => ".jpeg",
            PNG => ".png",
            SWF | SWC => ".swf",
            PSD => ".psd",
            BMP | WBMP => ".bmp",
            TIFF_II | TIFF_MM => ".tiff",
            IFF => ".iff",
            JPC => ".jpc",
            JP2 => ".jp2",
            JPX => ".jpx",
            JB2 => ".jb2",
            XBM => ".xbm",
            ICO => ".ico",
            WEBP => ".webp",
            AVIF => ".avif",
            HEIF => ".heif",
            _ => return Ok(Value::Bool(false)),
        },
        Err(_) => return Ok(Value::Bool(false)),
    };
    Ok(Value::str(&ext[usize::from(!include_dot)..]))
}

/// The "Error reading from %s!" notice (`%s` stops at a NUL byte).
fn error_reading(vm: &mut Vm, s: &mut Stream, input: &PhpStr) -> PResult<i32> {
    flush_notices(vm, s)?;
    let mut msg = b"Error reading from ".to_vec();
    msg.extend_from_slice(c_string(input));
    msg.push(b'!');
    vm.notice(PhpStr::from_vec(msg))?;
    Ok(filetype::UNKNOWN)
}

/// `php_getimagetype`: detect the file type from the first bytes.
fn getimagetype(vm: &mut Vm, s: &mut Stream, input: &PhpStr) -> PResult<i32> {
    let mut ft = [0u8; 12];
    if s.read(&mut ft[..3]) != 3 {
        return error_reading(vm, s, input);
    }

    // BYTES READ: 3
    if &ft[..3] == b"GIF" {
        return Ok(filetype::GIF);
    } else if ft[..3] == [0xff, 0xd8, 0xff] {
        return Ok(filetype::JPEG);
    } else if ft[..3] == SIG_PNG[..3] {
        if s.read(&mut ft[3..8]) != 5 {
            return error_reading(vm, s, input);
        }
        if ft[..8] == SIG_PNG {
            return Ok(filetype::PNG);
        }
        flush_notices(vm, s)?;
        vm.warning("PNG file corrupted by ASCII conversion")?;
        return Ok(filetype::UNKNOWN);
    } else if &ft[..3] == b"FWS" {
        return Ok(filetype::SWF);
    } else if &ft[..3] == b"CWS" {
        return Ok(filetype::SWC);
    } else if &ft[..3] == b"8BP" {
        return Ok(filetype::PSD);
    } else if &ft[..2] == b"BM" {
        return Ok(filetype::BMP);
    } else if ft[..3] == [0xff, 0x4f, 0xff] {
        return Ok(filetype::JPC);
    } else if &ft[..3] == b"RIF" {
        if s.read(&mut ft[3..12]) != 9 {
            return error_reading(vm, s, input);
        }
        if &ft[8..12] == b"WEBP" {
            return Ok(filetype::WEBP);
        }
        return Ok(filetype::UNKNOWN);
    }

    if s.read(&mut ft[3..4]) != 1 {
        return error_reading(vm, s, input);
    }
    // BYTES READ: 4
    if ft[..4] == [b'I', b'I', 0x2a, 0x00] {
        return Ok(filetype::TIFF_II);
    } else if ft[..4] == [b'M', b'M', 0x00, 0x2a] {
        return Ok(filetype::TIFF_MM);
    } else if &ft[..4] == b"FORM" {
        return Ok(filetype::IFF);
    } else if ft[..4] == [0x00, 0x00, 0x01, 0x00] {
        return Ok(filetype::ICO);
    }

    // WBMP may be smaller than 12 bytes, so delay error.
    let twelve_bytes_read = s.read(&mut ft[4..12]) == 8;

    // BYTES READ: 12
    if twelve_bytes_read && ft == SIG_JP2 {
        return Ok(filetype::JP2);
    }

    if s.rewind() == 0 && formats::is_image_avif(s) {
        return Ok(filetype::AVIF);
    }

    // See GH-20201: this needs to be after avif checks to avoid identifying avif as heif.
    if twelve_bytes_read
        && &ft[4..8] == b"ftyp"
        && (&ft[8..12] == b"mif1" || &ft[8..12] == b"heic" || &ft[8..12] == b"heix")
    {
        return Ok(filetype::HEIF);
    }

    // AFTER ALL ABOVE FAILED
    if formats::get_wbmp(s).is_some() {
        return Ok(filetype::WBMP);
    }

    if !twelve_bytes_read {
        return error_reading(vm, s, input);
    }

    if formats::get_xbm(s).is_some() {
        return Ok(filetype::XBM);
    }

    Ok(filetype::UNKNOWN)
}

/// `php_getimagesize_from_stream`.
fn getimagesize_from_stream(
    vm: &mut Vm,
    s: &mut Stream,
    input: &PhpStr,
    info: Option<&mut PhpArray>,
) -> PResult<Value> {
    let itype = getimagetype(vm, s, input)?;
    let result: Option<GfxInfo> = match itype {
        filetype::GIF => formats::handle_gif(s),
        filetype::JPEG => formats::handle_jpeg(vm, s, info)?,
        filetype::PNG => formats::handle_png(s),
        filetype::SWF => formats::handle_swf(s),
        filetype::SWC => formats::handle_swc(s),
        filetype::PSD => formats::handle_psd(s),
        filetype::BMP => formats::handle_bmp(s),
        filetype::TIFF_II => formats::handle_tiff(s, false),
        filetype::TIFF_MM => formats::handle_tiff(s, true),
        filetype::JPC => formats::handle_jpc(vm, s)?,
        filetype::JP2 => formats::handle_jp2(vm, s)?,
        filetype::IFF => formats::handle_iff(s),
        filetype::WBMP => formats::handle_wbmp(s),
        filetype::XBM => formats::handle_xbm(s),
        filetype::ICO => formats::handle_ico(s),
        filetype::WEBP => formats::handle_webp(s),
        filetype::AVIF => formats::handle_avif(s),
        filetype::HEIF => {
            if s.rewind() == 0 {
                formats::handle_avif(s)
            } else {
                None
            }
        }
        // IMAGE_FILETYPE_UNKNOWN (no registered handlers for other values).
        _ => None,
    };
    flush_notices(vm, s)?;

    let Some(result) = result else {
        return Ok(Value::Bool(false));
    };
    let mut arr = PhpArray::new();
    arr.insert(ArrayKey::Int(0), Value::Int(result.width as i64));
    arr.insert(ArrayKey::Int(1), Value::Int(result.height as i64));
    arr.insert(ArrayKey::Int(2), Value::Int(itype as i64));
    // The attribute string prints the unsigned dimensions with `%d`.
    arr.insert(
        ArrayKey::Int(3),
        Value::from(format!(
            "width=\"{}\" height=\"{}\"",
            result.width as i32, result.height as i32
        )),
    );
    if result.bits != 0 {
        arr.insert_str("bits", Value::Int(result.bits as i64));
    }
    if result.channels != 0 {
        arr.insert_str("channels", Value::Int(result.channels as i64));
    }
    arr.insert_str("mime", Value::str(image_type_to_mime(itype)));
    arr.insert_str("width_unit", Value::str("px"));
    arr.insert_str("height_unit", Value::str("px"));
    Ok(Value::array(arr))
}

/// Bytes to an OS path (PHP strings are byte strings).
pub(crate) fn path_from_bytes(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// Length of the `scheme` of a `scheme://...` (or `data:`) URL, as
/// `php_stream_locate_url_wrapper` finds it.
fn url_scheme_len(path: &[u8]) -> Option<usize> {
    let n = path
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
        .count();
    if path.get(n) == Some(&b':')
        && n > 1
        && (path[n + 1..].starts_with(b"//") || (n == 4 && path.starts_with(b"data:")))
    {
        Some(n)
    } else {
        None
    }
}

/// The file name to open for a `file://` URL, or `None` for a remote host.
fn file_url_path(p: &[u8], n: usize) -> Option<&[u8]> {
    let localhost = p.len() >= 17 && p[..17].eq_ignore_ascii_case(b"file://localhost/");
    if !localhost && p.get(n + 3).is_some_and(|&c| c != b'/') {
        return None;
    }
    // Skip past the protocol and ":/".
    let mut i = n + 1;
    if localhost {
        i += 11;
    }
    i += 1;
    while i < p.len() && p[i] == b'/' {
        i += 1;
    }
    Some(&p[i - 1..])
}

/// `php_stream_open_wrapper(path, "rb", STREAM_MUST_SEEK|REPORT_ERRORS|IGNORE_PATH)`.
///
/// Plain local files are read lazily with `std::fs`; every other wrapper goes
/// through `streams::read_all` (which yields a seekable in-memory copy, like
/// the `php://temp` PHP copies non-seekable streams into).
// TODO(streams): switch to the real stream objects once the streams package lands.
// TODO(open_basedir): `php_check_open_basedir` is not available in the runtime yet.
fn open_path(vm: &mut Vm, path: &PhpStr) -> PResult<Option<Stream>> {
    let p = path.as_bytes();
    if p.is_empty() {
        return Err(vm.throw_value_error("Path must not be empty"));
    }
    let local: Option<&[u8]> = match url_scheme_len(p) {
        None => Some(p),
        Some(n) if p[..n].eq_ignore_ascii_case(b"file") => match file_url_path(p, n) {
            Some(f) => Some(f),
            None => {
                let mut msg = b"Remote host file access not supported, ".to_vec();
                msg.extend_from_slice(p);
                vm.warning(PhpStr::from_vec(msg))?;
                return Ok(None);
            }
        },
        Some(_) => None,
    };
    match local {
        Some(file) => match std::fs::File::open(path_from_bytes(file)) {
            Ok(f) => Ok(Some(Stream::from_file(f))),
            Err(e) => {
                vm.warning(format!("Failed to open stream: {}", strerror(&e)))?;
                Ok(None)
            }
        },
        None => Ok(crate::standard::streams::read_all(vm, p)?
            .map(|data| Stream::from_memory(PhpStr::from_vec(data)))),
    }
}

const FROM_DATA: bool = false;
const FROM_PATH: bool = true;

/// `php_getimagesize_from_any`.
fn getimagesize_from_any(vm: &mut Vm, args: &mut Args, mode: bool) -> PResult<Value> {
    let input = args.str(0).clone();

    if mode == FROM_PATH && input.as_bytes().contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }

    // `argc == 2`: `$image_info` is (re)initialised to an empty array.
    let info_ref = if args.len() == 2 {
        let r = args.by_ref(1);
        r.set(Value::array(PhpArray::new()));
        Some(r)
    } else {
        None
    };

    let mut stream = if mode == FROM_PATH {
        match open_path(vm, &input)? {
            Some(s) => s,
            None => return Ok(Value::Bool(false)),
        }
    } else {
        Stream::from_memory(input.clone())
    };

    let mut info = info_ref.as_ref().map(|_| PhpArray::new());
    let result = getimagesize_from_stream(vm, &mut stream, &input, info.as_mut());
    if let (Some(r), Some(info)) = (&info_ref, info) {
        r.set(Value::Array(Rc::new(info)));
    }
    result
}

/// Get the size of an image as 4-element array
fn getimagesize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    getimagesize_from_any(vm, args, FROM_PATH)
}

/// Get the size of an image as 4-element array
fn getimagesizefromstring(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    getimagesize_from_any(vm, args, FROM_DATA)
}

pub static IMAGE_TYPE_TO_MIME_TYPE: NativeFunction = NativeFunction {
    sig: "image_type_to_mime_type(int $image_type): string",
    handler: image_type_to_mime_type,
};

pub static IMAGE_TYPE_TO_EXTENSION: NativeFunction = NativeFunction {
    sig: "image_type_to_extension(int $image_type, bool $include_dot = true): string|false",
    handler: image_type_to_extension,
};

pub static GETIMAGESIZE: NativeFunction = NativeFunction {
    sig: "getimagesize(string $filename, &$image_info = null): array|false",
    handler: getimagesize,
};

pub static GETIMAGESIZEFROMSTRING: NativeFunction = NativeFunction {
    sig: "getimagesizefromstring(string $string, &$image_info = null): array|false",
    handler: getimagesizefromstring,
};

pub fn register(r: &mut Registry) {
    use filetype::*;
    r.functions(&[
        &IMAGE_TYPE_TO_MIME_TYPE,
        &IMAGE_TYPE_TO_EXTENSION,
        &GETIMAGESIZE,
        &GETIMAGESIZEFROMSTRING,
    ]);

    // Same order as the generated registration of basic_functions.stub.php.
    for (name, value) in [
        ("IMAGETYPE_GIF", GIF),
        ("IMAGETYPE_JPEG", JPEG),
        ("IMAGETYPE_PNG", PNG),
        ("IMAGETYPE_SWF", SWF),
        ("IMAGETYPE_PSD", PSD),
        ("IMAGETYPE_BMP", BMP),
        ("IMAGETYPE_TIFF_II", TIFF_II),
        ("IMAGETYPE_TIFF_MM", TIFF_MM),
        ("IMAGETYPE_JPC", JPC),
        ("IMAGETYPE_JP2", JP2),
        ("IMAGETYPE_JPX", JPX),
        ("IMAGETYPE_JB2", JB2),
        // Only defined when zlib is built in (it is, like a default php build).
        ("IMAGETYPE_SWC", SWC),
        ("IMAGETYPE_IFF", IFF),
        ("IMAGETYPE_WBMP", WBMP),
        // keep alias
        ("IMAGETYPE_JPEG2000", JPC),
        ("IMAGETYPE_XBM", XBM),
        ("IMAGETYPE_ICO", ICO),
        ("IMAGETYPE_WEBP", WEBP),
        ("IMAGETYPE_AVIF", AVIF),
        ("IMAGETYPE_HEIF", HEIF),
        ("IMAGETYPE_UNKNOWN", UNKNOWN),
        ("IMAGETYPE_COUNT", FIXED_COUNT),
    ] {
        r.constant(name, value as i64);
    }
}
