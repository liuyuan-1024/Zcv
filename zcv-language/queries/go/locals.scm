[
  (source_file)
  (block)
  (function_declaration)
  (method_declaration)
  (func_literal)
  (for_statement)
  (if_statement)
  (expression_switch_statement)
  (type_switch_statement)
  (select_statement)
] @local.scope

(parameter_declaration
  name: (identifier) @local.definition)

(short_var_declaration
  left: (expression_list
    (identifier) @local.definition))

(var_spec
  name: (identifier) @local.definition)

(range_clause
  left: (expression_list
    (identifier) @local.definition))

(identifier) @local.reference
