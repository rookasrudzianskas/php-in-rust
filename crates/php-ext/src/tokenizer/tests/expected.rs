// Expected outputs of php-src's ext/tokenizer tests (generated from the .phpt files).

pub const TOKEN_NAMES: &str = r####"T_INCLUDE
T_INCLUDE_ONCE
T_EVAL
T_REQUIRE
T_REQUIRE_ONCE
T_LOGICAL_OR
T_LOGICAL_XOR
T_LOGICAL_AND
T_PRINT
T_PLUS_EQUAL
T_MINUS_EQUAL
T_MUL_EQUAL
T_DIV_EQUAL
T_CONCAT_EQUAL
T_MOD_EQUAL
T_AND_EQUAL
T_OR_EQUAL
T_XOR_EQUAL
T_SL_EQUAL
T_SR_EQUAL
T_BOOLEAN_OR
T_BOOLEAN_AND
T_IS_EQUAL
T_IS_NOT_EQUAL
T_IS_IDENTICAL
T_IS_NOT_IDENTICAL
T_IS_SMALLER_OR_EQUAL
T_IS_GREATER_OR_EQUAL
T_SL
T_SR
T_INC
T_DEC
T_INT_CAST
T_DOUBLE_CAST
T_STRING_CAST
T_ARRAY_CAST
T_OBJECT_CAST
T_BOOL_CAST
T_UNSET_CAST
T_NEW
T_EXIT
T_IF
T_ELSEIF
T_ELSE
T_ENDIF
T_LNUMBER
T_DNUMBER
T_STRING
T_STRING_VARNAME
T_VARIABLE
T_NUM_STRING
T_INLINE_HTML
T_ENCAPSED_AND_WHITESPACE
T_CONSTANT_ENCAPSED_STRING
T_ECHO
T_DO
T_WHILE
T_ENDWHILE
T_FOR
T_ENDFOR
T_FOREACH
T_ENDFOREACH
T_DECLARE
T_ENDDECLARE
T_AS
T_SWITCH
T_ENDSWITCH
T_CASE
T_DEFAULT
T_BREAK
T_CONTINUE
T_FUNCTION
T_CONST
T_RETURN
T_USE
T_GLOBAL
T_STATIC
T_VAR
T_UNSET
T_ISSET
T_EMPTY
T_CLASS
T_EXTENDS
T_INTERFACE
T_IMPLEMENTS
T_OBJECT_OPERATOR
T_DOUBLE_ARROW
T_LIST
T_ARRAY
T_CLASS_C
T_FUNC_C
T_PROPERTY_C
T_METHOD_C
T_LINE
T_FILE
T_COMMENT
T_DOC_COMMENT
T_OPEN_TAG
T_OPEN_TAG_WITH_ECHO
T_CLOSE_TAG
T_WHITESPACE
T_START_HEREDOC
T_END_HEREDOC
T_DOLLAR_OPEN_CURLY_BRACES
T_CURLY_OPEN
T_DOUBLE_COLON
T_DOUBLE_COLON
T_ABSTRACT
T_CATCH
T_FINAL
T_INSTANCEOF
T_PRIVATE
T_PROTECTED
T_PUBLIC
T_THROW
T_TRY
T_CLONE
T_HALT_COMPILER
UNKNOWN
UNKNOWN
Done
"####;

pub const HEREDOC_NOWDOC: &str = r####"Without TOKEN_PARSE:

Test case 1

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_END_HEREDOC ('INNER_END')

Test case 2

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_END_HEREDOC ('  INNER_END')

Test case 3

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_END_HEREDOC ('INNER_END')

Test case 4

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_END_HEREDOC ('  INNER_END')

Test case 5

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('a')

Test case 6

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab')

Test case 7

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('a
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 8

Line 1: T_OPEN_TAG ('<?php
')
Line 2: T_WHITESPACE (' 	')
Line 2: T_START_HEREDOC ('<<<'DOC'
')
Line 3: T_ENCAPSED_AND_WHITESPACE (' 	XXX
')
Line 4: T_END_HEREDOC (' 	DOC')

Test case 9

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 10

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab
')
Line 3: T_END_HEREDOC ('  INNER_END')

Test case 11

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('abc
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 12

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 13

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 14

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab
')
Line 3: T_END_HEREDOC ('  INNER_END')

Test case 15

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('abc
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 16

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 17

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
ab
	')

Test case 18

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
ab
	')

With TOKEN_PARSE:

Test case 1

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_END_HEREDOC ('INNER_END')

Test case 2

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_END_HEREDOC ('  INNER_END')

Test case 3

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_END_HEREDOC ('INNER_END')

Test case 4

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_END_HEREDOC ('  INNER_END')

Test case 5

ParseError: syntax error, unexpected end of file, expecting variable or heredoc end or "${" or "{$" on line 2

Test case 6

ParseError: syntax error, unexpected end of file, expecting variable or heredoc end or "${" or "{$" on line 2

Test case 7

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('a
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 8

ParseError: Invalid indentation - tabs and spaces cannot be mixed on line 3

Test case 9

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 10

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab
')
Line 3: T_END_HEREDOC ('  INNER_END')

Test case 11

ParseError: Invalid body indentation level (expecting an indentation level of at least 3) on line 2

Test case 12

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<INNER_END
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 13

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('ab
')
Line 3: T_END_HEREDOC ('INNER_END')

Test case 14

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab
')
Line 3: T_END_HEREDOC ('  INNER_END')

Test case 15

ParseError: Invalid body indentation level (expecting an indentation level of at least 3) on line 2

Test case 16

Line 1: T_OPEN_TAG ('<?php ')
Line 1: T_START_HEREDOC ('<<<'INNER_END'
')
Line 2: T_ENCAPSED_AND_WHITESPACE ('
')
Line 3: T_END_HEREDOC ('   INNER_END')

Test case 17

ParseError: syntax error, unexpected end of file, expecting variable or heredoc end or "${" or "{$" on line 4

Test case 18

ParseError: syntax error, unexpected end of file, expecting variable or heredoc end or "${" or "{$" on line 4
"####;

pub const PHPTOKEN_METHODS: &str = r####"[ 0] T_OPEN_TAG                 ignorable
[ 1] T_COMMENT                  ignorable
[ 2] T_WHITESPACE               ignorable
[ 3] T_DOC_COMMENT              ignorable
[ 4] T_WHITESPACE               ignorable
[ 5] T_FUNCTION                 meaningful
[ 6] T_WHITESPACE               ignorable
[ 7] T_STRING                   meaningful
[ 8] (                          meaningful
[ 9] )                          meaningful
[10] T_WHITESPACE               ignorable
[11] {                          meaningful
[12] T_WHITESPACE               ignorable
[13] T_ECHO                     meaningful
[14] T_WHITESPACE               ignorable
[15] T_CONSTANT_ENCAPSED_STRING meaningful
[16] ;                          meaningful
[17] T_WHITESPACE               ignorable
[18] }                          meaningful

Success:
bool(true)
bool(true)
bool(true)
bool(true)

Failure:
bool(false)
bool(false)
bool(false)
bool(false)

Error:
TypeError: PhpToken::is(): Argument #1 ($kind) must be of type string|int|array, float given
TypeError: PhpToken::is(): Argument #1 ($kind) must only have elements of type string|int, float given
Error: Typed property PhpToken::$id must not be accessed before initialization
Error: Typed property PhpToken::$text must not be accessed before initialization
Error: Typed property PhpToken::$id must not be accessed before initialization
Error: Typed property PhpToken::$text must not be accessed before initialization

Name of unknown token:
NULL
"####;

pub const NAMESPACED_NAMES: &str = r####"T_OPEN_TAG: "<?php
"
T_STRING: "Foo"
T_WHITESPACE: "
"
T_NAME_QUALIFIED: "Foo\Bar"
T_WHITESPACE: "
"
T_NAME_FULLY_QUALIFIED: "\Foo\Bar"
T_WHITESPACE: "
"
T_NAME_RELATIVE: "namespace\Foo"
T_WHITESPACE: "
"
T_STRING: "Foo"
T_WHITESPACE: " "
T_NS_SEPARATOR: "\"
T_WHITESPACE: " "
T_STRING: "Bar"
"####;

pub const BUG77966: &str = r####"T_OPEN_TAG: "<?php
"
T_CLASS: "class"
T_WHITESPACE: " "
T_STRING: "C"
T_WHITESPACE: " "
{: "{"
T_WHITESPACE: "
    "
T_USE: "use"
T_WHITESPACE: " "
T_STRING: "A"
T_WHITESPACE: " "
{: "{"
T_WHITESPACE: "
        "
T_STRING: "namespace"
T_WHITESPACE: " "
T_AS: "as"
T_WHITESPACE: " "
T_STRING: "bar"
;: ";"
T_WHITESPACE: "
    "
}: "}"
T_WHITESPACE: "
"
}: "}"
"####;

pub const BUG80462: &str = r####"T_OPEN_TAG
T_VARIABLE
T_WHITESPACE
=
T_WHITESPACE
T_VARIABLE
T_NULLSAFE_OBJECT_OPERATOR
T_STRING
(
)
;
"####;

pub const BUG81342: &str = r####"T_OPEN_TAG
T_VARIABLE
T_WHITESPACE
T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
T_WHITESPACE
T_VARIABLE
;
T_WHITESPACE
T_VARIABLE
T_WHITESPACE
T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
T_WHITESPACE
T_VARIABLE
;
T_WHITESPACE
"####;

pub const BUG76991: &str = r####"T_OPEN_TAG: <?php\n
T_START_HEREDOC: <<<TEST\n
T_VARIABLE: $a
T_ENCAPSED_AND_WHITESPACE: \n
T_END_HEREDOC:  TEST
;
T_WHITESPACE: \n\n
T_START_HEREDOC: <<<END\n
T_VARIABLE: $a
T_ENCAPSED_AND_WHITESPACE: \n
T_END_HEREDOC:  END
;
"####;

pub const PARSE_ERRORS: &str = r####"ParseError: Invalid numeric literal
T_OPEN_TAG (<?php )
T_STRING (var_dump)
(
T_LNUMBER (078)
)
;

ParseError: Invalid UTF-8 codepoint escape sequence
T_OPEN_TAG (<?php )
T_STRING (var_dump)
(
T_CONSTANT_ENCAPSED_STRING ("\u{xyz}")
)
;

ParseError: Invalid UTF-8 codepoint escape sequence: Codepoint too large
T_OPEN_TAG (<?php )
T_STRING (var_dump)
(
T_CONSTANT_ENCAPSED_STRING ("\u{ffffff}")
)
;

ParseError: Invalid numeric literal
T_OPEN_TAG (<?php )
T_STRING (var_dump)
(
T_LNUMBER (078)
T_WHITESPACE ( )
+
T_WHITESPACE ( )
T_LNUMBER (078)
)
;
"####;

pub const BAD_CHARACTER: &str = r####"T_OPEN_TAG 6
T_BAD_CHARACTER 1
T_WHITESPACE 1
T_STRING 3

T_OPEN_TAG 6
T_BAD_CHARACTER 1
T_WHITESPACE 1
T_STRING 3

T_OPEN_TAG 6
T_BAD_CHARACTER 1
T_BAD_CHARACTER 1
T_WHITESPACE 1
T_STRING 3
T_WHITESPACE 1
T_BAD_CHARACTER 1
"####;

pub const SEMI_RESERVED_MEMBER: &str = r####"L1: T_OPEN_TAG <?php

L2: T_STRING X
L2: T_DOUBLE_COLON ::
L2: T_STRING continue
;
L3: T_STRING X
L3: T_DOUBLE_COLON ::
L3: T_VARIABLE $continue
;
L4: T_VARIABLE $x
L4: T_OBJECT_OPERATOR ->
L4: T_VARIABLE $continue
;
L5: T_STRING X
L5: T_DOUBLE_COLON ::
L5: T_STRING continue
(
)
;
L6: T_VARIABLE $x
L6: T_OBJECT_OPERATOR ->
L6: T_STRING continue
(
)
;
L7: T_STRING X
L7: T_DOUBLE_COLON ::
L7: T_STRING class
;
L8: T_VARIABLE $x
L8: T_OBJECT_OPERATOR ->
L8: T_STRING __halt_compiler
(
)
;
L10: T_CLASS class
L10: T_STRING X
{
L11: T_CONST const
L11: T_STRING CONTINUE
=
L11: T_LNUMBER 1
;
L12: T_PUBLIC public
L12: T_VARIABLE $x
=
L12: T_STRING self
L12: T_DOUBLE_COLON ::
L12: T_STRING CONTINUE
+
L12: T_LNUMBER 1
;
}
Done
"####;

pub const SEMI_RESERVED_CONST: &str = r####"L1: T_OPEN_TAG <?php

L2: T_CLASS class
L2: T_STRING SomeClass
{
L3: T_CONST const
L3: T_STRING CONST
=
L3: T_LNUMBER 1
;
L4: T_CONST const
L4: T_STRING CONTINUE
=
(
L4: T_STRING self
L4: T_DOUBLE_COLON ::
L4: T_STRING CONST
+
L4: T_LNUMBER 1
)
;
L5: T_CONST const
L5: T_STRING ARRAY
=
[
L5: T_LNUMBER 1
,
L5: T_STRING self
L5: T_DOUBLE_COLON ::
L5: T_STRING CONTINUE
L5: T_DOUBLE_ARROW =>
[
L5: T_LNUMBER 3
,
L5: T_LNUMBER 4
]
,
L5: T_LNUMBER 5
]
;
}
Done
"####;

pub const BUG76437: &str = r####"T_OPEN_TAG_WITH_ECHO: "<?=" on line 1
bool(true)
T_CLOSE_TAG: "?>" on line 1
bool(true)
T_CLOSE_TAG: "?>
" on line 1
bool(true)
"####;
