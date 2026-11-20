# Rust Remote Workspace

一个 Windows 终端工作台：给同一台设备开多个 SSH 会话，用双栏 SFTP 传文件，也可以先预览差异，再手动同步目录。

## 快速配置

解压 [Windows 便携包](https://github.com/bestkojima/rust-remote-workspace/releases/download/windows-2026.11.20/rust-remote-windows-x64.zip)，双击 `START.cmd`。按 **N** 添加设备，填 SSH 地址、端口、用户名和密码或私钥路径。按 **Enter** 连接，与管理员核对完整指纹；一致才按小写 **y** 保存并进入终端。

包内已带 rclone 和 rathole，无需安装 Rust。使用加密私钥时，填好路径后按 **Shift+Tab** 输入口令。首次指纹不确定就按 **Esc**；今后同址换钥，工作台会拦截连接并保留旧记录。[如何从设备控制台核对 SHA256 指纹](docs/design/TUI_DESIGN.md#首次连接核对哪一串)

## 连上之后

- **再开一个终端：** 按 **F12** 返回设备列表，选同一台设备按 **Enter**。**F6** 切换会话，**F8** 只关闭当前会话。
- **传一个文件：** 按 **F2** 选设备。源栏按 **c** 选择，**Tab** 换到目标栏，**v** 粘贴。[双栏操作](docs/testing/FILES.md)
- **同步两个目录：** 在双栏选好本地与远端目录，按 **F7** 保存目录对。到 **F3** 预览并确认后才会复制；目标独有文件会保留。[同步指南](docs/design/SYNC_USE.md)
- **经 VPS 连接：** 在获授权的开发机启动 [rathole 代理](docs/design/TUNNEL_AGENT.md)，家庭端按 **F5 → T** [配置路由](docs/design/TUI_DESIGN.md#经-vps-打开开发机终端)。网关与开发机各核对一次指纹。

## 从源码运行

在仓库根目录运行，默认沿用当前用户的 TermiRs 设备表：

```powershell
cargo run --locked --bin rust-remote
```

想用独立设备表，在命令末尾加 `-- --config ./devices.local.toml`。要安装到 Cargo 的可执行目录，运行 `cargo install --path . --locked --bin rust-remote`。原 `termirs` 入口保留。

基于 [TermiRs 0.3.4](https://github.com/caelansar/termirs/tree/ab1910e4e251bf0b92271203b8f4dc25d8a5d015) 开发，保留上游 Git 历史和 [MIT 许可证](LICENSE)。[Windows 构建与测试](docs/testing/RELEASE.md) · [Core API](docs/interfaces/CORE_API.md) · [工程导航](docs/agent/FOR_AGENT_READ.md)
