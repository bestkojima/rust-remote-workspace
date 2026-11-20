# Rust Remote Workspace

把 TermiRs 扩展成一个低资源的 Rust CLI + Ratatui 远程工作台。默认中文交流，技术名词保留英文。说话直白，有取舍就拍板给理由；不用“这取决于”“视情况而定”充当结论，避开“不是……而是……”套句。

- 接手任务先看 [Agent 导航](docs/agent/FOR_AGENT_READ.md)。功能状态以运行证据为准；路线图只说明目标。
- 优先沿用 TermiRs 的 SSH、SFTP、终端模拟器和 Tokio runtime。隧道交给 rathole，同步交给 rclone。先做一个可验收的小切片，再决定要不要拆 crate。
- Core 不引入 Ratatui、Crossterm、egui。耗时操作在 UI 外运行，提供可取消的任务和有界事件；界面空闲时等待事件。本阶段不接 GUI 或 HTTP 服务。
- 改前读调用方和已有测试；写一个能暴露差异的验收用例，走通主路径，再处理该操作真正会遇到的失败。用锁定依赖测试；平台、命令、错误和未测项按实记录。
- 默认用临时目录、一次性凭证和本机测试服务。连真实公司设备、更改现有 SSH 安全配置、执行破坏性文件操作前，须确认对象、范围和授权。验证 Host Key；凭证放安全存储，仓库和日志均不得包含凭证。
- 新项目先 `git init`。保留上游 LICENSE、作者与基线提交记录；不要覆盖无关改动。仅在要求上传时使用 `gh`，缺少 `gh` 再用 `gitee`。
- 收尾：把完成项、实际测试、阻塞和下一步写进 [CURRENT_STATE](docs/agent/CURRENT_STATE.md)，同步 [任务台账](docs/agent/TASKS.md) 的状态与证据；接口或交互有变更时一并更新对应文档。
