(identifier) @variable

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z\\d_]*$"))

[
  "alignas"
  "alignof"
  "break"
  "case"
  "catch"
  "class"
  "concept"
  "const"
  "consteval"
  "constexpr"
  "constinit"
  "continue"
  "co_await"
  "co_return"
  "co_yield"
  "default"
  "delete"
  "do"
  "else"
  "enum"
  "explicit"
  "extern"
  "final"
  "for"
  "friend"
  "if"
  "inline"
  "mutable"
  "namespace"
  "new"
  "noexcept"
  "override"
  "private"
  "protected"
  "public"
  "requires"
  "return"
  "sizeof"
  "static"
  "struct"
  "switch"
  "template"
  "throw"
  "try"
  "typedef"
  "typename"
  "union"
  "using"
  "virtual"
  "volatile"
  "while"
] @keyword

[
  (string_literal)
  (raw_string_literal)
  (system_lib_string)
] @string

(char_literal) @string
(number_literal) @number
(comment) @comment

[
  (primitive_type)
  (sized_type_specifier)
  (type_identifier)
  (namespace_identifier)
] @type

(this) @variable.builtin
(null) @constant

(field_identifier) @property
(statement_identifier) @label

(call_expression
  function: (identifier) @function.call)

(call_expression
  function: (field_expression
    field: (field_identifier) @function.call))

(function_declarator
  declarator: (identifier) @function.definition)

(function_declarator
  declarator: (qualified_identifier
    name: (identifier) @function.definition))

(function_declarator
  declarator: (field_identifier) @function.definition)

(preproc_directive) @keyword
