# 可复用 Rust 接口：终端、隧道与文件传输

当前 `crates/remote-core` 提供 PTY 输入输出、后台会话集合和开发机代理的进程管理。Core 没有终端 UI 依赖。SSH 身份认证仍复用 TermiRs；绘制与键盘分派属于 TUI。

| 已实现的接口 | 输入 / 输出与作用 |
|---|---|
| `SessionId::allocate() -> SessionId` | 每个终端实例分配一个进程内 ID，可复制、比较与排序。不要用设备名当 ID，也不要跨进程保存它 |
| `ParkedTabs<T>::default()` | 创建空的后台集合。前台由适配器唯一持有 |
| `park(id, tab: T)` | 把前台资源移入后台；调用方保证该 ID 尚未停放 |
| `take_recent() -> Option<T>` | 移出最近停放的终端；空集合返回 None |
| `take_next(after: SessionId) -> Option<T>` | 按创建顺序挑出后继、到末尾回绕，并移出该资源 |
| `remove(id) -> Option<T>` | 仅移出该 ID 的资源；处理重复关闭时可安全忽略 None |
| `iter()` | 按停放顺序借用 (SessionId, &T)，不改变所有权 |

`T` 可以是 TUI 的连接句柄和屏幕，也可以是测试中的字符串；无需实现 UI trait。`src/app/terminal_tabs.rs` 将此集合接入前台模式。SSH 连接错误仍是 `termirs::AppError`，建连结果通过容量 1 的 mpsc 返回。SSH I/O 的取消、超时与生命周期见[事件协议](EVENT_PROTOCOL.md)。

## 开发机隧道进程

`remote_core::tunnel_agent::supervise` 接收 rathole 可执行文件、管理员提供的私有客户端配置、停止信号以及状态回调。`AgentState::Running` 报告已启动的子进程 PID，`Retrying` 给出退出状态与退避时间，`Stopped` 表示已主动停用。网络是否通到开发机由家庭端 SSH 验证；仅凭子进程存活无法得出结论。[运行与停止](../design/TUNNEL_AGENT.md)

## 不创建 TUI 即可打开 SSH 终端

使用保存的设备时，先用 `ConfigManager::apply_host_pin` 取该端点的有效公钥；经 VPS 的设备还须先将属于它的路由启动。用 `termirs::session::SshSession::connect_with_cancel(&device, cols, rows, timeout, &cancel)` 返回独立 SSH 会话。未知指纹或已知键变化都会返回具体的 `AppError`；调用方按[身份流程](../design/TUI_DESIGN.md#首次连接核对哪一串)决定是否重试。

在已有认证连接上，`remote_core::terminal::Terminal::open(channel, cols, rows, directory)` 申请 PTY 和 shell。用 `id()` 区分同设备的多个实例；`write` 发送输入，`resize` 上报尺寸并返回失败原因，`close` 仅关闭该通道。通过 `take_output()` 移出唯一 reader；`Output::read().await` 给出原始字节或 EOF，调用者可用 Tokio 的取消信号打断等待。终端组件不保存无界事件历史。独占 SSH 会话的适配器另负责关闭 transport，借用 SFTP 连接的临时 shell 只关自己的通道。

## 无 UI 的手动目录复制

`termirs::sync::preview_scoped` 接收 rclone 路径、设备、含方向/根目录/排除规则的 `SyncSpec`、远端目录授权、取消令牌和有界进度通道。未知设备先核对 SSH 指纹；选择路径不等于获得服务器目录隔离保证。经管理员确认的 SFTP 隔离根，或一次性的独占窗口，由 `RemoteScope` 与设备、账号、公钥和根路径绑定。

预览返回新增项、冲突项、跳过数及可批准的覆盖项。向 `execute` 交付这一份 `Preview` 和逐项批准的路径集合；任务消费预览，返回复制/跳过数及字节数。失败或取消后须重新预览，不复用旧清单。普通 SFTP 不提供目标版本条件替换；上传保留已有的冲突文件。

## 无 UI 的单文件传输

把本地文件传给已核对身份的设备，只需要两端路径。沿用终端相同的有效 Host Key 和路由；`SftpEndpoint::connect` 打开 SFTP，不申请 shell。设备端路径使用 UTF-8 和 `/`，本地端保留原生 `Path`。

```rust
use std::{path::Path, time::Duration};
use termirs::{Result, files::{copy_file, FileLocation, SftpEndpoint}, session::Connection};
use tokio_util::sync::CancellationToken;

async fn upload(device: &Connection, path: &Path, cancel: &CancellationToken) -> Result<()> {
    let peer = SftpEndpoint::connect(device, Duration::from_secs(10), cancel).await?;
    copy_file(FileLocation::Local(path), peer.file("/work/report.txt"), cancel, |_| {}).await
}
```

下载时交换两端；设备间复制时分别传入两个端点的 `file(path)`。端点可复用；调用方使用已有 Tokio runtime，传输期间无需创建 TUI、事件通道或另一个 runtime。

进度回调接收 `FileProgress { transferred, total }`，单位是字节；总大小未知时为 `None`。回调应立即返回。只有 `copy_file` 返回 `Ok(())` 才表示完整文件已发布。要取消，调用 `cancel.cancel()` 并等待结果：提交前响应取消会返回 `AppError::TransferCancelled`，保留原目标；进入提交后，以实际结果为准。传输先写目标同目录的临时文件；若远端断线导致回退或清理失败，错误信息给出需检查的路径。
