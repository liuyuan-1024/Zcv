[
  (module)
  (block)
  (function_definition)
  (class_definition)
  (lambda)
  (for_statement)
  (with_statement)
] @local.scope

(parameters
  (identifier) @local.definition)
(parameters
  (typed_parameter
    (identifier) @local.definition))
(parameters
  (default_parameter
    name: (identifier) @local.definition))
(parameters
  (typed_default_parameter
    name: (identifier) @local.definition))
(parameters
  (list_splat_pattern
    (identifier) @local.definition))
(parameters
  (dictionary_splat_pattern
    (identifier) @local.definition))

(assignment
  left: (identifier) @local.definition)
(assignment
  left: (pattern_list
    (identifier) @local.definition))
(for_statement
  left: (identifier) @local.definition)
(for_statement
  left: (pattern_list
    (identifier) @local.definition))

(identifier) @local.reference
