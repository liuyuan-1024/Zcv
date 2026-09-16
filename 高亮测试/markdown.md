# Markdown 高亮测试

用于检查 Markdown 结构、行内语法和围栏代码块的语法高亮。正文不包含 HTML。

一级标题
========

## 二级标题

### 三级标题

#### 四级标题

##### 五级标题

###### 六级标题

---

## 行内语法

普通文本，*斜体文本*，_另一段斜体_，**粗体文本**，__另一段粗体__，以及 ***粗斜体***。
行内代码：`let value = 42; # 赋值`；删除线：~~已废弃的描述~~；转义字符：\*不会变成斜体\*、\[不会成为链接\]。

自动链接：<https://github.com/liuyuan-1024/zcv.git>。
[Zcv仓库-内联链接](https://github.com/liuyuan-1024/zcv.git "Zcv 仓库")、[Zcv仓库-引用链接][Zcv仓库]、[Zcv仓库]。

本地文件链接：[Rust 示例](./main.rs)、[JSON 示例](./data.json)、[图片文件](./测试图片.jpeg)。

![图片替代文本](./测试图片.jpeg "测试图片")

单回车直接换行显示；空一行后才开始新的段落。
本地预览不使用行尾两个空格、反斜杠或 HTML 标签制造额外换行。

[Zcv仓库]: https://github.com/liuyuan-1024/zcv.git

## 引用与列表

> 一级引用包含 *斜体*、**粗体**、`代码` 和 [链接](https://zcv.dev)。
>
> > 二级引用。

- 无序列表
  - 嵌套列表
    - 更深一级
- [ ] 未完成任务
- [x] 已完成任务

1. 有序列表
2. 第二项
   1. 嵌套有序项
   2. 另一项

## 表格

| 左对齐 | 居中 | 右对齐 |
| :----- | :--: | -----: |
| `code` | **粗体** | 128 |
| [链接](https://zcv.dev) | *斜体* | ~~删除~~ |

## 数学公式

行内公式应和普通文本一起显示：$f(x) = x^2 + 2x + 1$，集合 $A = \{1, 2, 3\}$，积分 $\int_0^1 x^2\,dx = \frac{1}{3}$
独立公式应居中显示：
$$
\int_0^1 x^2\,dx = \frac{1}{3}
$$

复杂一点的公式：

$$
\sum_{k=1}^{n} k = \frac{n(n+1)}{2}, \qquad
e^{i\pi} + 1 = 0
$$

## 围栏代码块

下面的语言只测试围栏代码块中的语法高亮；普通 Markdown 正文不按语言代码高亮。

### Rust

```rust
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct User<'a> {
    name: &'a str,
    active: bool,
}

fn greeting(user: &User<'_>) -> String {
    format!("hello, {}", user.name)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let user = User { name: "Zcv", active: true };
    let mut counts = HashMap::new();
    counts.insert(user.name, 1usize);

    if user.active && counts["Zcv"] > 0 {
        println!("{}", greeting(&user));
    }
    Ok(())
}
```

### Python

```python
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Config:
    root: Path
    retries: int = 3


def load_config(path: str) -> Config:
    if not path.endswith(".toml"):
        raise ValueError(f"unsupported config: {path!r}")
    return Config(root=Path(path).parent)


match load_config("settings.toml"):
    case Config(root=root, retries=retries) if retries > 0:
        print(root, retries)
```

### JavaScript

```javascript
const endpoint = new URL("https://api.example.com/items");

async function fetchItems(limit = 20) {
  const response = await fetch(`${endpoint}?limit=${limit}`);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

fetchItems().then(items => console.log(items?.length ?? 0));
```

### TypeScript

```typescript
type Result<T> = { ok: true; value: T } | { ok: false; error: Error };

interface EditorOptions {
  readonly tabSize: number;
  language?: "rust" | "typescript";
}

function success<T>(value: T): Result<T> {
  return { ok: true, value };
}

const options: EditorOptions = { tabSize: 4, language: "rust" };
console.log(success(options));
```

### JSON

```json
{
  "name": "Zcv",
  "version": "0.8.53",
  "enabled": true,
  "limits": [1, 2, 3],
  "metadata": null
}
```

### TOML

```toml
title = "Zcv 设置"
enabled = true
timeout_ms = 1_500

[editor]
font_size = 14
font_family = "Iosevka"

[[languages]]
name = "Rust"
extensions = ["rs"]
```

### YAML

```yaml
name: zcv
enabled: true
ports:
  - 3000
  - 8080
editor:
  theme: dark
  command: "cargo test -p zcv-editor"
```

### Bash

```bash
#!/usr/bin/env bash
set -euo pipefail

project_root="${1:-.}"
for crate in zcv-editor zcv-language; do
  cargo test -p "$crate" --manifest-path "$project_root/Cargo.toml"
done
```

### SQL

```sql
SELECT u.id, u.name, COUNT(p.id) AS project_count
FROM users AS u
LEFT JOIN projects AS p ON p.owner_id = u.id
WHERE u.active = TRUE
GROUP BY u.id, u.name
HAVING COUNT(p.id) > 0
ORDER BY project_count DESC;
```

### CSS

```css
:root {
  --accent: hsl(204 100% 50%);
}

.preview-card:hover {
  color: var(--accent);
  border: 1px solid color-mix(in srgb, var(--accent), white 30%);
}
```

### HTML

```html
<!doctype html>
<main class="preview-card" data-state="ready">
  <h1>Zcv</h1>
  <p>Only highlighted inside this fenced code block.</p>
</main>
```

### C

```c
#include <stdio.h>

int main(void) {
    const char *name = "Zcv";
    printf("%s\\n", name);
    return 0;
}
```

### C++

```cpp
#include <iostream>
#include <string_view>

int main() {
    constexpr std::string_view name = "Zcv";
    std::cout << name << '\\n';
}
```

### Go

```go
package main

import "fmt"

func main() {
	message := "Zcv"
	fmt.Printf("%s\\n", message)
}
```

### Java

```java
import java.util.List;

record Project(String name, boolean open) {}

class Main {
    public static void main(String[] args) {
        var projects = List.of(new Project("Zcv", true));
        projects.stream().filter(Project::open).forEach(System.out::println);
    }
}
```

### Kotlin

```kotlin
data class Project(val name: String, val open: Boolean)

fun main() {
    val project = Project("Zcv", open = true)
    println(project.takeIf { it.open }?.name ?: "closed")
}
```

### Ruby

```ruby
class Project
  attr_reader :name

  def initialize(name)
    @name = name
  end
end

puts Project.new("Zcv").name
```

### Swift

```swift
struct Project: Codable {
    let name: String
    var isOpen: Bool
}

let project = Project(name: "Zcv", isOpen: true)
print(project.isOpen ? project.name : "closed")
```

### Lua

```lua
local project = { name = "Zcv", open = true }

if project.open then
  print(string.format("%s is ready", project.name))
end
```
