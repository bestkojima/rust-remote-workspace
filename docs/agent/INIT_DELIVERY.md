# 初始化交付清单 · 2026-11-19

基线：TermiRs `ab1910e4e251bf0b92271203b8f4dc25d8a5d015`。下面是工作树中的项目结构；目录内未改动的上游源文件和本地 Cargo 构建缓存从简。可在根目录运行 `git ls-files --cached --others --exclude-standard` 展开所有项目文件。

```text
E:\rust_remote_terminal
├── .agents/skills/{feature-development,project-review}/SKILL.md
├── .github/workflows/{build,release}.yml
├── AGENTS.md  Cargo.toml  Cargo.lock  LICENSE  README.md  .gitignore
├── assets/        examples/
├── crates/remote-core/{Cargo.toml,src/lib.rs}
├── src/
│   ├── main.rs  lib.rs  app.rs  workspace.rs  events.rs  async_ssh_client.rs
│   ├── bin/rust-remote.rs      app/terminal_tabs.rs
│   ├── config/  filesystem/  key_event/  terminal/  ui/
│   └── error.rs  file_edit.rs  mode_state.rs  search_state.rs  transfer.rs  utils.rs
├── tools/smoke_workspace.py
└── docs/
    ├── agent/{FOR_AGENT_READ,CURRENT_STATE,TASKS,INIT_DELIVERY}.md
    ├── product/PRD.md
    ├── architecture/{ARCHITECTURE,DATA_FLOW}.md
    ├── design/{TUI_DESIGN,SYNC_DESIGN}.md
    ├── interfaces/{CORE_API,EVENT_PROTOCOL,GUI_ADAPTER}.md
    ├── plan/{ROADMAP,GANTT}.md
    └── testing/{BASELINE,ACCEPTANCE,INTERACTIVE}.md
```

**相对基线修改**：`.github/workflows/build.yml`、`.gitignore`、`Cargo.toml`、`Cargo.lock`、`README.md`；`src/{app,async_ssh_client,events,lib,main}.rs`、`src/key_event/{connected,file_explorer,mod}.rs`、`src/ui/terminal.rs`。

**新增**：树中全部 `.agents/`、`AGENTS.md`、`crates/`、`docs/`、`tools/`，以及 `src/workspace.rs`、`src/app/terminal_tabs.rs`、`src/bin/rust-remote.rs`。

**Mermaid 图**：[系统架构](../architecture/ARCHITECTURE.md#系统边界)；[隧道连接、终端生命周期、SFTP 操作](../architecture/DATA_FLOW.md)；[同步与冲突](../design/SYNC_DESIGN.md)；[分阶段甘特图](../plan/GANTT.md)。
