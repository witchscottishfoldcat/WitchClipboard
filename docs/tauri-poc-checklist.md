# Tauri 正式迁移验收清单

更新日期：2026-08-03

## 2026-08-26 复验

- [x] `npm ci --ignore-scripts` 后 TypeScript 类型检查通过
- [x] Rust 自动测试 25 项通过，真实系统剪贴板 E2E 1 项通过
- [x] Electron 回滚自检 100 项通过；自检现在会先重建匹配 Electron ABI 的 SQLite 原生模块
- [x] `npm audit`：0 个已知漏洞
- [x] Windows x64 NSIS 本地构建与隐藏启动 smoke 通过；正式 logo 在 150% DPI 下使用原生 SMALL 24×24 / BIG 48×48 帧，未额外缩小图案；产物为 3,492,472 bytes，SHA-256 `A005F6A86B13DBAE8C3877E643AC614F8D1FD5A9368F508CFD35F5683572BEC1`
- [x] 10 次最终产物性能复验：隐藏态 14.18 MB / 2.66 MB，首次按需唤出 695.8 ms，热唤出 p50/p95 17.2/21.0 ms
- [x] 最近一次远端 Tauri CI 成功，包含 x64 测试及原生 ARM64 构建、安装和启动 smoke
- [ ] 正式签名 Release 工作流尚无运行记录；本机未配置 updater 私钥和 Authenticode 证书，不能以未签名本地产物替代

本次复验发现并修复了干净安装后 Electron 自检缺少原生模块重建的问题，并更新锁文件消除依赖审计漏洞。
签名发布、离线密钥备份以及 Electron 覆盖安装/回退仍属于下方维护者验收项。

## 已完成

- [x] React UI 保持不变，默认 `dev`、`build`、`dist` 已切换到 Tauri
- [x] Electron 构建和 100 项自检保留为显式回滚脚本
- [x] 文字、HTML、图片、`CF_HDROP`、来源进程、敏感格式和自动粘贴均由 Rust 实现
- [x] 主面板与迷你面板按需创建；Release 隐藏 60 秒后以无 WebView 模式快速重启
- [x] 托盘冷却、失焦收起、关闭仅隐藏、单实例、全局热键和 1–9 快速粘贴
- [x] 设置、开机自启、保留策略、标签、关联检索、经设备确认的局域网文字/图片/文件传输
- [x] 局域网文件按 256 KB 分片，提供进度、取消、失败状态、稳定 ID 上传续传和 HTTP Range 下载续传
- [x] WebDAV 历史端到端加密：AES-256-GCM 状态/Blob、SHA-256 校验、ETag 冲突重试和删除 tombstone
- [x] Chromium safeStorage/DPAPI、数据库派生密钥和 `ZTB1` Blob 完全兼容
- [x] SQLite3MultipleCiphers/sqleet 从同一 amalgamation 按目标架构编译
- [x] schema v4 → v6 前创建并验证在线加密备份，迁移使用事务
- [x] 密钥或数据库异常时明确提示且不覆盖旧库
- [x] Tauri API 不再继承 Electron fallback
- [x] Tauri 官方 updater 的检查、下载、进度、安装、跳过版本和静默检查已接入
- [x] GitHub Actions x64/ARM64 发布矩阵与 `latest.json` 流程已配置
- [x] TypeScript 检查通过；Rust 25 项自动测试通过，真实系统剪贴板 E2E 通过；Electron 100 项回归通过
- [x] 主面板与迷你面板背景透明度可持久化调节，默认 90%，不影响文字、图标和内容清晰度
- [x] 正式支持范围收紧为 Windows x64/ARM64，已删除无原生能力支撑的 Tauri macOS 打包入口
- [x] 可重复性能基准与显式授权的系统剪贴板 E2E 脚本已纳入仓库

## 构建与性能证据

- [x] Windows x64 NSIS：3.18 MB（Electron 1.5.0：103.28 MB）
- [x] 隐藏冷启动：1 个进程，14.98 MB Working Set / 2.64 MB Private
- [x] 正式 x64 产物自动化 10 次完整冷启动：p50 993.6 ms / p95 1051.5 ms
- [x] 纯后台首次按需建 WebView：652.5 ms；已有 WebView 的 10 次热唤出：p50 24.6 ms / p95 31.4 ms
- [x] 完整面板可见：7 个进程，411.31 MB Working Set / 313.70 MB Private
- [x] 面板隐藏 65 秒：原进程被无 WebView 后台进程替换，14.65 MB / 2.60 MB，0 个子进程
- [x] 在原生 `windows-11-arm` 干净 runner 上完成 ARM64 测试、NSIS 安装和启动 smoke
- [x] Electron 原有对照：p50 31.8 ms / p95 36.2 ms；Tauri 新自动化口径热态 24.6/31.4 ms，两者不做跨口径胜负结论
- [x] 产品取舍已记录：接受首次唤出回退，换取 96.9% 安装包缩减和约 15 MB 长期后台态

## 发布前必须由维护者完成

- [x] 生成唯一的 Tauri updater 密钥对，并在本机受保护目录保存私钥及 DPAPI 密码备份
- [x] 配置 GitHub `WCC_UPDATER_PUBLIC_KEY` 变量与两个 updater 签名 secrets
- [ ] 把 updater 私钥与密码再复制到真正离线的发布备份介质
- [ ] 购买/取得可信 Windows 代码签名 PFX，并配置 `WINDOWS_CERTIFICATE` 与密码 secret
- [ ] 在已安装 Electron 的快照机执行一次覆盖安装、回退和卸载保留数据测试
- [x] 退出运行中的正式实例后执行 Tauri 文字/HTML、图片、文件列表和自动粘贴的真实系统 E2E

未勾选项涉及离线保管、可信第三方证书或专用快照环境，不能由源码测试替代。
