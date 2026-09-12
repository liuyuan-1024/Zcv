(internal_module
  "namespace" @context
  name: (_) @name) @item

(enum_declaration
  "enum" @context
  name: (_) @name) @item

(type_alias_declaration
  "type" @context
  name: (_) @name) @item

(function_declaration
  "async"? @context
  "function" @context
  name: (_) @name
  body: (statement_block
    "{" @open
    "}" @close)) @item

(generator_function_declaration
  "async"? @context
  "function" @context
  "*" @context
  name: (_) @name) @item

(interface_declaration
  "interface" @context
  name: (_) @name) @item

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
