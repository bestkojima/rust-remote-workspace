# 从本次任务找入口

先在 [CURRENT_STATE](CURRENT_STATE.md) 找到上一次跑通的路径，再从 [TASKS](TASKS.md) 选待办、核对依赖。下面每一行是一条独立的阅读路径。

| 要解决的问题 | 先读 | 沿调用路径检查 |
|---|---|---|
| 页面、快捷键、焦点切换 | [五页布局与操作目标](../product/PRD.md)、[事件协议](../interfaces/EVENT_PROTOCOL.md) | `src/main.rs` → `src/app.rs`、`src/key_event/`、`src/ui/` |
| SSH 连接、后台会话与 Tab 隔离 | [架构](../architecture/ARCHITECTURE.md)、[Core API](../interfaces/CORE_API.md) | `src/async_ssh_client.rs`、`src/config/`、`src/terminal/`、`src/ui/terminal.rs` |
| 首次指纹确认、同址换钥拦截 | [本机身份验收](../testing/HOST_KEY.md)、[握手事件](../interfaces/EVENT_PROTOCOL.md) | `src/async_ssh_client.rs` 校验对端 → `src/app/host_key.rs` 核对 → `src/config/manager.rs` 保存 |
| 已存密码、私钥口令、旧版配置 | [凭证迁移与恢复](../testing/CREDENTIALS.md) | `src/config/encryption.rs` 选择平台保护，`src/config/manager.rs` 区分明确保存与自动写入 |
| 双栏路径、SFTP 传输 | [Files 验收](../testing/ACCEPTANCE.md#files) | `src/filesystem/`、`src/ui/file_explorer.rs`、`src/transfer.rs` |
| rathole、家庭端 VPS 转发与重连 | [开发机代理](../design/TUNNEL_AGENT.md)、[两段 SSH 验收](../testing/TUNNEL.md) | `crates/remote-core/src/tunnel_agent.rs` → `src/tunnel_home.rs` → `src/app/tunnels.rs` |
| 增量复制、目标文件冲突 | [Sync 规则](../design/SYNC_DESIGN.md)、[Sync 验收](../testing/SYNC_ENGINE.md) | `src/app/sync.rs` 收集授权 → `src/sync_engine/` 冻结预览、用 rclone 传输并逐文件提交 |
| 面向未来 GUI 的接口 | [Core API](../interfaces/CORE_API.md)、[事件协议](../interfaces/EVENT_PROTOCOL.md)、[GUI Adapter](../interfaces/GUI_ADAPTER.md) | 确认接口的实现状态，再找调用方；草案不能当成 Rust 符号引用 |
| Bug 或回归 | 当前状态里的复现条件、[验收路径](../testing/ACCEPTANCE.md) | 在相关模块找已有用例，固定触发输入；区分上游已有行为和本轮改动 |
| 阶段顺序与容量 | [路线图](../plan/ROADMAP.md)、[工期估算](../plan/GANTT.md) | 以任务台账中的验收证据更新进度 |

开发切片用 `$feature-development`；按范围审查候选改动用 `$project-review`。
