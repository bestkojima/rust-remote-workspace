# 经 VPS 连接开发机：本机集成验收

从仓库根目录运行以下命令，把 `$rathole` 指向解压的[上游 rathole 0.5.0 Windows x64 发布包](https://github.com/rathole-org/rathole/releases/tag/v0.5.0)中的 `rathole.exe`。验收不需要 Windows OpenSSH Server，也不读取用户的 SSH 配置。

```powershell
cargo build --locked --bin rust-remote
uv run tools/smoke_tunnel.py target/debug/rust-remote.exe --rathole $rathole
```

联验文件和同步时，加 `--files --rclone <rclone.exe 路径>`。这条路径先从 TUI 新建路由并分别核对两个指纹，然后在同一条受限通道上运行双栏 SFTP 和目录对的真实 rclone 操作。服务端只接受预定的回环目标，两个测试目录与凭证目录分开。

## 如何证明数据走过了两段连接

Windows 原生 TUI 连到测试 VPS 的 SSH 服务。这个网关只允许一个 `direct-tcpip` 目的地：VPS 回环上的 rathole 服务端口。开发机 agent 主动连入独立的 rathole 控制端口，将服务转到自己的 SSH 进程。网关与开发机分别用独立的 SSH 主机密钥和登录密钥；两个 SSH 服务分别记录是否收到用户认证请求。临时目录仅当前用户可读，rathole 使用 Noise 和每次新生成的服务 token。进程启动与子进程生命周期通过真实 PID 和监听地址检查。

- **从零建立路由：** 在 Settings → VPS tunnels 按 n，选择两个 SSH 配置、填写 VPS 回环服务端口并保存。按 Enter 检查网关指纹；拒绝不写入，接受后再启动。此时可以看到转发就绪，开发机仍未收到登录请求。按 o 核对第二个指纹并进入 shell，发送标记并读回设备端回显。
- **分别中断两段：** 停掉网关的 SSH，检查家端监听消失；同钥恢复后无需操作，家端重新连上。停掉 rathole server 但保留网关，尝试开 shell，检查错误和客户端身份验证计数；恢复后原 agent 自动重新注册服务，再开 shell 能收发。
- **拒绝换钥：** 保留地址和端口，分别替换设备和网关的主机密钥。界面须显示相应的新旧指纹，两种情况下旧 pin 都不能被覆盖，换钥端均不能收到用户认证请求。
- **手动停用：** 在网关重试时按 x；即使网关随后上线，本地也不再监听或尝试认证。主动结束开发机 agent，检查 rathole 子进程退出；到 VPS 服务口读取 SSH 横幅，服务端须不能再触达开发机。

只检查测试设施可在同一命令末尾加 `--fixture-only`。该模式使用独立 SSH 客户端和上游 rathole client，主动测试直达原始设备端口被网关拒绝，再通过允许的服务端口交换 SSH 数据；它单独通过不能代表工作台已通过。

## 验证范围

本机测试的所有监听均为 `127.0.0.1`，每次使用独立端口、配置和测试密钥。本轮按用户决定跳过真实跨 NAT 验收：未修改宿主防火墙，未连接公网 VPS 或公司设备。

2026-11-19：Windows x64 上已执行 `--fixture-only`，rathole 0.5.0（发布提交 `ebb764a`）与 AsyncSSH 2.24.1 组成的回环测试链路通过。

**工作台端到端：通过。** 2026-11-19 的 Windows x64 debug 候选（23:30:02）使用上述默认命令完整运行，退出码 0。两次握手展示的指纹与两个测试服务独立计算的值相符。仅保存网关公钥时，设备认证请求数为 0；手动打开后，设备经指定服务回传了键盘输入。断开网关后家端监听消失，同钥恢复时自动重连；重启 rathole server 后，原 agent 重新注册，第二次 shell 收发成功。替换任一主机密钥都得到正确的新旧指纹，拒绝发生在用户认证之前。重试时手动停止，网关再上线也未重连。

**停用后怎样判断设备下线？** 家端 x 撤销本地监听；开发机停止 agent 后，子进程退出，测试设备不再收到新 TCP 连接。VPS 上的 rathole 0.5.0 服务口属另一个进程：不访问时曾观察到它在 20 秒后仍监听回环。在完整通过的试验里，主动访问两次都拿不到 SSH 横幅，设备连接数未变；有探测流量时 1.4 秒后 VPS 监听释放。[上游服务端实现](https://github.com/rathole-org/rathole/blob/v0.5.0/src/server.rs)会在接受访问时向控制通道请求数据连接。只看 VPS 端口能否建立 TCP，判不出开发机是否在线。
