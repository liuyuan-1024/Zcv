[
  (entry_type)
  (string_type)
  (preamble_type)
] @keyword

[
  (key_brace)
  (key_paren)
] @label

(field name: (identifier) @property)
(string name: (identifier) @constant)
(token (identifier) @constant)
(number) @number

[
  (brace_word)
  (quote_word)
] @string

(command_name) @function

[
  (comment)
  (junk)
] @comment

["=" "#"] @operator
"," @punctuation.delimiter
