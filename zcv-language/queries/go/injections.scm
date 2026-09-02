; `/* sql */` 紧邻的字符串内容按 SQL 解析。
([
  (const_spec
    name: (identifier)
    "="
    (comment) @_comment
    value: (expression_list
      [
        (interpreted_string_literal
          (interpreted_string_literal_content) @injection.content)
        (raw_string_literal
          (raw_string_literal_content) @injection.content)
      ]))
  (var_spec
    name: (identifier)
    "="
    (comment) @_comment
    value: (expression_list
      [
        (interpreted_string_literal
          (interpreted_string_literal_content) @injection.content)
        (raw_string_literal
          (raw_string_literal_content) @injection.content)
      ]))
  (assignment_statement
    left: (expression_list)
    "="
    (comment) @_comment
    right: (expression_list
      [
        (interpreted_string_literal
          (interpreted_string_literal_content) @injection.content)
        (raw_string_literal
          (raw_string_literal_content) @injection.content)
      ]))
  (short_var_declaration
    left: (expression_list)
    ":="
    (comment) @_comment
    right: (expression_list
      [
        (interpreted_string_literal
          (interpreted_string_literal_content) @injection.content)
        (raw_string_literal
          (raw_string_literal_content) @injection.content)
      ]))
  (composite_literal
    body: (literal_value
      (keyed_element
        (comment) @_comment
        value: (literal_element
          [
            (interpreted_string_literal
              (interpreted_string_literal_content) @injection.content)
            (raw_string_literal
              (raw_string_literal_content) @injection.content)
          ]))))
  (expression_statement
    (call_expression
      (argument_list
        (comment) @_comment
        [
          (interpreted_string_literal
            (interpreted_string_literal_content) @injection.content)
          (raw_string_literal
            (raw_string_literal_content) @injection.content)
        ])))
]
  (#match? @_comment "^\\/\\*\\s*sql\\s*\\*\\/$")
  (#set! injection.language "sql"))
