#!/usr/bin/env python3
"""Extracts the `mbfl_encoding` descriptors of php-src's libmbfl.

Usage (from the repository root):

    python3 crates/php-ext/src/mbstring/mbfl/gen_encodings.py PHP_SRC/ext/mbstring [--stubs]

Writes `mbfl/encodings.rs` (the registry in `mbfl_encoding_ptr_list` order)
and `filters/mblen.rs` (the `mblen_table_*` arrays). Each descriptor itself
lives in the filter module that ports the C file defining it; with `--stubs`
the descriptors are printed per module so they can be pasted there.
"""

import os
import re
import sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else None
HERE = os.path.dirname(os.path.abspath(__file__))

FILE_MODULES = {
    "filters/mbfilter_base64.c": "base64",
    "filters/mbfilter_7bit.c": "sevenbit",
    "filters/mbfilter_htmlent.c": "htmlent",
    "filters/mbfilter_qprint.c": "qprint",
    "filters/mbfilter_ucs4.c": "ucs4",
    "filters/mbfilter_ucs2.c": "ucs2",
    "filters/mbfilter_utf32.c": "utf32",
    "filters/mbfilter_utf16.c": "utf16",
    "filters/mbfilter_utf8.c": "utf8",
    "filters/mbfilter_utf7.c": "utf7",
    "filters/mbfilter_utf7imap.c": "utf7imap",
    "filters/mbfilter_uuencode.c": "uuencode",
    "filters/mbfilter_singlebyte.c": "singlebyte",
    "filters/mbfilter_cjk.c": "cjk",
    "mbfl/mbfilter_8bit.c": "eightbit",
    "mbfl/mbfilter_pass.c": "pass",
    "mbfl/mbfilter_wchar.c": "wchar",
}

# Sub-modules of `filters::cjk` by encoding id.
CJK_SUBMODULES = {
    "jis": "iso2022", "2022jp": "iso2022", "2022jp_kddi": "iso2022", "2022jp_2004": "iso2022",
    "cp50220": "iso2022", "cp50221": "iso2022", "cp50222": "iso2022", "2022jpms": "iso2022",
    "2022kr": "iso2022",
    "sjis": "sjis", "sjis_mac": "sjis", "sjis_docomo": "sjis", "sjis_kddi": "sjis", "sjis_sb": "sjis",
    "sjis2004": "sjis", "cp932": "sjis", "sjiswin": "sjis",
    "euc_jp": "eucjp", "eucjp2004": "eucjp", "eucjp_win": "eucjp", "cp51932": "eucjp",
    "euc_cn": "chinese", "euc_tw": "chinese", "gb18030": "chinese", "cp936": "chinese",
    "gb18030_2022": "chinese", "big5": "chinese", "cp950": "chinese", "hz": "chinese",
    "euc_kr": "korean", "uhc": "korean",
}

NO_ENCODING = {
    "pass": "Pass", "wchar": "Wchar", "base64": "Base64", "uuencode": "Uuencode",
    "html_ent": "HtmlEnt", "qprint": "Qprint", "7bit": "SevenBit", "8bit": "EightBit",
    "ucs4": "Ucs4", "ucs4be": "Ucs4be", "ucs4le": "Ucs4le", "ucs2": "Ucs2", "ucs2be": "Ucs2be",
    "ucs2le": "Ucs2le", "utf32": "Utf32", "utf32be": "Utf32be", "utf32le": "Utf32le",
    "utf16": "Utf16", "utf16be": "Utf16be", "utf16le": "Utf16le", "utf8": "Utf8",
    "utf8_docomo": "Utf8Docomo", "utf8_kddi_a": "Utf8KddiA", "utf8_kddi_b": "Utf8KddiB",
    "utf8_sb": "Utf8Sb", "utf7": "Utf7", "utf7imap": "Utf7imap", "ascii": "Ascii",
    "euc_jp": "EucJp", "eucjp2004": "Eucjp2004", "sjis": "Sjis", "eucjp_win": "EucjpWin",
    "sjis_docomo": "SjisDocomo", "sjis_kddi": "SjisKddi", "sjis_sb": "SjisSb", "sjis_mac": "SjisMac",
    "sjis2004": "Sjis2004", "cp932": "Cp932", "sjiswin": "Sjiswin", "cp51932": "Cp51932",
    "jis": "Jis", "2022jp": "Iso2022jp", "2022jp_2004": "Iso2022jp2004",
    "2022jp_kddi": "Iso2022jpKddi", "2022jpms": "Iso2022jpms", "gb18030": "Gb18030",
    "gb18030_2022": "Gb18030_2022", "cp1252": "Cp1252", "cp1254": "Cp1254",
    "8859_1": "Iso8859_1", "8859_2": "Iso8859_2", "8859_3": "Iso8859_3", "8859_4": "Iso8859_4",
    "8859_5": "Iso8859_5", "8859_6": "Iso8859_6", "8859_7": "Iso8859_7", "8859_8": "Iso8859_8",
    "8859_9": "Iso8859_9", "8859_10": "Iso8859_10", "8859_13": "Iso8859_13",
    "8859_14": "Iso8859_14", "8859_15": "Iso8859_15", "euc_cn": "EucCn", "cp936": "Cp936",
    "euc_tw": "EucTw", "big5": "Big5", "cp950": "Cp950", "euc_kr": "EucKr", "2022kr": "Iso2022kr",
    "uhc": "Uhc", "hz": "Hz", "cp1251": "Cp1251", "cp866": "Cp866", "koi8r": "Koi8r",
    "koi8u": "Koi8u", "8859_16": "Iso8859_16", "armscii8": "Armscii8", "cp850": "Cp850",
    "cp50220": "Cp50220", "cp50221": "Cp50221", "cp50222": "Cp50222",
}

FLAGS = {
    "0": "0",
    "MBFL_ENCTYPE_SBCS": "ENCTYPE_SBCS",
    "MBFL_ENCTYPE_WCS2": "ENCTYPE_WCS2",
    "MBFL_ENCTYPE_WCS4": "ENCTYPE_WCS4",
    "MBFL_ENCTYPE_GL_UNSAFE": "ENCTYPE_GL_UNSAFE",
}


def strip_comments(s):
    s = re.sub(r"/\*.*?\*/", "", s, flags=re.S)
    return re.sub(r"//[^\n]*", "", s)


def c_str(tok):
    tok = tok.strip()
    if tok == "NULL":
        return None
    assert tok.startswith('"') and tok.endswith('"'), tok
    return tok[1:-1]


def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def parse():
    encodings = {}
    for rel, module in FILE_MODULES.items():
        src = strip_comments(open(os.path.join(ROOT, "libmbfl", rel), encoding="latin1").read())
        aliases = {}
        for m in re.finditer(r"static const char \*(\w+)\[\]\s*=\s*\{(.*?)\};", src, re.S):
            items = [c_str(x) for x in m.group(2).split(",") if x.strip()]
            aliases[m.group(1)] = [x for x in items if x is not None]
        for m in re.finditer(r"const mbfl_encoding mbfl_encoding_(\w+)\s*=\s*\{(.*?)\};", src, re.S):
            ident = m.group(1)
            f = [x.strip() for x in m.group(2).split(",")]
            f = [x for x in f if x != ""]
            assert len(f) == 12, (ident, f)
            encodings[ident] = dict(
                module=module, no=f[0], name=c_str(f[1]), mime=c_str(f[2]),
                aliases=[] if f[3] == "NULL" else aliases[f[3]],
                mblen=None if f[4] == "NULL" else f[4],
                flag=f[5], input=None if f[6] == "NULL" else f[6].lstrip("&"),
                output=None if f[7] == "NULL" else f[7].lstrip("&"),
                to_wchar=f[8], from_wchar=f[9],
                check=None if f[10] == "NULL" else f[10],
                cut=None if f[11] == "NULL" else f[11],
            )
        for m in re.finditer(r'DEF_SB(?:_TBL)?\((\w+),\s*("[^"]*"),\s*("[^"]*"),\s*(\w+)', src):
            ident = m.group(1)
            encodings[ident] = dict(
                module=module, no="mbfl_no_encoding_" + ident, name=c_str(m.group(2)),
                mime=c_str(m.group(3)), aliases=aliases[m.group(4)], mblen=None,
                flag="MBFL_ENCTYPE_SBCS", input=None, output=None,
                to_wchar="mb_%s_to_wchar" % ident, from_wchar="mb_wchar_to_%s" % ident,
                check=None, cut=None,
            )
    return encodings


def module_of(ident, e):
    if e["module"] == "cjk":
        return "cjk::" + CJK_SUBMODULES[ident]
    return e["module"]


def descriptor(ident, e):
    flags = " | ".join(FLAGS[x.strip()] for x in e["flag"].split("|"))
    mime = "Some(%s)" % rust_str(e["mime"]) if e["mime"] is not None else "None"
    aliases = "&[" + ", ".join(rust_str(a) for a in e["aliases"]) + "]"
    mblen = "Some(&mblen::%s)" % e["mblen"].upper() if e["mblen"] else "None"
    vt = lambda v: "Some(&%s)" % v.upper() if v else "None"
    fn = lambda v: "Some(%s)" % v if v else "None"
    no = NO_ENCODING[e["no"].replace("mbfl_no_encoding_", "")]
    return (
        "pub static MBFL_ENCODING_%s: Encoding = Encoding {\n"
        "    no: NoEncoding::%s,\n"
        "    name: %s,\n"
        "    mime_name: %s,\n"
        "    aliases: %s,\n"
        "    mblen_table: %s,\n"
        "    flag: %s,\n"
        "    input_filter: %s,\n"
        "    output_filter: %s,\n"
        "    to_wchar: %s,\n"
        "    from_wchar: %s,\n"
        "    check: %s,\n"
        "    cut: %s,\n"
        "};\n"
    ) % (
        ident.upper(), no, rust_str(e["name"]), mime, aliases, mblen, flags,
        vt(e["input"]), vt(e["output"]), e["to_wchar"], e["from_wchar"],
        fn(e["check"]), fn(e["cut"]),
    )


def registry(encodings):
    src = strip_comments(open(os.path.join(ROOT, "libmbfl/mbfl/mbfl_encoding.c")).read())
    body = re.search(r"mbfl_encoding_ptr_list\[\]\s*=\s*\{(.*?)NULL", src, re.S).group(1)
    order = [x.strip().lstrip("&").replace("mbfl_encoding_", "") for x in body.split(",") if x.strip()]
    lines = [
        "//! The encoding registry in `mbfl_encoding_ptr_list` order",
        "//! (`mb_list_encodings()` order). Generated by `gen_encodings.py`.",
        "",
        "use super::encoding::Encoding;",
        "use crate::mbstring::filters::*;",
        "",
        "pub static ENCODING_LIST: [&Encoding; %d] = [" % len(order),
    ]
    for ident in order:
        lines.append("    &%s::MBFL_ENCODING_%s," % (module_of(ident, encodings[ident]), ident.upper()))
    lines.append("];")
    return "\n".join(lines) + "\n"


def mblen_tables():
    out = ["//! `mblen_table_*` (byte length by leading byte). Generated by `gen_encodings.py`.", ""]
    for rel in ("filters/mbfilter_utf8.c", "filters/mbfilter_cjk.c"):
        src = strip_comments(open(os.path.join(ROOT, "libmbfl", rel), encoding="latin1").read())
        for m in re.finditer(r"const unsigned char (mblen_table_\w+)\[\]\s*=\s*\{(.*?)\};", src, re.S):
            nums = [int(x.strip(), 0) for x in m.group(2).split(",") if x.strip()]
            assert len(nums) == 256, m.group(1)
            out.append("pub static %s: [u8; 256] = [" % m.group(1).upper())
            for i in range(0, 256, 16):
                out.append("    " + ", ".join(str(n) for n in nums[i:i + 16]) + ",")
            out.append("];")
            out.append("")
    return "\n".join(out)


def main():
    if ROOT is None:
        sys.exit(__doc__)
    encodings = parse()
    with open(os.path.join(HERE, "encodings.rs"), "w") as f:
        f.write(registry(encodings))
    with open(os.path.join(HERE, "..", "filters", "mblen.rs"), "w") as f:
        f.write(mblen_tables())
    if "--stubs" in sys.argv:
        by_module = {}
        for ident, e in encodings.items():
            by_module.setdefault(module_of(ident, e), []).append((ident, e))
        for module, items in sorted(by_module.items()):
            print("// ===== %s" % module)
            for ident, e in items:
                print(descriptor(ident, e))


if __name__ == "__main__":
    main()
