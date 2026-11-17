[
  (line_comment)
  (block_comment)
] @comment

(command_name) @function
(_ command: _ @function)

[
  (title_declaration text: (curly_group) @text.title)
  (part text: (curly_group) @text.title)
  (chapter text: (curly_group) @text.title)
  (section text: (curly_group) @text.title)
  (subsection text: (curly_group) @text.title)
  (subsubsection text: (curly_group) @text.title)
  (paragraph text: (curly_group) @text.title)
  (subparagraph text: (curly_group) @text.title)
]

(begin name: (curly_group_text) @type)
(end name: (curly_group_text) @type)

(citation keys: (curly_group_text_list text: (text) @text.reference))
(label_definition name: (curly_group_label label: (label) @label))
(label_reference names: (curly_group_label_list label: (label) @text.reference))
(label_reference_range
  from: (curly_group_label label: (label) @text.reference)
  to: (curly_group_label label: (label) @text.reference))

(curly_group_path path: (path) @string)
(curly_group_path_list path: (path) @string)
(curly_group_glob_pattern pattern: (glob_pattern) @string)
(key_value_pair key: (text) @property)

(_ counter: (curly_group_word word: (word) @variable.parameter))
(_ supercounter: (curly_group_word word: (word) @variable.parameter))
(counter_declaration supercounter: (brack_group_word word: (word) @variable.parameter))
(curly_group_value value: (value_literal) @number)

((generic_command
   command: (command_name) @_command
   . arg: (curly_group . (text . (word) @variable.parameter .) .))
 (#eq? @_command "\\chinese"))

((generic_command
   command: (command_name) @_command
   . arg: (curly_group . (generic_command command: (command_name) @variable.parameter) .)
   . arg: (curly_group . (text . (word) @number .) .))
 (#any-of? @_command "\\setlength" "\\addtolength")
 (#match? @number "^[0-9]+(\\.[0-9]+)?(pt|pc|in|bp|cm|mm|dd|cc|sp|em|ex)$"))

((generic_command
   command: (command_name) @_command
   . arg: [
     (curly_group (text . (word) @property .) . "=")
     (curly_group "=" . (curly_group (text . (word) @property .) . "="))
   ])
 (#eq? @_command "\\ctexset"))

((generic_command
   command: (command_name) @_command
   . arg: (curly_group "=" .
     (curly_group "=" . (text . (word) @number .) . ["," "}"])))
 (#eq? @_command "\\ctexset")
 (#match? @number "^[0-9]+(\\.[0-9]+)?(pt|pc|in|bp|cm|mm|dd|cc|sp|em|ex)$"))

(hyperlink uri: (curly_group_uri uri: (uri) @text.uri))
(hyperlink command: "\\href" label: (curly_group) @markup.link.text)

(new_command_definition
  declaration: [
    (command_name) @function.definition
    (curly_group_command_name command: (command_name) @function.definition)
  ])
(environment_definition name: (curly_group_text text: (text) @type))

[
  (verbatim_environment verbatim: (comment) @text.literal)
  (listing_environment code: (source_code) @text.literal)
  (minted_environment code: (source_code) @text.literal)
]

[
  (inline_formula)
  (displayed_equation)
  (math_environment)
] @string
