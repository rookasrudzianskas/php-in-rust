//! Line diff in the format run-tests.php writes to `.diff` files
//! (`Differ` + `DiffOutputBuilder`).
//!
//! For `--EXPECTF--` tests every expected line is itself an `EXPECTF`
//! template, so lines are compared with the translated regex instead of
//! byte equality (exactly what `generate_diff()` does upstream).

use crate::expect::Matcher;

/// How expected lines are compared against actual lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    /// Byte equality (`--EXPECT--`, `--EXPECTREGEX--`).
    Exact,
    /// Expected lines are `EXPECTF` templates.
    Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Old,
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy)]
struct Entry<'a> {
    text: &'a [u8],
    kind: Kind,
    line: usize,
}

/// Description of the first mismatch, for one-line failure reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstDiff {
    /// Line number (1-based) in the expected text for removals, in the actual
    /// output for additions.
    pub line: usize,
    pub expected: Option<Vec<u8>>,
    pub actual: Option<Vec<u8>>,
}

/// A rendered diff plus a summary of where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReport {
    pub text: Vec<u8>,
    pub first: Option<FirstDiff>,
}

/// Maximum LCS table size (cells) before falling back to a coarse diff.
const MAX_CELLS_EXACT: usize = 8_000_000;
const MAX_CELLS_FORMAT: usize = 1_000_000;

struct LineEq<'a> {
    mode: LineMode,
    wanted: &'a [&'a [u8]],
    output: &'a [&'a [u8]],
    /// Lazily compiled template per expected line (`Some(None)` = could not
    /// be compiled, compare bytes instead).
    matchers: Vec<Option<Option<Matcher>>>,
}

impl<'a> LineEq<'a> {
    fn new(mode: LineMode, wanted: &'a [&'a [u8]], output: &'a [&'a [u8]]) -> Self {
        Self {
            mode,
            wanted,
            output,
            matchers: vec![None; wanted.len()],
        }
    }

    fn eq(&mut self, i: usize, j: usize) -> bool {
        let (w, o) = (self.wanted[i], self.output[j]);
        match self.mode {
            LineMode::Exact => w == o,
            LineMode::Format => {
                let slot = &mut self.matchers[i];
                let matcher = slot.get_or_insert_with(|| Matcher::expectf(w).ok());
                match matcher {
                    Some(m) => m.is_match(o),
                    None => w == o,
                }
            }
        }
    }
}

/// Compute matched index pairs (relative to the slices) of a longest common
/// subsequence of `w[ws..we]` and `o[os..oe]`.
fn lcs_pairs(
    eq: &mut LineEq<'_>,
    ws: usize,
    we: usize,
    os: usize,
    oe: usize,
) -> Vec<(usize, usize)> {
    let (n, m) = (we - ws, oe - os);
    let limit = match eq.mode {
        LineMode::Exact => MAX_CELLS_EXACT,
        LineMode::Format => MAX_CELLS_FORMAT,
    };
    if n == 0 || m == 0 || n.saturating_mul(m) > limit {
        return Vec::new();
    }
    let stride = m + 1;
    let mut table = vec![0u32; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * stride + j] = if eq.eq(ws + i, os + j) {
                table[(i + 1) * stride + j + 1] + 1
            } else {
                table[(i + 1) * stride + j].max(table[i * stride + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut pairs = Vec::new();
    while i < n && j < m {
        if eq.eq(ws + i, os + j) {
            pairs.push((ws + i, os + j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * stride + j] >= table[i * stride + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

fn render(entries: &[Entry<'_>], context: usize) -> Vec<u8> {
    let count = entries.len();
    let number_len = count.to_string().len().max(3);
    let ctx = context as isize;
    let mut out = Vec::new();
    let mut i: isize = 0;
    let blank = " ".repeat(number_len + 2);

    while (i as usize) < count {
        // Find the next difference.
        let mut next = i as usize;
        while next < count && entries[next].kind == Kind::Old {
            next += 1;
        }
        // No more differences: terminate.
        if next == count {
            out.extend_from_slice(b"--\n");
            break;
        }
        // Print a separator if necessary.
        if i < next as isize - ctx {
            out.extend_from_slice(b"--\n");
            i = next as isize - ctx;
        }
        // Leading context.
        while (i as usize) < next {
            out.extend_from_slice(blank.as_bytes());
            out.extend_from_slice(entries[i as usize].text);
            out.push(b'\n');
            i += 1;
        }
        // The differences themselves.
        while (i as usize) < count && entries[i as usize].kind != Kind::Old {
            let e = &entries[i as usize];
            out.extend_from_slice(format!("{:0number_len$}", e.line).as_bytes());
            out.extend_from_slice(if e.kind == Kind::Added { b"+ " } else { b"- " });
            out.extend_from_slice(e.text);
            out.push(b'\n');
            i += 1;
        }
        // Trailing context.
        let after = (i + ctx).min(count as isize);
        while i < after && entries[i as usize].kind == Kind::Old {
            out.extend_from_slice(blank.as_bytes());
            out.extend_from_slice(entries[i as usize].text);
            out.push(b'\n');
            i += 1;
        }
    }
    out
}

fn first_difference(entries: &[Entry<'_>]) -> Option<FirstDiff> {
    let start = entries.iter().position(|e| e.kind != Kind::Old)?;
    let run = entries[start..].iter().take_while(|e| e.kind != Kind::Old);
    let mut first = FirstDiff {
        line: 0,
        expected: None,
        actual: None,
    };
    for e in run {
        match e.kind {
            Kind::Removed if first.expected.is_none() => {
                first.expected = Some(e.text.to_vec());
                if first.line == 0 {
                    first.line = e.line;
                }
            }
            Kind::Added if first.actual.is_none() => {
                first.actual = Some(e.text.to_vec());
                if first.line == 0 {
                    first.line = e.line;
                }
            }
            _ => {}
        }
    }
    Some(first)
}

/// Diff `wanted` against `output` (both already normalised to `\n` endings).
pub fn generate_diff(wanted: &[u8], mode: LineMode, output: &[u8], context: usize) -> DiffReport {
    let w: Vec<&[u8]> = wanted.split(|b| *b == b'\n').collect();
    let o: Vec<&[u8]> = output.split(|b| *b == b'\n').collect();
    let mut eq = LineEq::new(mode, &w, &o);

    // Common prefix and suffix are peeled off first.
    let mut start = 0;
    while start < w.len() && start < o.len() && eq.eq(start, start) {
        start += 1;
    }
    let (mut we, mut oe) = (w.len(), o.len());
    while we > start && oe > start && eq.eq(we - 1, oe - 1) {
        we -= 1;
        oe -= 1;
    }

    let mut entries: Vec<Entry<'_>> = Vec::with_capacity(w.len().max(o.len()) + 2);
    for text in &w[..start] {
        entries.push(Entry {
            text,
            kind: Kind::Old,
            line: 0,
        });
    }

    let mut from_line = start + 1;
    let mut to_line = start + 1;
    let (mut wi, mut oi) = (start, start);
    for (i, j) in lcs_pairs(&mut eq, start, we, start, oe) {
        while wi < i {
            entries.push(Entry {
                text: w[wi],
                kind: Kind::Removed,
                line: from_line,
            });
            wi += 1;
            from_line += 1;
        }
        while oi < j {
            entries.push(Entry {
                text: o[oi],
                kind: Kind::Added,
                line: to_line,
            });
            oi += 1;
            to_line += 1;
        }
        entries.push(Entry {
            text: o[j],
            kind: Kind::Old,
            line: 0,
        });
        wi += 1;
        oi += 1;
        from_line += 1;
        to_line += 1;
    }
    while wi < we {
        entries.push(Entry {
            text: w[wi],
            kind: Kind::Removed,
            line: from_line,
        });
        wi += 1;
        from_line += 1;
    }
    while oi < oe {
        entries.push(Entry {
            text: o[oi],
            kind: Kind::Added,
            line: to_line,
        });
        oi += 1;
        to_line += 1;
    }
    for text in &w[we..] {
        entries.push(Entry {
            text,
            kind: Kind::Old,
            line: 0,
        });
    }

    DiffReport {
        text: render(&entries, context),
        first: first_difference(&entries),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(w: &str, o: &str) -> String {
        String::from_utf8(generate_diff(w.as_bytes(), LineMode::Exact, o.as_bytes(), 3).text)
            .unwrap()
    }

    #[test]
    fn single_changed_line_shows_context_without_separators() {
        // Everything fits in the context window: no `--` lines at all.
        let d = diff("a\nb\nc\nd\ne", "a\nb\nX\nd\ne");
        assert_eq!(d, "     a\n     b\n003- c\n003+ X\n     d\n     e\n");
        assert_eq!(diff("a\nb", "a\nX"), "     a\n002- b\n002+ X\n");
    }

    #[test]
    fn separators_mark_cut_context_at_both_ends() {
        let d = diff("1\n2\n3\n4\n5\n6\n7\n8\n9", "1\n2\n3\n4\nX\n6\n7\n8\n9");
        assert_eq!(
            d,
            "--\n     2\n     3\n     4\n005- 5\n005+ X\n     6\n     7\n     8\n--\n"
        );
    }

    #[test]
    fn far_apart_changes_get_a_separator_between() {
        let w = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
        let o = "X\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nY";
        let d = diff(w, o);
        assert!(d.contains("001- 1\n001+ X\n"), "{d}");
        assert!(d.contains("012- 12\n012+ Y\n"), "{d}");
        assert!(d.contains("\n--\n"), "{d}");
    }

    #[test]
    fn missing_and_extra_lines() {
        let d = diff("a\nb\nc", "a\nc");
        assert!(d.contains("002- b\n"), "{d}");
        assert!(!d.contains('+'), "{d}");
        let d = diff("a\nc", "a\nb\nc");
        assert!(d.contains("002+ b\n"), "{d}");
        assert!(!d.contains('-'), "{d}");
    }

    #[test]
    fn empty_actual_output() {
        let r = generate_diff(b"abc", LineMode::Exact, b"", 3);
        let text = String::from_utf8(r.text).unwrap();
        assert!(text.contains("001- abc\n"), "{text}");
        assert!(text.contains("001+ \n"), "{text}");
        let first = r.first.unwrap();
        assert_eq!(first.line, 1);
        assert_eq!(first.expected.as_deref(), Some(&b"abc"[..]));
        assert_eq!(first.actual.as_deref(), Some(&b""[..]));
    }

    #[test]
    fn identical_inputs_produce_only_the_terminator() {
        let r = generate_diff(b"a\nb", LineMode::Exact, b"a\nb", 3);
        assert_eq!(r.text, b"--\n");
        assert!(r.first.is_none());
    }

    #[test]
    fn format_mode_matches_lines_by_template() {
        let r = generate_diff(
            b"id: %d\nname: %s\nend",
            LineMode::Format,
            b"id: 5\nname: bob\nend",
            3,
        );
        assert!(r.first.is_none(), "{:?}", String::from_utf8_lossy(&r.text));

        let r = generate_diff(
            b"id: %d\nname: %s\nend",
            LineMode::Format,
            b"id: x\nname: bob\nend",
            3,
        );
        let first = r.first.unwrap();
        assert_eq!(first.line, 1);
        assert_eq!(first.expected.as_deref(), Some(&b"id: %d"[..]));
        assert_eq!(first.actual.as_deref(), Some(&b"id: x"[..]));
    }

    #[test]
    fn line_numbers_are_per_side() {
        let d = diff("a\nb\nc\nd", "a\nc\nd\ne");
        assert!(d.contains("002- b\n"), "{d}");
        assert!(d.contains("004+ e\n"), "{d}");
    }

    #[test]
    fn handles_binary_lines() {
        let r = generate_diff(b"a\n\xff\xfe", LineMode::Exact, b"a\n\xff", 3);
        assert!(r.text.windows(2).any(|w| w == b"\xff\xfe"));
    }

    #[test]
    fn large_diverging_outputs_fall_back_instead_of_hanging() {
        let w: String = (0..4000).map(|i| format!("w{i}\n")).collect();
        let o: String = (0..4000).map(|i| format!("o{i}\n")).collect();
        let r = generate_diff(w.as_bytes(), LineMode::Exact, o.as_bytes(), 3);
        assert!(r.first.is_some());
    }

    #[test]
    fn context_option_limits_context_lines() {
        let r = generate_diff(
            b"a\nb\nc\nd\ne\nf\ng",
            LineMode::Exact,
            b"a\nb\nc\nX\ne\nf\ng",
            1,
        );
        let text = String::from_utf8(r.text).unwrap();
        assert_eq!(text, "--\n     c\n004- d\n004+ X\n     e\n--\n");
    }
}
