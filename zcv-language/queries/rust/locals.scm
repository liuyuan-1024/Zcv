[
  (source_file)
  (block)
  (closure_expression)
  (for_expression)
  (match_arm)
] @local.scope

(parameter
  pattern: (identifier) @local.definition)

(let_declaration
  pattern: (identifier) @local.definition)

(for_expression
  pattern: (identifier) @local.definition)

(closure_parameters
  (identifier) @local.definition)

(identifier) @local.reference
