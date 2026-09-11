(tag_name) @tag

; 大写开头的标签名遵循组件命名约定。
((tag_name) @tag.component
  (#match? @tag.component "^[A-Z]"))

(doctype) @tag.doctype

(attribute_name) @attribute

[
  "\""
  "'"
  (attribute_value)
] @string

(comment) @comment

(entity) @string.special

"=" @punctuation.delimiter.html

[
  "<"
  ">"
  "<!"
  "</"
  "/>"
] @punctuation.bracket.html
