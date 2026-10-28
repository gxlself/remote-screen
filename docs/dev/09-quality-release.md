# 09 · 测试、发布与运维

本文回答三个问题：怎么证明它能用、怎么把它装到用户手上、Headscale 怎么配。指标以设计文档 §01、§11.1、§11.2 为准。

## 测试分层

| 层 | 工具 | 跑在哪 | 什么时候跑 | 挡不挡合并 |
|---|---|---|---|---|
| 单元测试 | `cargo nextest`、`go test`、Jest | CI（Linux） | 每个合并请求 | 挡 |
| 集成测试 | host-fake + rsp-client，在 Linux 网络命名空间里，用 `tc netem` 注入故障 | CI（Linux） | 每个合并请求 | 挡 |
| 构建 | Xcode（iOS App、Mac Host），Gradle（M2） | CI（macOS） | 每个合并请求 | 挡 |
| 模糊测试 | cargo-fuzz | CI 定时任务 | 每晚 | 不挡；有崩溃时开问题单 |
| 延迟回归 | host-fake 管线计时 | CI 定时任务 | 每晚 | 不挡；退化 > 20% 时报警 |
| 端到端 | Maestro（模拟器）+ 真机手测清单 | 开发机、真机 | 每个检查点、每次发版 | 发版前必须过 |
| 弱网与延迟 | netem 脚本、240 fps 拍摄 | 测试网络、真机 | 每个里程碑，每次改码率或传输 | 发版前必须过 |
| 安全测试 | 本文清单 | 测试 tailnet | 每个里程碑 | 发版前必须过 |

## 单元测试

- **覆盖率**：`rsp-core` 行覆盖 ≥ 85%（`cargo llvm-cov`），其他 crate 不设硬线。
- **决策表一律表驱动**：每一行写成一个测试用例，表改了测试跟着改。包括：
  - 鉴权 11 步（04）
  - 拨号失败归类（03 §10.3）
  - 码率控制规则（03 §7）
  - 网络判定（07）
  - 错误码 → 画板（07、10）
- **App（TS）**：只测 `src/features/` 里的逻辑，不做组件快照。界面靠画板走查。
- **原生（Swift / Kotlin）**：只测映射表，比如 `UIPress` → HID、`KeyEvent` → HID。其余靠真机。

## 集成测试

`host-fake` 是 `rs-host-agent --platform fake`：生成测试图样（带帧号和时间戳），用 openh264 编码，或循环播放预录的 HEVC；控制面、鉴权、配对、多路径和真 Host 走同一份 host-core 代码。

测试框架 `crates/host-fake/tests/`：每个用例建两个网络命名空间，用 veth 连起来，按需加 `tc netem`。CI 上整套 ≤ 5 分钟。命令：`just e2e-fake [过滤]`。

| 编号 | 用例 | 预期 |
|---|---|---|
| IT-01 | 已配对设备连接 | 首帧 ≤ 300 ms（本地） |
| IT-02 | 未配对的证书 | `AuthResult(DENIED, 181)` |
| IT-03 | 作废名单里的证书 | `DENIED 180` |
| IT-04 | 换一个 Host 证书 | 客户端报 150，不发 `ClientHello` |
| IT-05 | 丢掉全部 UDP | 250 ms 后 TCP 接上；`transport = TCP` |
| IT-06 | TCP 模式下恢复 UDP | 30 s 内升级回 QUIC，会话不断 |
| IT-07 | 5% 随机丢包，60 s | FEC 恢复率 ≥ 99%；关键帧请求 ≤ 2 次/分钟 |
| IT-08 | 主路断开，有备用路 | ≤ 1 s 恢复画面；断开期间的输入不注入 |
| IT-09 | 后台 20 s / 45 s / 120 s | 分别对应：直接恢复 / 用恢复令牌恢复 / 令牌过期，返回设备页 |
| IT-10 | `protocol = 2`；缺少必选功能位 | 170 / 171，`detail` 带双方版本 |
| IT-11 | 第 5 台设备连接 | `DENIED 142` |
| IT-12 | 第二台设备 VIEW，然后请求控制 | 控制者收到 `ControlRequest`；同意后两边的角色互换 |
| IT-13 | 需要本机确认，30 s 不点 | `DENIED 140`，`detail.reason = "timeout"` |
| IT-14 | 空闲超时 | 提前 60 s 发 `IdleWarning`，到时 `Goodbye(IDLE_TIMEOUT)` |
| IT-15 | 配对码连错 5 次 | 第 5 次后码作废，正确码也失败 |
| IT-16 | 剪贴板文本和图片往返 | 内容一致；超过上限时拒绝 |
| IT-17 | 切换显示器 | epoch + 1；旧 epoch 的分片全部丢弃 |
| IT-18 | 带宽 20 → 3 → 20 Mbps，每 20 s 阶跃 | 2 s 内收敛到可用带宽的 70–90%；不振荡 |
| IT-19 | 信任日志：无签名的 APPROVE | Host 拒绝执行，记安全日志 |
| IT-20 | 同一设备第二次连接 | 旧连接收到 `Goodbye(REPLACED)`，关闭码 3 |

Linux 平台层（host-linux）的桌面集成测试分四层：纯逻辑与 zbus 点对点 mock portal（CI）；PipeWire 守护进程；Xvfb；headless sway + xdg-desktop-portal-wlr。后三层用 feature desktop-it，在开发机上由 crates/host-linux/tests/desktop/run.sh 起环境，不进 CI。

## 模糊测试

| 目标 | 输入 | 所在 crate |
|---|---|---|
| `envelope` | 任意字节 → 控制面帧解码 | rsp-core |
| `video_header` | 单个数据报 | rsp-core |
| `reassembly` | 一串数据报（含 FEC 分片），检查重组器不崩、不越界、内存有上限 | rsp-core |
| `input_event` / `pointer` / `cursor` | input 流、指针数据报、cursor 流 | rsp-core |
| `clipboard_header` / `file_header` | 流头 | rsp-core |
| `tcp_record` | TCP 记录层 | rsp-transport |
| `pair_qr` | `rsp://pair?...` 字符串 | rsp-core |
| `pair_spake` | 配对状态机的消息序列 | rsp-core |
| `trust_statement` | `SignedStatement` 字节 | host-core |

- 语料库放 `fuzz/corpus/<目标>/`，用 03 §12 的测试向量做种子。
- 每晚每个目标跑 10 分钟；里程碑收尾时每个目标跑满 30 分钟。命令：`just fuzz <目标> 1800`。
- 发现崩溃：最小化后加进对应 crate 的回归测试，再修。

## 弱网

### 场景

参数都是单向的。`weak` 是设计文档 §11.1 的验收场景。

| 名称 | 延迟 | 抖动 | 丢包 | 带宽 | 用途 |
|---|---|---|---|---|---|
| `wifi-good` | 2 ms | 1 ms | 0 | 100 Mbps | 基线 |
| `wifi-busy` | 5 ms | 5 ms | 1% | 30 Mbps | 办公室 Wi-Fi |
| `5g-city` | 20 ms | 8 ms | 0.5% | 40 Mbps | 同城 5G 直连 |
| `derp` | 60 ms | 15 ms | 0.5% | 8 Mbps | 走中继 |
| `weak` | 100 ms | 20 ms | 2% | 10 Mbps | **验收**：画面连续，没有超过 500 ms 的卡顿 |
| `lossy` | 30 ms | 10 ms | 5% | 20 Mbps | 检验 FEC |
| `udp-blocked` | — | — | UDP 100% | — | 检验 TCP 回退 |
| `handover` | — | — | 每 60 s 断 3 s | — | 检验换路 |
| `bw-step` | 10 ms | — | 0 | 20 → 3 → 20 Mbps，每 20 s 一跳 | 检验码率控制 |

场景定义在 `tools/netem/profiles.toml`。

### 怎么注入

| 方式 | 适用 | 做法 |
|---|---|---|
| Linux 软路由（推荐） | 真机测试 | 一台 Linux 小主机做 Wi-Fi 热点（hostapd），手机连它；`just netem <场景> <网卡>` 调用 `tools/netem/linux.sh`，在热点网卡上加 `tc qdisc netem`。tailnet 流量也走这块网卡，一起受影响 |
| iPhone「网络链接调节器」 | 快速手测 | 设置 › 开发者 › Network Link Conditioner，选自定义配置，按上表填 |
| macOS dummynet | Host 侧 | `tools/netem/mac.sh` 用 `dnctl` + `pfctl` 对 UDP / TCP 7400 限速和丢包（只对物理网卡有效，tailnet 流量要对 WireGuard 的 UDP 端口加规则） |
| 网络命名空间 | CI | 见上面「集成测试」 |

每次测试导出数据：App 会话结束时，`-disconnectAfter` 会输出 2 Hz 的 `session.stats`（JSON lines）；Host 侧导出同一时段的 `HostStats`。两份放进 `docs/dev/latency/<版本>/<场景>/`。

## 延迟测量

### 管线内（自动）

- 每帧带 `capture_ts_us`，App 按对时结果换算到本机时钟（03 §9），统计 capture→decoded。
- HUD 实时显示；`-disconnectAfter` 输出整场的 p50、p95、p99 汇总；`rs-probe --json` 给 CI 用。
- **每晚回归**：CI 用 host-fake 在 `wifi-good` 场景下跑 60 s，记录 capture→receive p95。比 7 天中位数差 20% 以上时报警。

### Glass-to-glass（真机，每个版本）

方法和 [m0-latency §1.2](../m0-latency.md) 相同：

1. 被控屏放毫秒计时器 `tools/latency/timer.html`。它显示大号毫秒读数，同时有一个每帧翻转的色块。
2. 手机和被控屏并排，用另一台手机以 240 fps 拍摄 ≥ 10 s。
3. 逐帧抽 ≥ 20 个样本，读数相减，取 p50、p95、max。

| 维度 | 取值 |
|---|---|
| 场景 | 静态计时器、滚动网页、拖动窗口、终端快速输出 |
| 网络 | 同一 Wi-Fi；5G 直连（`tailscale status` 显示 direct）；DERP（ACL 临时禁掉直连）；TCP 模式（`udp-blocked`） |
| 对照 | 同样条件下测 RustDesk（IP 直连）和 Moonlight。桌面控制端的对照用 `tools/bench` 自动跑（见下节）；手机上的对照仍用拍摄法 |

结果记在 `docs/dev/latency/<版本>.md`。表格格式沿用 m0-latency §3，加一列「对照」。

### 对照基准（自动，每个版本）

`just bench run --lab <lab.json>` 在控制端 Mac 上依次跑本产品、RustDesk、Moonlight + Sunshine。

- Host 是 Windows 测试机，全屏显示 `tools/bench/page/bench.html` 的机器时间码。
- 控制端截取客户端窗口、解出时间码；两机经 SSH 对时，算出「Host 出帧 → 控制端窗口上屏」的延迟。
- 同时记录帧率、卡顿、带宽、Host CPU / GPU、静态场景 SSIM。
- 场景：静态桌面、滚动网页、视频播放、拖窗口。网络：`lan`、`wifi-good`、`weak`（`tools/netem/mac.sh`）。

结果：
- `summary.json` 放进 `docs/dev/latency/<版本>/bench/`，报告并入 `docs/dev/latency/<版本>.md`。
- `just bench judge` 与上一版本比较，规则见 [tools/bench/README.md](../../tools/bench/README.md)，只告警、不挡发版。

这个数字不含控制端面板出光，本产品走的又是 iOS 模拟器，所以不能直接对照 §01 的手机指标。

## 真机矩阵

来自设计文档 §11.1。「M1」「M2」表示从哪个里程碑起必须覆盖。

| 维度 | 设备 / 条件 | 起始 |
|---|---|---|
| 手机 | iPhone，iOS 18 和 iOS 26 各一台 | M1 |
| 平板 | iPad Pro + 妙控键盘；iPad mini | M1（基础）/ M2（指针锁定） |
| Android | Pixel 8（Android 16）；一台国产中端机（Android 14，有厂商省电策略） | M2 |
| Mac | Mac mini（macOS 15、26，双屏含竖屏）；MacBook Air | M1 |
| Windows | Windows 11 三台（NVIDIA、AMD、Intel 核显各一）；Windows 10 22H2 | M2 |
| 网络 | 同一 Wi-Fi；tailnet 直连（5G）；DERP；公司 VPN（UDP 通）；公司 VPN（只通 TCP）；`weak`；Wi-Fi 与 5G 来回切换 | M1 |
| 状态 | 首次配对、无人值守、登录窗口、锁屏、多显示器、两台设备同时连、版本不匹配、作废之后、公司 VPN 顶掉 Tailscale | M1（UAC 在 M2） |

没有公司 VPN 的测试环境时，用一台 Linux 跑 WireGuard 服务端模拟：地址池 10.99.0.0/16，路由到 10.30.12.0/24 的测试网段；只放 TCP 的情况用 iptables 丢掉 UDP。

## 端到端

### 自动（Maestro）

在模拟器里跑，Host 用 host-fake（Mac 上也能跑），电脑列表用预置的手动电脑和已配对主机。目录在 `app/e2e/`，命令 `just e2e-app ios`。

覆盖：引导、首页、配对（host-fake 用 `--print-pair-code` 打印配对码）、进入会话并断开、各个错误页（host-fake 用 `--fail <错误码>` 构造）、设置页。

### 手测清单（真机，每个检查点和每次发版）

| 编号 | 步骤 | 预期 | 起始 |
|---|---|---|---|
| E2E-01 | 全新安装 → 引导 → 首页 | 出现测试 Mac，状态正确 | M1 |
| E2E-02 | 扫码配对 → 在 Mac 上点允许 | `PairDone`；首页显示「已配对」 | M1 |
| E2E-03 | 远控：移动、单击、双击、右键、拖拽、滚动、中文输入、⌘Tab | 都正常；光标形状正确 | M1 |
| E2E-04 | 切换显示器；拔掉一块屏 | 画面跟着切；会话不断 | M1 |
| E2E-05 | 剪贴板双向 | 内容一致 | M1 |
| E2E-06 | 画质四档切换；HUD | 码率和分辨率随档位变化 | M1 |
| E2E-07 | 切到后台 20 s 再回来；2 分钟再回来 | 0.5 s 内出画面；自动恢复或回到设备页 | M1 |
| E2E-08 | 会话中 Wi-Fi 和 5G 互切 | ≤ 2 s 恢复，触摸不回放 | M1 |
| E2E-09 | iPad 观看，iPhone 控制，iPad 请求接管 | 符合画布 `SessionShared` 两态 | M1 |
| E2E-10 | SSH：Tailscale SSH（check 模式）、普通 SSH + 跳板机；断网 30 s | tmux 续上 | M1 |
| E2E-11 | 在 Host 菜单「已配对的设备」里作废 iPhone | iPhone 再连得到 `DENIED 180`，App 显示「本机已被作废」；测试服务器上那行公钥要手动删 | M1 |
| E2E-12 | 连一台 Host 0.2.x（低于要求） | `ErrVersion`，提示在电脑上升级 Host | M1 |
| E2E-13 | 手机关掉 Tailscale，连模拟公司 VPN，按内网 IP 连 | 走内网路；需要本机确认（按设置） | M1 |
| E2E-14 | Mac 注销到登录窗口 | `SessionLoginWindow` → 交给系统屏幕共享 | M1 |
| E2E-20 | Windows 重启 → 登录界面 → Ctrl+Alt+Del → 输入密码 | 登录成功，会话继续 | M2 |
| E2E-21 | 开着公司 VPN，用模式 B 连家里的 Mac；同时按内网连工位电脑 | 两边都能连 | M2 |
| E2E-22 | Android 会话中按返回 | 第一次收起工具栏，第二次回设备页，会话不断 | M2 |
| E2E-23 | 只用 VoiceOver / TalkBack：添加设备 → 连接 → 断开 | 能完成 | M2 |

- iPad 新建虚拟屏 → 拖一个窗口过去 → 断开 70 秒 → 虚拟屏消失、窗口回到主屏。
- Windows 结束服务进程 → 5 秒内虚拟屏消失。

## 安全测试

对应 04 的威胁核对表。每个里程碑全跑一遍，结果记在 `docs/dev/security/<版本>.md`。

| 编号 | 用例 | 方法 | 预期 |
|---|---|---|---|
| SEC-01 | 局域网扫描 | 同一 Wi-Fi 的另一台机器 `nmap -sU -sT -p 7400 <Mac 局域网地址>`，内网监听关着 | 端口关闭或无响应 |
| SEC-02 | 内网监听打开，来源不在白名单网段 | 从白名单外的网段发 QUIC Initial 和 TCP SYN | 没有任何响应（QUIC 不回包，TCP 被拒） |
| SEC-03 | 未配对设备经内网握手 | 用新生成的证书连 | 握手中止；Host 证书里只有随机 CN，没有主机名和用户名 |
| SEC-04 | tailnet 里不在 `allowed_users` 的用户 | 用测试账号的设备连 | `DENIED 130`；写安全日志 |
| SEC-05 | 在线猜配对码 | 脚本连续尝试 | 5 次错后码作废；所有来源 5 分钟内超过 20 次后一律拒绝 |
| SEC-06 | 中间人 | 用代理替换 Host 证书：配对时替换一次，配对后再替换一次 | 配对时确认失败；配对后报 150 |
| SEC-07 | 作废生效 | 在 Host 本机作废后，立刻用被作废的设备连这台 Host | `DENIED 180` |
| SEC-10 | 无签名的批准声明 | 往信任日志里直接写一条无签名的 `APPROVE_DEVICE` | Host 拒绝执行，写安全日志 |
| SEC-11 | 篡改更新包 | 篡改清单签名；篡改安装包；用别的 Team ID / 证书签的包 | 三种都拒绝，写日志 |
| SEC-12 | 重放 | 重放抓到的 `PairConfirm`；重复使用恢复令牌 | 都拒绝 |
| SEC-13 | 确认弹窗期间的远端输入 | 已连接的设备在弹窗出现时连续点击「允许」的位置 | 弹窗不响应；本机点击才有效 |
| SEC-14 | 日志泄露 | 在会话里输入特征字符串、复制特征内容，然后导出 App 和 Host 日志并 grep | 搜不到；IP 和主机名已打码 |
| SEC-15 | App 数据 | 检查加密备份和沙盒 | 设备私钥在安全芯片里，导不出；没有明文 SSH 私钥或密码 |
| SEC-17 | Host 本地 IPC | 用另一个本机用户连 `agent.sock` / 命名管道 | 连不上 |
| SEC-18 | 模糊测试 | 上面「模糊测试」的全部目标 | 每个跑满 30 分钟，不崩 |

## 性能预算

超出预算算缺陷，要开问题单。

| 项 | 预算 |
|---|---|
| App 冷启动到首页（有缓存） | ≤ 1.5 s（iPhone 15 级别机型） |
| 会话中 App 内存 | ≤ 250 MB；8 小时内不持续增长 |
| 会话中 App 耗电 | 1 小时 1080p60 ≤ 20% 电量（M1 实测后修订） |
| Host CPU | ≤ 15%（1080p60，硬件编码）；没有会话时 ≤ 0.5% |
| Host 内存 | ≤ 200 MB（有会话时）；≤ 60 MB（空闲时） |
| iOS 下载大小 | ≤ 40 MB |
| Mac DMG | ≤ 30 MB |

## CI

GitHub 上用 Actions（`.github/workflows/ci.yml`）；`.gitlab-ci.yml` 是同一套检查给自托管 runner 用的版本。两边都调 `tools/ci/check-linux.sh` / `check-mac.sh`，结构如下：

```
合并请求：
  lint        cargo fmt / clippy / go vet / tsc / eslint
  unit        cargo nextest、go test、jest
  integration just e2e-fake（Linux，需要 CAP_NET_ADMIN 建网络命名空间）
  build-mac   macOS runner：iOS App（模拟器 + 真机，不签名）、Mac Host（不签名）
每晚：
  fuzz        每个目标 10 分钟
  latency     host-fake 管线计时回归
  deny        cargo deny（新的安全公告）
打 tag（v*）：
  release     构建全部产物，交给发版机签名（见下）
```

- 缓存：`~/.cargo/registry`、`target/`（按 `Cargo.lock` 的哈希）、`~/go/pkg/mod`、pnpm store。
- macOS runner 用自己的 Mac mini（同时是测试 Host），注册成专用 runner。
- CI 上没有签名密钥。签名只在发版机器上做（02 · 密钥与敏感信息）。

## 签名与发布

### 产物与渠道

| 产物 | 签名 | 渠道 | 命令 |
|---|---|---|---|
| iOS / iPadOS App | Apple Distribution；App Store Connect API Key | TestFlight 内部组（测试版）→ 外部组（稳定版）；上 App Store 另行决定 | `just release-ios` |
| Mac Host | Developer ID Application；`notarytool` 公证 + `stapler` | 下载页 `[你的下载地址]` | `just release-mac` |
| Windows 虚拟显示器驱动（M3） | 正式：EV 证书 + Microsoft Partner Center attestation 签名；内测：自签名证书 | 随 MSI（以后）；测试机用 install.ps1 | `drivers/win-iddcx/build.ps1` |
| Windows Host（M2） | EV 代码签名（硬件令牌或云签名服务） | 同上；MSI 给 IT 静默部署 | `just release-win` |
| Mac 电脑端（M3） | Developer ID Application + hardened runtime；`notarytool` 公证 + `stapler` | 下载页 `[你的下载地址]`；`tauri-plugin-updater` 读同站 `desktop/latest.json` | `just release-desktop-mac` |
| Windows 电脑端（M3） | EV 代码签名（同 Windows Host） | 同上；NSIS 按用户安装 | `just release-desktop-win` |
| 电脑端更新清单 | Tauri updater 私钥（minisign，离线保存，与 Host 的 Ed25519 发布密钥分开） | 下载站 | 发布脚本生成 |
| Android App（M2） | 自有 app signing key | Play 内部测试 → 正式；APK 放 `[你的下载地址]` | `just release-android` |
| Host 更新清单 | Ed25519 发布私钥（离线保存） | 暂无（没有更新源，Host 手动更新，见 06 · 自动更新） | — |

- **Ed25519 发布密钥**：离线生成，私钥放密码管理器或硬件令牌，只在发版机器上使用。公钥编进 agent。换钥流程：新版 agent 同时内置新旧两把公钥，过渡一个版本后再删旧的。
- **Android 签名**：APK 渠道和 Play 必须是同一把签名密钥，否则两个渠道之间不能互相升级。做法是在 Play App Signing 里上传自己的 app signing key，不用 Google 生成的。
- **Android 版本号**：`versionName` 取根 `Cargo.toml` 的 `[workspace.package] version`；`versionCode = 主×1000000 + 次×10000 + 修订×100 + 构建序号`，构建序号为 0–99，同一版本重发只增加构建序号，`versionCode` 只增不减。
- **Android 签名材料**：默认读本地 gitignore 文件 `app/android/keystore.local.properties` 和其中指向的 keystore（如 `app/android/app/release.keystore`），缺字段或缺文件直接失败，不会退回 debug 签名。格式和可选的 Nacos 拉取见 [tools/release/README.md](../../tools/release/README.md)。仓库不记录地址、账号、口令或证书指纹。
- **Android 发布脚本**：`just release-android`（`tools/release/android.sh`）同时产出 AAB 和 APK，并由 `verify.sh` 检查 APK/AAB 签名一致、非 debug 签名、版本号、不可调试、64 位 ABI、16 KB 页对齐；AAB 上传 Play，APK 放下载页 `[你的下载地址]`。Play App Signing 上传的 app signing key 必须是同一把，见上条。
- **CocoaPods 版本**：`app/ios/Podfile.lock` 由 CocoaPods 1.17.0 生成。发布机要使用同一版本；否则 `pod install` 会改 `COCOAPODS:` 行和若干校验和。
- **host-ui sidecar 文件名**：`host-ui/src-tauri/tauri.conf.json` 的 `externalBin` 是 `binaries/rs-host-agent`；Tauri 按目标三元组把它解析为带后缀的文件（`host-ui/src-tauri/build.rs:7-21`），不是直接找无后缀文件。Windows 目标三元组是 `x86_64-pc-windows-msvc`（`tools/release/win.ps1:178、223-235`），因此 Tauri 的 Windows bundle 若要带 sidecar，文件须是 `host-ui/src-tauri/binaries/rs-host-agent-x86_64-pc-windows-msvc.exe`。macOS 见 `tools/release/mac/build.sh:80-95`：普通包放 `rs-host-agent-<target>`；universal 包另需 `rs-host-agent-universal-apple-darwin` 和两个瘦三元组各一份。Linux 交叉检查可设置 `TAURI_CONFIG='{"bundle":{"externalBin":[]}}'` 绕过该前置；`tools/release/win.ps1:237-244` 的正式 Windows 打包本身用 `--no-bundle` 和空 `externalBin` 构建 Tauri 托盘，agent 由 WiX 作为服务单独安装。
- **构建密钥脚本（可选）**：`app/scripts/nacos-build-secrets.sh` 只在 `BUILD_SECRETS_PROVIDER=nacos`（或给出了 Nacos 坐标 / `NACOS_CONFIG_FILE`）时启用，`NACOS_SERVER`、`NACOS_NAMESPACE`、`NACOS_GROUP`、`NACOS_DATA_ID` 都没有内置默认值，缺一个就在发请求前报错。pull 的网络请求失败或返回非 200 时，若本地密钥文件齐全则用本地文件并成功退出，否则报错退出；`--require-android-signing` 会同时要求 Android 签名属性和 keystore。
- **Android Rust 库**：`tools/release/android.sh` 的第 5 步编译 Rust 后，将 `RS_MOBILE_ANDROID_LIBS` 指向本次构建输出（默认 `app/native/core/android/src/main/jniLibs`，或 `RS_MOBILE_ANDROID_OUT` 指定的输出）；环境中预设的预编译库只在 `--skip-rust` 时使用。
- **iOS 版本号**：`CFBundleShortVersionString` 取根 `Cargo.toml` 的 `[workspace.package] version`；`CFBundleVersion` 与 Android `versionCode` 同一公式（构建序号 0–99）。`tools/release/ios.sh` 用 build setting 覆盖传给 xcodebuild；pbxproj 里的值由 `just bump` 维护，只影响本地调试构建。同一版本重传 TestFlight 只增加构建序号。
- **iOS 签名材料**：默认读 gitignore 的 `app/ios/asc-api-key.properties`（Key ID、Issuer ID）和 `app/AuthKey_<KeyID>.p8`（权限 600），也可以用上面的 Nacos 脚本 `pull --platform ios` 生成。Team ID 只在发版机上通过 `RS_IOS_TEAM_ID` 或 `--team-id` 提供，不进仓库。签名用自动签名 + `-allowProvisioningUpdates` + ASC API Key，发版机不需要登录 Xcode 账号。
- **iOS 发布脚本**：`just release-ios`（`tools/release/ios.sh`）archive → export（ExportOptions 运行时生成到临时目录，`app-store-connect`）→ `verify.sh` 检查 Apple Distribution 签名、Team、描述文件不可调试、版本号、arm64、PrivacyInfo → `--upload` 上传 TestFlight；`--dry-run` 不需要任何密钥，只做未签名 archive 和校验（CI 可用）。产出 IPA、dSYM zip、`SHA256SUMS`、`release-summary.json`，日志里的 Key/Issuer/Team 一律打码。
- 构建期密钥（证书、API Key、keystore）只放在本地 gitignore 的文件里，或由你自己的密钥服务在构建前拉下来。

### 发版步骤

1. `just bump 0.3.0` → 更新 `CHANGELOG.md` → 提交 → 打 tag `v0.3.0`。
2. CI 的 release 任务构建全部产物（不签名），放到制品库。
3. 发版机器：签名、公证（Mac）、签名（Windows），并上传 TestFlight。
4. 先发测试版：TestFlight 内部组；Host 的测试版包放下载页。
5. 自用 3 天，没有严重问题（崩溃、连不上、安全问题） → 发稳定版：TestFlight 外部组；Host 的稳定版包放下载页。
6. 观察崩溃和反馈；有问题就撤下稳定版包。

### 版本对应关系

- App 和 Host 同一个版本号（D19）。App 0.3.x 要求 Host ≥ 0.3.0。
- 发 App 新版本前，先确认自己的电脑上 Host 都已手动升到它要求的最低版本。否则会遇到 E170。

## Headscale

M1 开工前（I4）和上线前各核对一次。

### 需要的配置

- **tag**：`tag:rs-host`（装了 Host 的电脑）、`tag:server`（服务器）。在 `tagOwners` 里指定由管理员分配。
- **预授权密钥**：给 Host 批量部署时，`headscale preauthkeys create --user <user> --tags tag:rs-host --reusable --expiration 24h`。参数名按部署的 Headscale 版本核对。
- **DERP**：保留国内自建 DERP，打开 `verify_clients`。

### ACL 示例

只演示结构，地址是示例段。

```json
{
  "groups": { "group:rs-users": ["alice@"] },
  "tagOwners": {
    "tag:rs-host": ["group:rs-users"],
    "tag:server":  ["group:rs-users"]
  },
  "acls": [
    { "action": "accept", "src": ["group:rs-users"], "dst": ["tag:rs-host:7400", "tag:server:22"] }
  ],
  "ssh": [
    { "action": "check",  "src": ["group:rs-users"], "dst": ["tag:server"], "users": ["root"] },
    { "action": "accept", "src": ["group:rs-users"], "dst": ["tag:server"], "users": ["autogroup:nonroot"] }
  ]
}
```

- Host 之间不需要互通。
- 7400 同时放行 UDP 和 TCP（ACL 不写协议时两者都放）。

### 验证

| 检查 | 命令 | 预期 |
|---|---|---|
| 手机到 Host | App 诊断页探测 | UDP、TCP 都通 |
| 不该通的 | 服务器 `nc -vz <Host tailnet 地址> 7400` | 不通 |

## 没有中心服务

2026-10-24 起不再部署 Hub（[01 · D51](01-decisions.md#d51--移除-hub2026-10-24)）。会话本来就不经过它；原来由它提供的能力现在这样处理：

| 功能 | 现在 |
|---|---|
| 已配对设备连接 Host | 用本机保存的指纹与地址 |
| 设备目录 | 已配对电脑、手动电脑和 tailnet 节点（App 内节点，或填了 token 的 Tailscale / Headscale API）组成本机目录；只用系统 Tailscale 又没填 token 时，没装 Host 的电脑（`others`）为空 |
| 作废 | 在每台 Host 的菜单里本地移除设备；Headscale 上手动删节点；服务器上的公钥手动删 |
| 升级、唤醒 | 没有；Host 手动更新 |
| 配对 | 地址 + 六位码、扫码、配对文件；电脑系统密码配对没有入口 |
| 同步 | 没有跨设备同步，数据只保存在本机 |
| 连接记录 | 留在每台电脑本机（`audit.list`） |
