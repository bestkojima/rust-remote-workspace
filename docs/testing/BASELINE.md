# TermiRs 原版基线 · 2026-11-19

## 在哪里起步

本地工作目录原为空。原版验证使用独立的干净克隆（`master`）；本项目以同一提交为 Git 基线，保留提交历史和根目录 `LICENSE`。

- [TermiRs 源码快照](https://github.com/caelansar/termirs/tree/ab1910e4e251bf0b92271203b8f4dc25d8a5d015)：`ab1910e4e251bf0b92271203b8f4dc25d8a5d015`，Cargo package `termirs 0.3.4`，edition 2024。
- MIT，Copyright (c) 2025 caelansar；发布源代码或二进制时随附原版权和许可文本。
- 在 Windows 11 专业版 10.0.26200、Ryzen 7 9700X、x86_64 MSVC 上验证；Cargo/Rust 1.97.1、VS 2022 Build Tools。以下结果仅针对上游快照。

## 平台证据

| 平台与证据 | 实际观察 |
|---|---|
| Windows，本机编译 | 干净克隆执行 `cargo build --locked` 成功。四个上游代码警告涉及 Windows 条件编译（未用 import/变量、不可达语句）；另有传递依赖 num-bigint-dig 0.8.4 的未来兼容提示。 |
| Windows，本机交互 | 分别在 PTY 启动本机编译的 debug 可执行文件和同版本上游 release。两者均显示 `Connection List (0/0)`、`N: New` 与 `I: File Explorer`；各按 `q` 正常退出（0）。 |
| Windows，本机测试 | `cargo test --locked -- --skip test_connect_embedded_server_auto_load_key`：76 通过、0 失败、2 ignored、1 filtered；4 个 doctest 均为上游 ignored。 |
| Linux，上游 CI | [Ubuntu 测试作业](https://github.com/caelansar/termirs/actions/runs/41061589248/job/124082403300)成功：fmt、check、nextest --all-features。 |
| Windows，上游 CI | [Windows check 作业](https://github.com/caelansar/termirs/actions/runs/41061589248/job/124082402833)成功：cargo check --all-targets。 |
| 发布构建 | [对应 SHA 的 release 作业](https://github.com/caelansar/termirs/actions/runs/41063181379)成功，包括 Windows MSVC、Linux x86_64/aarch64 musl。 |

回环测试实际覆盖密码和显式测试密钥认证、会话读写、连接取消和超时、SFTP 大文件收发与往返、端口转发与掉线后恢复。测试进程单独指定空的 TEMP/TMP 目录；没有启动真实设备连接。过滤的自动选钥用例只改 `HOME`，而 Windows 的 `dirs 6.0.0` 从 Known Folder 读取用户主目录，运行会尝试真实账户的默认 SSH 密钥。应先给该用例注入测试密钥目录。

首次构建时 Cargo 的内置 Git 客户端获取 WezTerm 遇到 TLS 中断；用系统 Git 获取 Cargo.lock 的固定提交和相应子模块原始对象后，Cargo 按原依赖完成编译。无需改 manifest、lockfile 或系统 Git 配置。

本机没有可用 WSL；改造后的 Linux 运行情况须另行验证。原版上游的 CI 结果不能替本项目签收。

## 复用前要知道的两件事

1. 终端模拟器是 **WezTerm**。Cargo 锁定 forks：wezterm `7bc8751d`、russh-sftp `ed99dd32`、ssh2-config `1bcf78d5`。保留 Cargo.lock；编译阶段需要访问这些 Git 源及 WezTerm 子模块。
2. README 的 Host Key 验证有边界：`SshClient::check_server_key` 比对已有 pin，但首次连接遇到空 pin 会直接放行。密码加密的派生材料包含主机名和用户名；不要沿用此方案保存新隧道 token。首次连接验证和系统凭证存储是部署到真实网络前的独立验收项。

