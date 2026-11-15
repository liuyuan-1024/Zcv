([
  (entry "{" @open "}" @close)
  (string "{" @open "}" @close)
  (preamble "{" @open "}" @close)
  (entry "(" @open ")" @close)
  (string "(" @open ")" @close)
  (preamble "(" @open ")" @close)
] (#set! newline.only))

(token "\"" @open "\"" @close)

(token ["{" @open "}" @close]) @scope
(command ["{" @open "}" @close]) @scope
