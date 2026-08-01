(function_declaration
  "async"? @context
  "function" @context
  name: (_) @name) @item

(generator_function_declaration
  "async"? @context
  "function" @context
  "*" @context
  name: (_) @name) @item

(class_declaration
  "class" @context
  name: (_) @name) @item

(method_definition
  name: (_) @name) @item

(comment) @annotation
