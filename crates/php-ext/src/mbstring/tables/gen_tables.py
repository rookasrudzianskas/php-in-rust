#!/usr/bin/env python3
"""Generates the Rust conversion tables of ext/mbstring from php-src headers.

Usage (from the repository root):

    python3 crates/php-ext/src/mbstring/tables/gen_tables.py PHP_SRC/ext/mbstring

Every C array of the listed headers becomes a `pub static` with the C name in
upper case (`jisx0208_ucs_table` -> `JISX0208_UCS_TABLE`); scalar `int`
constants (`*_min`, `*_max`, `*_size`, `*_len`) become `pub const ...: u32`
with their values computed (php-src derives many of them with `sizeof`).
Numbers are written in hex without padding to keep the files compact.
"""

import os
import re
import sys

HEADERS = [
    ("libmbfl/filters/unicode_table_jis.h", "unicode_table_jis.rs"),
    ("libmbfl/filters/unicode_table_jis2004.h", "unicode_table_jis2004.rs"),
    ("libmbfl/filters/unicode_table_big5.h", "unicode_table_big5.rs"),
    ("libmbfl/filters/unicode_table_cns11643.h", "unicode_table_cns11643.rs"),
    ("libmbfl/filters/unicode_table_cp932_ext.h", "unicode_table_cp932_ext.rs"),
    ("libmbfl/filters/unicode_table_cp936.h", "unicode_table_cp936.rs"),
    ("libmbfl/filters/unicode_table_gb18030.h", "unicode_table_gb18030.rs"),
    ("libmbfl/filters/unicode_table_gb2312.h", "unicode_table_gb2312.rs"),
    ("libmbfl/filters/unicode_table_uhc.h", "unicode_table_uhc.rs"),
    ("libmbfl/filters/cp932_table.h", "cp932_table.rs"),
    ("libmbfl/filters/sjis_mac2uni.h", "sjis_mac2uni.rs"),
    ("libmbfl/filters/emoji2uni.h", "emoji2uni.rs"),
    ("libmbfl/filters/unicode_prop.h", "unicode_prop.rs"),
    ("libmbfl/mbfl/eaw_table.h", "eaw_table.rs"),
    ("unicode_data.h", "unicode_data.rs"),
    ("rare_cp_bitvec.h", "rare_cp_bitvec.rs"),
]

C_TYPES = {
    "unsigned short": ("u16", 2),
    "short": ("i16", 2),
    "unsigned int": ("u32", 4),
    "unsigned": ("u32", 4),
    "uint32_t": ("u32", 4),
    "int": ("i32", 4),
    "unsigned char": ("u8", 1),
}


def strip_comments(s):
    s = re.sub(r"/\*.*?\*/", "", s, flags=re.S)
    return re.sub(r"//[^\n]*", "", s)


def c_int(tok):
    tok = tok.strip()
    if not tok:
        return None
    if tok.startswith("'") and tok.endswith("'"):
        return ord(tok[1:-1])
    tok = re.sub(r"(?<=[0-9a-fA-F])[uUlL]+$", "", tok)
    tok = tok.replace("0X", "0x")
    return int(eval(tok))


def split_top(body):
    """Splits an initializer body at top-level commas."""
    items, depth, cur = [], 0, []
    for ch in body:
        if ch == "{":
            depth += 1
            if depth == 1:
                continue
        elif ch == "}":
            depth -= 1
            if depth == 0:
                continue
        if ch == "," and depth == 0:
            items.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if "".join(cur).strip():
        items.append("".join(cur))
    return [i for i in items if i.strip()]


def fmt(v):
    if v < 0:
        return "-0x%x" % -v if v < -9 else str(v)
    return "0x%x" % v if v > 9 else str(v)


def emit_array(out, name, rtype, values, per_line=12):
    out.append("pub static %s: [%s; %d] = [" % (name.upper(), rtype, len(values)))
    for i in range(0, len(values), per_line):
        out.append("    " + ", ".join(values[i:i + per_line]) + ",")
    out.append("];")
    out.append("")


def convert(path, rs_name):
    src = strip_comments(open(path, encoding="latin1").read())
    out = [
        "//! Generated from php-src `ext/mbstring/%s` by `gen_tables.py`; do not edit."
        % os.path.relpath(path, sys.argv[1]),
        "",
    ]
    sizes = {}

    # Anonymous struct arrays (eaw_table.h).
    for m in re.finditer(
        r"static const struct\s*\{([^}]*)\}\s*(\w+)\[\]\s*=\s*\{(.*?)\};", src, re.S
    ):
        fields = len(re.findall(r";", m.group(1)))
        name = m.group(2)
        rows = split_top(m.group(3))
        vals = []
        for r in rows:
            nums = [c_int(x) for x in r.split(",") if x.strip()]
            vals.append("[" + ", ".join(fmt(n) for n in nums) + "]")
        sizes[name] = len(rows)
        emit_array(out, name, "[u32; %d]" % fields, vals, per_line=6)
    src = re.sub(r"static const struct\s*\{[^}]*\}\s*\w+\[\]\s*=\s*\{.*?\};", "", src, flags=re.S)

    # Arrays of pointers to other arrays.
    for m in re.finditer(
        r"(?:static\s+)?const\s+unsigned short\s*\*\s*(\w+)\[\]\s*=\s*\{(.*?)\};", src, re.S
    ):
        name = m.group(1)
        refs = [x.strip() for x in m.group(2).split(",") if x.strip()]
        sizes[name] = len(refs)
        out.append(
            "pub static %s: [&[u16]; %d] = [%s];"
            % (name.upper(), len(refs), ", ".join("&" + r.upper() for r in refs))
        )
        out.append("")
    src = re.sub(
        r"(?:static\s+)?const\s+unsigned short\s*\*\s*\w+\[\]\s*=\s*\{.*?\};", "", src, flags=re.S
    )

    type_re = "|".join(sorted((re.escape(t) for t in C_TYPES), key=len, reverse=True))
    arr_re = re.compile(
        r"(?:static\s+)?const\s+(%s)\s+(\w+)\s*\[\s*\]((?:\s*\[\s*\d+\s*\])*)\s*=\s*\{(.*?)\};"
        % type_re,
        re.S,
    )
    for m in arr_re.finditer(src):
        ctype, name, dims, body = m.groups()
        rtype, _ = C_TYPES[ctype]
        inner = [int(d) for d in re.findall(r"\d+", dims)]
        if inner:
            rows = split_top(body)
            vals = []
            for r in rows:
                nums = [c_int(x) for x in r.split(",") if x.strip()]
                assert len(nums) == inner[0], (name, r)
                vals.append("[" + ", ".join(fmt(n) for n in nums) + "]")
            sizes[name] = len(rows)
            emit_array(out, name, "[%s; %d]" % (rtype, inner[0]), vals, per_line=6)
        else:
            nums = [c_int(x) for x in body.split(",") if x.strip()]
            sizes[name] = len(nums)
            emit_array(out, name, rtype, [fmt(n) for n in nums])
    src = arr_re.sub("", src)

    # Scalar constants.
    def eval_expr(expr):
        e = expr
        e = re.sub(
            r"sizeof\s*\(\s*(\w+)\s*\)\s*/\s*\(\s*sizeof\s*\(\s*[\w ]+\s*\)\s*\*\s*\d+\s*\)",
            lambda mm: str(sizes[mm.group(1)]),
            e,
        )
        e = re.sub(
            r"sizeof\s*\(\s*(\w+)\s*\)\s*/\s*sizeof\s*\(\s*[\w ]+\s*\)",
            lambda mm: str(sizes[mm.group(1)]),
            e,
        )
        e = " ".join(e.replace("0X", "0x").split())
        return int(eval(e))

    for m in re.finditer(
        r"(?:static\s+)?const\s+(?:unsigned\s+short|unsigned\s+int|unsigned|int)\s+(\w+)\s*=\s*([^;]+);",
        src,
    ):
        name, expr = m.groups()
        v = eval_expr(expr)
        assert v >= 0, (name, v)
        out.append("pub const %s: u32 = %s;" % (name.upper(), fmt(v)))
    for m in re.finditer(r"#define\s+(\w+)\s+(0x[0-9a-fA-F]+|\d+)\s*$", src, re.M):
        out.append("pub const %s: u32 = %s;" % (m.group(1).upper(), m.group(2)))
    out.append("")
    return "\n".join(out)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    root = sys.argv[1]
    here = os.path.dirname(os.path.abspath(__file__))
    for header, rs in HEADERS:
        text = convert(os.path.join(root, header), rs)
        with open(os.path.join(here, rs), "w") as f:
            f.write(text)
        print("wrote", rs)


if __name__ == "__main__":
    main()
