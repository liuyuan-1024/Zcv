[
  (select)
  (case)
  (insert)
] @indent

(column_definitions
  ")" @end) @indent

(subquery
  ")" @end) @indent

(cte
  ")" @end) @indent
