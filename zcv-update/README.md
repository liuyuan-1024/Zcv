# zcv-update

`zcv-update` 定义 Zcv 自动更新的可信清单、产物校验、压缩包约束和跨进程替换事务。它不持有 GPUI 状态，也不决定何时检查更新。

公共入口是 [`src/update.rs`](src/update.rs)。应用级策略和界面状态由 [`zcv/src/auto_update.rs`](../zcv/src/auto_update.rs) 的 `UpdateManager` 持有。

## 职责边界

```text
GitHub Release
  latest.json + latest.json.sig + Zcv_<version>_darwin_arm64.zip
                         ↓
zcv::UpdateManager：检查、下载、进度与重启时机
                         ↓
zcv-update：签名、哈希、归档、bundle 与事务校验
                         ↓
zcv-update-helper：应用退出后替换、启动确认与回滚
```

- `UpdateManager` 在应用启动时检查一次并自动下载可信更新；TopBar 只观察状态，用户只决定何时重启。
- 临时网络失败不在 TopBar 留下失败状态；永久产物错误通过一次性 Toast 告知用户。
- `zcv-update` 负责纯协议与文件系统操作，不能反向依赖应用 UI。
- helper 只消费应用进程已经验证并写入的事务，不下载更新，也不选择版本。

## 信任模型

验证顺序是安全边界的一部分：

1. 使用应用内置 Ed25519 公钥验证 `latest.json.sig`。
2. 签名通过后才解析清单并选择更高版本与目标平台产物。
3. 下载后核对文件大小和 SHA-256。
4. 检查 ZIP 路径与展开大小，解压完整 `Zcv.app`。
5. 验证 bundle 代码签名 seal 与版本一致性。
6. 写入版本化更新事务，交给独立 helper 完成原子替换。
7. 新版本启动后写入确认；未确认或替换失败时按事务协议回滚。

当前 macOS 包使用零费用 ad-hoc 签名。它提供 bundle 本地完整性校验，但不提供 Developer ID 身份或 Apple 公证。更新来源的身份与防篡改保证来自 Ed25519 清单签名；不要用 Team ID、Gatekeeper 或隐式网络信任替代这条链路。

签名私钥只存在于发布环境，不能提交到仓库、写入文档或打印到日志。源码只保存公钥。

## 发布产物

GitHub Release 必须同时包含：

- `latest.json`
- `latest.json.sig`
- `Zcv_<version>_darwin_arm64.zip`

`.github/workflows/release.yml` 在 `macos-14` 标准 runner 上构建 Apple Silicon 包，调用：

- `scripts/bundle-mac --no-dmg`：构建应用和 helper，组装并 ad-hoc 签名完整 app bundle。
- `scripts/release-update <私钥路径> --no-build`：生成清单、签名并核对私钥对应公钥与源码内置公钥一致。

发布 tag 必须是 `v<版本>`，并与根 `Cargo.toml` 的工作区版本一致。不要手工编辑清单中的大小或 SHA-256。

## 修改约束

- 清单与事务格式变更必须升级对应 schema 版本，并同步应用、helper、生成工具和测试。
- 更新必须替换并验证完整 app bundle，不只覆盖主可执行文件。
- 安装目标、暂存目录、确认文件和事务 ID 必须相互校验，不能接受事务外路径。
- 不增加静默回退、未签名清单或跳过哈希的兼容路径。
- 私钥轮换需要先更新应用内置公钥，再使用对应私钥发布后续版本。

## 验证

```bash
cargo check -p zcv-update --all-targets
cargo test -p zcv-update --all-targets
```

涉及应用策略时，还应运行 `zcv` 中自动更新相关测试；涉及打包或发布脚本时，应在 macOS 环境核对生成的三个 Release 资产和公钥一致性。
