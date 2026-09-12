[
  (program)
  (statement_block)
  (function_expression)
  (arrow_function)
  (function_declaration)
  (method_definition)
  (catch_clause)
] @local.scope

(pattern/identifier) @local.definition

(variable_declarator
  name: (identifier) @local.definition)

(required_parameter (identifier) @local.definition)
(optional_parameter (identifier) @local.definition)
(catch_clause
  parameter: (identifier) @local.definition)

(identifier) @local.reference
