//! Token kinds, numeric ids and display names.
//!
//! Token ids are assigned exactly like bison assigns them for php-src's
//! `zend_language_parser.y`: single-character tokens use their character code,
//! `END` is 0 and every named token gets a number starting at 258 in order of
//! first appearance in the grammar's declaration section (precedence
//! declarations included, which is why `T_THROW` is 258).

/// How bison names a token in syntax error messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BisonName {
    /// A token with a single fixed spelling, shown as `token "x"` when
    /// unexpected and `"x"` when expected.
    Literal(&'static str),
    /// A token class such as `identifier`; when unexpected the token text
    /// is appended (`identifier "foo"`).
    Descr(&'static str),
    /// A token without an alias in the grammar (bison prints its name).
    Raw(&'static str),
    /// `T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG`, which php-src always
    /// renders as `token "&"`.
    Amp,
}

macro_rules! tokens {
    (
        chars { $($cv:ident = $cc:literal;)* }
        named { $($v:ident = $id:literal, $name:literal, $disp:expr;)* }
    ) => {
        /// A lexical token kind.
        ///
        /// The discriminant is the token id used by `token_get_all()` /
        /// `PhpToken::$id`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        pub enum TokenKind {
            $(
                #[doc = concat!("The `", stringify!($cc), "` character token.")]
                $cv = $cc as u16,
            )*
            $(
                #[doc = concat!("`", $name, "`")]
                $v = $id,
            )*
        }

        impl TokenKind {
            /// All token kinds.
            pub const ALL: &'static [TokenKind] = &[$(TokenKind::$cv,)* $(TokenKind::$v,)*];

            /// The numeric token id (character code for single-character tokens).
            #[inline]
            pub fn id(self) -> u16 {
                self as u16
            }

            /// The `T_*` constant name (`None` for single-character tokens).
            ///
            /// Note that `token_name()` reports `T_PAAMAYIM_NEKUDOTAYIM` as
            /// `T_DOUBLE_COLON`; use [`TokenKind::token_name`] for that.
            pub fn name(self) -> Option<&'static str> {
                match self {
                    $(TokenKind::$cv => None,)*
                    $(TokenKind::$v => Some($name),)*
                }
            }

            /// The single character of a character token.
            pub fn as_char(self) -> Option<u8> {
                match self {
                    $(TokenKind::$cv => Some($cc),)*
                    _ => None,
                }
            }

            /// Look up a token kind by id.
            pub fn from_id(id: u16) -> Option<TokenKind> {
                match id {
                    $(x if x == $cc as u16 => Some(TokenKind::$cv),)*
                    $($id => Some(TokenKind::$v),)*
                    _ => None,
                }
            }

            /// How bison describes this token in syntax errors.
            pub fn display(self) -> BisonName {
                match self {
                    $(TokenKind::$cv => BisonName::Literal(char_str($cc)),)*
                    $(TokenKind::$v => $disp,)*
                }
            }

            /// The name in the grammar file (`';'` style for characters).
            pub fn grammar_name(self) -> std::borrow::Cow<'static, str> {
                match self {
                    $(TokenKind::$cv => std::borrow::Cow::Owned(format!("'{}'", $cc as char)),)*
                    $(TokenKind::$v => std::borrow::Cow::Borrowed($name),)*
                }
            }
        }
    };
}

fn char_str(c: u8) -> &'static str {
    const ALL: &str = "!\"$%&()*+,-./:;<=>?@[]^`{|}~";
    let i = ALL.bytes().position(|b| b == c).expect("character token");
    &ALL[i..i + 1]
}

tokens! {
    chars {
        Bang = b'!';
        DoubleQuote = b'"';
        Dollar = b'$';
        Percent = b'%';
        Amp = b'&';
        LParen = b'(';
        RParen = b')';
        Star = b'*';
        Plus = b'+';
        Comma = b',';
        Minus = b'-';
        Dot = b'.';
        Slash = b'/';
        Colon = b':';
        Semicolon = b';';
        Lt = b'<';
        Equals = b'=';
        Gt = b'>';
        Question = b'?';
        At = b'@';
        LBracket = b'[';
        RBracket = b']';
        Caret = b'^';
        Backtick = b'`';
        LBrace = b'{';
        Bar = b'|';
        RBrace = b'}';
        Tilde = b'~';
    }
    named {
        Throw = 258, "T_THROW", BisonName::Literal("throw");
        Include = 260, "T_INCLUDE", BisonName::Literal("include");
        IncludeOnce = 261, "T_INCLUDE_ONCE", BisonName::Literal("include_once");
        Require = 262, "T_REQUIRE", BisonName::Literal("require");
        RequireOnce = 263, "T_REQUIRE_ONCE", BisonName::Literal("require_once");
        LogicalOr = 264, "T_LOGICAL_OR", BisonName::Literal("or");
        LogicalXor = 265, "T_LOGICAL_XOR", BisonName::Literal("xor");
        LogicalAnd = 266, "T_LOGICAL_AND", BisonName::Literal("and");
        Print = 267, "T_PRINT", BisonName::Literal("print");
        Yield = 268, "T_YIELD", BisonName::Literal("yield");
        DoubleArrow = 269, "T_DOUBLE_ARROW", BisonName::Literal("=>");
        YieldFrom = 270, "T_YIELD_FROM", BisonName::Literal("yield from");
        PlusEqual = 271, "T_PLUS_EQUAL", BisonName::Literal("+=");
        MinusEqual = 272, "T_MINUS_EQUAL", BisonName::Literal("-=");
        MulEqual = 273, "T_MUL_EQUAL", BisonName::Literal("*=");
        DivEqual = 274, "T_DIV_EQUAL", BisonName::Literal("/=");
        ConcatEqual = 275, "T_CONCAT_EQUAL", BisonName::Literal(".=");
        ModEqual = 276, "T_MOD_EQUAL", BisonName::Literal("%=");
        AndEqual = 277, "T_AND_EQUAL", BisonName::Literal("&=");
        OrEqual = 278, "T_OR_EQUAL", BisonName::Literal("|=");
        XorEqual = 279, "T_XOR_EQUAL", BisonName::Literal("^=");
        SlEqual = 280, "T_SL_EQUAL", BisonName::Literal("<<=");
        SrEqual = 281, "T_SR_EQUAL", BisonName::Literal(">>=");
        PowEqual = 282, "T_POW_EQUAL", BisonName::Literal("**=");
        CoalesceEqual = 283, "T_COALESCE_EQUAL", BisonName::Literal("??=");
        Coalesce = 284, "T_COALESCE", BisonName::Literal("??");
        BooleanOr = 285, "T_BOOLEAN_OR", BisonName::Literal("||");
        BooleanAnd = 286, "T_BOOLEAN_AND", BisonName::Literal("&&");
        AmpersandNotFollowedByVarOrVararg = 287, "T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG", BisonName::Amp;
        AmpersandFollowedByVarOrVararg = 288, "T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG", BisonName::Literal("&");
        IsEqual = 289, "T_IS_EQUAL", BisonName::Literal("==");
        IsNotEqual = 290, "T_IS_NOT_EQUAL", BisonName::Literal("!=");
        IsIdentical = 291, "T_IS_IDENTICAL", BisonName::Literal("===");
        IsNotIdentical = 292, "T_IS_NOT_IDENTICAL", BisonName::Literal("!==");
        Spaceship = 293, "T_SPACESHIP", BisonName::Literal("<=>");
        IsSmallerOrEqual = 294, "T_IS_SMALLER_OR_EQUAL", BisonName::Literal("<=");
        IsGreaterOrEqual = 295, "T_IS_GREATER_OR_EQUAL", BisonName::Literal(">=");
        Pipe = 296, "T_PIPE", BisonName::Literal("|>");
        Sl = 297, "T_SL", BisonName::Literal("<<");
        Sr = 298, "T_SR", BisonName::Literal(">>");
        Instanceof = 299, "T_INSTANCEOF", BisonName::Literal("instanceof");
        IntCast = 300, "T_INT_CAST", BisonName::Literal("(int)");
        DoubleCast = 301, "T_DOUBLE_CAST", BisonName::Literal("(float)");
        StringCast = 302, "T_STRING_CAST", BisonName::Literal("(string)");
        ArrayCast = 303, "T_ARRAY_CAST", BisonName::Literal("(array)");
        ObjectCast = 304, "T_OBJECT_CAST", BisonName::Literal("(object)");
        BoolCast = 305, "T_BOOL_CAST", BisonName::Literal("(bool)");
        UnsetCast = 306, "T_UNSET_CAST", BisonName::Literal("(unset)");
        Pow = 307, "T_POW", BisonName::Literal("**");
        Clone = 308, "T_CLONE", BisonName::Literal("clone");
        ElseIf = 310, "T_ELSEIF", BisonName::Literal("elseif");
        Else = 311, "T_ELSE", BisonName::Literal("else");
        LNumber = 312, "T_LNUMBER", BisonName::Descr("integer");
        DNumber = 313, "T_DNUMBER", BisonName::Descr("floating-point number");
        String = 314, "T_STRING", BisonName::Descr("identifier");
        NameFullyQualified = 315, "T_NAME_FULLY_QUALIFIED", BisonName::Descr("fully qualified name");
        NameRelative = 316, "T_NAME_RELATIVE", BisonName::Descr("namespace-relative name");
        NameQualified = 317, "T_NAME_QUALIFIED", BisonName::Descr("namespaced name");
        Variable = 318, "T_VARIABLE", BisonName::Descr("variable");
        InlineHtml = 319, "T_INLINE_HTML", BisonName::Raw("T_INLINE_HTML");
        EncapsedAndWhitespace = 320, "T_ENCAPSED_AND_WHITESPACE", BisonName::Descr("string content");
        ConstantEncapsedString = 321, "T_CONSTANT_ENCAPSED_STRING", BisonName::Descr("quoted string");
        StringVarname = 322, "T_STRING_VARNAME", BisonName::Descr("variable name");
        NumString = 323, "T_NUM_STRING", BisonName::Descr("number");
        Eval = 324, "T_EVAL", BisonName::Literal("eval");
        New = 325, "T_NEW", BisonName::Literal("new");
        Exit = 326, "T_EXIT", BisonName::Literal("exit");
        If = 327, "T_IF", BisonName::Literal("if");
        EndIf = 328, "T_ENDIF", BisonName::Literal("endif");
        Echo = 329, "T_ECHO", BisonName::Literal("echo");
        Do = 330, "T_DO", BisonName::Literal("do");
        While = 331, "T_WHILE", BisonName::Literal("while");
        EndWhile = 332, "T_ENDWHILE", BisonName::Literal("endwhile");
        For = 333, "T_FOR", BisonName::Literal("for");
        EndFor = 334, "T_ENDFOR", BisonName::Literal("endfor");
        Foreach = 335, "T_FOREACH", BisonName::Literal("foreach");
        EndForeach = 336, "T_ENDFOREACH", BisonName::Literal("endforeach");
        Declare = 337, "T_DECLARE", BisonName::Literal("declare");
        EndDeclare = 338, "T_ENDDECLARE", BisonName::Literal("enddeclare");
        As = 339, "T_AS", BisonName::Literal("as");
        Switch = 340, "T_SWITCH", BisonName::Literal("switch");
        EndSwitch = 341, "T_ENDSWITCH", BisonName::Literal("endswitch");
        Case = 342, "T_CASE", BisonName::Literal("case");
        Default = 343, "T_DEFAULT", BisonName::Literal("default");
        Match = 344, "T_MATCH", BisonName::Literal("match");
        Break = 345, "T_BREAK", BisonName::Literal("break");
        Continue = 346, "T_CONTINUE", BisonName::Literal("continue");
        Goto = 347, "T_GOTO", BisonName::Literal("goto");
        Function = 348, "T_FUNCTION", BisonName::Literal("function");
        Fn = 349, "T_FN", BisonName::Literal("fn");
        Const = 350, "T_CONST", BisonName::Literal("const");
        Return = 351, "T_RETURN", BisonName::Literal("return");
        Try = 352, "T_TRY", BisonName::Literal("try");
        Catch = 353, "T_CATCH", BisonName::Literal("catch");
        Finally = 354, "T_FINALLY", BisonName::Literal("finally");
        Use = 355, "T_USE", BisonName::Literal("use");
        Insteadof = 356, "T_INSTEADOF", BisonName::Literal("insteadof");
        Global = 357, "T_GLOBAL", BisonName::Literal("global");
        Static = 358, "T_STATIC", BisonName::Literal("static");
        Abstract = 359, "T_ABSTRACT", BisonName::Literal("abstract");
        Final = 360, "T_FINAL", BisonName::Literal("final");
        Private = 361, "T_PRIVATE", BisonName::Literal("private");
        Protected = 362, "T_PROTECTED", BisonName::Literal("protected");
        Public = 363, "T_PUBLIC", BisonName::Literal("public");
        PrivateSet = 364, "T_PRIVATE_SET", BisonName::Literal("private(set)");
        ProtectedSet = 365, "T_PROTECTED_SET", BisonName::Literal("protected(set)");
        PublicSet = 366, "T_PUBLIC_SET", BisonName::Literal("public(set)");
        Readonly = 367, "T_READONLY", BisonName::Literal("readonly");
        Var = 368, "T_VAR", BisonName::Literal("var");
        Unset = 369, "T_UNSET", BisonName::Literal("unset");
        Isset = 370, "T_ISSET", BisonName::Literal("isset");
        Empty = 371, "T_EMPTY", BisonName::Literal("empty");
        HaltCompiler = 372, "T_HALT_COMPILER", BisonName::Literal("__halt_compiler");
        Class = 373, "T_CLASS", BisonName::Literal("class");
        Trait = 374, "T_TRAIT", BisonName::Literal("trait");
        Interface = 375, "T_INTERFACE", BisonName::Literal("interface");
        Enum = 376, "T_ENUM", BisonName::Literal("enum");
        Extends = 377, "T_EXTENDS", BisonName::Literal("extends");
        Implements = 378, "T_IMPLEMENTS", BisonName::Literal("implements");
        Namespace = 379, "T_NAMESPACE", BisonName::Literal("namespace");
        List = 380, "T_LIST", BisonName::Literal("list");
        Array = 381, "T_ARRAY", BisonName::Literal("array");
        Callable = 382, "T_CALLABLE", BisonName::Literal("callable");
        Line = 383, "T_LINE", BisonName::Literal("__LINE__");
        File = 384, "T_FILE", BisonName::Literal("__FILE__");
        Dir = 385, "T_DIR", BisonName::Literal("__DIR__");
        ClassC = 386, "T_CLASS_C", BisonName::Literal("__CLASS__");
        TraitC = 387, "T_TRAIT_C", BisonName::Literal("__TRAIT__");
        MethodC = 388, "T_METHOD_C", BisonName::Literal("__METHOD__");
        FuncC = 389, "T_FUNC_C", BisonName::Literal("__FUNCTION__");
        PropertyC = 390, "T_PROPERTY_C", BisonName::Literal("__PROPERTY__");
        NsC = 391, "T_NS_C", BisonName::Literal("__NAMESPACE__");
        End = 0, "END", BisonName::Descr("end of file");
        Attribute = 392, "T_ATTRIBUTE", BisonName::Literal("#[");
        Inc = 393, "T_INC", BisonName::Literal("++");
        Dec = 394, "T_DEC", BisonName::Literal("--");
        VoidCast = 395, "T_VOID_CAST", BisonName::Literal("(void)");
        ObjectOperator = 396, "T_OBJECT_OPERATOR", BisonName::Literal("->");
        NullsafeObjectOperator = 397, "T_NULLSAFE_OBJECT_OPERATOR", BisonName::Literal("?->");
        Comment = 398, "T_COMMENT", BisonName::Descr("comment");
        DocComment = 399, "T_DOC_COMMENT", BisonName::Descr("doc comment");
        OpenTag = 400, "T_OPEN_TAG", BisonName::Descr("open tag");
        OpenTagWithEcho = 401, "T_OPEN_TAG_WITH_ECHO", BisonName::Literal("<?=");
        CloseTag = 402, "T_CLOSE_TAG", BisonName::Literal("?>");
        Whitespace = 403, "T_WHITESPACE", BisonName::Descr("whitespace");
        StartHeredoc = 404, "T_START_HEREDOC", BisonName::Descr("heredoc start");
        EndHeredoc = 405, "T_END_HEREDOC", BisonName::Descr("heredoc end");
        DollarOpenCurlyBraces = 406, "T_DOLLAR_OPEN_CURLY_BRACES", BisonName::Literal("${");
        CurlyOpen = 407, "T_CURLY_OPEN", BisonName::Literal("{$");
        PaamayimNekudotayim = 408, "T_PAAMAYIM_NEKUDOTAYIM", BisonName::Literal("::");
        NsSeparator = 409, "T_NS_SEPARATOR", BisonName::Literal("\\");
        Ellipsis = 410, "T_ELLIPSIS", BisonName::Literal("...");
        BadCharacter = 411, "T_BAD_CHARACTER", BisonName::Descr("invalid character");
        Error = 412, "T_ERROR", BisonName::Raw("T_ERROR");
    }
}

impl TokenKind {
    /// The name reported by `token_name()`.
    pub fn token_name(self) -> &'static str {
        match self {
            TokenKind::PaamayimNekudotayim => "T_DOUBLE_COLON",
            other => other.name().unwrap_or("UNKNOWN"),
        }
    }

    /// Whether this is a single-character token (returned as a plain string
    /// by `token_get_all()`).
    pub fn is_char(self) -> bool {
        (self as u16) < 256 && self != TokenKind::End
    }

    /// Keywords that may be used as identifiers in member names
    /// (`semi_reserved` in the grammar).
    pub fn is_semi_reserved(self) -> bool {
        self.is_reserved_non_modifier()
            || matches!(
                self,
                TokenKind::Static
                    | TokenKind::Abstract
                    | TokenKind::Final
                    | TokenKind::Private
                    | TokenKind::Protected
                    | TokenKind::Public
                    | TokenKind::Readonly
            )
    }

    /// `reserved_non_modifiers` in the grammar.
    pub fn is_reserved_non_modifier(self) -> bool {
        use TokenKind::*;
        matches!(
            self,
            Include
                | IncludeOnce
                | Eval
                | Require
                | RequireOnce
                | LogicalOr
                | LogicalXor
                | LogicalAnd
                | Instanceof
                | New
                | Clone
                | Exit
                | If
                | ElseIf
                | Else
                | EndIf
                | Echo
                | Do
                | While
                | EndWhile
                | For
                | EndFor
                | Foreach
                | EndForeach
                | Declare
                | EndDeclare
                | As
                | Try
                | Catch
                | Finally
                | Throw
                | Use
                | Insteadof
                | Global
                | Var
                | Unset
                | Isset
                | Empty
                | Continue
                | Goto
                | Function
                | Const
                | Return
                | Print
                | Yield
                | List
                | Switch
                | EndSwitch
                | Case
                | Default
                | Break
                | Array
                | Callable
                | Extends
                | Implements
                | Namespace
                | Trait
                | Interface
                | Class
                | ClassC
                | TraitC
                | FuncC
                | MethodC
                | Line
                | File
                | Dir
                | NsC
                | Fn
                | Match
                | Enum
                | PropertyC
        )
    }

    /// Member modifiers (`member_modifier` in the grammar).
    pub fn is_member_modifier(self) -> bool {
        use TokenKind::*;
        matches!(
            self,
            Public
                | Protected
                | Private
                | PublicSet
                | ProtectedSet
                | PrivateSet
                | Static
                | Abstract
                | Final
                | Readonly
        )
    }
}

/// A token produced by [`crate::tokenize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// The token kind.
    pub kind: TokenKind,
    /// Byte offset of the token text in the source.
    pub start: u32,
    /// Length of the token text in bytes.
    pub len: u32,
    /// Line the token starts on (1-based).
    pub line: u32,
}

impl Token {
    /// The token's text.
    pub fn text<'a>(&self, source: &'a [u8]) -> &'a [u8] {
        let s = self.start as usize;
        &source[s..s + self.len as usize]
    }
}
