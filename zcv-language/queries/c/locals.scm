[
  (translation_unit)
  (compound_statement)
  (function_definition)
  (for_statement)
  (if_statement)
  (while_statement)
  (switch_statement)
] @local.scope

(parameter_declaration
  declarator: (identifier) @local.definition)

(init_declarator
  declarator: (identifier) @local.definition)

(identifier) @local.reference
