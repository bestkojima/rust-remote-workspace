# 事件协议：断线先找 SessionId

## 正在使用的进程内消息

`src/events.rs` 的 `termirs::AppEvent` 是 TUI 输入队列，不是可序列化的公共 GUI 协议。每个 CLI 进程一个 Tokio runtime，一个接收循环。Crossterm 键盘、鼠标、粘贴、窗口变化走 `Input(Event)`。

- **建连与身份**：用户选定设备后生成 `CancellationToken` 和容量 1 的结果通道。Connecting 页面等待结果，Esc 可取消；普通终端连接使用 10 秒默认超时。russh 在用户认证前检查服务端公钥。`UnknownHostKey { public_key, fingerprint }` 结束此次握手，TUI 停止连接计时，等待核对；只有明确接受且保存成功才可用已固定的 key 重新握手。`HostKeyChanged { endpoint, expected, received }` 是终止错误，没有覆盖操作。普通会话认证成功后创建 SSH PTY、独立的 WezTerm 状态，并启动 reader。
- **输出**：reader 先把字节写入自己的屏幕，再发送 `TerminalUpdate(SessionId)`。消息仅提示刷新，不能重放原始字节。主队列容量 100；发送端可等待容量。前台合并连续刷新，后台继续接收数据。
- **断开**：网络 reader 正常结束、SSH 关闭或传输中止时发 `Disconnect(SessionId)`。处理方按实例清理资源。切换页面不会取消 reader。显式关闭会取消 reader；迟到的旧事件可忽略。再次手动连接分配新 ID，不回放按键。
- **VPS 隧道**：`HomeTunnel` 的有界队列报告正在连接、转发就绪、服务端口不可达、重试、身份或配置被阻止、已停止。TUI 用设备 ID 和启动代际路由事件；旧隧道的停用消息不能关掉刚重启的路由。后台出现未信任的公钥时保留到该路由上复核，不打断另一个正在使用的终端。
- **文件传输**：`termirs::files::copy_file` 向本次调用的回调报告字节进度，返回值表明文件是否已发布；取消后也应等待该返回值。双栏页面将进度映射为自己的 `SftpProgress(ScpResult)`。无界面调用方不订阅 TUI 的事件队列。
- **Files 目录操作**：`FileActionDone` 唤醒界面，由对应的一次性结果通道取回路径选择、建目录或重命名的结果。
- **目录同步**：同一时间一个手动任务。后台通过有界队列更新进度，由 watch 保留最新快照；`SyncUpdate` 唤醒页面，终态经独立 oneshot 返回。取消令牌属于这次任务。
- **刷新与退出**：`Tick` 驱动建连/传输或选区滚动，空闲时停用定时重绘。TUI 退出后停止输入轮询，释放会话资源。

## 将来给 GUI 的契约（草案）

Core 应提供一致的会话/任务快照及有界订阅，先取快照建视图，再按实例与代际处理增量；订阅落后时重新取快照。进度更新可以合并，任务终态必须可从快照恢复。GUI 提交取消后应等待终态，再移除任务；网络断线、超时、主动取消必须能区分。VPS 转发已有限速重连；交互式 shell 断线后要手动重开。目前没有跨进程事件协议。
