(identifier) @variable

(this_expression) @variable.builtin
(super_expression) @variable.builtin

(user_type) @type

(user_type
  (identifier) @type)

(function_declaration
  name: (identifier) @function.definition)

(call_expression
  (identifier) @function.call)

(parameter
  (identifier) @variable.parameter)

[
  (line_comment)
  (block_comment)
  (shebang)
] @comment

[
  (number_literal)
  (float_literal)
] @number

((identifier) @boolean
  (#any-of? @boolean "true" "false" "null"))

(character_literal) @string
(string_literal) @string
(escape_sequence) @string.escape

[
  (class_modifier)
  (member_modifier)
  (function_modifier)
  (property_modifier)
  (platform_modifier)
  (variance_modifier)
  (parameter_modifier)
  (visibility_modifier)
  (reification_modifier)
  (inheritance_modifier)
] @keyword

[
  "if"
  "else"
  "when"
  "for"
  "while"
  "do"
  "try"
  "catch"
  "throw"
  "finally"
  "return"
  "throw"
  "val"
  "var"
  "enum"
  "class"
  "object"
  "interface"
  "companion"
  "package"
  "import"
  "fun"
] @keyword

(annotation) @attribute

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "="
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "&&"
  "||"
  "!"
  "?:"
] @operator
