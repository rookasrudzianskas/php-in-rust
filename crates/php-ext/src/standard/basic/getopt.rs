//! `getopt()` and its engine, `php_getopt()` from `main/getopt.c`.

use php_runtime::value::numeric::{Numeric, is_numeric_str};
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::BasicState;

const EOF: i32 = -1;
/// `PHP_GETOPT_INVALID_ARG`
const INVALID_ARG: i32 = -2;

/// `opt_struct`.
#[derive(Debug, Clone)]
struct Opt {
    /// `0` for long options, `b'-'` terminates the table.
    opt_char: u8,
    /// 0 = no parameter, 1 = required (`:`), 2 = optional (`::`).
    need_param: u8,
    opt_name: Option<Vec<u8>>,
}

/// The static state of `php_getopt()`.
#[derive(Default)]
struct GetoptState {
    optchr: usize,
    dash: bool,
    optidx: isize,
}

/// `parse_opts()`: the short option characters of the first argument.
fn parse_opts(opts: &[u8]) -> Vec<Opt> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < opts.len() && opts[i].is_ascii_alphanumeric() {
        let opt_char = opts[i];
        i += 1;
        let mut need_param = 0;
        if opts.get(i) == Some(&b':') {
            need_param = 1;
            i += 1;
            if opts.get(i) == Some(&b':') {
                need_param = 2;
                i += 1;
            }
        }
        out.push(Opt {
            opt_char,
            need_param,
            opt_name: None,
        });
    }
    out
}

impl GetoptState {
    /// `php_getopt(argc, argv, opts, &optarg, &optind, 0, 1)`.
    fn next(
        &mut self,
        argv: &[Vec<u8>],
        opts: &[Opt],
        optarg: &mut Option<Vec<u8>>,
        optind: &mut usize,
    ) -> i32 {
        let argc = argv.len();
        let at = |i: usize, j: usize| -> u8 { argv[i].get(j).copied().unwrap_or(0) };
        let mut arg_start: usize;

        self.optidx = -1;
        *optarg = None;

        if *optind >= argc {
            return EOF;
        }
        if !self.dash {
            if at(*optind, 0) != b'-' {
                return EOF;
            }
            if at(*optind, 1) == 0 {
                // "-" alone stands for stdin: the program handles it
                return EOF;
            }
        }
        if at(*optind, 0) == b'-' && at(*optind, 1) == b'-' {
            let mut arg_end = argv[*optind].len() - 1;

            // "--" ends the arguments unless followed by a known long option
            if at(*optind, 2) == 0 {
                *optind += 1;
                return EOF;
            }

            arg_start = 2;

            // <arg>=<val>: `memchr` scans [arg_start, arg_end)
            let window = &argv[*optind][arg_start..arg_end.max(arg_start)];
            if let Some(p) = window.iter().position(|&c| c == b'=') {
                arg_end = p;
                arg_start += 1;
            } else {
                arg_end -= 1;
            }

            loop {
                self.optidx += 1;
                let o = &opts[self.optidx as usize];
                if o.opt_char == b'-' {
                    *optind += 1;
                    return INVALID_ARG;
                }
                if let Some(name) = &o.opt_name {
                    let arg = &argv[*optind];
                    if name.len() == arg_end
                        && arg.len() >= 2 + arg_end
                        && arg[2..2 + arg_end] == name[..]
                    {
                        break;
                    }
                }
            }

            self.optchr = 0;
            self.dash = false;
            arg_start += opts[self.optidx as usize]
                .opt_name
                .as_ref()
                .map_or(0, Vec::len);
        } else {
            if !self.dash {
                self.dash = true;
                self.optchr = 1;
            }
            // "-:" is not a flag
            if at(*optind, self.optchr) == b':' {
                self.dash = false;
                *optind += 1;
                return INVALID_ARG;
            }
            arg_start = 1 + self.optchr;
        }
        if self.optidx < 0 {
            loop {
                self.optidx += 1;
                let o = &opts[self.optidx as usize];
                if o.opt_char == b'-' {
                    if at(*optind, self.optchr + 1) == 0 {
                        self.dash = false;
                        *optind += 1;
                    } else {
                        self.optchr += 1;
                    }
                    return INVALID_ARG;
                } else if at(*optind, self.optchr) == o.opt_char {
                    break;
                }
            }
        }
        let o = opts[self.optidx as usize].clone();
        if o.need_param != 0 {
            // -<arg> <val>, -<arg>=<val> or -<arg><val>
            self.dash = false;
            if at(*optind, arg_start) == 0 {
                *optind += 1;
                if *optind == argc {
                    if o.need_param == 1 {
                        return INVALID_ARG;
                    }
                // an optional value is not supported with the -<arg> <val> style
                } else if o.need_param == 1 {
                    *optarg = Some(argv[*optind].clone());
                    *optind += 1;
                }
            } else if at(*optind, arg_start) == b'=' {
                arg_start += 1;
                *optarg = Some(argv[*optind][arg_start.min(argv[*optind].len())..].to_vec());
                *optind += 1;
            } else {
                *optarg = Some(argv[*optind][arg_start..].to_vec());
                *optind += 1;
            }
            o.opt_char as i32
        } else {
            // several options specified as one (long options excluded)
            if arg_start >= 2 && !(at(*optind, 0) == b'-' && at(*optind, 1) == b'-') {
                if at(*optind, self.optchr + 1) == 0 {
                    self.dash = false;
                    *optind += 1;
                } else {
                    self.optchr += 1;
                }
            } else {
                *optind += 1;
            }
            o.opt_char as i32
        }
    }
}

fn getopt(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let options = args.str(0).clone();
    let long_opts: Option<Vec<PhpStr>> = match args.get(1) {
        Value::Array(a) => {
            let items: Vec<Value> = a.values().map(Value::unref).collect();
            let mut v = Vec::with_capacity(items.len());
            for it in items {
                v.push(vm.to_php_string(&it)?);
            }
            Some(v)
        }
        _ => None,
    };
    let rest_index = args.is_passed(2).then(|| args.by_ref(2));

    // Init $rest_index to 1
    if let Some(r) = &rest_index {
        r.set(Value::Int(1));
    }

    // argv comes from $_SERVER['argv'] / $argv: the CLI records it.
    let Some(argv) = vm.ext_data::<BasicState>().argv.clone() else {
        return Ok(Value::Bool(false));
    };
    let argv: Vec<Vec<u8>> = argv.iter().map(|a| a.to_vec()).collect();

    let mut opts = parse_opts(&options);
    if let Some(longs) = long_opts {
        for l in longs {
            let mut name = l.to_vec();
            let mut need_param = 0;
            if name.last() == Some(&b':') {
                need_param += 1;
                name.pop();
                if name.last() == Some(&b':') {
                    need_param += 1;
                    name.pop();
                }
            }
            // The C code stops the name at an embedded NUL.
            if let Some(z) = name.iter().position(|&c| c == 0) {
                name.truncate(z);
            }
            opts.push(Opt {
                opt_char: 0,
                need_param,
                opt_name: Some(name),
            });
        }
    }
    // php_getopt wants to identify the last param
    opts.push(Opt {
        opt_char: b'-',
        need_param: 0,
        opt_name: None,
    });

    let mut result = PhpArray::new();
    let mut st = GetoptState::default();
    let mut optind: usize = 1;
    let mut optarg: Option<Vec<u8>> = None;

    loop {
        let o = st.next(&argv, &opts, &mut optarg, &mut optind);
        if o == EOF {
            break;
        }
        // Skip unknown arguments.
        if o == INVALID_ARG {
            continue;
        }

        let optname: Vec<u8> = if o == 0 {
            opts[st.optidx as usize]
                .opt_name
                .clone()
                .unwrap_or_default()
        } else {
            let c = if o == 1 { b'-' } else { o as u8 };
            vec![c]
        };
        // keep the arg as binary, since the encoding is not known
        let val = match optarg.take() {
            Some(a) => Value::bytes(&a),
            None => Value::Bool(false),
        };

        let key = if !(optname.len() > 1 && optname[0] == b'0') {
            match is_numeric_str(&optname) {
                Some(Numeric::Int(n)) => ArrayKey::Int(n),
                _ => ArrayKey::Str(PhpStr::from(optname.clone())),
            }
        } else {
            ArrayKey::Str(PhpStr::from(optname.clone()))
        };
        match result.get_mut(&key) {
            Some(existing) => {
                // convert_to_array() on the first occurrence, then append
                let mut arr = match &*existing {
                    Value::Array(a) => (**a).clone(),
                    other => {
                        let mut a = PhpArray::new();
                        let _ = a.append(other.clone());
                        a
                    }
                };
                let _ = arr.append(val);
                *existing = Value::array(arr);
            }
            None => {
                result.insert(key, val);
            }
        }
    }

    // Set $rest_index to php_optind
    if let Some(r) = &rest_index {
        r.set(Value::Int(optind as i64));
    }
    Ok(Value::array(result))
}

pub static GETOPT: NativeFunction = NativeFunction {
    sig: "getopt(string $short_options, array $long_options = [], &$rest_index = null): array|false",
    handler: getopt,
};

pub fn register(r: &mut Registry) {
    r.function(&GETOPT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::basic::set_argv;
    use crate::testing::TestVm;
    use php_runtime::PhpRef;

    fn run(argv: &[&str], short: &str, long: &[&str]) -> (Value, i64) {
        let mut t = TestVm::new();
        set_argv(&mut t.vm, argv.iter().map(|s| PhpStr::from(*s)).collect());
        let r = PhpRef::new(Value::Null);
        let longs: PhpArray = long.iter().map(|s| Value::str(s)).collect();
        let v = t
            .call(
                "getopt",
                vec![
                    Value::str(short),
                    Value::array(longs),
                    Value::Ref(r.clone()),
                ],
            )
            .unwrap();
        (v, r.get().as_int().unwrap())
    }

    fn dump(v: &Value) -> String {
        match v {
            Value::Array(a) => {
                let parts: Vec<String> =
                    a.iter().map(|(k, v)| format!("{k}=>{}", dump(v))).collect();
                format!("[{}]", parts.join(","))
            }
            Value::String(s) => format!("{:?}", s.to_string_lossy()),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            o => format!("{o:?}"),
        }
    }

    #[test]
    fn short_and_long_options() {
        // getopt.phpt style
        let (v, idx) = run(
            &["prog", "-a", "-b", "-c", "--long", "--lv=5", "rest"],
            "abc::",
            &["long", "lv:"],
        );
        assert_eq!(
            dump(&v),
            r#"[a=>false,b=>false,c=>false,long=>false,lv=>"5"]"#
        );
        assert_eq!(idx, 6);
        // "-c x": an optional value is not taken from the next argument, so
        // parsing stops at "x"
        let (v, idx) = run(&["prog", "-c", "x", "-a"], "ac::", &[]);
        assert_eq!(dump(&v), "[c=>false]");
        assert_eq!(idx, 2);
    }

    #[test]
    fn values_attached_separated_and_repeated() {
        let (v, idx) = run(&["p", "-ofile", "-o", "x", "-p=5", "-p6"], "o:p:", &[]);
        assert_eq!(dump(&v), r#"[o=>[0=>"file",1=>"x"],p=>[0=>"5",1=>"6"]]"#);
        assert_eq!(idx, 6);
    }

    #[test]
    fn grouped_short_options_and_numeric_names() {
        let (v, _) = run(&["p", "-abc", "-1", "-2"], "abc12", &[]);
        assert_eq!(dump(&v), "[a=>false,b=>false,c=>false,1=>false,2=>false]");
    }

    #[test]
    fn stops_at_first_non_option_and_double_dash() {
        let (v, idx) = run(&["p", "-a", "file", "-b"], "ab", &[]);
        assert_eq!(dump(&v), "[a=>false]");
        assert_eq!(idx, 2);
        let (v, idx) = run(&["p", "-a", "--", "-b"], "ab", &[]);
        assert_eq!(dump(&v), "[a=>false]");
        assert_eq!(idx, 3);
    }

    #[test]
    fn unknown_options_are_skipped() {
        let (v, _) = run(&["p", "-x", "-a", "--nope", "--yes"], "a", &["yes"]);
        assert_eq!(dump(&v), "[a=>false,yes=>false]");
    }

    #[test]
    fn missing_required_value_is_skipped() {
        let (v, idx) = run(&["p", "-a"], "a:", &[]);
        assert_eq!(dump(&v), "[]");
        assert_eq!(idx, 2);
    }

    #[test]
    fn no_argv_means_false() {
        let mut t = TestVm::new();
        let r = t.call("getopt", vec![Value::str("a")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
    }
}
