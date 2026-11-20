---
name: feature-development
description: 在 Rust Remote Workspace 新增或修复可运行的 SSH、终端、文件或导航切片时，复用 TermiRs 实现并核对验收、接口与项目状态。
---

# 交付一个可操作的切片

1. 按仓库根目录的 `docs/agent/FOR_AGENT_READ.md` 找入口，在 `TASKS.md` 选任务；把完成条件写成用户可观察的操作，例如「关闭 B 后，A、C 的提示符仍分别响应」。
2. 从入口追到既有 transport、文件后端或终端缓冲区，先证明现有能力能否直接用；只为缺口增加状态或适配。改事件路由时看看后台会话和迟到事件会落到哪里。
3. 用临时目录或本机测试端写出能区分新旧行为的用例，实现后跑它和受影响的原有测试。交互改动再走一遍真实启动、输入、切换和退出；无可用终端时把缺失的交互证据单列。
4. 在 `docs/agent/CURRENT_STATE.md` 和 `docs/agent/TASKS.md` 记录实测命令、结果及未通过的验收项。改接口时更新 `docs/interfaces/` 对应页，保留已实现与草案的分界。
