# 任务台账

多终端、两段 SSH 隧道、Files 和手动 Sync 已通过 Windows 本机真实进程验收。[Release 实测](../testing/RELEASE.md)记录了运行入口、测试负载和峰值。本轮以 Windows 本机为交付范围；外网跨 NAT 验收按用户决定跳过。

| ID | 工作及影响文件 | 依赖 | 优先级 | 状态 | 验收条件 | 验证证据 |
|---|---|---|---|---|---|---|
| B0 | 固定并运行 TermiRs；LICENSE、Cargo.lock、docs/testing/BASELINE.md | 无 | P0 | 完成 | 原版编译、PTY 启停、隔离的上游测试；标明平台 | [原版记录](../testing/BASELINE.md) |
| H0 | 工程记忆和两项 Skill；AGENTS.md、.agents/skills、docs/agent、docs/plan | B0 | P0 | 完成 | 按任务可定位接口、测试、剩余项；两个 Skill 可解析 | [导航](FOR_AGENT_READ.md)；两个 SKILL.md 通过 quick_validate |
| N1 | 五页工作台与 CLI；src/workspace.rs、app.rs、main.rs、bin/ | B0 | P0 | 完成 | 五页可键盘切换；可回到原表单和 SFTP；空结果不连错设备 | [本轮检查](CURRENT_STATE.md)，workspace 的键控与窄屏渲染测试 |
| C1 | 抽取标签身份和停放集合；crates/remote-core、docs/interfaces | B0 | P1 | 完成 | 无 UI 依赖；App 真正使用，旧事件按实例识别 | [Core API](../interfaces/CORE_API.md)，Core 单测 |
| T2 | SSH 多终端；src/app/terminal_tabs.rs、async_ssh_client.rs、events.rs、key_event、ui/terminal.rs | N1、C1 | P1 | 完成（Windows 回环） | 同一设备 3 个独立 shell；B 断开再连，A/C 保留原会话并继续收发 | [原生控制台：三路收发、断线后新 shell、ANSI 颜色、远端 PTY 尺寸](../testing/INTERACTIVE.md) |
| K2 | 首次 Host Key 与凭证边界；src/async_ssh_client.rs、config/、app/host_key.rs | B0 | P0 | 完成（Windows 回环） | 未知指纹明确接受或拒绝；换钥拒绝且旧 pin 保留；保存失败可见；取消不留 pin | [真实进程、配置重启、原地址换钥、失败注入](../testing/HOST_KEY.md) |
| K2S | 设备密码与私钥口令的录入及安全保存；src/config/、设备表单 | K2 | P0 | Windows 完成；Unix 未验 | 修改密钥路径不沿用旧口令，不把私钥口令当登录密码；旧表可迁移，保存失败不丢原记录 | [凭证表单回归、DPAPI 独立解锁、两次 SSH 密码登录及锁冲突](../testing/CREDENTIALS.md) |
| A2 | 无 UI 的 SSH/PTY 服务；remote-core::terminal、termirs::session | C1、T2 | P1 | Windows 本机完成 | 共享连接打开、独立 reader、输入、resize、关闭；保持多终端隔离 | [已实现 API](../interfaces/CORE_API.md)；headless_terminal_uses_the_same_core_pty_as_the_workspace |
| R3 | rathole 生命周期、SSH 本地转发；Core 进程管理、Devices / Settings | K2、T2、本机回环测试设备 | P0 | 本机完成；外网实测跳过 | 经受限转发到达设备，独立验证两段身份；掉线重连，手动停止后回收进程 | [端到端：建路由、两级 Host Key、断线恢复、停用](../testing/TUNNEL.md)；[配置权限与子进程清理](../design/TUNNEL_AGENT.md) |
| F4 | 双栏 Files 与无 UI 的单文件传输；src/filesystem/、termirs::files | N1、R3 | P2 | Windows 本机完成 | 导航、建目录、重命名、删除确认；双栏和直接调用共用可取消的传输，失败保留原件 | [Files 实测](../testing/FILES.md)；[单文件接口](../interfaces/CORE_API.md)；两段 SSH + rathole 的 --files 路径 |
| S5 | 多目录对、rclone 单向复制、逐项确认和历史 | F4、K2 | P3 | Windows 本机完成 | 首次、增量、目标独有项、版本变化、取消与重试、无效路径 | [操作与原生验收](../design/SYNC_USE.md)；[执行器](../testing/SYNC_ENGINE.md) |
| Q6 | Windows 本机集成、性能、发布构建与便携包；tools/package-windows.ps1、docs/testing | R3 本机链路、F4、S5 | P1 | Windows 便携包已交付；Linux 暂缓 | Release 实际运行；记录启动、空闲、三终端、扫描/传输峰值；解压后能启动 | [Windows Release 与便携包](../testing/RELEASE.md) |
| G6 | GitHub 源码仓库、Windows 下载与 README 快速配置 | Q6 | P1 | 私有仓库交付 | 登录有权限的账号，从 README 可下载便携包并配置第一台设备 | [源代码](https://github.com/bestkojima/rust-remote-workspace) · [Windows Release](https://github.com/bestkojima/rust-remote-workspace/releases/tag/windows-2026.11.20) |
| P7 | 可选限时配对；后续 ADR、设备配置 | R3 安全验收 | P4 | 延后 | 先评审是否值得开发控制服务；不得复用 rathole token 作短码 | [产品约束](../product/PRD.md) |
