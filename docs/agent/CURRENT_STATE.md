# 当前可用：Windows 本地远程工作台

无需登录 GitHub 即可[下载 Windows x64 便携包](https://github.com/bestkojima/rust-remote-workspace/releases/download/windows-2026.11.20/rust-remote-windows-x64.zip)。解压并双击 `START.cmd`，按 [README 快速配置](../../README.md#快速配置) 连接第一台设备。同设备可开多个独立终端；需要转发时从 **F5 → T** 启动路由。用 **F2** 做单次 SFTP 操作，在双栏目录选好后按 **F7** 创建手动同步 Profile。[操作指南](../design/TUI_DESIGN.md) · [构建与打包](../testing/RELEASE.md)

| 已实现的路径 | 证据 |
|---|---|
| 终端与设备身份 | 三个真实 SSH shell 独立收发、断线重开，支持 ANSI 与远端尺寸变化。[多终端实测](../testing/INTERACTIVE.md)。首次公钥明确确认后才保存，换钥保留旧记录并拦截。[Host Key 实测](../testing/HOST_KEY.md) |
| 设备凭证 | 指定私钥后按 Shift+Tab 填解锁口令，换钥不沿用旧口令。Windows 用当前用户 DPAPI 保护密码与私钥口令；旧表在原账号显式保存才迁移。[录入、迁移与恢复](../testing/CREDENTIALS.md) |
| 隧道 | 家端验证网关，只向指定 VPS 回环服务端口转发。开发机独立核对 SSH 身份。真实 rathole 0.5.0 子进程和两个独立 SSH 服务在本机验证掉线、重连和手动停用。[两段连接](../testing/TUNNEL.md) |
| Files | 本地/远端双栏选址、建目录、重命名、删除确认、双向收发。取消或失败时原目标保留；成功换源会清理来自旧源的选区。[原生 Files 验收](../testing/FILES.md) |
| Sync | 多 Profile、排除、只读预览、默认不覆盖、手动确认、取消、错误历史；使用 rclone 1.72.1。下载时可有条件地替换经过明确批准的本地文件；SFTP 上传保留已有冲突项。[操作](../design/SYNC_USE.md) · [引擎实测](../testing/SYNC_ENGINE.md) |
| 设置 | F5 → E 编辑新设备端口、历史行数、工具路径和下次启动的日志级别。历史和日志有上限；关闭日志的默认配置不会创建诊断日志。原生设置、取消、重启见 `tools/smoke_settings.py`。 |
| 可复用接口 | Core 无 UI 依赖，提供 PTY、SessionId、后台标签集合与代理进程管理。应用库的 `termirs::session`、`termirs::files`、`termirs::sync` 分别提供终端、单文件传输和目录同步，无须创建 TUI。[Core 与适配器](../interfaces/CORE_API.md) |

## 验收范围

- **Windows Release。** 此前在本机跑通双跳 SSH、Files、Sync 和重启登录。当前工作区有 **104 项测试通过**，包括独立的私钥口令表单、单文件传输与取消，以及大批量同步提示。按本轮要求未重复整套交互验收。[版本与测量记录](../testing/RELEASE.md)

- **本轮跳过外网跨 NAT 验收。** 隧道已有 Windows 回环实测；未连接公网 VPS，未验证真实防火墙或跨主机链路。
- **本轮只验收 Windows。** Linux 构建、运行和性能均未实测，按用户决定暂缓，不阻塞 Windows 交付。
- **目录与会话。** SFTP 浏览按目录加载，大单目录仍需容纳其条目。Files 打开时沿用上游先建 shell 再建 SFTP 的路径。隧道会重连，断线 shell 通过手动新建恢复。双向同步、自动扫描、配对服务及 GUI 不在首版。
- **代码基线。** TermiRs `ab1910e4e251bf0b92271203b8f4dc25d8a5d015`。[原版实测](../testing/BASELINE.md) · [本机工作树与文件清单](WORKTREE.md) · [阶段台账](TASKS.md)。
