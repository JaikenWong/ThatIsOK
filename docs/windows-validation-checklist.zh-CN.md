# Windows 验证清单

在真实 Windows 机器上运行，发布前必须通过。

## 范围

本清单验证：

- 打包应用可安装并启动
- 浮动岛正常工作
- 托盘正常工作
- 快捷键正常工作
- Hook 注入指向打包后的 `AgentIsOK.exe`
- Hook 运行时正确返回审批决策
- 同步正常或优雅降级
- 无残留 Electron 或 `node` 依赖

## 测试环境

测试前记录：

- Windows 版本
- 架构：`x64` / `arm64`
- 构建所用终端：`PowerShell` / `cmd`
- 已有哪些 agent 凭证：Claude Code / Codex / Cursor / Antigravity / MiniMax / DeepSeek / OpenCode / Kiro

## 构建

从干净检出：

```bash
npm install
npm run tauri:build
```

预期结果：

- 命令退出码 `0`
- NSIS 安装包存在于：
  - `src-tauri/target/release/bundle/nsis/`
- 安装包文件存在且为最新

阻断发布：

- 构建需要手动修补
- 安装包输出缺失

## 安装

1. 使用默认用户级安装运行 NSIS 安装包。
2. 确认开始菜单存在 `AgentIsOK`。
3. 确认 Windows 应用列表中存在卸载入口。

预期结果：

- 安装无需 `node`
- 安装过程无控制台错误

阻断发布：

- 安装包引用缺失运行时
- 安装路径指向开发机或仓库路径

## 启动

1. 从开始菜单启动。
2. 等待浮动岛出现。

预期结果：

- 浮动岛出现在主显示器顶部居中附近
- 窗口透明且始终置顶
- 应用默认不打开调试控制台

阻断发布：

- 窗口不出现
- 窗口失去置顶行为
- 首次启动崩溃

## 托盘

1. 确认托盘图标出现。
2. 左键点击托盘图标。
3. 右键 → 打开托盘菜单。

预期结果：

- 左键点击展开并聚焦浮动岛
- 托盘菜单包含：
  - `Open Home`
  - `Running Agents`
  - `Usage & Providers`
  - `Approval Rules`
  - `---`
  - `Sync Now`
  - `---`
  - `Install Hooks`
  - `Remove Hooks`
  - `---`
  - `AgentIsOK v{version}`
  - `Quit`
- `Quit` 完全退出应用

阻断发布：

- 托盘图标缺失
- 菜单项无响应
- 退出后仍有后台进程

## 窗口行为

1. 拖动折叠状态的 pill。
2. 反复展开和折叠。
3. 尽可能触发警告状态。

预期结果：

- 拖动流畅
- 无视觉重叠
- 展开高度适配内容
- 警告条不破坏布局

阻断发布：

- 拖动严重卡顿
- 内容重叠
- 布局跳动或裁剪严重

## 快捷键

验证：

- `Ctrl + Shift + Space` 切换浮动岛（展开并聚焦）
- 有待审批时：
  - `Ctrl + Alt + A` 批准
  - `Ctrl + Alt + L` 永久批准（创建规则）
  - `Ctrl + Alt + D` 拒绝

预期结果：

- 快捷键立即生效
- 快捷键注册失败时浮动岛显示警告，而非静默失败

阻断发布：

- 快捷键静默失败
- 触发错误操作
- 快捷键绑定失败时无警告

## Hook 注入

启动打包应用一次，然后检查：

- `%USERPROFILE%\.codex\hooks.json`
- `%USERPROFILE%\.claude\settings.json`
- `%USERPROFILE%\.gemini\config\hooks.json`

预期结果：

- 托管条目存在
- 命令指向打包后的 `AgentIsOK.exe`
- 命令包含：
  - `--hook-source`
  - `--hook-event`
- 命令不包含：
  - `node`
  - 仓库本地 `.js` hook 路径

PowerShell 检查：

```powershell
Select-String -Path "$env:USERPROFILE\.codex\hooks.json" -Pattern "AgentIsOK.exe","--hook-source","--hook-event"
Select-String -Path "$env:USERPROFILE\.claude\settings.json" -Pattern "AgentIsOK.exe","--hook-source","--hook-event"
Select-String -Path "$env:USERPROFILE\.gemini\config\hooks.json" -Pattern "AgentIsOK.exe","--hook-source","--hook-event"
Select-String -Path "$env:USERPROFILE\.codex\hooks.json" -Pattern "node|hook-bridge\.js"
Select-String -Path "$env:USERPROFILE\.claude\settings.json" -Pattern "node|hook-bridge\.js"
```

阻断发布：

- hook 命令指向开发路径
- hook 命令依赖 `node`
- 托管 hook 条目缺失

## Hook 运行时

1. 触发 Codex 权限请求。
2. 触发 Claude 权限请求。
3. 分别使用 批准 / 永久批准 / 拒绝 各一次。

预期结果：

- 浮动岛自动展开
- 显示正确的来源标识
- 决策返回给发起工具
- `永久批准` 持久化审批规则

持久化审批证据：

- `永久批准` 后 `%APPDATA%\AgentIsOK\approval-rules.json` 存在

阻断发布：

- 审批未返回给调用方
- 显示错误的来源/工具
- 持久化规则未保存

## Hook 服务器

应用运行时：

```powershell
netstat -ano | findstr 45873
```

预期结果：

- `127.0.0.1:45873` 上有本地监听

阻断发布：

- 端口从未绑定
- 绑定冲突时无应用内警告

## 同步

1. 点击 `Sync`。
2. 等待数据刷新。
3. 分别测试有效凭证和缺失凭证。

预期结果：

- UI 保持响应
- 有效 provider 更新
- 缺失 provider 优雅降级
- 一个或多个 provider 失败时浮动岛显示紧凑警告

有凭证时需验证的 provider：

- Claude Code
- Codex
- Cursor
- MiniMax
- Antigravity (Gemini)
- DeepSeek
- OpenCode
- Kiro

阻断发布：

- 同步期间应用卡死
- 单个 provider 失败导致整个刷新崩溃
- 同步失败静默无提示

## 设置

1. 展开浮动岛。
2. 用 `-` / `+` 修改 `Sync cadence`。
3. 关闭并重新打开应用。

预期结果：

- UI 中值立即变化
- 下次定时同步立即使用新间隔
- 重启后值持久化

配置证据：

- `%APPDATA%\AgentIsOK\defaults.json` 包含 `syncIntervalMinutes`

阻断发布：

- 控件仅改变 UI 不改变运行时
- 重启后值重置

## 无旧运行时残留

安装产物和 hook 配置不得引用：

- `electron`
- `electron-builder`
- `hook-bridge.js`
- 仓库本地源码路径

PowerShell 抽查：

```powershell
Get-ChildItem "$env:LOCALAPPDATA","$env:APPDATA" -Recurse -ErrorAction SilentlyContinue |
  Select-String -Pattern "hook-bridge\.js|electron" -ErrorAction SilentlyContinue
```

阻断发布：

- 安装运行时仍依赖 Electron 残留

## 发布记录归档

随发布说明或 QA 工单保存：

- 安装包文件名
- 空闲浮动岛截图
- 待审批状态截图
- 警告条截图
- `hooks.json` 托管命令副本
- `%APPDATA%\AgentIsOK\defaults.json` 副本
- 构建日期和 git commit

## 发布门槛

以下任一条件为真则不得发布：

- 应用依赖 `node`
- hook 命令指向开发路径
- 托盘操作无响应
- 快捷键静默失败
- hook 绑定静默失败
- 审批未返回给工具
- 同步导致应用崩溃
- 同步失败无提示
- 设置值未持久化
- 打包 Windows 产物缺失
