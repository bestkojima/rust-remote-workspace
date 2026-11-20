# GUI 如何复用同一个 Core

直接使用 [Core PTY 与 SSH 适配器](CORE_API.md#不创建-tui-即可打开-ssh-终端)，不需要创建 Ratatui App。为每个实例保留独立的输出 reader，按 SessionId 处理 EOF 和关闭；GUI 自行决定视口和焦点。

| 谁负责 | 本轮位置 | 接入 GUI 前的动作 |
|---|---|---|
| 连接认证、PTY 与通道关闭 | `termirs::session` + `remote_core::terminal` | 直接使用相同的连接与输入输出；将 UnknownHostKey 和 HostKeyChanged 分别展示 |
| 终端解析 / 渲染 | `src/ui/terminal.rs` 将 WezTerm 屏幕与 Ratatui 绘制放在一起 | 暴露视口快照/增量；GUI 绘制字符单元。不要把 ANSI 流当普通文本 |
| 键盘和焦点 | `src/workspace.rs`、`src/key_event/` | GUI 按钮转成同一组打开、输入、resize、关闭操作；焦点由各自 UI 管理 |
| 手动目录同步 | `termirs::sync` | 直接调用 `preview_scoped` 和 `execute`；两者共用已核实的设备身份。界面显示目录授权与冻结清单，逐项收集批准，用有界通道接进度 |
| 单文件传输 | [`termirs::files`](CORE_API.md#无-ui-的单文件传输) | 复用 `SftpEndpoint`，将源和目标传给 `copy_file`。用 `FileProgress` 更新进度，取消按钮触发令牌；等调用返回再显示已完成、已取消或错误。Files 双栏也走这个执行器 |

GUI 适配完成的判据：同一个 Core 用例能在无 UI 的测试里开关三个会话，并由两个适配器订阅同一模型；GUI 不调用 TUI 回调、不依赖 TermiRs 的 App。本轮不编写 egui 工程、HTTP 服务或前端桥。
