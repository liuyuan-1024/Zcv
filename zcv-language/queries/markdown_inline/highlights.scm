(emphasis) @text.emphasis

(strong_emphasis) @text.strong

(code_span) @text.literal

(strikethrough) @text.strike

[
  (link_text)
  (link_label)
  (image_description)
] @text.reference

[
  (link_destination)
  (uri_autolink)
  (email_autolink)
] @text.uri

[
  (emphasis_delimiter)
  (code_span_delimiter)
] @punctuation.delimiter

[
  (backslash_escape)
  (hard_line_break)
] @string.escape

(image
  [
    "!"
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(inline_link
  [
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(shortcut_link
  [
    "["
    "]"
  ] @punctuation.delimiter)
