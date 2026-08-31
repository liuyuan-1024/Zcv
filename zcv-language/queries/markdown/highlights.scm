[
  (paragraph)
  (pipe_table)
] @text

(indented_code_block) @text.literal

[
  (atx_heading)
  (setext_heading)
] @text.title

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
  (thematic_break)
] @punctuation.special

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (block_quote_marker)
  (block_continuation)
] @punctuation.special

(pipe_table_header
  "|" @punctuation.delimiter)

(pipe_table_row
  "|" @punctuation.delimiter)

(pipe_table_delimiter_row
  "|" @punctuation.delimiter)

(pipe_table_delimiter_cell
  "-" @punctuation.delimiter)

(fenced_code_block_delimiter) @punctuation.delimiter

(info_string) @label

(code_fence_content) @text.literal

(link_reference_definition) @text.reference

(link_destination) @text.uri

(backslash_escape) @string.escape
