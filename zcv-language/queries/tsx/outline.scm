(function_declaration
  "async"? @context
  "function" @context
  name: (_) @name
  body: (statement_block
    "{" @open
    "}" @close)) @item

(class_declaration
  "class" @context
  name: (_) @name
  body: (class_body
    "{" @open
    "}" @close)) @item

(method_definition
  name: (_) @name) @item

(lexical_declaration
  ["let" "const" "var"] @context
  (variable_declarator
    name: (identifier) @name) @item)
