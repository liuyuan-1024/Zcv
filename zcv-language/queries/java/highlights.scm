; 变量

(identifier) @variable

; 方法

(method_declaration
  name: (identifier) @function.method)

(method_invocation
  name: (identifier) @function.method)

(super) @function.builtin

; 注解

(annotation
  name: (identifier) @attribute)

(marker_annotation
  name: (identifier) @attribute)

"@" @operator

; 类型

(type_identifier) @type

(interface_declaration
  name: (identifier) @type)

(class_declaration
  name: (identifier) @type)

(enum_declaration
  name: (identifier) @type)

((field_access
  object: (identifier) @type)
  (#match? @type "^[A-Z]"))

((scoped_identifier
  scope: (identifier) @type)
  (#match? @type "^[A-Z]"))

((method_invocation
  object: (identifier) @type)
  (#match? @type "^[A-Z]"))

((method_reference
  . (identifier) @type)
  (#match? @type "^[A-Z]"))

(constructor_declaration
  name: (identifier) @type)

[
  (boolean_type)
  (integral_type)
  (floating_point_type)
  (void_type)
] @type.builtin

; 常量

((identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z\\d_]+$"))

; 内置值

(this) @variable.builtin

; 字面量

[
  (hex_integer_literal)
  (decimal_integer_literal)
  (octal_integer_literal)
  (decimal_floating_point_literal)
  (hex_floating_point_literal)
] @number

[
  (character_literal)
  (string_literal)
] @string

(escape_sequence) @string.escape

[
  (true)
  (false)
  (null_literal)
] @constant.builtin

[
  (line_comment)
  (block_comment)
] @comment

; 关键字

[
  "abstract"
  "assert"
  "break"
  "case"
  "catch"
  "class"
  "continue"
  "default"
  "do"
  "else"
  "enum"
  "exports"
  "extends"
  "final"
  "finally"
  "for"
  "if"
  "implements"
  "import"
  "instanceof"
  "interface"
  "module"
  "native"
  "new"
  "non-sealed"
  "open"
  "opens"
  "package"
  "permits"
  "private"
  "protected"
  "provides"
  "public"
  "requires"
  "record"
  "return"
  "sealed"
  "static"
  "strictfp"
  "switch"
  "synchronized"
  "throw"
  "throws"
  "to"
  "transient"
  "transitive"
  "try"
  "uses"
  "volatile"
  "when"
  "while"
  "with"
  "yield"
] @keyword
