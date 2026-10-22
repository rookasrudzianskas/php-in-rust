//! Script-run checking for `(*sr:...)` / `(*script_run:...)`, modelled on
//! PCRE2's `pcre2_script_run.c`: all characters must come from a single
//! script (taking Script_Extensions into account, with Common and Inherited
//! characters allowed anywhere and the Han-based Japanese/Chinese/Korean
//! combinations accepted), and all decimal digits from one digit set.

use crate::chars;
use crate::unicode::{self, scripts};

const HAN_COMBOS: [u8; 4] = [
    scripts::HIRAGANA,
    scripts::KATAKANA,
    scripts::BOPOMOFO,
    scripts::HANGUL,
];

/// The code point of the zero of the decimal digit set containing `c`.
fn digit_zero(c: u32) -> u32 {
    let mut start = c;
    let mut n = 0;
    while n < 9 && start > 0 && unicode::chartype(start - 1) == unicode::UCP_ND {
        start -= 1;
        n += 1;
    }
    c - ((c - start) % 10)
}

pub(crate) fn script_run(s: &[u8], from: usize, to: usize, utf: bool) -> bool {
    let mut p = from;
    let mut chars_seen = 0usize;
    let mut require: Option<Vec<u8>> = None;
    let mut zero: Option<u32> = None;
    while p < to {
        let (c, l) = if utf {
            chars::decode_utf8(s, p)
        } else {
            (s[p] as u32, 1)
        };
        p += l;
        chars_seen += 1;
        let sc = unicode::script(c);
        if sc == scripts::UNKNOWN {
            if chars_seen == 1 && p >= to {
                return true;
            }
            return false;
        }
        if unicode::chartype(c) == unicode::UCP_ND {
            let z = digit_zero(c);
            match zero {
                None => zero = Some(z),
                Some(old) if old != z => return false,
                _ => {}
            }
        }
        let scx = unicode::script_extensions(c);
        let mut set: Vec<u8> = if scx.is_empty() {
            vec![sc]
        } else {
            scx.to_vec()
        };
        if !set.contains(&sc) {
            set.push(sc);
        }
        if set.len() == 1 && (sc == scripts::COMMON || sc == scripts::INHERITED) {
            continue;
        }
        set.retain(|&x| x != scripts::COMMON && x != scripts::INHERITED);
        if set.is_empty() {
            continue;
        }
        if set.contains(&scripts::HAN) {
            for h in HAN_COMBOS {
                if !set.contains(&h) {
                    set.push(h);
                }
            }
        }
        if HAN_COMBOS.iter().any(|h| set.contains(h)) && !set.contains(&scripts::HAN) {
            set.push(scripts::HAN);
        }
        require = Some(match require {
            None => set,
            Some(r) => {
                let inter: Vec<u8> = r.into_iter().filter(|x| set.contains(x)).collect();
                if inter.is_empty() {
                    return false;
                }
                inter
            }
        });
    }
    true
}
