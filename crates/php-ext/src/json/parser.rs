//! Port of `ext/json/json_parser.y`.
//!
//! php-src drives the re2c scanner with a bison LALR(1) grammar. This parser
//! accepts the same language, pulls tokens from the scanner at exactly the
//! points where bison needs a lookahead (so that the position reported by
//! `json_last_error_msg()` is that of the same token), raises the same error
//! codes from the same grammar actions, and keeps the bison stack size so the
//! "memory exhausted" limit (`YYMAXDEPTH`) of extremely deep inputs behaves
//! alike. It is iterative: nesting depth is limited by `$depth` and bison's
//! stack, not by the Rust stack.

use php_runtime::{ArrayKey, ClassRef, ObjRef, PhpArray, Value};

use super::OBJECT_AS_ARRAY;
use super::error::{ErrorCode, ErrorDetails};
use super::scanner::{Scanner, Tok};

/// bison's `YYMAXDEPTH`: the parser stack cannot hold this many entries.
const YYMAXDEPTH: usize = 10000;

/// Accumulates the members of a container being parsed.
enum Acc {
    /// `json_validate()`: nothing is built.
    None,
    Array(PhpArray),
    /// Object mode before the first member is reduced. php-src creates the
    /// object at that moment, i.e. after the first value was parsed, which
    /// determines object handle numbers.
    ObjectPending,
    Object(ObjRef),
}

struct Frame {
    is_object: bool,
    acc: Acc,
    /// The key waiting for its value (objects).
    key: Vec<u8>,
    /// Bison stack entries below the opening bracket.
    base: usize,
}

enum St {
    /// A value is expected; the token may already have been read.
    Value(Option<Tok>),
    /// A value was completed.
    AfterValue(Value),
    /// Just after `{` or `[`.
    AfterOpen,
    /// After `,` in an object.
    ExpectKey,
    ExpectColon,
}

pub struct Parser<'a> {
    sc: Scanner<'a>,
    options: i32,
    max_depth: i32,
    depth: i32,
    std_class: Option<ClassRef>,
    validate: bool,
    stack: Vec<Frame>,
}

/// Parses `src` (`php_json_yyparse`). With `validate`, no values are built
/// (`php_json_get_validate_methods`). `std_class` is `stdClass`, needed to
/// decode objects unless `JSON_OBJECT_AS_ARRAY` is set.
pub fn parse(
    src: &[u8],
    options: i32,
    max_depth: i32,
    std_class: Option<ClassRef>,
    validate: bool,
) -> Result<Value, ErrorDetails> {
    let mut p = Parser {
        sc: Scanner::new(src, options, !validate),
        options,
        max_depth,
        depth: 1,
        std_class,
        validate,
        stack: Vec::new(),
    };
    p.run()
}

impl Parser<'_> {
    /// `php_json_parser_error_details()` for `code`.
    fn fail(&self, code: ErrorCode) -> ErrorDetails {
        ErrorDetails {
            code,
            line: self.sc.line,
            column: self.sc.error_column(),
        }
    }

    /// `php_json_yyerror()`: a syntax error unless the scanner reported one.
    fn syntax_error(&self, tok: &Tok) -> ErrorDetails {
        let code = if matches!(tok, Tok::Error) {
            self.sc.errcode
        } else {
            ErrorCode::Syntax
        };
        self.fail(code)
    }

    /// bison's stack overflow (`yyexhaustedlab`).
    fn check(&self, entries: usize) -> Result<(), ErrorDetails> {
        if entries >= YYMAXDEPTH {
            Err(self.fail(ErrorCode::Syntax))
        } else {
            Ok(())
        }
    }

    fn new_object(&self) -> ObjRef {
        let class = self
            .std_class
            .clone()
            .expect("stdClass is needed to decode objects");
        ObjRef::new(class)
    }

    /// Pops the finished container (`array_end` / `object_end` reductions).
    fn close(&mut self, empty: bool, n: &mut usize) -> Result<Value, ErrorDetails> {
        let fr = self.stack.pop().expect("open container");
        if empty {
            // `elements`/`members` reduced from the empty production.
            self.check(fr.base + 3)?;
        }
        self.check(fr.base + 4)?;
        *n = fr.base + 1;
        self.depth -= 1;
        Ok(match fr.acc {
            Acc::None => Value::Null,
            Acc::Array(a) => Value::array(a),
            Acc::ObjectPending => Value::Object(self.new_object()),
            Acc::Object(o) => Value::Object(o),
        })
    }

    fn run(&mut self) -> Result<Value, ErrorDetails> {
        // Entries on bison's state stack (initially just state 0).
        let mut n: usize = 1;
        let mut st = St::Value(None);
        loop {
            st = match st {
                St::Value(pending) => {
                    let tok = match pending {
                        Some(t) => t,
                        None => self.sc.scan(),
                    };
                    match tok {
                        Tok::LBrace | Tok::LBracket => {
                            // PHP_JSON_DEPTH_INC (a mid-rule action: no lookahead is read).
                            if self.max_depth != 0 && self.depth >= self.max_depth {
                                return Err(self.fail(ErrorCode::Depth));
                            }
                            self.depth += 1;
                            let base = n;
                            self.check(base + 1)?;
                            self.check(base + 2)?;
                            n = base + 2;
                            let is_object = matches!(tok, Tok::LBrace);
                            let acc = if self.validate {
                                Acc::None
                            } else if !is_object || self.options & OBJECT_AS_ARRAY != 0 {
                                Acc::Array(PhpArray::new())
                            } else {
                                Acc::ObjectPending
                            };
                            self.stack.push(Frame {
                                is_object,
                                acc,
                                key: Vec::new(),
                                base,
                            });
                            St::AfterOpen
                        }
                        Tok::Null => St::AfterValue(self.scalar(Value::Null, n)?),
                        Tok::True => St::AfterValue(self.scalar(Value::Bool(true), n)?),
                        Tok::False => St::AfterValue(self.scalar(Value::Bool(false), n)?),
                        Tok::Int(i) => St::AfterValue(self.scalar(Value::Int(i), n)?),
                        Tok::Double(d) => St::AfterValue(self.scalar(Value::Float(d), n)?),
                        Tok::Str(s) => {
                            let v = if self.validate {
                                Value::Null
                            } else {
                                Value::bytes(&s)
                            };
                            St::AfterValue(self.scalar(v, n)?)
                        }
                        other => return Err(self.syntax_error(&other)),
                    }
                }
                St::AfterOpen => {
                    let tok = self.sc.scan();
                    let is_object = self.stack.last().expect("open container").is_object;
                    if is_object {
                        match tok {
                            Tok::RBrace => St::AfterValue(self.close(true, &mut n)?),
                            Tok::RBracket => return Err(self.fail(ErrorCode::StateMismatch)),
                            Tok::Str(k) => {
                                let fr = self.stack.last_mut().expect("open container");
                                fr.key = k;
                                // key, then ':'
                                let base = fr.base;
                                self.check(base + 3)?;
                                n = base + 4;
                                St::ExpectColon
                            }
                            other => return Err(self.syntax_error(&other)),
                        }
                    } else {
                        match tok {
                            Tok::RBracket => St::AfterValue(self.close(true, &mut n)?),
                            Tok::RBrace => return Err(self.fail(ErrorCode::StateMismatch)),
                            other => St::Value(Some(other)),
                        }
                    }
                }
                St::ExpectKey => {
                    let tok = self.sc.scan();
                    match tok {
                        Tok::Str(k) => {
                            let fr = self.stack.last_mut().expect("open container");
                            fr.key = k;
                            let base = fr.base;
                            self.check(base + 5)?;
                            n = base + 6;
                            St::ExpectColon
                        }
                        other => return Err(self.syntax_error(&other)),
                    }
                }
                St::ExpectColon => {
                    let tok = self.sc.scan();
                    match tok {
                        Tok::Colon => St::Value(None),
                        other => return Err(self.syntax_error(&other)),
                    }
                }
                St::AfterValue(v) => {
                    let Some(fr) = self.stack.last_mut() else {
                        // `start: value PHP_JSON_T_EOI`
                        let tok = self.sc.scan();
                        return match tok {
                            Tok::Eoi => Ok(v),
                            other => Err(self.syntax_error(&other)),
                        };
                    };
                    n = fr.base + 3;
                    let is_object = fr.is_object;
                    if is_object {
                        let key = std::mem::take(&mut fr.key);
                        self.add_member(key, v)?;
                    } else if let Acc::Array(a) = &mut fr.acc {
                        // `array_append` cannot fail short of memory exhaustion.
                        let _ = a.append(v);
                    }
                    let tok = self.sc.scan();
                    let base = self.stack.last().expect("open container").base;
                    if is_object {
                        match tok {
                            Tok::Comma => {
                                self.check(base + 4)?;
                                St::ExpectKey
                            }
                            Tok::RBrace => St::AfterValue(self.close(false, &mut n)?),
                            Tok::RBracket => return Err(self.fail(ErrorCode::StateMismatch)),
                            other => return Err(self.syntax_error(&other)),
                        }
                    } else {
                        match tok {
                            Tok::Comma => {
                                self.check(base + 4)?;
                                n = base + 4;
                                St::Value(None)
                            }
                            Tok::RBracket => St::AfterValue(self.close(false, &mut n)?),
                            Tok::RBrace => return Err(self.fail(ErrorCode::StateMismatch)),
                            other => return Err(self.syntax_error(&other)),
                        }
                    }
                }
            };
        }
    }

    /// A scalar token was shifted at stack size `n`.
    fn scalar(&self, v: Value, n: usize) -> Result<Value, ErrorDetails> {
        self.check(n + 1)?;
        Ok(v)
    }

    /// `member: key ':' value` / `member ',' key ':' value`:
    /// `object_create` (first member) and `object_update`.
    fn add_member(&mut self, key: Vec<u8>, v: Value) -> Result<(), ErrorDetails> {
        let invalid_name = !key.is_empty() && key[0] == 0;
        let needs_object = matches!(self.stack.last().map(|f| &f.acc), Some(Acc::ObjectPending));
        if needs_object {
            let obj = self.new_object();
            self.stack.last_mut().expect("open container").acc = Acc::Object(obj);
        }
        let fr = self.stack.last_mut().expect("open container");
        match &mut fr.acc {
            Acc::None => {}
            Acc::Array(a) => {
                // `zend_symtable_update`: numeric string keys become integers.
                a.insert(ArrayKey::from_bytes(&key), v);
            }
            Acc::Object(o) => {
                if invalid_name {
                    return Err(self.fail(ErrorCode::InvalidPropertyName));
                }
                o.set_prop(&key, v);
            }
            Acc::ObjectPending => unreachable!("object was created above"),
        }
        Ok(())
    }
}
