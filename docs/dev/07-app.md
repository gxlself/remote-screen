# 07 · App

对应设计文档 §09、§9.13（Android）、§9.14（无障碍），交互以画布为准。本文定：页面怎么拆、JS 和原生和 Rust 各管什么、接口长什么样。

## 分层

```
┌──────────────────────────────────────────────────────────────────────┐
│ TypeScript（app/src）                                                 │
│   页面 · 导航 · 文案 · 设置 · 设备目录 · 错误页 · 配对流程            │
│   状态：zustand（本地）+ TanStack Query（设备目录）+ MMKV（持久化）  │
├───────────────────────────┬──────────────────────────────────────────┤
│ RsCore（uniffi，TurboModule）│ 原生组件（Fabric）                      │
│   目录、配对、会话控制、    │   RemoteSessionView：解码上屏、手势、     │
│   键盘/文本、SSH 控制、     │     本地光标、放大镜、HUD 数据来源        │
│   诊断、日志、设置         │   TerminalView：SwiftTerm / terminal-view │
│                           │   CodeScanner：扫配对二维码               │
├───────────────────────────┴──────────────────────────────────────────┤
│ rs-mobile（Rust 静态库）：rsp-client · rs-ssh                         │
│   会话表 SessionId → Session；C ABI 给原生组件挂视频和终端          │
├──────────────────────────────────────────────────────────────────────┤
│ 平台：Secure Enclave / StrongBox 签名回调 · 网络接口 · （M2）libtailscale │
└──────────────────────────────────────────────────────────────────────┘
```

## 目录

```
app/
├─ src/
│  ├─ app/                 # 根组件、导航、主题、启动流程（App 锁、首次引导判断）
│  ├─ screens/             # 一个画板（或一组状态）一个页面组件，见下表
│  ├─ components/          # 设备卡片、路径标记、HUD、底部弹层、开关、按钮…（对照画布 Style）
│  ├─ features/
│  │  ├─ directory/        # 设备目录：已配对 + 手动添加 + tailnet 节点 + 本地缓存 + mDNS 合并、候选地址
│  │  ├─ session/          # 会话列表、会话控制 hooks、错误归类
│  │  ├─ pairing/          # 扫码解析、配对流程
│  │  ├─ ssh/              # 服务器、终端标签、命令片段、known_hosts 提示
│  │  ├─ network/          # 公司网络登记、VPN 判定
│  │  ├─ diagnostics/      # 连接测试、日志导出
│  │  └─ settings/
│  ├─ native/              # RsCore 生成的绑定 + 原生组件的 TS 声明
│  ├─ i18n/zh.ts           # 全部文案
│  └─ theme/tokens.ts      # 设计 token（画布 Style）
├─ ios/                    # RN iOS 工程；原生组件在 ios/RemoteScreen/Native/
├─ android/                # RN Android 工程；原生组件在 android/app/src/main/java/.../native/
└─ rust/                   # build-ios.sh / build-android.sh / uniffi 配置
```

### 设计 token（`theme/tokens.ts`）

| token | 值 | token | 值 |
|---|---|---|---|
| `bg` | `#0D1114` | `text` | `#ECF0F2` |
| `card` | `#151A1E` | `text2` | `#A9B4BB` |
| `cardLine` | `#232B31` | `text3` | `#86929A` |
| `raised` | `#1C2328` | `accent` | `#7DA8FF`（其上文字 `#0A1630`） |
| `selected` | `#2E3840` | `good` | `#5DD39A` |
| `relay` | `#F4B24E` | `bad` | `#FF8873` |
| `hostControlled` | `#FF9F43` | `winAccent` | `#4CC2FF` |

字体 Geist / Geist Mono（打包进 App）。路径标记：直连 = 绿色圆点；中继 = 琥珀色菱形；内网 / 局域网 = 蓝色 7 px 圆角方块；离线 = 虚线框（形状 + 颜色 + 文字三重区分，§9.14）。

## 页面与画板对照

路由名就是页面组件名。「里程碑」指这个页面第一次要做完的版本。

### 手机 · 主流程

| 画板 | 路由 / 组件 | 里程碑 | 数据来源 |
|---|---|---|---|
| `Onboard` | `OnboardTailnet` | M1 | `RsCore.network.status()`：有无 100.x 地址 |
| `Onboard2` | `OnboardDiscover` | M1 | 本机目录（已配对、tailnet 设备） |
| `Onboard3` | `OnboardProtect` | M1 | App 锁设置 |
| `Main` | `Home`（底部标签「设备」） | M1 | 目录 + 会话列表 + 预热状态 |
| `DeviceSheet` | `DeviceSheet`（底部弹层） | M1 | 目录条目 + 探测结果 |
| `Pair` | `PairAdd`（扫码 / 输入码两个标签） | M1 | `CodeScanner` |
| `PairWaiting` | `PairWaiting` | M1 | 配对事件 |
| `PairDone` | `PairDone` | M1 | — |
| — | `PairPassword`（用电脑的账户密码配对） | M2 | `pairing.startWithPassword`；需要先有 Host 的指纹，移除 Hub 后没有可信来源，入口隐藏 |
| — | `PairFile`（导入配对文件） | M2 | `pairing.loadFile` → `PairWaiting({fileId})` |
| `MainVPN` | `Home` 的 VPN 横幅状态 | M1 | 网络判定（本文 · 网络判定） |
| `AddByAddress` | `AddByAddress` | M1 | 本机存储 |
| `DeviceRoutes` | `DeviceRoutes` | M1 | 候选地址 + 每路最近一次结果 |
| `AddServer` | `AddServer` | M1 | 本机存储 |
| `Sessions` | `Sessions`（底部标签「会话」） | M1 | `RsCore.sessions.list()` + SSH 会话 |
| `Terminal` | `Terminal` | M1 | `TerminalView` |
| `TerminalKeyboard` | `Terminal` 键盘弹出状态 | M1 | — |
| `TerminalCheck` | `RootConfirm`（弹窗） | M1 | Tailscale SSH check 模式的认证地址 |
| `Files` | `Files`（底部标签「文件」） | M2 | SFTP + 文件通道 |
| `Settings` | `Settings`（底部标签「设置」） | M1 | 各处 |
| `SSHKeys` | `SshKeys` | M1 | `RsCore.ssh.keys()` |
| `AppLock` | `AppLock` | M1 | 设置 |
| `Diagnostics` | `Diagnostics` | M1 | `RsCore.diag.probe()`、日志导出 |
| `A11y` | 不是单独页面：`Home` 在最大字号 + 读屏下的验收样张 | M2 | — |

### 手机 · 远控会话（横屏）

全部是 `Session` 路由内的状态或浮层，画面本身是 `RemoteSessionView`。

| 画板 | 组件 / 状态 | 里程碑 |
|---|---|---|
| `SessionConnecting` | `phase = connecting`：显示正在试的路和进度 | M1 |
| `Session` | 触控板模式，工具栏收起 / 展开 | M1 |
| `SessionKeyboard` / `SessionKeyboardWin` | `KeyboardBar`（修饰键栏，Mac / Windows 两套布局）+ 系统键盘 | M1 / M2 |
| `SessionQuality` | `QualitySheet`：自动 / 清晰 / 流畅 / 省流量 + HUD 开关 | M1 |
| `SessionDisplays` | `DisplaySheet`；M3：Host 有 `VIRTUAL_DISPLAY` 且自己是控制者时，面板底部出现「按本机屏幕新建虚拟屏」。点了以后等结果（最多 15 秒），成功就切过去，失败就在面板里提示。虚拟屏在列表里标「虚拟屏」；自己建的那块带「移除」。 | M1 |
| `SessionClipboard` | `ClipboardSheet` | M1 |
| `SessionMore` | `MoreSheet`：锁屏、结束后锁屏、Ctrl+Alt+Del（Win）、断开 | M1 |
| `SessionCoach` | `GestureCoach`：首次进入会话时 | M1 |
| `SessionLocalUse` | `input_paused_local = true` 时的顶部条 | M1 |
| `SessionIdle` | 收到 `IdleWarning` | M1 |
| `SessionShared` | `role = VIEWER`，`view` / `taken` 两态 | M1 |
| `SessionRelay` | `path = TAILNET_DERP` 的 HUD 与降档提示 | M1 |
| `SessionTCP` | `transport = TCP` 的 HUD | M1 |
| `SessionReconnect` | `phase = reconnecting` | M1 |
| `SessionLoginWindow` | `RS-E190` / `RS-E191`，两态；M1 只做「交给系统屏幕共享」和说明，内置登录窗口采集在 M3 | M1 |
| `AdapterSession` | 内置 VNC / RDP 会话，凭据与证书提示由 `RsCore.adapters` 驱动 | M3 |
| `SessionError`、`Err*` | `SessionEnded` 页，按错误码渲染，见本文 · 错误页 | M1 |

### iPad、Android

| 画板 | 组件 | 里程碑 |
|---|---|---|
| `iPadHome` | `Home` 的宽屏布局（两栏） | M1 基础布局 |
| `iPadSession` | 触屏模式 + 放大镜 | M1 |
| `iPadSplit` | 分屏时的会话（不锁指针） | M2 |
| `AndroidMain` | `Home`（Material 3 组件变体） | M2 |
| `AndroidVPN` | `Home` 的 VPN 冲突横幅 + 底部说明面板 | M2 |
| `AndroidSession` | `Session`（手势排除区 + 返回键处理） | M2 |
| `AndroidNotify` | 前台服务通知 + 快捷设置磁贴（原生，不是 RN 页面） | M2 |

## 导航

```
RootStack
├─ Onboarding（OnboardTailnet → OnboardDiscover → OnboardProtect）     首次启动
├─ AppLockGate                                                          需要时覆盖在最上层
├─ MainTabs
│  ├─ 设备：Home → DeviceSheet（弹层）→ DeviceRoutes
│  ├─ 会话：Sessions
│  ├─ 文件：Files（M2）
│  └─ 设置：Settings → SshKeys / AppLock / Diagnostics / Networks / AddByAddress / AddServer
├─ Pair 流程（PairAdd → PairWaiting → PairDone）                       模态
│  └─ M2：PairPassword / PairFile                                       同一模态组
├─ Session（横屏，全屏，隐藏状态栏）                                    模态
├─ Terminal                                                             普通推入
└─ SessionEnded（错误页，横屏）
```

会话页强制横屏（iPad 跟随设备方向）；退出会话恢复竖屏。

导航器外的全局组件（`IncomingPairLinks`）挂在 `AppRoot.tsx` 的 `<NavigationContainer>` 里、`<RootNavigator />` 之后，用 `navigationRef`（`app/src/app/navigation/navigationRef.ts`）跳转，跳转前先判 `navigationRef.isReady()`。

M2 路由参数：

| 路由 | 参数 |
|---|---|
| `PairPassword` | `{ hostId: string }`（密码不进路由参数） |
| `PairFile` | `{ text?: string; uri?: string }`，可省略 |
| `PairWaiting` | `{ qrText }` / `{ addr, port?, code }` / `{ fileId }`（M2 加 `fileId`） |

## 状态与数据

| 数据 | 放哪 | 说明 |
|---|---|---|
| 设备目录 | TanStack Query `['directory']`，本机合成：已配对主机 + 手动电脑 + tailnet 节点 +（填了 token 时）Tailscale / Headscale API 的设备列表；`features/hub/localDirectory.ts` 的 `buildLocalDirectory` | 在线状态靠探测：首页可见时每 30 秒一轮，同时最多 8 个，单个上限 1.5 秒，结果缓存在 MMKV；没探测过的显示上次结果 |
| 会话列表 | zustand `sessionsStore`，由 RsCore 事件驱动 | |
| 网络状态 | zustand `networkStore`，原生事件驱动 | |
| 设置 | MMKV + zustand | 需要 Rust 知道的（tailnet 模式、画质默认值）同步给 `RsCore.settings.set()` |
| 秘密 | 不在 JS | 设备密钥、SSH 私钥、密码只在原生和 Rust。**例外 2**：内置 VNC / RDP 的账户与密码经 `adapters.submitCredentials` 一次性传给 Rust；已保存密码只在原生安全存储与 Rust 之间流动。**例外**：电脑的系统账户密码（`PairPassword` 页）经 JS 输入框一次性传给原生（`pairing.startWithPassword`），不进路由参数、状态库、MMKV、日志，提交后立刻清空输入框和局部变量。配对文件的口令不例外：只留在 Rust（`loadFile` 按 `fileId` 缓存 10 分钟），JS 只拿摘要 |

## RsCore（uniffi 控制面）

TS 侧形状（由 uniffi-bindgen-react-native 从 Rust 生成，下面是约定的接口，不是生成结果）：

```ts
// 启动
RsCore.init(cfg: { dataDir: string; appVersion: string; platform: 'ios'|'ipados'|'android'; filesDir?: string }): Promise<void>
RsCore.events.subscribe(cb: (e: CoreEvent) => void): Unsubscribe   // 会话、配对、网络、SSH 等事件统一从这里来

// 身份
RsCore.identity.ensure(deviceName: string): Promise<{ deviceId: string; fp: string }>   // 首次生成密钥和证书（经原生签名回调）

// 设备目录与「同步」数据
// 没有 RsCore 方法：目录由 App 用 hosts.paired()、tailnet.status() 和 Tailscale / Headscale API 设备列表合成；
// 服务器、片段、网络、手动电脑这些「同步」命名空间只读写本机 KV（MMKV `sync.local.<ns>`），不跨设备同步。

// 配对
RsCore.pairing.parseQr(text: string): PairTarget | null
RsCore.pairing.start(target: PairTarget | { addr: string; port?: number; code: string } | { fileId: string }): Promise<PairOutcome>   // 过程中发 pairing 事件；addr 是按地址 + 配对码配对（04 · 按地址 + 配对码）；fileId 来自 loadFile（M2）
RsCore.pairing.startWithPassword(hostId: string, account: string, password: string): Promise<PairOutcome>   // M2
RsCore.pairing.loadFile(src: { text: string } | { uri: string }): Promise<PairFileSummary>   // 口令留在 Rust，按 fileId 缓存 10 分钟（M2）
RsCore.pairing.cancel(): void                                         // 也取消密码配对和文件配对
RsCore.hosts.paired(): Promise<PairedHost[]>
RsCore.hosts.forget(hostId: string): Promise<void>

// 会话（控制面）
RsCore.sessions.prewarm(hostIds: string[]): void
RsCore.sessions.open(hostId: string, opts: OpenOpts): Promise<SessionId>   // 多路径拨号在 Rust 里做
RsCore.sessions.close(id: SessionId): void
RsCore.sessions.list(): SessionSummary[]
RsCore.sessions.setQuality(id, q: 'auto'|'clear'|'smooth'|'eco'): void
RsCore.sessions.setAudio(id: SessionId, enabled: boolean): void   // Host 不支持声音时静默忽略
RsCore.sessions.selectDisplay(id, displayId: number): void
RsCore.sessions.createVirtualDisplay(id, opts: VirtualDisplayOpts): number   // 返回 requestId；结果经 session.vdisplay 事件（M3）
RsCore.sessions.releaseVirtualDisplay(id, displayId: number): void           // 移除本会话建的虚拟屏（M3）
RsCore.sessions.requestControl(id): void
RsCore.sessions.respondControl(id: SessionId, requesterDeviceId: string, allow: boolean): void
RsCore.sessions.action(id, kind: 'lock'|'ctrlAltDel'|'lockOnEnd', enable?: boolean): void
RsCore.sessions.key(id, usage: number, down: boolean, mods: number): void
RsCore.sessions.text(id, text: string): void
RsCore.sessions.combo(id, combo: 'cmdTab'|'cmdSpace'|'missionControl'|'altTab'|'winKey'|...): void
RsCore.sessions.clipboardSend(id, text: string): void
RsCore.sessions.background(id, inBackground: boolean): void

// 诊断
RsCore.diag.probe(hostId: string): Promise<ProbeId>     // 结果以事件逐条返回（每条路：结果、耗时、原因）
RsCore.diag.probeAddress(addr: string, port?: number): Promise<ProbeId>  // 未指定端口时使用 rsp_core::DEFAULT_PORT = 7400
RsCore.logs.export(opts: { mask: boolean }): Promise<string /* zip 路径 */>
RsCore.logs.write(level, tag, msg): void                // JS 日志也写进同一套文件
RsCore.crash?.recordJs(report: JsCrashReport): boolean   // `crash` 可选；native crashRecordJs；false 表示拒收或落盘失败

// M3 内置 VNC / RDP（可选命名空间；适配器会话沿用 sessions 的输入与 close）
RsCore.adapters?.available(): boolean
RsCore.adapters?.open(target: AdapterTarget, opts: AdapterOpenOpts): Promise<SessionId>
RsCore.adapters?.close/list/submitCredentials/cancelCredentials/saved/forget/canSave/typePassword/sendCtrlAltDel/trustCert/resize/selectScreen

// SSH
RsCore.ssh.servers(): Promise<SshServer[]>
RsCore.ssh.open(serverId: string, opts?): Promise<SshId>    // 终端字节经 C ABI 给 TerminalView，不经 JS
RsCore.ssh.close(id: SshId): void
RsCore.ssh.keys(): Promise<SshKey[]>; RsCore.ssh.createKey(kind: 'se-p256'|'ed25519', name): Promise<SshKey>
RsCore.ssh.installKey(serverId, keyId): Promise<void>
RsCore.ssh.trustHostKey(serverId, fp: string): Promise<void>
RsCore.ssh.trustedHostKeys(): Promise<TrustedHostKey[]>
RsCore.ssh.forgetHostKey(serverId: string): Promise<void>
RsCore.files.list(hostId: string, path: string | null): Promise<FileListing>
RsCore.files.download(hostId: string, remotePath: string): Promise<TransferId>
RsCore.files.upload(hostId: string, localPath: string, remoteDir: string | null): Promise<TransferId>
RsCore.sftp.list(serverId: string, path: string | null): Promise<FileListing> // 无需先开终端
RsCore.sftp.download(serverId: string, remotePath: string): Promise<TransferId>
RsCore.sftp.upload(serverId: string, localPath: string, remoteDir: string | null): Promise<TransferId>
RsCore.transfers.list(): FileTransfer[]
RsCore.transfers.pause(id: TransferId): void
RsCore.transfers.resume(id: TransferId): Promise<void>
RsCore.transfers.cancel(id: TransferId): void
RsCore.transfers.remove(id: TransferId): void // 只删已结束的记录，不删已下载的文件

// App 内 tailnet（D42）
RsCore.tailnet.available(): Promise<boolean>
RsCore.tailnet.status(): Promise<TailnetStatus>
RsCore.tailnet.login(): Promise<void>
RsCore.tailnet.logout(): Promise<void>

// 网络与设置
RsCore.network.status(): Promise<NetworkStatus>
RsCore.settings.set(key: string, value: unknown): void
```

`crash` 是可选字段（`crash?:`）：未实现该能力的 Core（当前为 `FakeCore`）没有这个字段；真实 `createRsCore` 总会提供该对象，旧原生包缺少 uniffi 绑定时其方法抛 `RsCoreError('unsupported')`，不是字段缺失。`JsCrashReport` 的参数为 `{name: string | null, message: string, stack: string | null, isFatal: boolean, osVersion: string | null}`。原生崩溃也写入同一组崩溃报告：iOS 订阅 MetricKit（`app/ios/RemoteScreen/Native/Crash/CrashNativeCollector.swift`），把崩溃诊断按 `app/ios/NativeLogic/Sources/NativeLogic/Crash/CrashInbox.swift` 的格式写成 `<incident>-metrickit-<index>.json`；Android 11（API 30）及以上用 `ApplicationExitInfo`（`NativeCrashCollector.kt`），只取 7 天内的 crash / native crash / ANR，每次最多 5 条，写成 `<incident>-exitinfo-<pid>.json`。两者都落在 `crashes/native/`，各保留最近 10 个文件。Core 启动时 `ingest_at_start`（`crates/rs-mobile/src/rscore/crash_native.rs`）把最多 20 个有效文件转成 `kind: "nativeCrash"` 的普通报告，导入成功后删除收件箱文件；坏文件、超过 256 KiB 或超过 7 天的文件也删除。原生崩溃与 JS 崩溃、Rust panic 共用同一个 `crashes/` 目录；报告只留在本机，不上传，过期文件会清理。

**启动**：
- `dataDir` 为 `<Application Support>/RemoteScreen/core` 的绝对路径，由原生建好后经 `initialProperties` 传给 JS，不进 iCloud / iTunes 备份（见 04 · 数据存放一览）。
- 真实 Core 先 `init` 完成，页面才能用；设置的首次 `settings.set` 在 `init` 之后发。
- `sessions.open` 的 `opts.maxFps`（可选）是本机屏幕最高刷新率，Rust 用作 `ClientHello.caps.max_fps`，夹到 1–240，省略时为 60。
- `sessions.open` 的 `opts.maxWidth?: number` 是期望的最大编码宽度，Rust 用作 `ClientHello.caps.max_width`，夹到 640–7680，省略时 1920；`opts.codec?: 'hevc' | 'h264'` 是唯一的解码能力（`caps.decode`），省略时 HEVC。手机 App 不传；电脑端按屏幕宽度和硬解能力传。
- `sessions.open` 的 `OpenOpts.audio?: boolean` 在握手时请求远端声音；不传 = 不请求。
- `sessions.setAudio` 在会话中切换远端声音；它不加入 `GAP_METHODS`。当前适配层桩抛 `RsCoreError('unsupported', 'sessions.setAudio')`，uniffi 桩返回同名 unsupported，由 AU2E / AU2G 接线；`RsMobileBindings.sessionsSetAudio?` 可选，iOS 生成绑定由 AU2H 在 Mac 上重建。
- iOS 声音会话用 `.playback` + `.mixWithOthers`，不做后台播放；控制者和旁观者都收声音。
- `settings.set('device.lowPower', boolean)` 由 App 按电量推送（电量 < 20% 或低电量模式），不是用户设置、不进下方设置表；Rust 收到后给所有活动会话的 `Feedback.low_power` 置位，新会话用当前值。
- `settings.set('device.appActive', boolean)` 由 App 在 AppState 变为 active / background 时推送（inactive 忽略），默认 true；它是运行时提示，不进下方设置表，Rust 据此决定 App 内节点何时停。
- `tailnet.available/status/login/logout` 不加入 `GAP_METHODS`。本构建是否带 App 内 tailnet 由 `available()` 返回；绑定缺失（旧原生包）时 `available()` resolve(false)，其余方法拒绝为 `RsCoreError('unsupported')`。有绑定但能力不可用时 `status()` 的 state 为 `unavailable`。节点已启动且 needsLogin 时 `login()` 重新发起交互登录，新的 authUrl 经 `tailnet.state` 到达；`logout()` 登出 App 内节点，之后状态为 needsLogin。Headscale 地址由用户填（`tailnet.controlUrl`，Rust 规范化）；OIDC 交互登录，authUrl 用系统浏览器 `platform.openUrl` 打开（D42）。

上述补登的 6 个方法（`sessions.respondControl`、`diag.probeAddress`、`ssh.trustedHostKeys`、`ssh.forgetHostKey`、`network.status`、`settings.set`）已进入 TS 接口契约；原生适配层未提供能力时须抛 `RsCoreError('unsupported')`，页面捕获后隐藏相应入口，不能伪报成功（原生适配实现待跟进）。`respondControl` 的 requesterDeviceId 对应当前 `session.controlRequest.requester.deviceId`；`probeAddress` 的结果仍经 `probe.result` 返回，`hostId` 可为 null；不带端口时使用 RSP 默认端口 `rsp_core::DEFAULT_PORT = 7400`。`forgetHostKey` 删除指定服务器已信任的 SSH 主机密钥，不能顺便删除服务器配置。`settings.set` 的键与值域见下方 U1E 设置表。

声音接收契约：`rsp-client::audio::AudioSink` 同步接收解析后的 0x11 包，`push(seq, capture_ts_us, payload, recv_us)` 不得阻塞超过 1 ms，`reset()` 清空新连接的缓冲。`SessionParams.audio` 默认 false，`audio_sink` 默认 None；None 时不声明 `AUDIO_OPUS`。AU2D 实现以下 `SessionHandle` 方法，本次只定签名、不放桩：

```rust
/// 会话中开关远端声音。Host 没声明 AUDIO_OPUS 时静默忽略。
pub fn set_audio(&self, enabled: bool);
```

**新配对方式（M2）**：

- `loadFile` 失败：内容不是配对文件 → `RsCoreError('internal', 'not a pairing file')`（message 固定，页面据此显示「这不是 Remote Screen 的配对文件」）；`uri` 不是 `file://`（如 Android `content://`）→ `RsCoreError('unsupported')`。已过期的文件照常返回，`expired: true`。
- 密码 / 文件配对时 rsp-client 把 `PAIR_PASSWORD` / `PAIR_FILE` 并进 `required_features`，Host 太旧以 170 结束。页面另外用已知的 Host 功能位提前判断（`hasHostFeature`），不支持就不显示入口。
- uniffi 导出（名字即契约）。生成的 `react-native-rs-core` 在 Mac 上重新生成之前，`bindings.ts` 里这几个是可选成员；缺失时 `createRsCore` 抛 `RsCoreError('unsupported')`：

| Rust（`#[uniffi::export]`） | bindings.ts | 返回 JSON |
|---|---|---|
| `pairing_start_password(host_id, account, password)` | `pairingStartPassword(hostId, account, password)` | `PairOutcome` |
| `pairing_load_file(src_json)` | `pairingLoadFile(srcJson)` | `PairFileSummary` |
| `pairing_start(arg_json)`（已有） | 不变 | `arg_json` 多一种 `{"fileId":"…"}` |

内置 VNC / RDP 的导出（旧原生包缺绑定时由 `createRsCore` 抛 `RsCoreError('unsupported')`）：

| Rust（`#[uniffi::export]`） | bindings.ts | 返回 |
|---|---|---|
| `adapters_open(target_json, opts_json)` | `adaptersOpen(targetJson, optsJson)` | `SessionId` |
| `adapters_close(id)` | `adaptersClose(id)` | — |
| `adapters_list()` | `adaptersList()` | `AdapterSummary[]` JSON |
| `adapters_submit_credentials(id, user, password, domain, save)` | `adaptersSubmitCredentials(id, user, password, domain, save)` | — |
| `adapters_cancel_credentials(id)` | `adaptersCancelCredentials(id)` | — |
| `adapters_saved(target_json)` | `adaptersSaved(targetJson)` | `SavedCredentialInfo` JSON |
| `adapters_forget(target_json)` | `adaptersForget(targetJson)` | — |
| `adapters_can_save()` | `adaptersCanSave()` | `boolean` |
| `adapters_type_password(id)` | `adaptersTypePassword(id)` | — |
| `adapters_send_ctrl_alt_del(id)` | `adaptersSendCtrlAltDel(id)` | — |
| `adapters_trust_cert(id, accept)` | `adaptersTrustCert(id, accept)` | — |
| `adapters_resize(id, view_width, view_height, scale)` | `adaptersResize(id, viewWidth, viewHeight, scale)` | — |
| `adapters_select_screen(id, index)` | `adaptersSelectScreen(id, index)` | — |

```ts
type PairFileSummary = {
  fileId: string; hostId: string; hostName: string; fp: string;
  addrs: string[]; expiresAt: Micros; expired: boolean;
};
// Host 功能位（与 rsp-core features.rs 同值，03 §11）
const HOST_FEATURE_PAIR_PASSWORD = 2 ** 19;
const HOST_FEATURE_PAIR_FILE = 2 ** 20;
function hasHostFeature(features: number, bit: number): boolean;  // Math.floor(features / bit) % 2，不用位运算
// PairOutcome：'locked' 改为 { status: 'locked'; retryAfterS?: number }；新增 { status: 'methodDisabled' }
```

会话与补登方法使用的 TS 类型：

```ts
type Micros = number;  // Unix 微秒；原生 u64 经适配层转换
type SessionId = string;  // 原生 u64 ID 转十进制字符串
type NetPath = 'tailnetDirect' | 'tailnetDerp' | 'intranet' | 'lan' | 'manual';
type SessionPhase = 'connecting' | 'authenticating' | 'pendingApproval'
  | 'streaming' | 'reconnecting' | 'ended';
type DialAttempt = {
  path: NetPath;
  addr: string;
  state: 'trying' | 'ok' | 'failed' | 'skipped';
  ms?: number;
  reason?: string;
};
type Participant = {
  deviceId: string; deviceName: string; user: string;
  path: NetPath | null; since: Micros;
};
type TrustedHostKey = {
  serverId: string; keyType: string; fp: string; addedAt: Micros;
};
type SshServer = {
  serverId: string; name: string; host: string; port: number; user: string;
  jump: string | null;
  auth: 'tailscale' | 'deviceKey' | 'password';
  tailscaleSsh: boolean | null;
  os?: string;
  source: 'directory' | 'manual';
};
type TailnetNodeState =
  | 'noState' | 'needsLogin' | 'needsMachineAuth' | 'stopped'
  | 'starting' | 'running' | 'unknown' | 'unavailable';
type TailnetRoute = 'system' | 'embedded';
type EmbeddedNodeSummary = {
  state: TailnetNodeState;
  hostname: string;
  ips: string[];
  derpRegion: string;
  derpRegionName: string;
};
type TailnetRouteStatus = {
  mode: 'system' | 'embedded' | 'auto';
  active: TailnetRoute;
  available: boolean;
  controlUrlSet: boolean;
  embedded: EmbeddedNodeSummary | null;
};
// RS-T1D tailnet_status() 的 JSON（字段名同 RS-T1A「状态 JSON」，state 为 camelCase）
type TailnetSelf = {
  hostname: string; dnsName: string; ips: string[]; online: boolean;
  derpRegion: string; derpRegionName: string;
};
type TailnetPeer = {
  hostname: string; dnsName: string; ips: string[]; os: string; online: boolean;
  tags: string[]; direct: boolean; curAddr: string; relay: string; peerRelay: string;
  lastHandshakeUs: number; rxBytes: number; txBytes: number;
};
type TailnetStatus = {
  state: TailnetNodeState;
  authUrl: string | null;
  self: TailnetSelf | null;
  magicDnsSuffix: string;
  peers: TailnetPeer[];
};
type NetworkStatus = {
  ifaces: { name: string; addrs: string[]; isVpn: boolean; workProfile?: boolean }[];
  tailnet?: TailnetRouteStatus;
};
```

`CoreEvent` 是带 `type` 字段的联合类型：`session.state`、`session.role`、`session.stats`（2 Hz）、`session.error`、`session.displays`、`session.vdisplay`、`session.clipboard`、`session.idleWarning`、`session.controlRequest`、`pairing.progress`、`pairing.done`、`probe.result`、`ssh.state`、`ssh.hostKeyPrompt`、`ssh.authUrl`（Tailscale SSH check 模式）、`tailnet.state`、`adapter.state`、`adapter.certPrompt`、`adapter.clipboard`、`adapter.clipboardOther`、`adapter.screens`、`network.changed`。

```ts
// 下列分支属于 CoreEvent 联合类型
type SessionStateEvent = {
  type: 'session.state'; sessionId: SessionId; hostId: string; phase: SessionPhase;
  path?: NetPath; transport?: 'quic' | 'tcp'; derpRegion?: string;
  dial?: DialAttempt[]; approvalTimeoutS?: number; resumed?: boolean;
  reconnectReason?: 'pathLost' | 'hostUpdating' | 'resume';
};
type IdleWarningEvent = {
  type: 'session.idleWarning'; sessionId: SessionId;
  secondsLeft: number; lockOnEnd: boolean;
};
type ControlRequestEvent = {
  type: 'session.controlRequest'; sessionId: SessionId;
  requester: Participant; timeoutS: number;
};
type TailnetStateEvent = {
  type: 'tailnet.state';
  state: TailnetNodeState;
  /** needsLogin 时的登录地址；只用来打开浏览器，不得写日志、不得进诊断导出 */
  authUrl: string | null;
  ips: string[];
  derpRegion: string;
  message: string | null;
};
type AdapterStateEvent = {
  type: 'adapter.state'; sessionId: SessionId; protocol: AdapterProtocol;
  phase: AdapterPhase; credFields?: AdapterCredField[]; credError: boolean;
  width?: number; height?: number; serverName?: string;
  error?: AdapterErrorInfo;
};
type AdapterCertPromptEvent = {
  type: 'adapter.certPrompt'; sessionId: SessionId; fp: string; oldFp?: string;
};
type AdapterClipboardEvent = {
  type: 'adapter.clipboard'; sessionId: SessionId; text: string;
};
/** 电脑上复制了文字以外的内容（截图、文件）；内容不传过来。 */
type AdapterClipboardOtherEvent = {
  type: 'adapter.clipboardOther'; sessionId: SessionId;
};
type AdapterScreensEvent = {
  type: 'adapter.screens'; sessionId: SessionId; screens: AdapterScreen[];
};
```

等待确认显示 `pendingApproval` 和 `approvalTimeoutS`；正在拨号的路径列表使用 `dial`，不能用缺失的 path 字段代替尝试结果。`session.idleWarning` 驱动空闲倒计时，`session.controlRequest` 驱动接管确认。网络变更为 `{type:'network.changed', status:NetworkStatus}`。

`tailnet.state` 的 authUrl 只用来打开系统浏览器，不进日志、不进诊断导出；`TailnetStatus.authUrl` 遵循同一规则。网络状态里的 App 内节点摘要 `EmbeddedNodeSummary` 不含 authUrl；摘要变化时 Rust 照常发 `network.changed`，页面不需要另听事件就能拿到路由状态。`NetworkStatus.tailnet` 未提供时省略该键；节点没启动时 `TailnetRouteStatus.embedded` 为 null，已启动（含 starting / needsLogin）时给出摘要。

### U1E 设置键与值域

以 `app/src/features/settings/schema.ts`、`store.ts` 及 `app/src/native/rscore/types.ts` 为准。设置均由 App 以 `settings.<key>` 为 MMKV 键保存，并进入 zustand；`RsCore.settings.set(key, value)` 传原始 key，不带 `settings.` 前缀。当前仅 `sync: true` 的七项在 store 初始化和修改时发送到原生；表中「同步原生」即此标记，不表示跨设备同步。原生不支持时，store 保留本地值并只告警一次（原生设置应用实现待跟进）。

| 键 | 值域 | 默认值 | 同步原生 |
|---|---|---|---|
| `quality.default` | `"auto" \| "clear" \| "smooth" \| "eco"`（Quality） | `"auto"` | 是 |
| `session.mode.phone` | `"trackpad" \| "touch"`（SessionMode） | `"trackpad"` | 否 |
| `session.mode.tablet` | `"trackpad" \| "touch"`（SessionMode） | `"touch"` | 否 |
| `session.hud` | boolean | `false` | 否 |
| `session.coachShown` | boolean | `false` | 否 |
| `tailnet.mode` | `"system" \| "embedded" \| "auto"`（TailnetMode） | `"system"` | 是 |
| `tailnet.controlUrl` | string \| null（http(s) URL，≤ 512） | null | 是 |
| `prewarm.enabled` | boolean | `true` | 是 |
| `lock.onOpen` | boolean | `true` | 否 |
| `lock.beforeSession` | boolean | `false` | 否 |
| `lock.sensitiveHosts` | string[]（host_id 列表） | `[]` | 否 |
| `networks` | NetworkEntry[]，元素为 `{id: string, name: string, cidrs: string[], vpnPools: string[]}` | `[]` | 是（Rust 据此算内网网段和 VPN 地址池） |
| `recording.maxTotalMb` | `1024 \| 2048 \| 4096 \| 8192` | `4096` | 是 |
| `recording.retentionDays` | `7 \| 30 \| 90 \| null` | `30` | 是 |
| `onboarding.done` | boolean | `false` | 否 |
| `notify.enabled` | boolean | `false` | 否 |
| `session.cursorSpeed` | number，U1E schema 未规定上下界 | `1.4` | 否 |
| `session.naturalScroll` | boolean | `true` | 否 |
| `session.haptics` | boolean | `true` | 否 |
| `session.sound` | boolean | `false` | 否（全局设置，不同步）：打开会话时是否请求声音；会话中「更多」里也能切换 |
| `net.preferLocal` | boolean | `true` | 否 |
| `net.warnVpnConflict` | boolean | `true` | 否 |
| `terminal.fontSize` | number，U1E schema 未规定上下界 | `13` | 否 |
| `terminal.tmux` | boolean | `true` | 否 |
| `hosts.prefs` | `Record<string, Partial<HostPrefs>>`，以 host_id 为键 | `{}` | 否 |
| `clipboard.autoSync` | boolean | `true` | 否 |

`HostPrefs` 为 `{routePolicy: "auto" | "tailnet" | "intranet", tcpFallback: boolean, autoUpdateAddr: boolean, lockOnEnd: boolean}`；每台 Host 允许只保存部分覆盖值，读取时分别合并默认值 `"auto"`、`true`、`true`、`false`。值域按 TS 类型定义；当前 store 读盘主要校验基础类型，不能把宽松的运行时校验当作更大的契约值域。

### 设备详情

App 设备详情中的 Host / 设备指纹展示统一引用 [04 · 标识](04-identity-security.md#标识)，底层仍使用完整 SPKI 指纹做身份校验（显示层实现待跟进）。

### SSH

连接 `deviceKey` 类服务器时用设备公钥登录；公钥由 App 通过现有会话装进 `authorized_keys`，行里带 `rs-device=<device_id>` 标记。没有 SSH 用户证书，也没有服务端清理公钥用的维护密钥。

### SFTP 浏览与传输

浏览使用已认证 SSH 连接上的 russh-sftp `SftpSession`，保留默认流水线；列表目录在前，再按名称不区分大小写排序，同名比较回退到原始名称。浏览错误区分 `Unavailable`、`NotFound`、`PermissionDenied`、`AlreadyExists`、`NotEmpty`、`Failure`、`Protocol` 和 SSH 连接错误。

chunk 默认 **32 KiB（32768 字节）**。开始传输时记录源文件 size 与 mtime；续传前，下载须重新读取远端源文件的 size / mtime 与开始时记录比对，上传须比对本地源文件的 size / mtime。任一不一致都从 0 重传，不沿用旧的半截文件或进度。

一致性检查通过后，下载保持 russh-sftp 默认流水线，续传点为本地半截文件长度。上传显式在途窗口 `W=16`；设一次请求块长为 chunk、远端半截长度为 L，续传起点为 `floor(max(0, L - W × chunk) / chunk) × chunk`，回退覆盖可能尚未确认的窗口。进度报告连续已确认前缀，不能把乱序确认的最大 offset 当作进度。Rust 侧 `crates/rs-ssh/src/sftp_transfer.rs` 已提供传输与一致性检查，App / RsCore 传输接线实现待跟进。

## 会话录制（M3 V5）

**做什么（08 M3「会话录制：本地录制，加密保存」）**
- App（iOS 和 Android）在 Rust 层录下当前会话**收到的**数据：视频帧（原样保存 Annex-B 的 H.265/H.264 NAL，带 frame_id、epoch、关键帧标记和时间戳）、StreamConfig、光标位置和形状、远端声音包（Opus）。这些数据按块写进自有容器 `.rsrec`。
- 每份录制生成一把随机的 32 字节文件密钥，用 AES-256-GCM 逐块加密。文件密钥交给原生「密钥包裹」回调（新 C ABI，见 C4），用系统钥匙串或 Android Keystore 里的主密钥包裹后，存进文件头。Rust 不保存明文密钥。如果原生回调不可用，就拒绝录制，绝不退回明文。
- 管理：录制列表、删除、总量上限、保留期清理、崩溃后恢复最后一块。
- 回放：把录下的帧当作一个「假会话」，重新喂给现有的解码和显示路径：
  - 视频：`rs_session_attach` 遇到回放 id 时转给录制模块（C9）；
  - 声音：在 `AudioTable` 里按回放 id 登记；
  - 光标：照常回放。
  回放页复用 `RemoteSessionView`。
- 导出 MP4（第二批）：只有用户点「分享」时才解密并封装成一个临时 MP4，交给系统分享面板，用完删除。封装器自己写在 `rs-recording` 里，**不加新 crate**。第一版只导出视频，不含声音。

**明确不做**
- Host 端录制、云端存储或上传、录下输入事件、剪辑、同时录多块屏、录 SSH 终端、电脑控制端（V6）录制。
- App 在后台时不录。会话进后台后流会暂停，录制里写一条 Gap 记录。
- 回放倍速、回放进画中画（V3 不覆盖回放页）、跨设备迁移录制（主密钥只留在本机，本来就不进备份）。

**默认取舍**（文档未规定的，都是「起草时的默认，待维护者确认」）
- **隐私**：App 开始和停止录制时，用 `RecordingNotice{active}` 告诉 Host。Host 在「正在被控制」胶囊上显示「录制中」，并写审计事件 `recording`（`duration_s` 为录制时长，`detail = {role}`）。如果 Host 版本旧、没有 `RECORDING_NOTICE` 特性位，仍允许录制，但开始前确认框明确提示「这台电脑不会显示录制提示」。App 内录制期间会话页常驻红色「录制中」标签，第一次录制前弹出说明。
- **存储**：
  - 位置是 `<dataDir>/recordings/`。dataDir 本身不进备份；iOS 的文件保护级别是 `CompleteUntilFirstUserAuthentication`。
  - 总量上限默认 4 GB，可选 1、2、4、8 GB（设置项 `recording.maxTotalMb`）。单份最长 2 小时，到时自动停止。
  - 可用空间低于 1 GB 时不允许开始；录制中低于 500 MB 时自动停止。达到总量上限时也自动停止。**不会为了腾空间自动删除旧录制。**
  - 保留期默认 30 天，可选 7、30、90 天或永久（`recording.retentionDays`）。每次列出录制或开始录制前，删掉超过保留期的已完成录制。
- **崩溃时的完整性**：
  - 录制中的文件叫 `<id>.rsrec.part`。每块单独认证（GCM tag，AAD 绑定文件头摘要、块序号和块头）。最多每 5 秒 fsync 一次，所以崩溃时最多丢最后约 5 秒，外加一个写到一半的块。
  - 下次列出录制或开始录制时，扫描所有 `.part` 文件，跳过正在写的那份：从头逐块校验，在第一个不完整或校验失败的块处截断。然后补写一个 FINAL 块和文件尾，再改名为 `.rsrec`。FINAL 块里是 End 记录：
    - 坏块后面已经没有数据（写到一半）：`reason = "recovered"`；
    - 坏块后面还有能通过校验的块（中间损坏）：`reason = "damaged"`，列表里显示「已损坏」，仍然可以回放到坏块之前。
  - 文件头（含包裹后的密钥）在创建文件时就写入并 fsync，所以只要文件存在就能恢复。
- **关键帧**：开始录制时请求一个关键帧（原因 DECODER_RESET），之后每 30 秒再请求一次，作为回放时的跳转点。Host 节流时，请求可能被合并（起草默认；如果 Host 本来就有周期关键帧，可以去掉这一条）。
- **回放跳转**：跳到目标时间之前最近的一个关键帧，从那里开始播放；显示的位置也落在那个关键帧上。不做「从关键帧快速解到目标帧」。

### 数据格式要点

- 文件：
  - `header | chunk* | FINAL chunk | trailer`。
  - 字节序一律大端。
  - 文件头明文。块的密文 = AES-256-GCM(明文)，nonce 为 4 字节 0 加上 8 字节块序号。
  - AAD = SHA-256(文件头) ‖ 21 字节块头。
- 块头（21 字节明文）：`ct_len u32 | index u64 | flags u8（bit0 FINAL，bit1 KEYFRAME_START）| t_start_us u64`。
- 文件尾（16 字节）：`final_chunk_offset u64 | b"RSRECEND"`。
- 块的明文是若干条记录。每条记录是 `kind u8 | t_us u64 | len u32 | body`。`t_us` 是录制起点之后的媒体时间：超过 3 秒的空档会被压成 100 ms，并写一条 Gap。
- 记录种类：

| kind | 名称 | 内容 |
|---|---|---|
| 1 | Meta | 第 0 块的第一条，JSON |
| 2 | Config | StreamConfig |
| 3 | Video | 视频帧 |
| 4 | CursorPos | 光标位置 |
| 5 | CursorShape | 光标形状 |
| 6 | CursorUse | 复用已发过的光标形状 |
| 7 | CursorHidden | 光标隐藏或恢复 |
| 8 | Audio | 声音包 |
| 9 | Gap | 空档 |
| 10 | End | FINAL 块里的唯一一条，JSON |

  读到不认识的 kind 时跳过。
- 关键帧一定开启一个新块。这个块的开头是一段「状态前导」：最新的 Config、CursorPos，以及 CursorShape 加上 CursorUse 或 CursorHidden。这样从任何一个关键帧块开始都能独立解码。

| 错误码（`CoreError.message` 前缀） | 种类 | 含义 |
|---|---|---|
| `recording.unsupported` | Unsupported | 原生包没有密钥包裹回调（旧版本） |
| `recording.keyUnavailable` | Internal | 主密钥暂时拿不到（钥匙串锁着等） |
| `recording.notStreaming` | Http 409 | 会话不在播放状态 |
| `recording.alreadyRecording` | Http 409 | 这个会话已经在录 |
| `recording.storageFull` | Http 507 | 达到总量上限 |
| `recording.diskLow` | Http 507 | 可用空间不足 1 GB |
| `recording.notFound` | Http 404 | 录制不存在 |
| `recording.damaged` | Http 422 | 文件损坏，无法打开 |
| `recording.busy` | Http 429 | 已经有一个导出在进行（同一时间只做一个导出；回放同一时间也只有一个，但新开回放会先关掉旧的，不报这个错） |
| `recording.io` | Internal | 读写文件失败 |
| `recording.badCommand` | Internal | 回放命令 JSON 无法解析（App 自身的错误） |

## 原生组件

适配器会话的 `sessionId >= 2^62` 使用 `rs_raw_attach` / `rs_raw_copy`（STREAM_CONFIG codec 0），输入和关闭继续调用 `sessions.*`，Rust 按 ID 号段分流。

### RemoteSessionView（Fabric）

| 类别 | 名称 | 说明 |
|---|---|---|
| props | `sessionId: string` | 挂到哪个会话（`rs_session_attach`） |
| props | `mode: 'trackpad' \| 'touch'` | 触控板 / 直接触控（iPad 默认 touch） |
| props | `keyboardInset: number` | 键盘弹出时画面上移的距离 |
| props | `toolbarCollapsed: boolean` | Android 返回第一次收起工具栏 |
| props | `reduceMotion: boolean` | 去掉扫描线、脉冲 |
| commands | `zoomReset()`、`centerCursor()`、`showMagnifier(on)` | |
| events | `onGesture({kind})` | 给 JS 用于首次手势引导、统计 |
| events | `onViewport({zoom, x, y})` | 本地放大后的视口：`zoom` 倍数，`x`/`y` 为可见区左上角（归一化，按 1/zoom 正方形算，供小地图用）。视口感知编码不经这个事件，见下 |
| events | `onPointerLock({mouse, locked, reason})` | iPad 外接鼠标 / 触控板的指针锁定状态，状态变化时发；reason：noMouse / notStreaming / hidden / escaped / locked / notFullScreen / pending（JS 侧未知值按 pending 处理，`pointerLockStore.ts`）。JS 用它显示「分屏时不锁定」「指针已释放」提示；Android 不发 |
| events | `onVideoStats({codec, decodeP50Ms, …})` | 适配器会话的 `codec` 为 `raw`，`decodeP50Ms` 是原始帧拷贝耗时 |

组件内部（不经 JS）：

- 视频：`rs_session_attach(session_id, ctx, on_frame, on_event)`，回调里拿到 `RsFrame`（沿用 M0 结构，加 `display_id`、`epoch`），交给 `VTDecompressionSession` → `AVSampleBufferDisplayLayer`（D21），`DisplayImmediately`，不设抖动缓冲。
- 解码器重建、积压清理按 [03 §5.3](03-protocol.md#53-接收端)。
- 光标：本地 `CALayer` 画，形状来自 cursor 流事件，位置本地计算。
- 手势 → `rs_input_pointer(session, x, y)`（数据报）/ `rs_input_button(...)` / `rs_input_scroll(...)`，高频事件不过 JS。
- 视口回报（M3，[03 §5.5](03-protocol.md#55-视口感知编码m3)）：视口每次变化（含跟随光标、键盘让位、旋转）调 `rs_session_set_viewport`，可见区按视图边界与画面区域求交（含 letterbox 那一边、扣掉键盘），手势或缩放动画进行中带 `RS_VIEWPORT_ACTIVE`；每收到 `RS_EV_STREAM_CONFIG` 再调一次。节流、去重、换算成 Host 像素在 Rust 里做。
- 裁剪渲染：`RS_EV_STREAM_CONFIG` 的 `src_*` 不是 0,0,1,1 时，画面区域仍按 `display_w:display_h` 的宽高比算，解码画面贴到其中 `src` 那一块；新几何等该 epoch 第一帧上屏时一起切换；裁剪期间用最后一帧整屏画面垫底。

### 手势（设计文档 §9.4，画布 `SessionCoach`）

| 手势 | 触控板模式 | 直接触控模式 |
|---|---|---|
| 单指拖动 | 移动光标（带加速曲线） | 光标跟手指走并拖动（按住左键） |
| 单指轻点 | 左键单击，连点为双击（M0 已实现） | 在手指位置单击 |
| 双指轻点 | 右键 | 右键 |
| 长按后拖动 | 按住左键拖拽 | 放大镜 + 精确定位（`iPadSession`） |
| 双指拖动 | 滚动（带 phase 和惯性） | 滚动 |
| 双指捏合 | 本地放大画面（不发给电脑） | 同左 |
| 三指轻点 | 显示 / 隐藏工具栏 | 同左 |
| 双指双击 | 切换触控板 / 直接触控 | 同左 |

### TerminalView（Fabric）

- iOS 包 SwiftTerm 的 `TerminalView`；Android 包 Termux `terminal-view`（M2）。
- props：`sshId`、`fontSize`、`theme`；commands：`sendSequence(str)`（键盘附件栏的 Esc、Tab、Ctrl、方向键）、`copySelection()`、`paste()`。
- 字节流：`rs_term_attach(ssh_id, ctx, on_output)`；输入 `rs_term_input(ssh_id, bytes)`；尺寸变化 `rs_term_resize(ssh_id, cols, rows)`。

### C ABI（`rs_mobile.h`）

连接由 `RsCore.sessions.open` 建立，C ABI 只负责「挂上去」和高频输入。

```c
typedef uint64_t RsSessionId;
typedef uint64_t RsSshId;

// 视频与会话事件；回调在库的工作线程上，一次一个；指针只在回调期间有效
bool rs_session_attach(RsSessionId id, void *ctx, RsFrameCb on_frame, RsEventCb on_event);
void rs_session_detach(RsSessionId id);          // 返回后不再有回调

// 高频输入（坐标归一化到当前显示器 0..1）
void rs_input_pointer(RsSessionId id, float x, float y);
void rs_input_button(RsSessionId id, int32_t button, bool down, float x, float y, uint8_t clicks);
void rs_input_scroll(RsSessionId id, float dx, float dy, uint8_t phase, uint8_t momentum);
void rs_input_touch(RsSessionId id, uint8_t touch_id, uint8_t phase, float x, float y, float pressure);
void rs_input_release_all(RsSessionId id);

// 解码侧回报（03 §5.3、§7）：RemoteSessionView 内部调用，不经 JS；会话不存在时静默忽略
// reason 取 KeyframeRequest.Reason：2 DECODER_RESET（解码失败、VT 会话重建）、4 BACKLOG（解码器未就绪或积压 > 2 帧）；
// 丢包（LOSS）由 Rust 收端自己判定和请求，App 不发。其他值忽略。last_good_frame 由 Rust 填。
void rs_session_request_keyframe(RsSessionId id, uint8_t reason);
// 每解出并提交显示一帧调一次；Rust 聚合进 Feedback 的 decode_p50_us、decode_queue、render_fps
// decode_us：送进解码器到拿到图像；latency_us：RsFrame.received_us 到提交显示；queue：此刻待解码帧数
void rs_session_report_decoded(RsSessionId id, uint64_t decode_us, uint64_t latency_us, uint32_t queue);
// M3 视口回报；x/y/w/h 为当前显示器可见区域归一化坐标，flags 为 RS_VIEWPORT_ACTIVE / RS_VIEWPORT_CROP_OK
#define RS_VIEWPORT_ACTIVE 1u
#define RS_VIEWPORT_CROP_OK 2u
void rs_session_set_viewport(uint64_t id, float x, float y, float w, float h,
                             uint32_t view_w_px, uint32_t view_h_px, uint32_t flags);

// 终端
bool rs_term_attach(RsSshId id, void *ctx, void (*on_output)(void *ctx, const uint8_t *buf, size_t len));
void rs_term_detach(RsSshId id);
void rs_term_input(RsSshId id, const uint8_t *buf, size_t len);
void rs_term_resize(RsSshId id, uint16_t cols, uint16_t rows);

// 原生签名器（启动时注册一次；04 · 密钥与证书）
typedef int32_t (*RsSignCb)(void *ctx, const uint8_t *msg, size_t len, uint8_t *sig_out, size_t *sig_len);
void rs_register_signer(void *ctx, RsSignCb sign, const uint8_t *spki_der, size_t spki_len);
```

## 生命周期与后台（iOS）

| 时刻 | 动作 |
|---|---|
| 进入后台 | 所有远控会话 `background(true)` → `PauseStream`；`beginBackgroundTask` 保留约 30 秒；SSH 连接不动（tmux 会兜底） |
| 30 秒内回前台 | `background(false)` → `ResumeStream`，Host 立即出关键帧，目标 0.5 秒出画面（设计文档 §9.11） |
| 超过 30 秒 | Rust 发 `Goodbye(BACKGROUND)` 并关闭连接；令牌在旧连接结束后 60 秒内可恢复，旧连接存活时也可用于换路。回前台自动恢复，超时回设备页；成功后换新令牌、epoch + 1 并重置重组器，`resumed=false` 按新会话，恢复失败映射 199（完整恢复接线实现待跟进，见 03 §4.5） |
| 会话中 | `isIdleTimerDisabled = true`；电量 < 20% 时 `Feedback.low_power = true` |
| 被系统杀掉 | 下次启动不自动重连，只在「会话」页列出上次的会话供一键重连 |

**iOS 画中画（M3 V3）**
- 只在仅观看（`role = viewer`）、画面在播（`streaming`）、没有遮罩时可用；控制者不能进。和 Android 一致。
- 进入：回桌面时自动进入（`canStartPictureInPictureAutomaticallyFromInline`，受系统「自动开启画中画」设置控制）；或在会话「更多」里点「画中画」。
- 画中画期间不发 PauseStream，会话照常收流；解码改为把压缩帧直接交给 `AVSampleBufferDisplayLayer`（直通），切换时请求关键帧（DECODER_RESET）。
- 小窗里的暂停 = `sessions.background(id, true)`（PauseStream，沿用 §4.5 的 30 s / 60 s 时间线）；播放 = `background(id, false)`。
- 退出：小窗「还原」回到会话页；关闭小窗且 App 在后台时，500 ms 后按普通后台处理；画中画期间去过后台再从别处回到前台时，关闭画中画。
- 会话结束、角色变成控制者、会话视图离开窗口时关闭画中画。
- Info.plist 带 `UIBackgroundModes = audio`（画中画要求）；远端声音仍然不在后台播放。

## 网络判定

用于首页横幅（`MainVPN`）、拨号前剪枝（03 §10.1）和错误归类。原生模块 `NetInfoPlus` 提供本机所有接口（名称、地址、是否 VPN 类型），iOS 用 `NWPathMonitor` + `getifaddrs`，Android 用 `ConnectivityManager`（`NetworkCapabilities.TRANSPORT_VPN`）。

| 观察 | 判定 | 界面 |
|---|---|---|
| `network.status.tailnet.active = 'embedded'` 且节点 running | tailnet 在线（模式 B） | 正常，横幅不提示「公司 VPN 顶掉 Tailscale」 |
| 有 100.64.0.0/10 地址 | tailnet 在线（模式 A） | 正常 |
| 没有 100.x，有 VPN 接口且地址落在「其他网络」登记的 VPN 地址池里（如 10.99.4.27 ∈ 10.99.0.0/16） | 公司 VPN 顶掉了 Tailscale | 横幅「公司 VPN 已连接，tailnet 已断开」+「改用 App 内连接」（本构建带 App 内连接时可用） |
| 没有 100.x，也没有公司 VPN | tailnet 未连接 | 横幅「Tailscale 没有连接」+ 打开 Tailscale 的按钮 |
| 本机地址落在某个登记的内网网段 | 人在公司 | 内网候选优先 |
| Android：VPN 在工作资料里 | 不冲突 | 不提示（设计文档 §4.10） |

「其他网络」在 设置 › 其他网络 里登记（名称、内网网段、VPN 地址池），也可以从已配对 Host 记下的网段一键生成；只存本机。

## 错误页

`SessionEnded` 按错误码选画板。文案、按钮完全按画布；下表只定按钮行为。

| 码 | 画板 | 主按钮 | 次按钮 |
|---|---|---|---|
| 101 | `SessionError` | 「已打开，重试」→ 重新 `open` | 「改用…」→ 系统屏幕共享 / 远程桌面 |
| 110 | `ErrOffline` | 「重试」 | 「查看同网段设备」 |
| 120 | `ErrNoHost` | 「用屏幕共享 / 远程桌面连接」（M3 V10） | 「返回」 |
| 130 | `ErrPolicy` | 「复制策略片段」 | 「返回」 |
| 140 | `ErrDenied` | 「再请求」（剩余次数） | 「返回」 |
| 150 | `ErrFingerprint` | 「取消」 | 「我重装过，重新配对」→ 二次确认后删除旧指纹，进入配对 |
| 160 | `ErrVPN` | 「我已连接，重试」 | 「改用 App 内连接」（M2） |
| 161 | `ErrPort` | 「复制诊断信息」 | 「在电脑上怎么打开」 |
| 170 | `ErrVersion` | 「返回」（说明要在电脑上手动升级 Host） | 「改用远程桌面」 |
| 171 | `ErrVersion` 变体 | 「去更新 App」→ App Store / Google Play / TestFlight | — |
| 180 | 通用错误页 | 「重新配对」 | — |
| 181 | 通用错误页 | 「去配对」→ `PairAdd` | — |
| 190 | `SessionLoginWindow`（停在登录窗口） | 「用系统屏幕共享登录」→ `vnc://<tailnet 地址>` 交给 VNC App | 「返回」 |
| 191 | `SessionLoginWindow`（FileVault 未解锁） | 「通过 SSH 解锁磁盘」（系统支持时才显示） | 「返回」 |

其余码用通用错误页：标题 + 一句话 + 「重试」「返回」，并在 `Diagnostics` 的日志里记录完整上下文。

## iPad

| 项 | M1 | M2 |
|---|---|---|
| 首页 | 两栏布局（`iPadHome`） | — |
| 会话 | 默认直接触控 + 放大镜（`iPadSession`）；外接键盘全键映射（⌘ 键直通，除被系统拦截的） | 外接鼠标 / 触控板的指针锁定（全屏时，`iPadSplit` 说明分屏时不锁） |
| 帧率 | 跟随 120 Hz ProMotion，`ClientCaps.max_fps = 120` | — |
| Apple Pencil | — | 悬停移动光标、点按左键（设计文档 §9.10） |

## Android（M2）

iOS 画中画见上文「iOS 画中画」。

| 项 | 做法 |
|---|---|
| 视觉 | 同一套 token；组件形状用 Material 3（顶栏 64、导航栏 80 带 64×32 胶囊、开关 52×32、对话框圆角 28、Snackbar），可点区域 ≥ 48dp；`components/` 里按平台出变体 |
| RemoteSessionView | Kotlin：`MediaCodec`（`KEY_LOW_LATENCY = 1`，Android 11+）→ `SurfaceView`；手势用 `GestureDetector` + 自定义多指识别 |
| 返回手势 | 会话中左右各申请 200dp 高的 `systemGestureExclusionRects`；返回第一次收起工具栏 + Snackbar「再返回一次回到设备页，会话不会断开」，第二次回设备页，不断开（`AndroidSession`） |
| 后台 | 前台服务 + 常驻通知「正在连接 Studio Mac mini · 42:18」，按钮「回到会话」「断开」；终端会话同样（`AndroidNotify`）。前台服务类型待按 Play 政策确认（候选 `specialUse`，见设计文档 §12） |
| 通知权限 | Android 13+ 首次进入会话前申请 `POST_NOTIFICATIONS` |
| 快捷设置磁贴 | `TileService`「Remote Screen」：空闲时一点连最近的电脑，会话中一点回到会话 |
| 画中画 | 仅观看时可进系统画中画 |
| 密钥 | AndroidKeyStore（StrongBox，没有就 TEE）；生物识别用 `BiometricPrompt` |
| VPN | 判定见上；工作资料里的 VPN 不提示冲突（`AndroidVPN`） |
| 构建 | `cargo ndk -t arm64-v8a -t x86_64` 产出 `jniLibs`；uniffi 生成 Kotlin 侧与 JSI 侧绑定 |

## 无障碍（设计文档 §9.14，画布 `A11y`）

- 所有可点元素有 `accessibilityLabel`；图标按钮的标签写在 `i18n/zh.ts` 的 `a11y` 段。
- 设备卡片的读屏文本：「Studio Mac mini，直连 6 毫秒，2 块屏幕，按钮。轻点两下开始远控，长按看详情。」
- 支持系统最大字号：卡片变单列，设备名折行不截断（`numberOfLines` 不设上限）。
- 会话里读屏只聚焦工具栏；`RemoteSessionView` 整体是一个元素，标签「Studio Mac mini 远端画面，双指双击切换直接触控」。
- 读取系统「减弱动态效果」，传给 `reduceMotion`。
- 验收：只用 VoiceOver / TalkBack 完成「添加设备 → 连接 → 断开」（09）。

## 调试入口

沿用 M0 的启动参数，便于自动化：

| 参数 | 作用 |
|---|---|
| `-autoConnect <host_id 或 ip>` | 启动后直接打开会话。`host_id`（16 个小写字符）直接用；IPv4、`IPv4:port`、`[IPv6]:port`、裸 IPv6、DNS 名按地址匹配，先查已配对、再查本机目录，端口不参与匹配。等引导完成、App 解锁后才进 `Session`；每次启动只触发一次。日志 `RS-AUTOCONNECT` |
| `-disconnectAfter <秒>` | 从 `streaming` 起计时，到时关闭会话并输出整场汇总（样本数、rtt p50 / p95、fps 平均、解码 p50 / p95）。1–86400 |
| `-selfTestMove YES` | 暂不实现（需要从 JS 驱动指针的接口），只记一条 `unsupported` 日志 |
| `-devHarness selftest -harnessArg host:port` | M0 自检页（原 `-autoConnect host:port` 的行为） |

原生把这些参数和启动配置经 `initialProperties` 传给 JS，值都是字符串：`dataDir`、`autoConnect`、`disconnectAfter`、`selfTestMove`、`devHarness`、`harnessArg`。

## M2 文件接口契约

`TransferId = string`。浏览和传输 DTO 以 `crates/rs-mobile/src/rscore/dto_files.rs` 与 `app/src/native/rscore/types.ts` 为准，字段逐一对应，JSON 使用 camelCase；可空字段保留 null。Computer 来源 `{kind: "computer", hostId}`，Server 来源 `{kind: "server", serverId}`。文件列表超过 10 页（1 万条）未取完时 `truncated = true`；Computer 的 `netPath` 是当前连接路径，Server 为 null。

统一事件增加 `{type: "file.transfer", transfer: FileTransfer}` 和 `{type: "file.transferRemoved", transferId: string}`。传输的 `startedAt` / `updatedAt` 为 unix 微秒，upload 指手机 → 远端；download 的 `localPath` 只在完成后有值。upload 的 `remotePath` 表示远端目录（null = 默认），`savedName` 是电脑最终采用的文件名。

UniFFI 导出都使用字符串参数 / JSON 返回：异步 `files_list(host_id, path?) -> FileListing JSON`、`files_download(host_id, remote_path) -> transfer_id`、`files_upload(host_id, local_path, remote_dir?) -> transfer_id`；SFTP 对应 `sftp_list` / `sftp_download` / `sftp_upload`，首参是 server_id，不是 SSH 会话 id。同步 `transfers_list() -> Vec<FileTransfer> JSON`、`transfers_pause(transfer_id)`、`transfers_cancel(transfer_id)`、`transfers_remove(transfer_id)`；异步 `transfers_resume(transfer_id)`。

`*_download` / `*_upload` 只做参数和本地文件存在等本地检查就返回 id，网络错误体现在传输状态；`*_list` 错误直接抛出。`CoreError.message` 以稳定码开头，后接 `: 说明`：

| 稳定码 | CoreError |
|---|---|
| `files.notFound` / `sftp.notFound` | Http 404 |
| `files.permissionDenied` / `files.notAllowed` / `files.outsideRoot` / `sftp.permissionDenied` | Http 403 |
| `files.disabled` / `files.unsupported` | Unsupported |
| `files.offline` / `sftp.offline` | Network |
| `files.busy` | Http 429 |
| `sftp.hostKeyUnknown` / `sftp.authRequired` / `sftp.unavailable` | Http 401 / 401 / 501 |
| 其他 | Internal |

`CoreConfig.filesDir` 可选，None 默认 `<dataDir>/files`。`NativeRsFiles` TurboModule 的 `filesDir = Documents/Received`（`UIFileSharingEnabled` 使其在「文件」App 可见），`inboxDir = <tmp>/RsInbox`。`pickFiles()` 使用 UIDocumentPicker asCopy 多选，复制到 inboxDir 后返回 `{uri, name, size}`，size -1 为未知，取消返回 []；`saveToFiles(uri, name)` 使用 forExporting，返回是否保存；`importIncoming(url)` 把文件 URL 复制到 inboxDir，非文件 URL 返回 null；`setTransfersActive(active)` 在有传输时申请后台执行时间，全部结束时释放。上传结束可删 inbox 临时副本。

真 RsCore 初始化需把原生 `filesDir` 传给 `init`，启动接线由独立卡完成。下载完成后使用 `PlatformServices.share` 系统分享面板，面板已有「存储图像 / 存储到文件」，不单做存到相册。Host 主动推文件、iOS Share Extension、File Provider 都为 **M3**。

## 电脑端（M3，D46）

- 代码：`desktop/`（Tauri 2 外壳，React + Vite）、`crates/rs-desktop`（Rust）。
- RsCore 用法：电脑端在 Rust 里直接构造 `CoreConfig`（`platform` 为 `macos` / `windows`），注入平台签名器后 `install_core`。前端通过 Tauri 命令 `rscore_call(name, args)` 调 rs-mobile 现成的 JSON 字符串包装函数，返回的 JSON 与本文 RsCore 类型一致；事件经 Tauri 事件 `rscore://event` 转发，形状同 `EventSink.onEvent(seq, payload)`。
- 会话窗口是平台原生窗口（不经 WebView）：Mac 用 VideoToolbox + `AVSampleBufferDisplayLayer`，Windows 用 FFmpeg d3d11va + DXGI。输入映射在 Rust（`rs-desktop` 的 keymap / keys / pointer / scroll）。
- 键盘：会话窗口有焦点时全部按键发给远端；单独按下再松开左 Ctrl + 左 Alt（Mac ⌃⌥）释放 / 恢复捕获。Windows 全屏时用低级键盘钩子截获 Win、Alt+Tab。Mac 上 ⌘Tab、⌘空格、调度中心走菜单「发送组合键」。
- 第一版不做：剪贴板、远端声音、文件、SSH、内嵌 tailnet。
