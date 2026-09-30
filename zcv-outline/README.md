# zcv-outline

`zcv-outline` 提供工作区的大纲面板。

语法数据由 `zcv-editor` 提供，`zcv-outline` 只负责当前活动编辑器的订阅、筛选、树形折叠和导航；通用树行的行高、缩进与引导竖线由 `zcv-ui` 提供。

大纲折叠状态由 `OutlinePanel` 独占，并从当前语法大纲派生可见行。`OutlineItem` 仍属于 `zcv-language` 的不可变语法结果，不携带 UI 状态。

符号行在通用树行的行首列按需放置 `TreeDisclosure`：点击箭头折叠，点击标题导航；目录与文件行显示文件图标并由整行处理操作。行首列的宽度由 `TreeRowFrame` 统一确定。
