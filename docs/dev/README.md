# Remote Screen 设计文档

> These design documents are written in Chinese. The [README](../../README.md) and
> [CONTRIBUTING](../../CONTRIBUTING.md) cover the essentials in English; issues and pull requests in English are
> welcome.

本目录回答「为什么这么做、具体怎么做」：技术决策、协议规范、身份与安全、被控端、App、质量与发布。

文中的「设计文档」（章节号如 §4.7）和「画布」（画板名如 `ErrVersion`）是项目早期的产品方案和交互稿，没有公开；
引用它们的地方保留编号，方便对照历史讨论，读代码时可以忽略。

## 目录

| 文件 | 内容 | 什么时候看 |
|---|---|---|
| [01-decisions.md](01-decisions.md) | 技术决策 D1–D51：定了什么、为什么、放弃了什么 | 改架构之前 |
| [02-repo-env.md](02-repo-env.md) | 仓库结构、依赖方向、开发环境、`just` 命令、代码约定 | 第一次搭环境 |
| [03-protocol.md](03-protocol.md) | RSP/1 规范：传输、通道、消息、视频分片与 FEC、码率控制、多路径、会话恢复 | 改 `rsp-*`、Host、客户端时 |
| [04-identity-security.md](04-identity-security.md) | 设备密钥、双向证书、配对、WhoIs、本机确认、作废、权限模型 | 改鉴权和配对时 |
| [06-host.md](06-host.md) | 被控端：Mac / Windows / Linux 进程模型、模块、平台接口、IPC、审计、更新、打包 | 改 Host 时 |
| [07-app.md](07-app.md) | App：React Native 结构、原生组件接口、Rust 绑定、后台、Android 差异 | 改 App 时 |
| [09-quality-release.md](09-quality-release.md) | 测试（单测、模糊测试、弱网、延迟测量、真机矩阵）、CI、签名发布、Headscale 配置 | 发版、加测试时 |
| [10-appendix.md](10-appendix.md) | 错误码、端口、文件路径、配置项、日志字段、术语 | 随时查 |
| [spikes/](spikes/) | 早期技术验证的结论：Secure Enclave 密钥做 TLS 客户端证书、RN 新架构 + uniffi | 想知道某个方案为什么可行 |
| [../m0-latency.md](../m0-latency.md) | 第一版原型的端到端延迟测量方法和读数 | 做性能优化时 |
| [../ROADMAP.md](../ROADMAP.md) | 现状和接下来要做的事 | 想参与时 |

## 一页看懂

```
 手机 / iPad / Android / 电脑                                             被控电脑
┌────────────────────────────┐                                    ┌──────────────────────────┐
│ RN / Tauri 页面 (TypeScript)│                                    │ 系统自带：屏幕共享 / 远程桌面│
│  ├ RsCore (uniffi) ────────┼── 控制面：配对、SSH、诊断            │           / sshd          │
│  ├ 会话视图（原生）          │                                    ├──────────────────────────┤
│  │  解码上屏·手势·光标 ◄────┼── VNC tcp 5900 / RDP tcp 3389 ──────┤ （可选）Remote Host        │
│  │                    ◄────┼── RSP/1 QUIC udp 7400 / TLS tcp 7400┤  rs-host-agent (Rust)     │
│  └ 终端视图（原生）◄────────┼── SSH tcp 22（russh）               │  采集→编码→分片+FEC        │
│ rs-mobile / rs-desktop      │                                    │  注入·光标·剪贴板·声音·文件 │
└────────────────────────────┘                                    └──────────────────────────┘
        局域网 · 公司 VPN · 自建 tailnet（Tailscale / Headscale），设备之间直连，没有中转服务器
```

三条贯穿所有代码的规则：

1. **热路径不经过 JS。** 视频帧、触摸手势、指针、终端字节都在原生层和 Rust 之间直接走。JS 只负责页面、参数和事件。
2. **身份两层都要对上。** 走 tailnet 时 WhoIs + 设备证书；走内网时只有设备证书，因此默认更严格（见 [04](04-identity-security.md)）。
3. **每个失败都有错误码和下一步。** 协议里的错误码一一对应断开页（`RS-E101` … `RS-E1xx`，见 [10-appendix](10-appendix.md#错误码)）。

## 文档维护

- 改协议：先改 [03-protocol.md](03-protocol.md) 和 `proto/`，同一个提交里改实现和测试向量。
- 改决策：在 [01-decisions.md](01-decisions.md) 新增或修改条目，写明原因；废止的条目保留编号并注明被哪条取代。
