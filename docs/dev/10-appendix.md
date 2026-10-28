# 10 · 附录

随时查的表：错误码、结束原因、端口、文件路径、配置项、日志字段、术语。地址、域名都是示例。

## 错误码

规则：

- 界面上统一显示为 `RS-E<码>`。协议里 `ErrorInfo.code` / `AuthResult.error` 只带数字。
- 每个码必须有对应的画板。没有专门画板的用**通用错误页**：标题 + 一句话 + 「重试」「返回」。
- 文案以画板为准，这里只写含义和按钮行为。
- 新增错误码要先在这里登记，再写代码（08 · 完成的定义）。

| 码 | 含义 | 由谁判定 | 画板 | 主按钮 → 动作 | 次按钮 → 动作 | 起始 |
|---|---|---|---|---|---|---|
| 101 | 电脑缺少系统权限（屏幕录制、辅助功能，或 macOS 定期确认没点） | Host：握手时 | `SessionError` | 「已打开，重试」→ 重新打开会话 | 「改用…」→ 系统屏幕共享 / 远程桌面 | M1 |
| 110 | 电脑离线，而且没有可用的内网或局域网路 | App：目录 + 拨号结果 | `ErrOffline` | 「重试」 | 「查看同网段设备」 | M1 |
| 120 | 节点在线，但 Host 没在运行（7400 端口拒绝连接） | App | `ErrNoHost` | 「用屏幕共享 / 远程桌面连接」（M3 V10） | 「返回」 | M1 |
| 130 | 被策略拦下：ACL 没放行（UDP、TCP 都超时），或用户不在 `allowed_users` | App（拨号）/ Host（WhoIs） | `ErrPolicy` | 「复制策略片段」 | 「返回」 | M1 |
| 131 | 这台设备不允许经内网连接（该路径权限为 deny） | Host | 通用错误页 | 「返回」 | — | M1 |
| 132 | 电脑关闭了「允许被远程控制」 | Host | 通用错误页 | 「返回」 | — | M1 |
| 140 | 电脑前的人拒绝了，或 30 秒没响应（`detail.reason = "timeout"`） | Host | `ErrDenied` | 「再请求」（显示剩余次数） | 「返回」 | M1 |
| 141 | 接管请求被拒绝 | Host | 不结束会话：Toast，然后回到 `SessionShared` 的观看状态 | — | — | M1 |
| 142 | 会话已满（1 个控制者 + 3 个观看者） | Host | 通用错误页 | 「重试」 | 「返回」 | M1 |
| 150 | 证书指纹不符：这个地址上是另一台电脑，或者 Host 重装过 | App | `ErrFingerprint` | 「取消」 | 「我重装过，重新配对」→ 二次确认 → 删除旧指纹 → 配对 | M1 |
| 160 | 只有内网路，本机却没有公司 VPN 接口 | App | `ErrVPN` | 「我已连接，重试」 | 「改用 App 内连接」（M2） | M1 |
| 161 | 有到内网网段的路由，但 UDP 和 TCP 7400 都超时 | App | `ErrPort` | 「复制诊断信息」 | 「在电脑上怎么打开」 | M1 |
| 170 | Host 版本太旧（拨号前用已知的 Host 版本判定，握手时再查一次） | App / Host | `ErrVersion` | 「返回」（说明要在电脑上手动升级 Host） | 「改用远程桌面」 | M1 |
| 171 | App 版本太旧 | Host | `ErrVersion`（App 太旧的变体） | 「去更新 App」→ App Store / Google Play / TestFlight | — | M1 |
| 180 | 本机已被作废 | Host | 通用错误页 | 「重新配对」→ 删除设备密钥，按首次配对走 | — | M1 |
| 181 | 这台设备还没和这台电脑配对 | Host | 通用错误页 | 「去配对」→ `PairAdd` | — | M1 |
| 190 | Mac 停在登录窗口，或切到了别的用户（FileVault 已关，tailnet 在线） | Host：`ServerHello` / `AuthResult` | `SessionLoginWindow`（第一态） | 「用系统屏幕共享登录」→ `vnc://<tailnet 地址>` 交给 VNC App | 「返回」 | M1 |
| 191 | Mac 重启后停在 FileVault 解锁界面，tailnet 不在线 | App：目前没有判定来源（原来由 Hub 心跳里的 `filevault` 提供），错误码和画板保留 | `SessionLoginWindow`（第二态） | 「通过 SSH 解锁磁盘」（系统支持时才显示） | 「返回」 | M1 |
| 199 | 其他错误（内部错误、未知错误） | 任一方 | 通用错误页 | 「重试」 | 「返回」；日志里记完整上下文 | M1 |
| 200 | 连不上电脑（主机 / 网络不可达、解析失败、15 s 超时） | App：内置适配器 | 通用错误页（`AdapterSession` 内） | 「重试」→ 重新连 | 「用其他 App 打开」→ 外部 App；「返回」 | M3 |
| 201 | 屏幕共享 / 远程桌面认证失败 3 次 | App：内置适配器 | 同上 | 「重试」→ 重新连并问凭据 | 「返回」 | M3 |
| 202 | 电脑只提供不支持的认证方式（VNC 安全类型、RDP 安全协议） | App：内置适配器 | 同上 | 「用其他 App 打开」 | 「返回」 | M3 |
| 203 | 远程桌面证书变了，用户没有信任 | App：内置适配器 | 同上 | 「重试」 | 「返回」 | M3 |
| 204 | 内置适配器连接中断（服务器断开、协议错误、网络断） | App：内置适配器 | 同上 | 「重试」 | 「返回」 | M3 |
| 205 | 电脑在线但没打开屏幕共享 / 远程桌面（端口被拒） | App：内置适配器 | 同上（V10 加「怎么打开」引导） | 「重试」 | 「返回」 | M3 |

### 结束原因（`Goodbye.reason`）

| 原因 | QUIC 关闭码 | App 表现 |
|---|---|---|
| `USER_CLOSED` | 0 | 回到设备页 |
| `HOST_CLOSED_BY_LOCAL_USER` | 0 | 通用结束页「电脑前的人断开了连接」，按钮「重新连接」「返回」 |
| `IDLE_TIMEOUT` | 5 | 通用结束页「空闲超时」，按钮「重新连接」 |
| `REPLACED` | 3 | 不提示：同一设备的新连接已经接管（例如恢复成功） |
| `REVOKED` | 6 | 按 RS-E180 处理 |
| `BACKGROUND` | 0 | App 自己发的，不显示 |
| `HOST_SHUTDOWN` | 4 | 「电脑正在关机或重启」→ 回设备页 |
| `HOST_UPDATING` | 4 | `SessionReconnect`，文案「电脑正在更新」；轮询目录，新版本上线后自动重连（约 40 s） |
| `PERMISSION_LOST` | 2 | 按 RS-E101 处理 |
| 协议错误 | 1 | 按 RS-E199 处理，日志带最后 20 条控制消息的类型（不带内容） |

## 端口

| 端口 | 协议 | 谁监听 | 用途 | 经过哪里 |
|---|---|---|---|---|
| 7400 | UDP | Host | RSP/1 over QUIC | tailnet；打开内网连接后也经内网 |
| 7400 | TCP | Host | RSP/1 over TLS（UDP 不通时回退） | 同上 |
| 22 | TCP | 服务器 | SSH；Tailscale SSH | tailnet / 内网（跳板机） |
| 41641 | UDP | tailscaled | WireGuard（默认端口，可变） | 公网 / 局域网 |
| 3478 | UDP | DERP | STUN | 公网 |
| 443 | TCP | Headscale、DERP | 控制面、中继 | 公网，`hs.[你的域名]` |
| 5353 | UDP | Host | mDNS 广播（`_rsp._udp`），家里局域网发现 | 局域网 |
| 9 | UDP | — | Wake-on-LAN 魔术包（广播；Host 平台层保留发送能力，没有触发路径） | 局域网 |
| 5900 | TCP | Mac | 内置 VNC（M3） | tailnet 优先 |
| 3389 | TCP | Windows | 内置 RDP（M3） | tailnet 优先 |

## 文件与路径

| 平台 | 内容 | 路径 |
|---|---|---|
| iOS / iPadOS | Rust 数据库（证书、已配对 Host、known_hosts、恢复令牌） | `Library/Application Support/rs/rs.db` |
| | 日志（7 天） | `Library/Application Support/rs/logs/` |
| | 设置（MMKV） | `Library/Application Support/mmkv/` |
| | 设备私钥 | 安全芯片，钥匙串标签 `ai.gxlself.remotescreen.device` |
| Android | 同上 | `/data/data/ai.gxlself.remotescreen/files/rs/` |
| Mac Host | App | `/Applications/Remote Host.app` |
| | 配置、配对表、信任日志、IPC socket | `~/Library/Application Support/Remote Host/`（`config.toml`、`host.db`、`agent.sock`），0600 |
| | 日志（14 天） | `~/Library/Logs/Remote Host/` |
| | 主机私钥 | 登录钥匙串，标签 `ai.gxlself.remotehost.key` |
| | LaunchAgent | `Remote Host.app/Contents/Library/LaunchAgents/ai.gxlself.remotehost.agent.plist` |
| Windows Host（M2） | 程序 | `C:\Program Files\Remote Host\` |
| | 配置、配对表、主机私钥（DPAPI）、日志 | `C:\ProgramData\RemoteHost\`（`config.toml`、`host.db`、`host.key`、`logs\`） |
| | 服务 / 管道 | 服务 `RemoteHostService`；`\\.\pipe\RemoteHost.ui`、`\\.\pipe\RemoteHost.session.<会话号>` |
| 服务器 | App 装的公钥 | `~/.ssh/authorized_keys` 里带 `rs-device=<device_id>` 标记的行 |

## Host 配置（`config.toml`）

Mac 和 Windows 相同。界面改设置时经 IPC `settings.set` 写入，agent 热加载；手改文件后要重启 agent。

```toml
[access]
allow_remote_control  = true               # 总开关；关闭后一律 RS-E132
allowed_users         = ["alice@"]      # tailnet 路径的用户白名单，空 = 只允许本机 Tailscale 登录的用户
attended_mode         = false              # 「有人值守」：每次连接都要本机确认
max_viewers           = 3
idle_timeout_min      = 30                 # 0 = 不超时
lock_on_end           = false              # 会话结束后锁屏（App 里也能按会话改）
pause_on_local_input  = true               # 本机有操作时暂停远端输入

[intranet]                                 # 内网 / 局域网连接，默认关
enabled               = false
interfaces            = ["en0"]            # 只在这些网卡上监听
allow_sources         = ["10.99.0.0/16", "192.168.31.0/24"]   # 来源网段白名单
confirm_each          = false              # 「内网连接每次确认」（功能位 14，Host 0.3.2 起）
allow_pairing         = false              # 「允许新设备经内网配对」

[clipboard]
to_phone              = true
from_phone            = true

[files]
enabled               = true               # 总开关
from_phone            = true               # 手机 → 电脑
to_phone              = true               # 电脑 → 手机（FileRequest）
browse                = true               # FileList
dest_dir              = ""                 # HostServices.downloads_dir；Mac ~/Downloads，Windows 下载 Known Folder
browse_roots          = []                 # 下载、桌面、文稿三个目录；Mac ~/Downloads、~/Desktop、~/Documents；Windows 对应 Known Folder
max_file_mb           = 0                  # 不另设上限，仍受 1 TiB 限制
partial_max_age_days  = 7                  # 启动时清理更旧的 .rs-*.part

[video]
codec                 = "auto"             # auto / hevc / h264
max_fps               = 60
encoder               = "auto"             # Windows：auto / nvenc / amf / qsv / mf / openh264

[update]
channel               = "stable"           # stable / beta
auto_install          = true               # false = 只提醒

[log]
level                 = "info"
```

`[files]` 中 `~` 按用户主目录展开。默认禁区 `~/Library`、`~/.ssh`、`~/.gnupg`、`~/.Trash`、state_dir 在用户把浏览根配置为主目录时仍生效。

设备级别的设置（权限三档、按路径的权限）存在配对表里，不在这个文件里。旧文件里的 `[hub]` 节不再读取，加载时只告警。

## App 设置

存在 MMKV 里。带 * 的会同步给 `RsCore.settings.set()`。

| 键 | 默认 | 含义 |
|---|---|---|
| `quality.default` * | `auto` | 画质默认档：auto / clear / smooth / eco |
| `session.mode.phone` / `session.mode.tablet` | `trackpad` / `touch` | 默认操作模式 |
| `session.hud` | `false` | 显示 HUD |
| `session.coachShown` | `false` | 是否已显示过手势引导 |
| `tailnet.mode` * | `system` | system / embedded / auto（M2） |
| `tailnet.controlUrl` * | `null` | 用户填的 Headscale 地址：string \| null（http(s) URL，≤ 512） |
| `prewarm.enabled` * | `true` | 首页预热 |
| `lock.onOpen` | `true` | 打开 App 时 Face ID |
| `lock.beforeSession` | `false` | 进入远控前 Face ID |
| `lock.sensitiveHosts` | `[]` | 被标记为敏感的 host_id |
| `networks` * | `[]` | 「其他网络」：名称、内网网段、VPN 地址池 |

## 日志字段

Rust 侧用 `tracing` 的结构化字段。导出的文件是 JSON lines。

| 字段 | 例子 | 说明 |
|---|---|---|
| `ts` | `1792465134000000` | unix 微秒 |
| `level` | `info` | |
| `comp` | `rsp.client` / `host.auth` | 组件 |
| `session` | `s-7f3a` | 会话 id（随机，不含设备信息） |
| `host` | `h7v2m9q3k8x4tb1a` | host_id |
| `device` | `k3jq7x2m9p4tbw8a` | device_id |
| `peer` | `100.64.0.12:51820` | 对端地址；导出时打码成 `100.64.•.•` |
| `path` | `tailnet_direct` / `tailnet_derp` / `intranet` / `lan` | |
| `transport` | `quic` / `tcp` | |
| `code` | `150` | 错误码 |
| `event` | `auth.decision` / `dial.attempt` / `fec.recovered` / `rate.change` | 事件名，方便 grep |
| `dur_ms` | `23` | 耗时 |

**不记录**：键入的文本、剪贴板内容、文件内容、终端输入输出、密码、私钥、配对码。用户名和主机名只在本地日志里有，导出时打码。

### 关键事件

| `event` | 什么时候记 | 主要字段 |
|---|---|---|
| `dial.attempt` / `dial.result` | 每条候选路 | `path`、`transport`、`dur_ms`、`code` |
| `auth.decision` | Host 每次鉴权 | `device`、`path`、规则序号、结果 |
| `session.start` / `session.end` | 会话开始 / 结束 | `role`、`reason`、`dur_ms`、延迟汇总 |
| `stats` | 会话中每 10 s 一次 | fps、码率、丢包、FEC 恢复、decode p50/p95、rtt |
| `path.switch` | 换路 | 新旧 `path`、`dur_ms` |
| `trust.apply` | Host 执行信任声明 | `seq`、kind、`device` |
| `upgrade.*` | 更新各步骤 | 版本、结果 |
| `security.*` | 指纹不符、验签失败、限速触发、WhoIs 不一致 | 相关 id |

## 术语

| 术语 | 含义 |
|---|---|
| Host | 装在被控电脑上的 Remote Host（界面 + agent） |
| agent | `rs-host-agent`，Host 里常驻的后台进程 |
| tailnet | Headscale 管理的 Tailscale 网络 |
| 模式 A / 模式 B | 手机用系统 Tailscale App 入网 / 用 App 内嵌的 tsnet 入网 |
| 路径（path） | 一次连接走的路：tailnet 直连、DERP 中继、内网、局域网 |
| DERP | Tailscale 的中继服务器，直连不通时用 |
| WhoIs | 向 tailscaled 查询某个 tailnet 地址属于哪个用户、哪个节点 |
| 指纹（fp） | 证书公钥（SPKI）的 SHA-256 |
| 预热 | 首页提前和常用电脑做 TLS 握手，点开时省掉握手时间 |
| 控制者 / 观看者 | 一台电脑同一时间只有一个控制者，最多 3 个观看者 |
| 本机确认 | 电脑前的人点「允许」后会话才开始 |
| 作废 | 让一台丢失的手机或平板永久失去访问权；不能撤销 |
| 信任日志 | 按序号排列的信任声明（作废、要求确认、批准、登记第二节点），Host 按顺序执行；目前没有下发来源（04 · 信任声明） |
| epoch | 视频流的代数；分辨率、显示器、编码器变化时加 1 |
| FEC | 前向纠错：多发冗余分片，丢了也能还原，不用等重传 |
| 画板 | 设计画布里的一个页面或状态，如 `ErrVersion` |
