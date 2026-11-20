# Windows 本机交付的工作树 · 2026-11-20

基线为 TermiRs `ab1910e4e251bf0b92271203b8f4dc25d8a5d015`。SSH、VT 缓冲、双栏 SFTP 与 Tokio 的复用边界见[系统架构图](../architecture/ARCHITECTURE.md)，构建、运行及未验项见[本机验收](../testing/RELEASE.md)。

```text
E:\rust_remote_terminal
├── .agents/skills/                 开发与评审 Skill
├── .github/workflows/              构建与标签发布
├── assets/、examples/              上游资源与示例
├── crates/remote-core/src/         PTY、会话集合、代理进程
├── src/
│   ├── bin/、app/、key_event/、ui/   CLI 入口与界面
│   ├── config/、filesystem/         设备配置及本地 / SFTP 操作
│   ├── terminal/、sync_engine/      终端模型与手动目录复制
│   └── workspace/、utils/           工作台设置与共用工具
├── tools/                          本地测试设备与原生控制台验收
├── docs/
│   ├── agent/、architecture/、design/、interfaces/
│   └── plan/、product/、testing/
└── target/                         本机构建，不进 Git
    ├── dist/                       Windows x64 交付 ZIP
    ├── release/                    rust-remote.exe、termirs.exe
    └── local-tools/                本次验收用的 rclone、rathole
```

下列清单按目录合并文件名，忽略 Git 排除项。根目录以 `.` 表示。

## 修改（32 个文件）

- `.github/workflows/`：`build.yml`、`release.yml`
- `./`：`.gitignore`、`Cargo.lock`、`Cargo.toml`、`README.md`
- `src/`：`app.rs`、`async_ssh_client.rs`、`error.rs`、`events.rs`、`file_edit.rs`、`lib.rs`、`main.rs`、`transfer.rs`、`utils.rs`
- `src/config/`：`encryption.rs`、`manager.rs`
- `src/filesystem/`：`dir_walker.rs`、`mod.rs`、`sftp_file.rs`
- `src/key_event/`：`connected.rs`、`connection_list.rs`、`file_explorer.rs`、`form.rs`、`mod.rs`、`port_forwarding.rs`、`scp.rs`
- `src/ui/`：`connection.rs`、`file_explorer.rs`、`popup.rs`、`scp.rs`、`terminal.rs`

## 新增（78 个文件）

- `.agents/skills/feature-development/`：`SKILL.md`
- `.agents/skills/project-review/`：`SKILL.md`
- `./`：`AGENTS.md`
- `crates/remote-core/`：`Cargo.toml`
- `crates/remote-core/src/`：`lib.rs`、`terminal.rs`、`tunnel_agent.rs`
- `crates/remote-core/src/tunnel_agent/`：`windows_acl.rs`
- `docs/agent/`：`CURRENT_STATE.md`、`FOR_AGENT_READ.md`、`INIT_DELIVERY.md`、`TASKS.md`、`WORKTREE.md`
- `docs/architecture/`：`ARCHITECTURE.md`、`DATA_FLOW.md`
- `docs/design/`：`SYNC_DESIGN.md`、`SYNC_USE.md`、`TUI_DESIGN.md`、`TUNNEL_AGENT.md`
- `docs/interfaces/`：`CORE_API.md`、`EVENT_PROTOCOL.md`、`GUI_ADAPTER.md`
- `docs/plan/`：`GANTT.md`、`ROADMAP.md`
- `docs/product/`：`PRD.md`
- `docs/testing/`：`ACCEPTANCE.md`、`BASELINE.md`、`CREDENTIALS.md`、`FILES.md`、`HOST_KEY.md`、`INTERACTIVE.md`、`RELEASE.md`、`SYNC_ENGINE.md`、`TUNNEL.md`
- `src/app/`：`host_key.rs`、`sync.rs`、`terminal_tabs.rs`、`tunnels.rs`
- `src/app/sync/`：`editor.rs`、`view.rs`
- `src/bin/`：`rust-remote.rs`
- `src/config/`：`credential_unix.rs`、`credential_windows.rs`、`sync_profiles.rs`、`tunnels.rs`、`workspace_defaults.rs`
- `src/filesystem/`：`actions.rs`、`file_copy.rs`、`single_file.rs`
- `src/sync_engine/`：`fs.rs`、`local_dir.rs`、`mod.rs`、`private_dir.rs`、`publish.rs`、`rclone.rs`、`scope.rs`、`tests.rs`
- `src/`：`tunnel_home.rs`、`workspace.rs`
- `src/utils/`：`bounded_log.rs`
- `src/workspace/`：`settings.rs`
- `tools/`：`measure_sync_load.py`、`measure_workspace.py`、`package-windows.ps1`、`smoke_credential_login.py`、`smoke_files.py`、`smoke_host_key.py`、`smoke_settings.py`、`smoke_shared_config.py`、`smoke_sync.py`、`smoke_sync_engine.py`、`smoke_tunnel.py`、`smoke_tunnel_agent.ps1`、`smoke_workspace.py`
- `tools/packaging/`：`START.cmd`、`WINDOWS.txt`
- `tools/packaging/licenses/`：`rclone-1.72.1.txt`、`rathole-0.5.0.txt`
