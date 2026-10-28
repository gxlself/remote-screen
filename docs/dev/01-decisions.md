# 01 · 技术决策

每条决策写明：定了什么、为什么、放弃了什么。改决策要在这里改，并在提交说明里写原因。

## 总表

| 编号 | 领域 | 决定 | 状态 |
|---|---|---|---|
| D1 | App 外壳 | React Native 新架构（Fabric + TurboModules）+ TypeScript；远控画面和终端是原生组件 | 定 |
| D2 | Rust ↔ App | 同一个 Rust 静态库 `rs-mobile` 暴露两层：控制面用 uniffi（JS 经 uniffi-bindgen-react-native），热路径用手写 C ABI（Swift / Kotlin 直接调） | 定，spike S2 验证 |
| D3 | 协议编码 | 控制面 protobuf（prost）；视频、音频、指针、输入用定长二进制 | 定 |
| D4 | Hub 接口 | tailnet 内的 HTTP + JSON，OpenAPI 3.1 描述；Go 端 oapi-codegen 生成，Rust 端手写 serde 类型 + 契约测试 | 废止，D51 |
| D5 | 传输 | QUIC（quinn 0.11）为主；UDP 不通时 TCP + TLS 1.3 回退，复用同一套消息 | 定（M0 已用 quinn） |
| D6 | 抗丢包 | 每帧 Reed-Solomon 纠删码（`reed-solomon-simd`），冗余 5–30% 随丢包率变化 | 定 |
| D7 | 设备身份 | 安全芯片里的 P-256 设备密钥，自签 X.509 作为 TLS 客户端证书；rustls 自定义签名回调到原生层签名；指纹 = SPKI 的 SHA-256 | 定，spike S1 验证 |
| D8 | 配对 | SPAKE2 + 6 位码（扫码时额外带 Host 指纹）；Host 本机点「允许」才生效 | 定 |
| D9 | 信任链 | 「加权限」（批准新设备）必须由已配对设备签名，Host 验签；「减权限」（作废、要求本机确认）不需要签名。未签名的指令只能让 Host 更严格 | 定；下发来源随 Hub 废止（D51），格式和 Host 规则保留 |
| D10 | Host 界面 | Tauri 2（菜单栏 / 托盘、设置、向导、确认弹窗）；采集和网络在独立的 agent 进程，二者用本地 JSON-RPC 通信 | 定 |
| D11 | Mac 进程模型 | `Remote Host.app` 内含 `rs-host-agent`，用 `SMAppService` 注册为 LaunchAgent，系统权限归属到 App | 定 |
| D12 | Windows 编码 | FFmpeg（LGPL，动态链接）的 `hevc_nvenc` / `hevc_amf` / `hevc_qsv`，D3D11 硬件帧零拷贝；兜底 Media Foundation 硬件 MFT；最后 openh264 | 定，M2 开工前复核 |
| D13 | Host 更新 | agent 自己做更新：版本清单 Ed25519 签名 + 平台代码签名双重校验，会话结束后安装，失败回滚；Mac、Windows 同一套逻辑 | 定；清单原来由 Hub 下发，现在没有更新源（D51） |
| D14 | SSH | Rust `russh` + `russh-sftp`；终端渲染 iOS 用 SwiftTerm，Android 用 Termux `terminal-view` | 定 |
| D15 | SSH 公钥作废 | 装公钥时给行加标记，同时装一把只能执行清理命令的 Hub 维护密钥；作废时 Hub 用它删行 | 废止，D51 |
| D16 | App 内 tailnet | `libtailscale`（Go，c-archive）+ Rust 侧 `Dialer` 抽象；QUIC 经自定义 `AsyncUdpSocket` 跑在 tsnet 上 | M2，spike S3 提前验证 |
| D17 | App 状态与存储 | zustand（本地状态）+ TanStack Query（设备目录）+ MMKV（持久化）；导航 react-navigation 7 | 定 |
| D18 | 日志 | Rust 统一用 `tracing`，按天滚动；App 的 JS 日志转给 Rust 写 | 定 |
| D19 | 版本 | 语义化版本；0.2.x 是 M1 开发版，协议可以随时改；**0.3.0 冻结 RSP/1**，之后只加功能位 | 定 |
| D20 | 工具链 | `just` 作为统一任务入口；cargo workspace；pnpm 管 App；`tailnet/` 是独立 Go module | 定 |
| D21 | iOS 上屏 | 继续用 `AVSampleBufferDisplayLayer`（M0 已验证，之后支持画中画）；只有实测证明 Metal 更快才换 | 定 |
| D22 | 键盘编码 | 输入事件统一用 USB HID 用法码（Keyboard page 0x07）+ 修饰键位；Host 各自映射到 macOS 虚拟键码 / Windows 扫描码 | 定 |
| D23 | Hub 传输安全 | tailnet 内明文 HTTP（WireGuard 已加密），WhoIs 认人；不做 TLS（Headscale 不支持 `tailscale cert`） | 废止，D51 |
| D24 | Hub 存储 | SQLite（`modernc.org/sqlite`，纯 Go，无 CGO），内置 SQL 迁移 | 废止，D51 |
| D25 | 换路与恢复 | 活连接可消费一次性恢复令牌，断开后保留 60 s；轮换令牌、epoch + 1、输入重绑；500/250/1500 ms 判死；暂停 45 s 自动恢复 | 定，2026-10-21；部分实现待跟进 |
| D26 | Host 所属用户 | AgentIdentity.user 与空 allowed_users 同取 Tailscale Self 用户登录名 | 定，2026-10-21；信任同步部分随 Hub 废止（D51） |
| D27 | 剪贴板 | macOS 轮询间隔统一 300 ms | 定，2026-10-21 |
| D28 | 控制权保留 | 在线控制者的同用户交接可立即进行；断开保留期内新连接接管必须本机确认 | 定，2026-10-21 |
| D29 | 观看者上限 | 使用配置 max_viewers，默认 3 | 定，2026-10-21；鉴权入口实现待跟进 |
| D30 | 唤醒冲突 | wakeHost 的 409 区分 already_online、no_wake_nic、no_relay | 废止，D51 |
| D31 | 重复作废 | agentRevokeDevice 的 409 使用 already_revoked | 废止，D51 |
| D32 | 更新完成 | 心跳增加 ready / rolled_back，目标版本自检 ready 后 job 才 done | 废止，D51 |
| D33 | SSH 安装记录 | ssh_installs 增加 port、jump_user、host_key | 废止，D51 |
| D34 | 审计去重 | (host_id, event, ts_us, seq)，同一微秒内编号，去重表保留 7 天 | 定，2026-10-21；本地审计记录保留 ts_us 与 seq，去重表随 Hub 废止（D51） |
| D35 | D15 安装闭环 | 已登录设备 GET /api/v1/maint-key；非 root 脚本回退 ~/.local/bin，记录并绑定实际路径 | 废止，D51 |
| D36 | 离开控制台 | 会话中停止采集并 Goodbye 190；连接前 AuthResult DENIED 190，ServerHello 不加字段 | 定，2026-10-21；策略已有、接线待跟进 |
| D37 | 平台诊断 | Platform 增加屏幕共享 / FileVault 是否开启的查询 | 定，2026-10-21；实现待跟进 |
| D38 | host-ui IPC | approval.pending 补拿确认请求；security.confirm_new_devices 默认 true；补主机、会话、设备及动作 / 事件能力 | 定，2026-10-21；实现待跟进 |
| D39 | 指纹展示 | SPKI SHA-256 前 10 字节，Crockford base32 大写 16 字符，每 4 个空格分组 | 定，2026-10-21；实现待跟进 |
| D40 | 会话审计口径 | 恢复不拆会话；保留期过期补记 session_end；记 role_change；file 只记数量和字节 | 定，2026-10-22 |
| D41 | Host 安装页地址 | 来自 Hub 配置 [install] url，经 GET /v1/me 的 install_url 下发；App 不写死，未配置时隐藏发链接、SSH 安装置灰 | 废止，D51 |
| D42 | App 内 tailnet 接入 | Headscale 地址由用户填（tailnet.controlUrl，Rust 规范化）；OIDC 交互登录，authUrl 用系统浏览器打开；接入方式三档（手机上的 Tailscale / App 内连接 / 自动）；App 内节点只出不进，生命周期归 Rust 控制器 | 定，2026-10-23；实现待跟进 |
| D43 | 同一设备的第二个 tailnet 节点 | 设备自己签 BIND_NODE（信任日志 kind 4）；Host 鉴权第 6 步认主节点或第二节点；每台设备最多一个第二节点；Host 对还没配对的签名者先暂存声明，配对后验签生效 | 定，2026-10-23；登记入口随 Hub 废止（D51），格式和 Host 规则保留 |
| D44 | 虚拟显示器 | Mac 用私有 CGVirtualDisplay + 运行时探测；Windows 自研 IddCx UMDF 驱动只造屏，采集仍走 DXGI Desktop Duplication，服务经 IOCTL 控制并以 5 秒租约防残留；虚拟屏归属建它的会话，保留期内保留 | 定，2026-10-23；实现待跟进 |
| D45 | SSH 用户证书 | Hub 当 Ed25519 用户 CA，给设备 P-256 SSH 密钥签 12 小时证书（principal rs:<login>）；服务器账号装 cert-authority 行；作废后停止签发，没有公钥行的服务器最迟 12 小时内失效；D15 清理保留给存量公钥行 | 废止，D51 |
| D48 | 内置 VNC / RDP 客户端 | VNC 自写 RFB 3.8 客户端（`rs-vnc`，只用工作区已有的 des / aes / md5 / num-bigint / flate2）；RDP 用 IronRDP（MIT/Apache-2.0，纯 Rust），不用设计文档 §5.3 的 FreeRDP。理由：纯 Rust、tokio 异步，直接接我们的流和工作区的 rustls，不用给 iOS / Android 交叉编 C 和 OpenSSL 再写 FFI。IronRDP 钉主分支提交（git 依赖：已发布版本依赖的 curve25519-dalek 预发布版与 russh 冲突），RDP 带 EGFX / H.264、远端声音、纯文本剪贴板、动态分辨率与多显示器（C13）。画面走原始帧 C ABI，输入按 ID 号段复用 `rs_input_*`。 | 定，M3 |
| D47 | Linux 被控第一版 | 新 crate host-linux；Wayland 走 xdg-desktop-portal（RemoteDesktop + ScreenCast 同一会话）+ PipeWire SHM，X11 走 XShm / XDamage / XTest；编码 FFmpeg LGPL 动态库运行时加载（VA-API / NVENC），兜底 openh264；systemd user 单元随图形会话起停，AppImage 分发；不支持登录界面、wlroots 输入、无 XWayland 时的剪贴板 | 定，M3；实现待跟进 |
| D49 | 原生优先、免安装连接 | 先用系统屏幕共享 / 远程桌面；205 引导打开系统共享，仍不行才安装 Host；未安装 Host 的电脑通过 `others`、Bonjour 或手动扫描发现 | 定，2026-10-24 |
| D46 | 电脑端控制端 | Tauri 2 外壳（设备列表、配对、批准、设置，复用 host-ui 组件与 App 纯 TS）+ 同进程平台原生会话窗口（Mac：AppKit + VideoToolbox + AVSampleBufferDisplayLayer；Windows：Win32 + FFmpeg d3d11va + DXGI 翻转链）；不用 WebView WebCodecs；Rust 直接依赖 rs-mobile，不抽共享 crate；第一版先 Mac 后 Windows | 定，2026-10-23 |
| D50 | Hub 可选 | App 与 Host 不配 Hub 地址时仍可通过 tailnet 和本机存储完整工作；默认不配，Hub 提供批准、吊销、唤醒、升级、SSH 证书、审计和同步等增强 | 被 D51 取代 |
| D51 | 移除 Hub | 2026-10-24 移除 Hub：自用、零安装，不再部署 rs-hub；设备列表走本机 tailnet 和 Tailscale / Headscale API；Hub 代码保存在标签 `hub-last` | 定，2026-10-24 |

## 逐条说明

### D1 · React Native + 原生热路径

- **为什么**：设计文档 §10 定了 RN，团队熟悉。列表、设置、文件、配对这些页面占 App 的大部分，用 RN 写得快，iOS 和 Android 共用一套。
- **代价**：远控画面和终端必须原生。M0 的 Swift 代码（`VideoDecoder`、`RemoteViewController` 里的手势）直接搬进 Fabric 原生组件 `RemoteSessionView`，不重写。
- **放弃**：纯原生（SwiftUI + Compose）。体验最好，但两套 UI，人力翻倍；只有在 S2 spike 证明 RN 新架构撑不住时才回退到这条路。

### D2 · 两层绑定

```
             ┌──────────── JS（RN 页面）
             │  RsCore.*  （uniffi-bindgen-react-native 生成的 TurboModule）
rs-mobile ───┤
 (Rust)      │  rs_session_attach / rs_input_* / rs_term_*   （手写 C ABI，头文件 rs_mobile.h）
             └──────────── Swift / Kotlin 原生组件（RemoteSessionView、TerminalView）
```

- 会话对象由 Rust 持有，存在一张 `SessionId → Session` 表里。JS 用 `RsCore.sessions.open()` 拿到 `sessionId`；原生组件用同一个 `sessionId` 调 `rs_session_attach()` 挂上视频回调。所以会话可以在没有画面的情况下存在（「会话」页的列表、后台保持 30 秒）。
- 键盘、文本、快捷组合这类低频输入走 uniffi 也够快；指针移动和手势走 C ABI。
- 回退方案：如果 uniffi-bindgen-react-native 在新架构下出问题，改成手写一个 TurboModule，通过 C ABI 传 JSON 字符串。接口形状不变，只换胶水层。

### D3 · 控制面 protobuf，数据面二进制

- 控制面消息多、会演进（版本协商、功能位、多显示器、多设备、诊断），protobuf 的「不认识的字段直接忽略」正好用于设计文档 §5.2 的版本兼容。
- 视频分片每秒几千个，头部要定长、零分配；输入事件要小。这两类保持 M0 的手写二进制，格式见 [03-protocol](03-protocol.md)。
- M0 的 `core/src/control.rs` 整体替换，`wire.rs`、`video.rs`、`input.rs`、`clock.rs` 保留并扩展。

### D4 · Hub 用 HTTP + JSON（已废止）

2026-10-24 移除 Hub（D51）：接口定义、契约测试样例和 Go 服务端都已删除，原文在标签 `hub-last`。

### D6 · Reed-Solomon

- 一帧切成 k 个等长分片，再算 m 个冗余分片；收到任意 k 个就能还原。丢包率在 1–5% 的 Wi-Fi 和 5G 上，比「丢了就等关键帧」平滑得多。
- `reed-solomon-simd` 支持最多 32768 个分片，在 Apple Silicon 和 x86 上都有 SIMD；要求分片字节数是偶数，[03-protocol](03-protocol.md#5-视频分片与-fec) 已按此设计。

### D7 · 设备密钥做客户端证书

- 私钥不出安全芯片，所以 rustls 拿不到私钥。做法：实现 `rustls::sign::SigningKey`，`sign()` 时回调原生层（iOS `SecKeyCreateSignature`，Android `Signature` + AndroidKeyStore）。签名是同步调用，耗时约 1–5 ms，只在握手时发生一次。
- 证书只是公钥的载体：首次启动时用 `rcgen` 的远程签名接口生成一张 10 年有效的自签证书，存在 App 沙盒里（证书是公开信息）。
- 指纹改为 **SPKI 的 SHA-256**（M0 用的是整张证书的 SHA-256）。好处：重签证书不改指纹。Host 证书同理。

### D8 · SPAKE2 配对

6 位码只有约 20 位熵。如果直接拿它做 HMAC，在线中间人截到一次交互就能离线穷举。SPAKE2 是口令认证密钥交换：中间人每次连接只能猜一次，Host 错 5 次就作废这个码。扫码时二维码里还带 Host 指纹，App 先比对指纹再做 SPAKE2，两道保险。详见 [04](04-identity-security.md#配对)。

### D9 · 加权限要签名，减权限不用

设计文档 §08「设备信任」一行防的是「控制面被攻破后伪造节点」。如果 Host 无条件执行别人发来的「新增设备」，发送方被攻破就等于所有电脑被攻破。原则：**未签名的指令只能让 Host 更严格，不能让它更宽松。**

- **加权限**（已配对设备批准新手机，M2）：批准声明由旧设备的设备密钥签名，Host 验证签名者是自己的已配对设备后才加入新设备。
- **减权限**（作废设备、要求某设备每次本机确认）：不需要签名，Host 立即执行。
- 未签名的指令最多能作废设备（拒绝服务），不能新增控制者。格式见 [04](04-identity-security.md#信任声明)。

2026-10-24 移除 Hub（D51）后，信任声明没有下发来源；格式和 Host 规则保留，以后接入新的来源时直接复用。

### D10 / D11 · Host 进程模型

- 采集、编码、网络必须常驻，不能依赖界面进程活着；界面挂了不影响会话。
- macOS 的屏幕录制和辅助功能权限按「负责进程」归属。agent 放在 App 包里、用 `SMAppService.agent(plistName:)` 注册，系统设置里显示的是「Remote Host」，用户只授权一次。M0 的「授权给终端」只是开发期行为。
- Windows 是服务 + 会话进程 + 托盘三件套，见 [06-host](06-host.md#windows)。

### D12 · Windows 编码走 FFmpeg

- 设计文档写的是分别接 NVENC / AMF / QSV SDK。三家 SDK 各写一遍，工作量和维护成本都很高。FFmpeg 已经把三家封装成同一个接口，并且支持 D3D11 硬件帧输入（DXGI 截到的纹理直接喂编码器，不经 CPU），Sunshine 也是这么做的。
- 许可：只用 LGPL 组件、动态链接 `avcodec` / `avutil`，随安装包分发 DLL 并附许可证说明。
- 兜底顺序：FFmpeg 硬件编码 → Media Foundation 硬件 MFT（`CODECAPI_AVLowLatencyMode`）→ openh264（软编，只有 H.264）。
- M2 开工前用三张显卡各测一次，确认低延迟参数可用后再锁定。

### D13 · 自研更新器

- Sparkle 需要在界面进程里跑，交互式更新为主；我们需要的是「会话结束后静默安装」「失败回滚」，Mac 和 Windows 逻辑一样。agent 自己做更简单可控。
- 校验两道：清单和安装包有 Ed25519 签名（公钥编进 agent）；安装包本身还要过平台代码签名校验（`SecStaticCodeCheckValidity` 要求同一 Team ID；Windows `WinVerifyTrust` 要求同一证书主题）。
- 流程见 [06-host](06-host.md#自动更新)。清单原来由 Hub 下发，2026-10-24 移除 Hub（D51）后没有更新源，Host 手动更新，本地流水线保留。

### D15 · SSH 公钥作废（已废止）

2026-10-24 移除 Hub（D51）：不再装维护公钥和 `rs-keyclean` 脚本，作废设备后不会自动清理服务器上的公钥。App 装的公钥行仍带 `rs-device=<device_id>` 标记，作废后由用户在服务器上自行删除。原做法在标签 `hub-last`。

### D16 · App 内 tailnet

- tsnet（Go）编成静态库（iOS）/ 共享库（Android），只提供拨出（UDP / TCP），**不提供 `tailscale_listen`**：App 内节点只出不进（2026-10-23 定）。
- Rust 侧定义 `trait Dialer { dial_udp(addr) -> Box<dyn AsyncUdpSocket>; dial_tcp(addr) -> TcpLike; }`：模式 A 用系统 socket，模式 B 用 libtailscale 返回的 socketpair fd。quinn 的 `AsyncUdpSocket` 包一层「只有一个对端」的 UDP。
- 风险是 tsnet 的 UDP 吞吐和 iOS 后台限制，所以在 M1 用 2 天做 spike S3，M2 才正式做。
- 节点生命周期归 Rust 控制器（RS-T3C / RS-T3D）：前台或有活动连接时在线，进后台 30 s 后停（needsLogin 时不停），回前台 rebind 或重启。
- 选路：模式 B 下 tailnet 地址和 tailnet 名字走 App 内节点，其他走系统网络（RS-T3A）。

### D25 · 换路与恢复（2026-10-21）

旧连接存活时即可消费恢复令牌，断开后的保留窗口为 60 s，单次使用；成功换新 session_token，TCP 输入重新绑定，epoch + 1 并重置客户端重组器，Host 直接出关键帧。静默 500 ms 后每 250 ms Ping，1.5 s 无回应判死；resumed=false 按新会话，恢复最终失败映射 199。Host 暂停 45 s 后自动恢复推流。避免先断后连造成额外中断、旧输入令牌复用和暂停永久不恢复；不沿用「必须先断开才可恢复」的限制。45 s 当前实现为关闭会话，须跟进；完整契约和其余接线缺口见 03 §4.5 / §10.4。

### D26 · Host 所属用户（2026-10-21）

AgentIdentity.user 取 LocalAPI status 的 Self 用户登录名，与 allowed_users 为空时的本机用户同源。这样信任声明的用户比对有确定的身份来源，不用系统账户或 tag 所有者反推身份（04 · 信任声明、06 · 本机身份与审计）。

### D27 · 剪贴板轮询（2026-10-21）

macOS NSPasteboard changeCount 每 300 ms 查询一次，以 06 为准。统一已有实现与文档，取消早期任务清单里 500 ms 的口径。

### D28 · 同用户接管边界（2026-10-21）

「立即交接」仅适用于当前控制者在线且同用户；控制者断开后的 60 s 保留期内，同用户的新连接接管必须本机确认，原设备凭有效令牌恢复走恢复流程。避免新连接绕过保留角色的确认要求；不把在线交接规则套到离线保留期（03 §4.3、06 · 会话管理）。

### D29 · 观看者配置（2026-10-21）

观看者上限使用 max_viewers，默认 3，超限 DENIED 142。让配置成为唯一上限来源；不在鉴权中写死 3（鉴权入口实现待跟进；03 §4.3、04 · 连接时的鉴权、06 · 推流管线）。

### D30 – D35 · 已废止（D51）

D30 唤醒冲突、D31 重复作废、D32 新版本自检完成、D33 SSH 安装记录字段、D35 SSH 维护公钥与脚本路径，都是 Hub 接口或数据表上的细则，随 Hub 一起废止，原文在标签 `hub-last`。D34 审计去重：本地审计记录仍带 `ts_us` 与 `seq`（见 [06 · 本机身份与审计](06-host.md#本机身份与审计)），去重表随 Hub 废止。

### D36 · 控制台离开时序（2026-10-21）

快速用户切换或进入登录窗口导致 Host 离开控制台时，停止采集并发含错误码 190 的 Goodbye；连接前用 AuthResult DENIED 190，ServerHello 不增加控制台字段。普通锁屏仍按本机控制台处理。避免采到其他用户桌面；不通过扩充 ServerHello 传递拒绝状态（策略已有、会话接线实现待跟进；03 §4.6、04 · 鉴权、06 · 其他）。

### D37 · 平台诊断查询（2026-10-21）

Platform 增加屏幕共享、FileVault 是否开启两项查询，给诊断页真实状态；不能把 FileVault 已开启等同于当前磁盘未解锁。具体方法签名及未知状态表示仍待决定（实现待跟进；06 · 平台接口）。

### D38 · host-ui IPC 与新设备确认（2026-10-21）

增加 approval.pending 补拿未处理请求，解决 --prompt 拉起界面时漏掉通知的问题；「新设备连接前先问我」对应 security.confirm_new_devices，默认 true。补齐主机名、会话路径 / 延迟 / 显示器、设备平台 / 指纹 / 配对时间、断开并锁屏、观看改回控制及作废事件的能力，避免界面用假字段或只依靠实时通知。现有 IPC 字段已就地登记；实现没有且本次未指定的扩展线名 / 参数仍待决定（实现待跟进；06 · 本地 IPC）。

### D39 · 统一指纹展示（2026-10-21）

App、host-ui 统一取 SPKI SHA-256 前 10 字节，Crockford base32 编成 16 个大写字符、每 4 个空格分组，例如 `7K2M 9QXA 4D1F 8ZPW`。便于跨界面核对；不再显示首尾十六进制片段。完整 fp 的校验 / 线上编码以及设备 ID 算法不变（显示层实现待跟进；03 §1.1、04 · 标识、06 · host-ui、07 · RsCore）。

### D40 · 会话审计口径（2026-10-22）

一次会话在连接记录里只占一对开始 / 结束：保留期内恢复不写新的 session_start，保留期过期时补记 session_end；角色互换记 role_change {from, to}；文件只记 file {op, files, bytes}，不记名字。App 切后台、切网络是常事，拆成多段会让「时长」和「次数」都失真；文件审计不记名字，和「detail 不含内容数据」一致（06 · 审计事件、06 · 会话）。

### D41 · Host 安装页地址来自 Hub（已废止）

Host 安装页地址原来由 Hub 配置下发。2026-10-24 移除 Hub（D51）后没有这个来源，App 不再提供「发安装链接」和「通过 SSH 安装」入口。

### D42 · App 内 tailnet 接入（2026-10-23）

Headscale 地址由用户填（`tailnet.controlUrl`，Rust 规范化）；OIDC 交互登录，authUrl 用系统浏览器打开；接入方式三档（手机上的 Tailscale / App 内连接 / 自动）；App 内节点只出不进，生命周期归 Rust 控制器。理由：

1. 地址由用户填：Headscale 地址随部署而定；画板 `Onboard` 已有「Headscale 地址」输入框。
2. OIDC 交互登录 + 系统浏览器：画板 `Onboard` 写的是用 OIDC 账号登录；现在没有 App 内浏览器的原生模块，`platform.openUrl` 已用于 SSH 的 authUrl。预授权 key 以后另议。
3. 三档：08 T3 要求「自动」；只在本构建带 App 内连接时出现第三档。
4. 只出不进：Host 不会主动连手机，App 内节点不需要监听，少一个暴露面。

### D43 · 同一设备的第二个 tailnet 节点（2026-10-23）

打开 App 内连接后同一台手机有两个 tailnet 节点，Host 配对记录只记一个。设备用自己的密钥签 BIND_NODE（信任日志 kind 4），Host 鉴权第 6 步认主节点或第二节点。理由：

1. 设备自己签：签名证明是这台设备，别人不能凭空给设备加节点。
2. 后配对的 Host 也要认：Host 处理日志时还不认识签名者，就把声明原样暂存（每台设备留最新一条，总数有上限），等这台设备配对或经批准后用它的证书验签，通过才生效。否则之后每台新配对的 Host 走 App 内节点都要本机确认。
3. 旧 Host 把 kind 4 当未知类型跳过，之后不会补处理。

登记入口原来在 Hub（`POST /v1/me/devices/{device_id}/nodes`），2026-10-24 移除 Hub（D51）后没有入口，App 不再产生这类声明；格式和 Host 规则保留。

### D45 · SSH 用户证书（已废止）

Hub 当 SSH 用户 CA 的方案随 Hub 一起废止（D51），CA、签发接口、`tools/sshca` 和 App 里的证书页都已删除，原文在标签 `hub-last`。SSH 一直用设备公钥登录。

### D47 · Linux 被控第一版（M3）

Linux 电脑在已登录的图形会话里可被手机控制，平台层是新 crate `host-linux`。理由：

1. Wayland 下普通程序拿不到屏幕和输入，只能经 xdg-desktop-portal；RemoteDesktop 和 ScreenCast 放在同一个会话里，用户只授权一次，`restore_token` 让之后免弹窗。X11 会话直接用 XShm / XDamage / XTest。
2. FFmpeg 用 LGPL 动态库、运行时 dlopen：不把 GPL 代码链进来，构建期也不需要任何 -dev 包；没有可用硬件编码时退回 openh264（只有 H.264）。
3. 用 systemd user 单元而不是 system 服务：portal 和 PipeWire 都属于用户的图形会话，system 服务拿不到。代价是不支持登录界面和锁屏界面上的操作。
4. AppImage 一个包覆盖主流发行版；自动更新只校验 Ed25519 清单签名（Linux 没有平台代码签名）。
5. 第一版不做：登录界面、wlroots 上的输入注入（只能看）、没有 XWayland 时的剪贴板、Wayland 下的本机键盘操作检测、账户密码配对。

**D49 · 原生优先、免安装连接（2026-10-24）**：连接电脑先用系统自带的屏幕共享 / 远程桌面（内置 VNC / RDP 客户端，D48），电脑上不装东西；端口被拒（RS-E 205）引导用户打开系统共享，仍不行才引导安装 Host。Host 为兼容路径，提供系统协议没有的功能。没装 Host 的 tailnet 电脑经本机目录的 `others`（App 内 tailnet 节点，或 Tailscale / Headscale API 设备列表）列出；局域网 Mac 用 Bonjour `_rfb._tcp` 发现，局域网 Windows 由用户触发扫描。不考虑 Windows 家庭版。

### D46 · 电脑端控制端（2026-10-23）

- **为什么不用 WebCodecs**：WebView 合成至少多一帧；Windows 上 Tauri IPC 传二进制慢；HEVC 支持依赖平台；WKWebView 截不到系统快捷键。原生路径和 iOS 已达标的路径相同。
- **为什么不抽共享 crate**：rs-mobile 的 RsCore 已与移动端解耦（`Core` + `api_*` + `tables`），桌面端以 rlib 依赖即可。抽取要搬约 40 个文件，与在跑的移动端任务冲突，留到 rs-mobile 稳定后再做。
- **限制**：第一版用系统 Tailscale，电脑节点必须是不带标签的用户节点；装了 Remote Host（`tag:rs-host`）的电脑不能同时当控制端，等电脑端内嵌 tailnet 后解决。
- **密钥偏离**：没有安全芯片的 Mac 退回登录钥匙串里的软件 P-256；没有 TPM 的 Windows 退回软件 KSP（见 04）。

### D51 · 移除 Hub（2026-10-24）

**2026-10-24 移除 Hub：自用、零安装，不再部署 rs-hub；设备列表走本机 tailnet 和 Tailscale / Headscale API；Hub 代码保存在标签 `hub-last`。**

- **为什么**：这个项目自用，目标是零安装（D49）。会话本来就不经过 Hub（D50 已经证明不配 Hub 也能完整工作），Hub 只剩批准、吊销、唤醒、升级、SSH 证书、审计、同步这些增强，维护一台服务器和一套 Go 服务不值得。火山上的 rs-hub 已下线清理。
- **现在的做法**：设备目录由 App 本机合成（已配对 + 手动添加 + tailnet 节点 + Tailscale / Headscale API 设备列表）；作废在每台 Host 本机做；Host 手动更新；连接记录留在每台电脑本机；服务器、片段、网络等数据只存在手机本机。
- **废止的决策**：D4、D15、D23、D24、D30–D35、D41、D45、D50。D9、D13、D26、D34、D43 只保留 Host 侧的格式与规则。
- **后续清理（开源整理）**：删除了 `CandidateSource::Hub` 和只靠 Hub 数据的断开原因推断；`SignedStatement.hub_seq` 改成 `reserved`，序号只作为 Host 本机日志的元数据；不再接受空 `signer_fp` 的「下发」声明，四种声明都必须由本机已配对的设备签名（见 04 · 信任声明）；App 的 `features/hub` 改名 `features/data`。仍保留：`bound_nodes`、`Platform::send_wol` 和缩略图的平台实现、uniffi 接口里的 `hub_code` / `HubDisplay` 名称（改名要在 Mac 上重新生成绑定）。
- **兼容**：`--notify-uninstall` 作为隐藏的空操作参数保留（`--no-hub` 已在后续清理中删除）；旧配置里的 `[hub]`、手机设置里的 `hub.url`、`crash.upload`、桌面设置里的 `hub_url` 读到时忽略。
- **数据**：Hub 上同步过的数据不迁移，接受丢失。
- **恢复**：代码、接口定义、文档和部署脚本都在标签 `hub-last`。

## 与设计文档不一致的地方

「设计文档」指项目早期的产品与技术方案（未公开）。下表记录实现与它不同的地方，供对照旧讨论时参考。

| 设计文档 | 原写法 | 开发方案 | 原因 |
|---|---|---|---|
| §10 技术栈 · Host 更新 | Sparkle 2（macOS） | 自研更新器（D13） | 静默安装、回滚，两个平台一套逻辑 |
| §05 · Windows 编码 | 分别接 NVENC / AMF / QSV，其次 MF | FFmpeg 统一接三家，其次 MF（D12） | 工作量和维护成本 |
| §08 设备信任、§8.1 作废 | 未说明下发指令的信任链 | 批准新设备要由已配对设备签名；未签名的指令只能让 Host 更严格（D9） | 下发方被攻破不应等于所有电脑被攻破 |
| §10 · 仓库结构 | `core/ host/ app/ hub/ proto/` | Rust 统一放 `crates/`，见 [02](02-repo-env.md#目标结构) | crate 数量变多，集中管理 |
| §6.3 · 预热 | 「提前建立 QUIC 连接（只握手不推流）」 | 只做 TLS 握手，不打开控制流、不触发本机确认 | 预热不应在被控端产生会话记录或弹窗 |
| §4.7 · 证书指纹 | 未定义 | SPKI 的 SHA-256 | 重签证书不变指纹 |
