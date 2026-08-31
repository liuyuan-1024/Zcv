(bare_key) @type

(quoted_key) @string

(pair
  (bare_key) @property)

(pair
  (dotted_key
    (bare_key) @property))

(boolean) @boolean

(comment) @comment

(string) @string

[
  (integer)
  (float)
] @number

[
  (offset_date_time)
  (local_date_time)
  (local_date)
  (local_time)
] @string.special

[
  "."
  ","
] @punctuation.delimiter

"=" @operator

[
  "["
  "]"
  "[["
  "]]"
  "{"
  "}"
] @punctuation.bracket
