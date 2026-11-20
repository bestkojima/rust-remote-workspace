# 连接、会话和文件各走哪条路

## 经 VPS 建立隧道

```mermaid
sequenceDiagram
    participant C as 公司开发机
    participant V as VPS
    participant H as 家庭工作台
    C->>V: 经授权的 rathole client 主动认证连接
    V->>V: 仅在回环暴露转发来的 SSH 服务
    H->>V: 校验 VPS host key，建立受限 SSH 本地转发
    H->>C: 经转发通道校验开发机 host key，以设备密钥认证
    alt 链路断开
      H->>H: 展示断开，限制重试频率，允许手动停止
    end
```

rathole 接收客户端连接的端口与回环上的转发服务端口是两回事。[rathole 的 server/client 配置](https://github.com/rathole-org/rathole)分别描述它们；开放前只允许指定客户端来源及必要端口。

## 普通终端如何建立信任、切换和关闭

```mermaid
stateDiagram-v2
    [*] --> Connecting: 选设备并连接
    Connecting --> VerifyKey: 首次遇到此地址的公钥
    VerifyKey --> Connecting: 用户核对指纹并按 y，保存成功后重新握手
    VerifyKey --> Closed: 拒绝 / 保存失败
    Connecting --> Closed: 公钥与已存记录不符
    Connecting --> Foreground: 公钥匹配，用户认证及 PTY 成功
    Connecting --> Closed: 超时 / 取消 / 失败
    Foreground --> Background: 回设备页或切标签
    Background --> Background: 数据只进入自己的 VT 缓冲
    Background --> Foreground: 恢复
    Foreground --> Closed: 手动关闭或 SSH 断开
    Background --> Closed: 所属 SSH 断开
    Closed --> [*]
```

重连创建新实例；上一实例迟到的事件不能关闭新终端。文件浏览器借用 SSH handle 打开的临时 shell 在原浏览器上下文内返回。

## 双栏复制和无界面调用共用单文件传输

```mermaid
flowchart LR
    D[设备选择] --> S[认证并打开 SFTP]
    S --> L[本地 / 远端目录]
    L --> Pick[c 选择条目]
    Pick --> Dest[Tab 切目标栏]
    Dest --> Paste[v 开始传输]
    Paste --> Batch[队列逐项处理]
    Batch --> Job[termirs::files::copy_file]
    Api[无界面调用方] --> Job
    Job --> Event[进度 / 发布成功 / 取消或错误]
    Event --> L
    L --> Delete[d 删除]
    Delete --> Confirm{确认删除?}
    Confirm -->|取消| L
    Confirm -->|确认| Remove[执行并刷新]
```

两栏的选区和快捷键属于 TUI。[单文件接口](../interfaces/CORE_API.md)接收实际的来源和目标，在目标侧暂存，传完才发布。整目录同步另走 [Sync 预览与确认](../design/SYNC_DESIGN.md)。
