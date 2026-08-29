# Tauri 发布与 Electron 迁移手册

## 本地验证

```powershell
npm ci --ignore-scripts
npm run typecheck
npm run selftest
npm run selftest:electron
npm run dist:win
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\benchmark-tauri.ps1 -Iterations 10
```

`selftest:electron` 会先按当前 Electron ABI 重建 SQLite 原生模块，因此上述流程在
`npm ci --ignore-scripts` 的干净安装后也可以直接执行。

所有运行时试验必须设置独立目录：

```powershell
$env:WCC_TAURI_DATA_DIR = "$PWD\out\tauri-release-test"
src-tauri\target\release\witch-clipboard.exe --hidden
```

退出所有 Electron/Tauri 实例后，运行会改变当前系统剪贴板的显式 E2E：

```powershell
npm run selftest:system-clipboard
```

该测试覆盖 HTML、图片和 `CF_HDROP` 文件列表从系统剪贴板进入加密历史、再写回系统剪贴板的完整 Rust 链路。

## updater 密钥（暂不启用）

当前发布流程暂不生成 updater 签名文件，也不要求 GitHub 配置 updater 密钥。
推送 `app-v<version>` tag 后，工作流直接构建未签名的 x64/ARM64 NSIS 和便携版 ZIP。

## Windows Authenticode 证书（暂不启用）

当前公开发布暂不配置 Windows Authenticode 证书，因此 EXE/NSIS 安装包可能触发 SmartScreen
提示，也不会生成 updater 签名文件。
后续如果配置 PFX 或 Azure Artifact Signing，再把 Authenticode 签名步骤加入发布工作流。

普通 `main` 推送会在全新的 `windows-11-arm` 原生 ARM64 runner 上运行 Rust 测试、构建未签名
ARM64 NSIS，并静默安装后启动 8 秒做 smoke test；产物只作为 CI artifact，不得公开发行。

x64/ARM64 矩阵故意设为 `max-parallel: 1`。`tauri-action` 对 `latest.json` 采用读取、合并、删除、
重传流程；两个架构并行会存在丢失其中一个 `windows-<arch>` 条目的竞态。发布草稿转正前必须检查
`latest.json` 同时包含 `windows-x86_64` 与 `windows-aarch64`。

## 从 Electron 覆盖安装

1. 备份 `%APPDATA%\WitchCat-Clipboard`，确认 Electron 已完全退出。
2. 运行 Tauri NSIS；应用标识与数据目录保持不变，不建立第二份历史。
3. 首次启动会读取原 `master.key`、`Local State`、sqleet 数据库和加密 Blob。
4. v4 数据库会先在线备份到 `migration-backups`，验证成功后才事务升级为 v6。
5. 验证历史数量、图片、标签、搜索、粘贴和快捷键。
6. 若启动报告密钥/数据库错误，停止操作并重新安装 Electron 回滚；Tauri 不会重建或覆盖原库。

## 回滚

schema v5 新增可空 `html` 列，v6 新增独立的 `sync_tombstones` 表；原条目表字段保持兼容。
回滚时退出 Tauri并安装保留的 Electron 安装包；不要删除用户数据。若需要恢复迁移前快照，先关闭两种客户端，
再由维护者从 `migration-backups` 恢复，禁止在应用运行时替换数据库文件。
