# 在获授权的开发机上启动隧道

管理员准备好 rathole **客户端专用配置**后，在开发机前台运行：

```sh
rust-remote tunnel-agent --config /path/to/private/client.toml --rathole /path/to/rathole
```

`--rathole` 可省略，默认从 PATH 查找。这里的 `--config` 紧跟在 **tunnel-agent** 后，指向原生 rathole TOML；平时用 `rust-remote --config devices.toml` 打开的仍是工作台。

## 启动前核对三个端点

| 在哪里配置 | 应指向哪里 |
|---|---|
| 开发机：`client.remote_addr` | 已获授权的 VPS 控制入口。非回环地址要求 TLS，或使用经可信渠道核对的服务器公钥与默认 Noise NK；公网控制端口按组织策略限制来源。 |
| 开发机：`client.services.<name>.local_addr` | 本机 SSH 服务的数字回环地址，如 `127.0.0.1:22`。工作台代理只接受回环 TCP 服务。 |
| VPS：`server.services.<name>.bind_addr` | **VPS 回环地址**上的独占端口。家庭端通过已验证的 VPS SSH 本地转发进入；不要在公网地址监听该服务端口。 |

管理员分别给客户端与 VPS 服务端配置对应服务的私有 token。管理员先建好私有目录和文件，再写入配置：Linux 下使用当前服务账号或 root 拥有的 `0600` / `0400` 文件；Windows 下文件所有者及允许访问的账号仅限运行账号、SYSTEM、Administrators。推荐从组织的凭证管理系统挂载或交付这个文件，不将 token 输入命令、工作台设备表或日志。变更配置时先停代理，改好权限和内容后再启动。

具体 TOML 字段见 [rathole v0.5.0 配置定义](https://github.com/rathole-org/rathole/blob/v0.5.0/src/config.rs)；TLS 和 Noise 公钥如何核对，见[上游传输说明](https://github.com/rathole-org/rathole/blob/v0.5.0/docs/transport.md)。

## 看状态、停止与恢复

- `agent: running (pid N)` 表示 rathole 客户端进程已启动。通过家庭工作台的 VPS 转发打开开发机 shell，才证明端到端连通。
- 控制链路临时中断由 rathole 自行重连。若子进程意外退出，代理显示退出状态以及下次启动等待时间：1、2、4、8、16 秒，然后每 30 秒一次。连续运行一分钟后，下次意外退出重新从 1 秒开始。
- 按 **Ctrl+C** 停止代理并回收子进程；Unix 也支持 SIGTERM。上级进程通过管道管理代理时，加 `--stop-on-stdin-close` 并持有其标准输入，关闭管道即可停止，退避期间同样生效。停止后不会自行重启。
- 本地权限、TOML 语法或允许的服务端点不符合要求时，代理返回错误。rathole 原始输出不转发，避免上游解析错误将带 token 的行写入日志。

Windows 在仓库根目录用 PowerShell 7 复验进程管理：

```powershell
.\tools\smoke_tunnel_agent.ps1 .\target\debug\rust-remote.exe C:\tools\rathole.exe
```

实测 Windows 11 / rathole 0.5.0：过宽 DACL 在启动子进程前被拒绝；私有文件语法错误没有回显其中的 token。主动结束测试子进程后，代理等待 1 秒才重新启动；关闭控制管道后两个进程都退出，在重试等待中关闭管道也没有拉起新的进程。
