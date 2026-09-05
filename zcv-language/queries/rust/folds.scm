; 折叠查询：@fold 捕获的节点即一个可折叠范围。
;
; 折叠范围从块起点（如 `{` 所在行）延伸到块终点，起点行在折叠后保留，折叠箭头（crease）显示在该行。
; 因此这里捕获的是 item 的块体而不是整个 item，这样 `fn foo() {` 签名行折叠后仍保留。

; 具名 item 的块体
(function_item
  body: (block) @fold)

(impl_item
  body: (declaration_list) @fold)

(mod_item
  body: (declaration_list) @fold)

(foreign_mod_item
  body: (declaration_list) @fold)

(trait_item
  body: (declaration_list) @fold)

(struct_item
  body: (field_declaration_list) @fold)

(struct_item
  body: (ordered_field_declaration_list) @fold)

(union_item
  body: (field_declaration_list) @fold)

(enum_item
  body: (enum_variant_list) @fold)

; 表达式块
(if_expression
  consequence: (block) @fold)

(loop_expression
  body: (block) @fold)

(while_expression
  body: (block) @fold)

(for_expression
  body: (block) @fold)

(match_expression
  body: (match_block) @fold)

; 宏定义、宏调用与 use 声明
;
; use 按单个声明独立折叠：连续 use 不合并，避免单行 use 被卷进折叠组；
; 单行 use 与单行宏调用由语言层按单行过滤丢弃。
[
  (macro_definition
    "}" @fold.end)
  (macro_definition
    "]" @fold.end)
  (macro_definition
    ")" @fold.end)
] @fold

[
  (macro_invocation
    (token_tree "}" @fold.end))
  (macro_invocation
    (token_tree "]" @fold.end))
  (macro_invocation
    (token_tree ")" @fold.end))
] @fold

[
  (use_declaration
    argument: (use_list "}" @fold.end))
  (use_declaration
    argument: (scoped_use_list
      list: (use_list "}" @fold.end)))
] @fold

; 注释块：连续单行注释折叠为一个组
(block_comment) @fold

(line_comment)+ @fold
