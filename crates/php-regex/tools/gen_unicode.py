#!/usr/bin/env python3
"""Generate crates/php-regex/src/unicode/tables.rs.

The generated tables back the PCRE2-10.44-compatible Unicode property API in
crates/php-regex/src/unicode/mod.rs. PCRE2 10.44 uses Unicode 15.0.0.

Data sources (no network access needed):

* Python's ``unicodedata`` (Unicode 15.0.0 in Python 3.12): General_Category,
  Bidi_Class and Bidi_Mirrored of assigned code points, canonical combining
  class (for Grapheme_Link), character names, and case mappings of code points
  new in Unicode 14/15.
* Perl's ``Unicode::UCD`` (Unicode 13.0.0 in Perl 5.34): Script,
  Script_Extensions, Grapheme_Cluster_Break, Extended_Pictographic, the other
  binary properties, Bidi_Mirroring_Glyph, Bidi_Class of unassigned code
  points, Simple_Case_Folding and Block.
* ``php-src-ref/ext/pcre/pcre2lib/pcre2_ucp.h``: the PCRE2 enum orders, which
  are the numeric values used by the generated tables.

Code points that are assigned in Unicode 15.0 (per Python) but unassigned in
Perl's 13.0 data get values inferred from their name, general category and
case mappings (see ``infer_script`` and ``infer_new_code_point``). Changes that
Unicode 14.0/15.0 made to code points that already existed in 13.0 are listed
explicitly in ``OLD_CODE_POINT_UPDATES``.

PCRE2 quirks reproduced here (as in PCRE2's maint/GenerateUcd.py):

* Bidi_Mirrored is the set of code points that have a Bidi_Mirroring_Glyph
  (BidiMirroring.txt), not the UnicodeData.txt flag.
* other_case comes from Simple_Case_Folding (statuses C and S); non-ASCII
  members of a caseless set that contains an ASCII character (U+017F, U+212A)
  have no other case, so that PCRE2_EXTRA_CASELESS_RESTRICT works.

Usage: python3 tools/gen_unicode.py [--dump per_code_point.json]
"""

import json
import os
import re
import subprocess
import sys
import unicodedata
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
ROOT = os.path.dirname(os.path.dirname(CRATE))
UCP_H = os.path.join(ROOT, "php-src-ref", "ext", "pcre", "pcre2lib", "pcre2_ucp.h")
OUT = os.path.join(CRATE, "src", "unicode", "tables.rs")

MAX = 0x110000

# ---------------------------------------------------------------------------
# PCRE2 enum orders
# ---------------------------------------------------------------------------


def read_enums():
    src = open(UCP_H).read()
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    enums = [re.findall(r"\bucp_(\w+)", e) for e in re.findall(r"enum\s*\{(.*?)\};", src, re.S)]
    gentypes, chartypes, bprops, bidis, gbs, scripts = enums
    assert bprops[-1] == "Bprop_Count" and scripts[-1] == "Script_Count"
    bprops, scripts = bprops[:-1], scripts[:-1]
    bidis = [b[len("bidi") :] for b in bidis]
    gbs = [g[len("gb") :] for g in gbs]
    assert len(gentypes) == 7 and len(chartypes) == 30 and len(bprops) == 52
    assert len(bidis) == 23 and len(gbs) == 15
    return gentypes, chartypes, bprops, bidis, gbs, scripts


GENTYPES, CHARTYPES, BPROPS, BIDIS, GBS, SCRIPTS = read_enums()
CT = {n: i for i, n in enumerate(CHARTYPES)}
BP = {n: i for i, n in enumerate(BPROPS)}
BIDI = {n: i for i, n in enumerate(BIDIS)}
GB = {n: i for i, n in enumerate(GBS)}
SC = {n: i for i, n in enumerate(SCRIPTS)}
SC_UNKNOWN = SC["Unknown"]

# ISO 15924 codes of the scripts that Perl's Unicode 13 data does not know.
NEW_SCRIPT_ALIASES = {
    "Cypro_Minoan": ["Cpmn"],
    "Old_Uyghur": ["Ougr"],
    "Tangsa": ["Tnsa"],
    "Toto": ["Toto"],
    "Vithkuqi": ["Vith"],
    "Kawi": ["Kawi"],
    "Nag_Mundari": ["Nagm"],
}

# Binary property aliases that Perl does not report (Grapheme_Link is
# deprecated and missing from Unicode::UCD).
EXTRA_BPROP_ALIASES = {"Grapheme_Link": ["Gr_Link"]}

# Unicode 14.0/15.0 changes to code points that were already assigned in 13.0.
OLD_CODE_POINT_UPDATES = {
    # Script / Script_Extensions (long names; the first scx entry list replaces
    # the whole Script_Extensions value).
    "script": {0x16FE2: "Han", 0x16FE3: "Han"},
    "scx": {
        0x060C: ["Arabic", "Hanifi_Rohingya", "Nko", "Syriac", "Thaana", "Yezidi"],
        0x061B: ["Arabic", "Hanifi_Rohingya", "Nko", "Syriac", "Thaana", "Yezidi"],
        0x061F: ["Adlam", "Arabic", "Hanifi_Rohingya", "Nko", "Syriac", "Thaana", "Yezidi"],
        0x0640: [
            "Adlam", "Arabic", "Hanifi_Rohingya", "Mandaic", "Manichaean", "Old_Uyghur",
            "Psalter_Pahlavi", "Sogdian", "Syriac",
        ],
        0xFD3E: ["Arabic", "Nko"],
        0xFD3F: ["Arabic", "Nko"],
        0x10100: ["Cypriot", "Cypro_Minoan", "Linear_B"],
        0x10101: ["Cypriot", "Cypro_Minoan", "Linear_B"],
        0x10AF2: ["Manichaean", "Old_Uyghur"],
        # New in 14.0, listed here because its Script_Extensions is not {Script}.
        0x1DFA: ["Syriac"],
    },
    # Grapheme_Cluster_Break: AHOM VOWEL SIGN A/AA left SpacingMark in 14.0.
    "gcb": {0x11720: "Other", 0x11721: "Other"},
    # Binary properties set (True) or cleared (False).
    "bprops": {
        "Alphabetic": ({0x0C04, 0x0F82, 0x0F83, 0x11080, 0x11081}, True),
        "Lowercase": ({0x10FC, 0xAB69}, True),
        "Cased": ({0x10FC, 0xAB69}, True),
        "Diacritic": ({0x1714, 0x1ABE, 0x1DFB, 0x1DFC, 0x11046}, True),
    },
}

# Ranges whose unassigned code points default to Bidi_Class=AL since 14.0/15.0
# (Arabic Extended-B and Arabic Extended-C); Perl's 13.0 data says R.
NEW_AL_DEFAULT_RANGES = [(0x0870, 0x089F), (0x10EC0, 0x10EFF)]

# Emoji_Modifier_Base characters added in Emoji 14.0/15.0.
NEW_EMOJI_MODIFIER_BASE = [(0x1FAC3, 0x1FAC5), (0x1FAF0, 0x1FAF8)]

# New modifier letters and marks are Diacritic, except these.
NEW_NOT_DIACRITIC = [(0x0ECE, 0x0ECE), (0xA7F2, 0xA7F4), (0x11F41, 0x11F42), (0x13440, 0x13440), (0x1E4EB, 0x1E4EF)]

# Blocks of pictographs whose new So characters are emoji (Emoji and
# Emoji_Presentation); other new symbols near them are not.
EMOJI_BLOCKS = [(0x1F300, 0x1F64F), (0x1F680, 0x1F6FF), (0x1F7E0, 0x1F7FF), (0x1F900, 0x1F9FF), (0x1FA70, 0x1FAFF)]

# ---------------------------------------------------------------------------
# Perl (Unicode 13.0) data
# ---------------------------------------------------------------------------

PERL_SCRIPT = r"""
use strict;
use warnings;
use Unicode::UCD qw(prop_invmap prop_invlist prop_aliases prop_value_aliases);
use JSON::PP;
my %out;
$out{version} = Unicode::UCD::UnicodeVersion();
for my $p (qw(General_Category Script Script_Extensions Grapheme_Cluster_Break
              Bidi_Class Bidi_Mirroring_Glyph Block Simple_Case_Folding)) {
    my ($l, $m, $f, $d) = prop_invmap($p);
    die "no invmap for $p" unless $l;
    $out{invmap}{$p} = { starts => $l, values => $m, format => $f };
}
for my $p (@ARGV) {
    my @l = prop_invlist($p);
    $out{invlist}{$p} = \@l;
    $out{aliases}{$p} = [prop_aliases($p)];
}
my (undef, $m) = prop_invmap('Script');
my %seen;
for my $s (grep { !$seen{$_}++ } @$m) {
    $out{script_aliases}{$s} = [prop_value_aliases('sc', $s)];
}
print JSON::PP->new->canonical->encode(\%out);
"""


def run_perl():
    props = [p for p in BPROPS if p != "Grapheme_Link"]
    res = subprocess.run(["perl", "-e", PERL_SCRIPT, *props], capture_output=True, text=True, check=True)
    data = json.loads(res.stdout)
    assert data["version"] == "13.0.0", data["version"]
    for p in props:
        assert data["invlist"][p], "Perl has no data for " + p
    return data


def expand(invmap, conv=lambda v: v):
    starts, values = [int(x) for x in invmap["starts"]], invmap["values"]
    out = [None] * MAX
    for i, s in enumerate(starts):
        e = starts[i + 1] if i + 1 < len(starts) else MAX
        v = conv(values[i])
        for c in range(s, min(e, MAX)):
            out[c] = v
    return out


def invlist_to_bools(lst):
    lst = [int(x) for x in lst]
    out = bytearray(MAX)
    for i in range(0, len(lst), 2):
        e = lst[i + 1] if i + 1 < len(lst) else MAX
        for c in range(lst[i], e):
            out[c] = 1
    return out


def norm(s):
    return re.sub(r"[_\-\s]", "", s).lower()


# ---------------------------------------------------------------------------
# Heuristics for code points new in Unicode 14.0/15.0
# ---------------------------------------------------------------------------

# Name prefixes that do not spell the script name.
EXTRA_SCRIPT_PREFIXES = {
    "CJK UNIFIED IDEOGRAPH": "Han",
    "CJK COMPATIBILITY IDEOGRAPH": "Han",
    "CANADIAN SYLLABICS": "Canadian_Aboriginal",
    "EGYPTIAN HIEROGLYPH": "Egyptian_Hieroglyphs",
    "ANATOLIAN HIEROGLYPH": "Anatolian_Hieroglyphs",
    "COMBINING CYRILLIC": "Cyrillic",
    "MODIFIER LETTER CYRILLIC": "Cyrillic",
    "COMBINING": "Inherited",
    "MODIFIER LETTER": "Latin",
}

SCRIPT_PREFIXES = sorted(
    [(s.upper().replace("_", " "), s) for s in SCRIPTS if s not in ("Unknown", "Common", "Inherited")]
    + list(EXTRA_SCRIPT_PREFIXES.items()),
    key=lambda kv: -len(kv[0]),
)

IDEOGRAPH_PREFIXES = ("CJK UNIFIED IDEOGRAPH", "CJK COMPATIBILITY IDEOGRAPH", "TANGUT", "KHITAN SMALL SCRIPT CHARACTER", "NUSHU CHARACTER")


def infer_script(c, name, gc, block, perl_assigned, script):
    """Script of a new code point: name prefix, else marks are Inherited, else
    the script of the nearest Unicode 13 code point in the same block, else
    Common."""
    n = name.replace("-", " ")
    for prefix, s in SCRIPT_PREFIXES:
        if n == prefix or n.startswith(prefix + " "):
            return SC[s]
    if CHARTYPES[gc] in ("Mn", "Me"):
        return SC["Inherited"]
    blk = block[c]
    if blk != "No_Block":
        for d in range(1, 0x1000):
            for x in (c - d, c + d):
                if 0 <= x < MAX and block[x] == blk and perl_assigned[x]:
                    return script[x]
    return SC["Common"]


def changes(f, s):
    nfd = unicodedata.normalize("NFD", s)
    return f(nfd) != nfd


def in_ranges(c, ranges):
    return any(a <= c <= b for a, b in ranges)


def gcb_from_gc(g):
    if g in ("Mn", "Me"):
        return "Extend"
    if g == "Mc":
        return "SpacingMark"
    if g in ("Cc", "Zl", "Zp", "Cf"):
        return "Control"
    return "Other"


def infer_new_code_point(c, gc, name, script, perl_bits):
    """Return (gcb, {bprop: bool}) for a code point new in Unicode 14/15."""
    g = CHARTYPES[gc]
    ch = chr(c)
    n = name.replace("-", " ")

    pcm = g == "Cf" and name.startswith("ARABIC") and "MARK ABOVE" in name
    gcb = gcb_from_gc(g)
    if pcm or (g == "Lo" and "SIGN REPHA" in name):
        gcb = "Prepend"

    letter = g in ("Lu", "Ll", "Lt", "Lm", "Lo", "Nl")
    mark = g in ("Mn", "Mc", "Me")
    # Dependent vowels and nasalization/aspiration signs are Other_Alphabetic;
    # viramas, nuktas, tone marks and other diacritics are Diacritic.
    vowelish = re.search(r"VOWEL SIGN|CANDRABINDU|ANUSVARA|VISARGA|CONSONANT SIGN|LENGTH MARK", name) is not None
    combining_letter = re.match(r"COMBINING (LATIN|CYRILLIC) SMALL LETTER", name) is not None
    v = {}
    v["Alphabetic"] = letter or (mark and (vowelish or combining_letter))
    diacritic_mark = mark and not (vowelish or combining_letter or "VARIATION SELECTOR" in name)
    v["Diacritic"] = (diacritic_mark or g == "Lm") and not in_ranges(c, NEW_NOT_DIACRITIC)
    lower_lm = g == "Lm" and re.match(r"MODIFIER LETTER (?!SUPERSCRIPT)|CYRILLIC SUBSCRIPT SMALL LETTER", name) is not None
    v["Lowercase"] = g == "Ll" or lower_lm
    v["Uppercase"] = g == "Lu"
    v["Cased"] = v["Lowercase"] or v["Uppercase"] or g == "Lt"
    v["Case_Ignorable"] = g in ("Mn", "Me", "Cf", "Lm", "Sk")
    v["Changes_When_Lowercased"] = changes(str.lower, ch)
    v["Changes_When_Uppercased"] = changes(str.upper, ch)
    v["Changes_When_Titlecased"] = changes(str.title, ch)
    v["Changes_When_Casefolded"] = changes(str.casefold, ch)
    v["Changes_When_Casemapped"] = (
        v["Changes_When_Lowercased"] or v["Changes_When_Uppercased"] or v["Changes_When_Titlecased"]
    )
    v["ID_Start"] = v["XID_Start"] = letter
    v["ID_Continue"] = v["XID_Continue"] = letter or g in ("Mn", "Mc", "Nd", "Pc")
    v["Grapheme_Extend"] = g in ("Mn", "Me")
    v["Grapheme_Base"] = g not in ("Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp", "Mn", "Me")
    v["Variation_Selector"] = "VARIATION SELECTOR" in name
    v["Default_Ignorable_Code_Point"] = perl_bits["Default_Ignorable_Code_Point"] or v["Variation_Selector"]
    v["Ideographic"] = g == "Lo" and name.startswith(IDEOGRAPH_PREFIXES)
    v["Unified_Ideograph"] = name.startswith("CJK UNIFIED IDEOGRAPH")
    v["Extender"] = g == "Lm" and "TRIANGULAR COLON" in name
    v["Math"] = g == "Sm"
    v["Dash"] = g == "Pd"
    v["Quotation_Mark"] = "QUOTATION MARK" in name
    sterm = g == "Po" and re.search(
        r"DANDA|FULL STOP|QUESTION MARK|EXCLAMATION MARK|END OF TEXT|PANTI|PAMADA|PUNCTUATION (BAR|TWO|FOUR)", name
    )
    v["Sentence_Terminal"] = bool(sterm)
    v["Terminal_Punctuation"] = bool(sterm) or (g == "Po" and re.search(r"COMMA|COLON|SEMICOLON", name) is not None)
    if script == SC["Latin"]:
        sd = re.search(r"SMALL (LETTER )?[IJ]( WITH|$)", n)
    elif script == SC["Cyrillic"]:
        sd = re.search(r"SMALL (LETTER )?(BYELORUSSIAN UKRAINIAN I|JE)( WITH|$)", n)
    else:
        sd = None
    v["Soft_Dotted"] = bool(sd) and not mark
    v["Prepended_Concatenation_Mark"] = pcm
    emoji = g == "So" and perl_bits["Extended_Pictographic"] and in_ranges(c, EMOJI_BLOCKS)
    v["Emoji"] = v["Emoji_Presentation"] = emoji
    v["Emoji_Modifier_Base"] = in_ranges(c, NEW_EMOJI_MODIFIER_BASE)
    return gcb, v


# ---------------------------------------------------------------------------
# Build per-code-point data
# ---------------------------------------------------------------------------


def build():
    perl = run_perl()
    inv = perl["invmap"]

    perl_gc = expand(inv["General_Category"])
    perl_assigned = bytearray(1 if g != "Cn" else 0 for g in perl_gc)
    block = expand(inv["Block"])
    has_mirror = expand(inv["Bidi_Mirroring_Glyph"], lambda v: v != "")

    sc_by_norm = {norm(s): i for i, s in enumerate(SCRIPTS)}

    def sc_id(name):
        return sc_by_norm[norm(name)]

    script = expand(inv["Script"], sc_id)

    def scx_conv(v):
        if isinstance(v, list):
            return tuple(sorted(sc_id(x) for x in v))
        return (sc_id(v),)

    scx_raw = expand(inv["Script_Extensions"], scx_conv)

    gcb_map = {"ExtPict_XX": "Other", "E_Base": "Other", "E_Base_GAZ": "Other", "Glue_After_Zwj": "Other", "E_Modifier": "Extend"}
    gcb = expand(inv["Grapheme_Cluster_Break"], lambda v: GB[gcb_map.get(v, v)])
    bidi = expand(inv["Bidi_Class"], lambda v: BIDI[v])

    scf_inv = inv["Simple_Case_Folding"]
    assert scf_inv["format"] == "a"
    scf = list(range(MAX))
    st, vals = [int(x) for x in scf_inv["starts"]], scf_inv["values"]
    for i, s in enumerate(st):
        v = int(vals[i])
        if v == 0:
            continue
        e = st[i + 1] if i + 1 < len(st) else MAX
        for c in range(s, e):
            scf[c] = v + (c - s)

    bools = []
    for p in BPROPS:
        if p == "Grapheme_Link":
            bools.append(bytearray(MAX))
        else:
            bools.append(invlist_to_bools(perl["invlist"][p]))

    # Python (Unicode 15.0) data.
    assert unicodedata.unidata_version == "15.0.0", unicodedata.unidata_version
    gc = [CT[unicodedata.category(chr(c))] for c in range(MAX)]
    new_cps = [c for c in range(MAX) if gc[c] != CT["Cn"] and not perl_assigned[c]]
    gone = [c for c in range(MAX) if gc[c] == CT["Cn"] and perl_assigned[c]]
    assert not gone, "code points unassigned in 15.0 but assigned in 13.0"

    # Old code points whose general category changed: re-derive the
    # properties that follow directly from it.
    for c in range(MAX):
        if perl_assigned[c] and gc[c] != CT[perl_gc[c]]:
            g = CHARTYPES[gc[c]]
            gcb[c] = GB[gcb_from_gc(g)]
            bools[BP["Grapheme_Extend"]][c] = g in ("Mn", "Me")
            bools[BP["Grapheme_Base"]][c] = g not in ("Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp", "Mn", "Me")
            bools[BP["Case_Ignorable"]][c] = g in ("Mn", "Me", "Cf", "Lm", "Sk")

    for c in new_cps:
        name = unicodedata.name(chr(c), "")
        s = infer_script(c, name, gc[c], block, perl_assigned, script)
        script[c] = s
        scx_raw[c] = (s,)
        perl_bits = {p: bool(bools[BP[p]][c]) for p in BPROPS}
        g, v = infer_new_code_point(c, gc[c], name, s, perl_bits)
        gcb[c] = GB[g]
        for p, val in v.items():
            bools[BP[p]][c] = 1 if val else 0
        folded = chr(c).casefold()
        if len(folded) == 1 and folded != chr(c):
            scf[c] = ord(folded)
        has_mirror[c] = True

    upd = OLD_CODE_POINT_UPDATES
    for c, s in upd["script"].items():
        script[c] = SC[s]
        scx_raw[c] = (SC[s],)
    for c, lst in upd["scx"].items():
        scx_raw[c] = tuple(sorted(SC[s] for s in lst))
    for c, g in upd["gcb"].items():
        gcb[c] = GB[g]
    for p, (cps, val) in upd["bprops"].items():
        for c in cps:
            bools[BP[p]][c] = 1 if val else 0

    # Properties taken from Python for every assigned code point.
    for c in range(MAX):
        if gc[c] != CT["Cn"]:
            ch = chr(c)
            bidi[c] = BIDI[unicodedata.bidirectional(ch)]
            bools[BP["Bidi_Mirrored"]][c] = 1 if unicodedata.mirrored(ch) and has_mirror[c] else 0
            bools[BP["Grapheme_Link"]][c] = 1 if unicodedata.combining(ch) == 9 else 0
        elif in_ranges(c, NEW_AL_DEFAULT_RANGES):
            bidi[c] = BIDI["AL"]

    # Extended_Pictographic overrides the grapheme break property "Other".
    ep = bools[BP["Extended_Pictographic"]]
    for c in range(MAX):
        if ep[c]:
            assert gcb[c] == GB["Other"], hex(c)
            gcb[c] = GB["Extended_Pictographic"]

    # Script extensions: keep only lists that differ from {script}.
    scx = [None] * MAX
    for c in range(MAX):
        lst = scx_raw[c]
        if lst != (script[c],):
            scx[c] = lst
            # PCRE2 orders its script enum so that every script that occurs in
            # a Script_Extensions list sorts before Unknown.
            for s in lst:
                assert s < SC_UNKNOWN or s == script[c], (hex(c), SCRIPTS[s])

    # Case folding -> other case and caseless sets (PCRE2's GenerateUcd.py).
    other_case = [0] * MAX
    for c in range(MAX):
        f = scf[c]
        if f != c:
            other_case[c] = f
            if other_case[f] == 0:
                other_case[f] = c
    classes = defaultdict(set)
    for c in range(MAX):
        if scf[c] != c:
            classes[scf[c]].update((c, scf[c]))
    caseless_sets = sorted(sorted(s) for s in classes.values() if len(s) >= 3)
    for s in caseless_sets:
        if s[0] < 128:
            for c in s:
                if c >= 128:
                    other_case[c] = 0

    aliases = {}
    for p in BPROPS:
        aliases[p] = list(perl["aliases"].get(p, [])) + EXTRA_BPROP_ALIASES.get(p, [])
    script_aliases = {}
    for s in SCRIPTS:
        if s in NEW_SCRIPT_ALIASES:
            script_aliases[s] = NEW_SCRIPT_ALIASES[s]
        else:
            key = next(k for k in perl["script_aliases"] if norm(k) == norm(s))
            script_aliases[s] = perl["script_aliases"][key]

    return {
        "gc": gc,
        "script": script,
        "scx": scx,
        "gcb": gcb,
        "bidi": bidi,
        "bools": bools,
        "other_case": other_case,
        "caseless_sets": caseless_sets,
        "bprop_aliases": aliases,
        "script_aliases": script_aliases,
        "new_cps": new_cps,
    }


# ---------------------------------------------------------------------------
# Property name table (PCRE2's utt)
# ---------------------------------------------------------------------------

PT = {"ANY": 0, "LAMP": 1, "GC": 2, "PC": 3, "SC": 4, "SCX": 5, "ALNUM": 6, "SPACE": 7, "PXSPACE": 8, "WORD": 9, "UCNC": 11, "BIDICL": 12, "BOOL": 13}


def property_names(d):
    entries = {}

    def add(name, ptype, value):
        name = "l&" if name == "L&" else norm(name)
        old = entries.get(name)
        assert old is None or old == (ptype, value), (name, old, (ptype, value))
        entries[name] = (ptype, value)

    add("Any", "ANY", 0)
    add("L&", "LAMP", 0)
    add("LC", "LAMP", 0)
    for i, g in enumerate(GENTYPES):
        add(g, "GC", i)
    for i, ct in enumerate(CHARTYPES):
        add(ct, "PC", i)
    add("Xan", "ALNUM", 0)
    add("Xps", "PXSPACE", 0)
    add("Xsp", "SPACE", 0)
    add("Xwd", "WORD", 0)
    add("Xuc", "UCNC", 0)
    for i, s in enumerate(SCRIPTS):
        pt = "SCX" if i < SC_UNKNOWN else "SC"
        for n in [s] + d["script_aliases"][s]:
            add(n, pt, i)
    for i, p in enumerate(BPROPS):
        for n in [p] + d["bprop_aliases"][p]:
            add(n, "BOOL", i)
    for i, b in enumerate(BIDIS):
        add("bidi" + b, "BIDICL", i)
    return sorted((n.encode(), PT[t], v) for n, (t, v) in entries.items())


# ---------------------------------------------------------------------------
# Rust output
# ---------------------------------------------------------------------------


def chunks(items, per_line, indent="    "):
    lines = []
    for i in range(0, len(items), per_line):
        lines.append(indent + ", ".join(items[i : i + per_line]) + ",")
    return "\n".join(lines)


def ranges(values):
    """Run-length encode a per-code-point list into (starts, values)."""
    starts, vals = [], []
    prev = object()
    for c, v in enumerate(values):
        if v != prev:
            starts.append(c)
            vals.append(v)
            prev = v
    return starts, vals


def inversion_list(bits):
    out = []
    state = 0
    for c in range(MAX):
        if bool(bits[c]) != state:
            out.append(c)
            state = bool(bits[c])
    return out


def hexs(xs):
    return ["0x%X" % x for x in xs]


def emit(d, names):
    o = []
    w = o.append
    stats = d["stats"]
    w("// Unicode 15.0.0 property tables for the PCRE2-compatible engine.")
    w("//")
    w("// @generated by crates/php-regex/tools/gen_unicode.py -- DO NOT EDIT.")
    w("// Values use the enum numbering of PCRE2 10.44's pcre2_ucp.h.")
    w("")
    w("#![allow(clippy::unreadable_literal)]")
    w("")
    w("/// Script identifiers (`ucp_<Script>` in pcre2_ucp.h).")
    w("pub mod scripts {")
    for i, s in enumerate(SCRIPTS):
        w("    pub const %s: u8 = %d;" % (s.upper(), i))
    w("}")
    w("")
    w("/// Boolean property identifiers (`ucp_<Property>` in pcre2_ucp.h).")
    w("pub mod bprops {")
    for i, p in enumerate(BPROPS):
        w("    pub const %s: u8 = %d;" % (p.upper(), i))
    w("}")
    w("")
    w("pub(super) const SCRIPT_COUNT: usize = %d;" % len(SCRIPTS))
    w("pub(super) const BPROP_COUNT: usize = %d;" % len(BPROPS))
    w("")
    w("#[rustfmt::skip]")
    w("pub(super) static SCRIPT_NAMES: [&str; SCRIPT_COUNT] = [")
    w(chunks(['"%s"' % s for s in SCRIPTS], 6))
    w("];")
    w("")

    # chartype: two-stage table with deduplicated 128-entry blocks.
    blocks = {}
    stage1 = []
    stage2 = []
    gc = d["gc"]
    for b in range(MAX // 128):
        blk = tuple(gc[b * 128 : (b + 1) * 128])
        if blk not in blocks:
            blocks[blk] = len(blocks)
            stage2.extend(blk)
        stage1.append(blocks[blk])
    stats["chartype_blocks"] = len(blocks)
    w("/// General category, stage 1: block number for `c >> 7`.")
    w("#[rustfmt::skip]")
    w("pub(super) static CHARTYPE_STAGE1: [u16; %d] = [" % len(stage1))
    w(chunks([str(x) for x in stage1], 24))
    w("];")
    w("")
    w("/// General category, stage 2: %d deduplicated blocks of 128 entries." % len(blocks))
    w("#[rustfmt::skip]")
    w("pub(super) static CHARTYPE_STAGE2: [u8; %d] = [" % len(stage2))
    w(chunks([str(x) for x in stage2], 32))
    w("];")
    w("")

    def range_table(name, doc, values, vtype="u8"):
        starts, vals = ranges(values)
        stats[name.lower() + "_ranges"] = len(starts)
        w("/// %s" % doc)
        w("#[rustfmt::skip]")
        w("pub(super) static %s_STARTS: [u32; %d] = [" % (name, len(starts)))
        w(chunks(hexs(starts), 12))
        w("];")
        w("")
        w("#[rustfmt::skip]")
        w("pub(super) static %s_VALUES: [%s; %d] = [" % (name, vtype, len(vals)))
        w(chunks([str(v) for v in vals], 24))
        w("];")
        w("")

    range_table("SCRIPT", "Script: `SCRIPT_VALUES[i]` applies from `SCRIPT_STARTS[i]` up to the next start.", d["script"])
    range_table("GB", "Grapheme break property (Extended_Pictographic merged in), as ranges.", d["gcb"])
    range_table("BIDI", "Bidi class, as ranges.", d["bidi"])

    # Script extensions.
    sets = sorted({s for s in d["scx"] if s})
    set_index = {s: i + 1 for i, s in enumerate(sets)}
    scx_idx = [set_index[s] if s else 0 for s in d["scx"]]
    stats["scx_sets"] = len(sets)
    w("/// Script_Extensions lists; index 0 means Script_Extensions == {Script}.")
    w("#[rustfmt::skip]")
    w("pub(super) static SCX_SETS: [&[u8]; %d] = [" % (len(sets) + 1))
    w("    &[],")
    for s in sets:
        w("    &[%s]," % ", ".join(str(x) for x in s))
    w("];")
    w("")
    range_table("SCX", "Script_Extensions: index into `SCX_SETS`, as ranges.", scx_idx, "u16" if len(sets) > 255 else "u8")

    # Boolean properties: one inversion list per property.
    total = 0
    w("/// Boolean properties as inversion lists: sorted boundaries, and a code point")
    w("/// has the property when an odd number of boundaries are <= it.")
    w("#[rustfmt::skip]")
    w("pub(super) static BPROP_LISTS: [&[u32]; BPROP_COUNT] = [")
    for i, p in enumerate(BPROPS):
        lst = inversion_list(d["bools"][i])
        total += len(lst)
        w("    // %s" % p)
        w("    &[")
        w(chunks(hexs(lst), 12, "        "))
        w("    ],")
    w("];")
    w("")
    stats["bprop_boundaries"] = total

    # Case data.
    oc = d["other_case"]
    pairs = [(c, oc[c]) for c in range(MAX) if oc[c]]
    stats["other_case_pairs"] = len(pairs)
    w("/// Other-case mapping, sorted by code point.")
    w("#[rustfmt::skip]")
    w("pub(super) static OTHER_CASE: [(u32, u32); %d] = [" % len(pairs))
    w(chunks(["(0x%X, 0x%X)" % p for p in pairs], 6))
    w("];")
    w("")
    cs = d["caseless_sets"]
    stats["caseless_sets"] = len(cs)
    w("/// Case-equivalence classes with three or more members, each sorted.")
    w("#[rustfmt::skip]")
    w("pub(super) static CASELESS_SETS: [&[u32]; %d] = [" % len(cs))
    for s in cs:
        w("    &[%s]," % ", ".join(hexs(s)))
    w("];")
    w("")
    members = sorted((c, i) for i, s in enumerate(cs) for c in s)
    w("/// Members of `CASELESS_SETS`, sorted by code point, with their set index.")
    w("#[rustfmt::skip]")
    w("pub(super) static CASELESS_MEMBERS: [(u32, u8); %d] = [" % len(members))
    w(chunks(["(0x%X, %d)" % m for m in members], 8))
    w("];")
    w("")

    # Property names.
    stats["names"] = len(names)
    w("/// Names accepted by `\\p{..}` (PCRE2's `utt` table), sorted bytewise:")
    w("/// (normalized name, PT_* type, value).")
    w("#[rustfmt::skip]")
    w("pub(super) static PROPERTY_NAMES: [(&str, u8, u16); %d] = [" % len(names))
    for n, t, v in names:
        w('    ("%s", %d, %d),' % (n.decode(), t, v))
    w("];")
    return "\n".join(o) + "\n"


def main():
    dump = None
    if len(sys.argv) == 3 and sys.argv[1] == "--dump":
        dump = sys.argv[2]
    elif len(sys.argv) != 1:
        sys.exit(__doc__)
    d = build()
    d["stats"] = {"new_code_points": len(d["new_cps"])}
    names = property_names(d)
    src = emit(d, names)
    with open(OUT, "w") as f:
        f.write(src)
    d["stats"]["bytes"] = len(src)
    print(json.dumps(d["stats"], indent=1))
    if dump:
        out = {
            "gc": d["gc"],
            "script": d["script"],
            "scx": [list(s) if s else [] for s in d["scx"]],
            "gcb": d["gcb"],
            "bidi": d["bidi"],
            "bools": [inversion_list(b) for b in d["bools"]],
            "other_case": d["other_case"],
            "caseless_sets": d["caseless_sets"],
            "names": [(n.decode(), t, v) for n, t, v in names],
            "new_cps": d["new_cps"],
        }
        with open(dump, "w") as f:
            json.dump(out, f)


if __name__ == "__main__":
    main()
