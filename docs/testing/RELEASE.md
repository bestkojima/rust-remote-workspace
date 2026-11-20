# Windows 发布构建与本地验收

Windows x64 便携包：`target/dist/rust-remote-windows-x64.zip`。全部解压后双击 `START.cmd`，程序会在本进程的 PATH 中优先查找随包的 rclone 1.72.1 和 rathole 0.5.0。设备表中已明确配置的工具路径仍然有效。操作说明见包内 `README.txt`。

当前包生成于 2026-11-20 08:53，ZIP 为 30,388,119 字节；包内 `rust-remote.exe` 为 6,935,552 字节。

从源码构建，在项目根目录运行：

```powershell
cargo build --workspace --release --locked
.\target\release\rust-remote.exe
```

同一工作区还会生成兼容入口 `termirs.exe`。直连 SSH 和双栏 SFTP 不要求安装额外程序。跨网隧道由 VPS 上的 rathole 服务端与开发机代理管理的 rathole 客户端构成；按目录对复制时，工作台调用本地 rclone 1.72.1。

本机在 `target/local-tools/` 留有本次验收用的两个可执行文件，不改系统 PATH。启用 Sync 时，在 **F5 → E** 的 rclone 字段填入 `E:\rust_remote_terminal\target\local-tools\rclone-1.72.1\rclone.exe`。开发机代理的 `--rathole` 参数可指定 `target/local-tools/rathole-0.5.0/rathole.exe`。这一目录和 Cargo 的构建产物一样，执行 `cargo clean` 会删除。

Release 和上述两个工具就绪后，运行 `.\tools\package-windows.ps1` 可离线重打本地交付包。脚本只收录程序、启动入口、说明和许可证；不会收集设备配置。

## 从哪里接手代码

TermiRs 基线是 `ab1910e4e251bf0b92271203b8f4dc25d8a5d015`；保留其 SSH 认证、VT 解析器、双栏浏览器和 SFTP 传输。当前工作树主要切口如下：

```text
crates/remote-core/src/    terminal.rs、tunnel_agent.rs、会话集合
src/app/                   终端标签、首次信任、VPS 路由、Sync 页面状态
src/config/                设备/公钥、路由、目录对和工作台默认值
src/filesystem/            目录操作、文件复制、SFTP 适配
src/sync_engine/           预览、目录范围确认、rclone、暂存与提交
src/workspace.rs           五页导航；Settings 编辑器在 src/workspace/
src/tunnel_home.rs         家端 SSH 转发与重连
tools/                     可丢弃设备、真实控制台与资源测量
```

[工作树与新增、修改文件清单](../agent/WORKTREE.md) · [系统结构](../architecture/ARCHITECTURE.md) · [连接与会话时序](../architecture/DATA_FLOW.md) · [同步写入流程](../design/SYNC_DESIGN.md) · [Core 契约](../interfaces/CORE_API.md)。

## 用什么证明主路径可用

| 路径 | 验收方法 |
|---|---|
| SSH 身份 / 转发 | [首次确认、拒绝、重启、换钥](HOST_KEY.md)。远端端口转发另用两条并发回显流核对收发；停止后两个客户端都收到 EOF。 |
| 三个远程终端 | [后台输出、独立断线、手动重连、尺寸与 ANSI](INTERACTIVE.md)。 |
| 家庭端 → VPS → 设备 | [受限回环链路](TUNNEL.md)，独立的 VPS/开发机 SSH 服务与登录凭证、rathole 0.5.0 服务端、受管理的客户端。本轮跳过外网跨 NAT 实测。 |
| 单文件与目录对 | [Files](FILES.md) 覆盖双向复制及取消后原件保留；[Sync](../design/SYNC_USE.md) 核对冻结清单、范围、冲突、取消、重试与历史。[执行器](SYNC_ENGINE.md) 另验证链接和中断。 |

2026-11-20 的 Windows 便携版通过 `cargo fmt --all --check`、`cargo check --workspace --all-targets --all-features --locked` 和 Release 构建。运行 `cargo test --workspace --all-features --locked`，**104 项通过**（Core 2、应用 102），0 项失败；默认跳过 3 个测试及 4 个文档示例。相比此前的 98 项，新覆盖了私钥口令的录入和隔离、无界面单文件收发与取消、大批量同步提示。本次没有重跑整套原生交互或性能验收；以下单列之前实测的构建。

以下命令在临时设备上分别验收指纹状态与完整转发链路。它们共用工作台二进制，各自生成测试密钥，不读取日常设备配置：

```powershell
$rclone = (Resolve-Path target/local-tools/rclone-1.72.1/rclone.exe).Path
$rathole = (Resolve-Path target/local-tools/rathole-0.5.0/rathole.exe).Path
uv run tools/smoke_host_key.py target/release/rust-remote.exe
uv run tools/smoke_credential_login.py target/release/rust-remote.exe
uv run tools/smoke_tunnel.py target/release/rust-remote.exe --files --rathole $rathole --rclone $rclone
```

**凭证及转发的实测构建：2026-11-20 00:32:07，6,929,408 字节。** 用 Windows 接口独立解开一次性测试密码，确认首尾空格完整；拒绝删除写入后，同一设备仍能登录，编辑和重启后再次登录成功。调试日志不含测试口令或其密文。原生转发测试也验证了修改后的规则重启可见，并发通道在手动停止时均关闭。[凭证迁移与恢复](CREDENTIALS.md)。本轮 00:29:18 的构建另外通过完整的双跳 SSH、Files 与 Sync 链路。

资源测量所用 Release（二进制生成于 00:14:20，6,927,360 字节）通过完整的双跳复测。在同一路由上完成 shell 回显、双栏 SFTP 的创建/改名/上传/下载/删除，以及 rclone 的预览与复制。分别更换网关或设备公钥，两次都在用户认证前被拦截。停用后两次探测均未触达设备，受管子进程已退出。同一构建的独立 `smoke_sync.py` 也通过了取消目录授权、撤销及重启的交互用例。

## 一次 Release 运行消耗多少资源

Windows 11 专业版 build 26200，AMD Ryzen 7 9700X，16 个逻辑处理器、15.2 GiB 可见物理内存。Rust 1.97.1，x86_64 MSVC，release / fat LTO。以下来自 2026-11-20 00:14:20 构建；两个负载依次运行。控制台 120 × 30，服务端 AsyncSSH 2.24.1，仅监听本机。

| 负载 | 工作台工作集 | 子进程工作集 |
|---|---:|---:|
| 首屏设备列表；启动到可见 78 ms（50 ms 轮询） | 空闲 9.6 MiB | 无 |
| 远程 shell 可读写（1 个 / 3 个） | 11.7 / 12.2 MiB | 无 |
| 一个终端保留，打开 Files 并上传 1 MiB，91 个样本 | 峰值 13.8 MiB | 无 |
| Sync 扫描：源含 64 × 4 KiB 与 1 × 8 MiB；目标含 64 个相同小文件及一个独有哨兵；耗时 3.03 s，98 个样本 | 峰值 13.4 MiB | 峰值 48.3 MiB |
| Sync 执行：只上传 1 个 8 MiB 的新文件；耗时 3.01 s，98 个样本 | 峰值 13.8 MiB | 峰值 57.7 MiB |

工作台空闲 5.01 秒的 CPU 为单个逻辑核的 0.62%。工作集是 Windows 在给定时刻报告的驻留内存；操作期间以 20 ms 为目标间隔采样。子进程按同一时刻求和，与工作台分列；不含 Python 测试服务或控制台宿主。

```powershell
uv run tools/smoke_workspace.py target/release/rust-remote.exe
uv run tools/measure_sync_load.py target/release/rust-remote.exe $rclone
```

第二条命令在临时根目录生成并回读测试负载。运行结束原目标哨兵未变，收到的 8 MiB 与源逐字节相同。

## 平台范围

本机未安装 WSL，Rust 只安装了 Windows MSVC 目标，无可用的 Linux 测试机。GitHub 的 [Windows 便携版](https://github.com/bestkojima/rust-remote-workspace/releases/tag/windows-2026.11.20) 使用上述本机构建。Linux 构建、运行及性能尚未验收；仓库保留了 Ubuntu 的 workspace 检查和测试入口。
