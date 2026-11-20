# 单人顺序开发的工期估算

以 2026-11-19 为估算起点，按工作日排期（排除周末，不计等待授权、设备排队和假期）。下图是预算，实际进度看[任务台账](../agent/TASKS.md)。上游依赖首次构建、三会话实测和同步覆盖策略是最可能改变估算的三处。

```mermaid
gantt
    title Rust Remote Workspace · 阶段预算
    dateFormat YYYY-MM-DD
    axisFormat %m-%d
    excludes weekends
    section 工程基线
    Phase 0 固定上游与构建运行记录  :base, 2026-11-19, 2d
    Phase 1 页面导航与 Core 边界   :nav, after base, 3d
    Phase 2 三终端及隔离验收       :tabs, after nav, 4d
    section 远程工作路径
    Phase 3 受控隧道与重连         :tunnel, after tabs, 4d
    Phase 4 双栏文件操作与取消     :files, after tunnel, 3d
    Phase 5 手动复制与冲突验收     :copy, after files, 5d
    section 发布候选
    Phase 6 双平台联测与资源测量   :release, after copy, 3d
```

原双平台估算共 24 个工作日。本轮只交付 Windows 本机版；Linux 暂缓，Phase 3 的真实跨 NAT 验收按用户决定跳过。
