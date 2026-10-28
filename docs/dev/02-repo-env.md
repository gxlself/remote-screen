# 02 · 仓库、环境与约定

## 目标结构

```
remote-screen/
├─ Cargo.toml                  # Rust workspace
├─ rust-toolchain.toml         # 固定 Rust 版本与 target
├─ justfile                    # 统一入口：just check / just host / just ios …
├─ deny.toml                   # cargo-deny：许可证白名单（禁止 GPL/AGPL）、漏洞
├─ .config/nextest.toml        # cargo-nextest 配置
├─ proto/
│  ├─ rsp/v1/control.proto     # 控制面消息（03-protocol）
│  ├─ rsp/v1/streams.proto     # 光标、剪贴板、文件等流内消息头
│  ├─ rsp/v1/trust.proto       # 信任声明（作废 / 批准 / 要求确认 / 登记第二节点），04 · 信任声明
├─ crates/
│  ├─ rs-audio/                # 声音：Opus 封装、分帧、静音门限、抖动缓冲、播放器、测试信号与指标
│  ├─ rsp-core/                # 协议本体，无 I/O：wire、control、video v2 + FEC、input、pointer、clock、ratectl
│  ├─ rsp-files/               # 文件通道 0x04 的收发引擎，host-core 与 rs-mobile 共用
│  ├─ rsp-transport/           # quinn 与 TCP 端点、TLS（mTLS、SPKI 固定、外部签名）、通道复用、Dialer 抽象
│  ├─ rsp-client/              # 客户端会话引擎：多路径拨号、会话状态机、重组与 FEC、反馈、恢复、预热
│  ├─ rs-ssh/                  # russh 封装：认证、known_hosts、PTY、tmux、跳板机、SFTP
│  ├─ rs-tailnet/              # 进程内嵌 tailnet 节点的 Rust 绑定
│  ├─ rs-mobile/               # App 用的 Rust 库：uniffi 控制面 + C ABI 热路径（取代 M0 的 rsffi）
│  ├─ rs-desktop/              # 电脑端控制端：键码 / 指针映射、会话控制器、RsCore 门面，Mac（Swift）/ Windows 平台胶水（M3）
│  ├─ rs-vnc/                  # RFB 3.8 客户端：握手、VNC/ARD 认证、Raw/CopyRect/ZRLE、假服务器
│  ├─ rs-rdp/                  # IronRDP 封装：TLS（TOFU）、NLA、位图、指针、输入映射
│  ├─ host-core/               # 被控端平台无关部分：监听、鉴权、配对、会话、管线、更新、IPC
│  ├─ host-mac/                # macOS 平台层（ObjC shim：SCK、VideoToolbox、CGEvent、光标、剪贴板、电源）
│  ├─ host-win/                # Windows 平台层（DXGI、FFmpeg / MF、SendInput、服务、会话进程、防火墙）
│  ├─ host-linux/              # Linux 平台层（PipeWire / xdg-desktop-portal / X11、FFmpeg VA-API / NVENC、openh264）
│  ├─ host-fake/               # 测试用平台层：合成画面 + 记录注入的输入，CI 在 Linux 上跑全链路
│  └─ rs-host-agent/           # 被控端常驻进程（bin），按 cfg(target_os) 装配平台层
├─ drivers/
│  └─ win-iddcx/               # Windows 虚拟显示器驱动（IddCx，UMDF 2，C++；VS 2022 + WDK 构建，M3）
├─ host-ui/                    # Tauri 2：菜单栏 / 托盘、设置、首次向导、确认弹窗
├─ desktop/                    # 电脑端控制端的 Tauri 2 外壳（M3）：设备列表、配对、设置；会话窗口在 rs-desktop
├─ app/                        # React Native App（07-app）
│  ├─ src/
│  ├─ ios/                     # RN iOS 工程 + 原生组件（Swift）
│  ├─ android/                 # RN Android 工程 + 原生组件（Kotlin）
│  └─ rust/                    # 构建 rs-mobile：xcframework、jniLibs、uniffi 绑定生成
├─ tailnet/                    # Go module：进程内嵌 tailnet 节点
│  ├─ include/                 # rs_tailnet.h C ABI 头
│  ├─ cmd/rstailnet/           # 导出 C ABI 的 main 包
│  ├─ internal/                # 节点、网卡与测试支持
│  │  ├─ node/                 # 内嵌 tsnet 节点
│  │  ├─ ifaces/               # Android 11+ 上用 getifaddrs 取网卡
│  │  ├─ testnet/              # 进程内测试 tailnet
│  │  └─ testcontrol/          # 上游 tailscale 测试控制面拷贝（BSD 许可）
│  └─ scripts/                 # build-host.sh、build-ios.sh、build-android.sh、abi_smoke.c
├─ tools/
│  ├─ ci/                      # check-linux.sh、check-mac.sh，本地和 CI 共用
│  ├─ probe/                   # rs-probe（M0，保留）
│  ├─ netem/                   # 弱网脚本
│  ├─ latency/                 # glass-to-glass 计时页与读数整理，含 faultnet-proxy（弱网代理 crate）
│  ├─ fuzz/                    # run-all.sh
│  └─ release/                 # mac.sh、win.ps1、android.sh 发布脚本
├─ fuzz/                       # cargo-fuzz 目标
└─ docs/
   ├─ dev/                     # 本方案
   │  └─ spikes/               # spike 结论，每个一页
   └─ m0-latency.md
```

依赖方向（只允许向下依赖）：

| crate | 依赖的工作区 crate |
|---|---|
| rsp-core | — |
| rs-audio | — |
| rsp-transport | rsp-core |
| rsp-files | rsp-core、rsp-transport |
| rsp-client | rsp-core、rsp-transport |
| rs-ssh | rsp-transport |
| rs-tailnet | rsp-transport |
| host-core | rsp-core、rsp-transport、rsp-files、rs-audio |
| host-mac | rsp-core、host-core |
| host-win | rsp-core、host-core |
| host-linux | rsp-core、host-core、host-win（只用纯逻辑模块：ffmpeg_abi、encode_model、capture_model、activity_model、clip_convert） |
| host-fake | rsp-core、rsp-transport、rsp-client、rs-audio、host-core |
| rs-host-agent | rsp-core、host-core、host-mac、host-win、host-linux、host-fake |
| rs-mobile | rsp-core、rsp-transport、rsp-client、rsp-files、rs-audio、rs-ssh、rs-tailnet、rs-vnc、rs-rdp |
| rs-desktop | rsp-core、rsp-transport、rsp-client、rs-mobile（开发依赖 host-core、host-win，只做映射对照测试） |
| rs-vnc | — |
| rs-rdp | — |

`rsp-core` 不允许依赖 tokio、quinn 或任何平台 API，保证能在单测和模糊测试里跑。

`rs-tailnet` 的 `TailnetConfig.verbose` 在 Rust 侧是 `bool`；传入 Go ABI 的 `Config.Verbose` 是 `int`，边界转换为 `false → 0`、`true → 1`。Go 端仍校验 `0..=2`，Rust 公共配置不会产生 2。


## 开发环境

### 所有人

| 工具 | 版本 | 安装 |
|---|---|---|
| Rust | 固定在 `rust-toolchain.toml`（当前 1.96） | `rustup` 自动按文件安装 |
| just | 最新 | `brew install just` |
| cargo 插件 | — | `cargo install cargo-nextest cargo-deny cargo-ndk`；模糊测试另装 `cargo install cargo-fuzz`（需 nightly） |
| Tailscale | 最新 | 登录自建 Headscale：`tailscale up --login-server https://hs.[你的域名]` |

`rust-toolchain.toml`：

```toml
[toolchain]
channel = "1.96"
components = ["rustfmt", "clippy", "rust-src"]
targets = [
  "aarch64-apple-darwin", "x86_64-apple-darwin",
  "aarch64-apple-ios", "aarch64-apple-ios-sim",
  "aarch64-linux-android", "x86_64-linux-android",
]
```

Windows 机器另加 `x86_64-pc-windows-msvc`（在那台机器上用 `rustup target add`）。

### Mac（Host + iOS）

| 工具 | 版本 | 说明 |
|---|---|---|
| macOS | 15 或 26 | 测试矩阵两个版本都要（09-quality-release） |
| Xcode | 26.x | 含 iOS 26 SDK；`xcode-select -p` 指向它 |
| Node | 22 LTS | `brew install node@22` |
| pnpm | 9+ | `corepack enable` |
| CocoaPods | 1.16+ | RN iOS 仍用 Pods |
| Watchman | 最新 | RN 打包器 |
| Tauri CLI | 2.x | `cargo install tauri-cli --version "^2"` |
| uniffi-bindgen-react-native | 与 `app/package.json` 一致 | 作为 App 的 devDependency 安装，不全局装 |

### Android

| 工具 | 版本 |
|---|---|
| Android Studio | 最新稳定版 |
| SDK Platform | 36（Android 16） |
| NDK | 27.1（`ndkVersion` 写进 `app/android/build.gradle`） |
| JDK | 17 |

`app/android/local.properties` 要写 `sdk.dir=`（不进 git）。命令行构建不依赖 `ANDROID_HOME` 环境变量。

### Go（tailnet）

| 工具 | 版本 |
|---|---|
| Go | 1.26（`tailnet/go.mod` 里 `go 1.26`） |

### Windows（M2）

| 工具 | 说明 |
|---|---|
| Windows 11 + VS 2022 Build Tools | MSVC、Windows SDK |
| Windows Driver Kit（与 SDK 同版本）或 EWDK | 只在构建 `drivers/win-iddcx` 时需要；装在构建驱动的那台 Windows 上 |
| Rust msvc target | `x86_64-pc-windows-msvc` |
| FFmpeg 共享库 | LGPL 构建，版本固定，放 `crates/host-win/third_party/ffmpeg/`（不进 git，`just fetch-ffmpeg` 下载并校验 SHA-256） |
| WiX Toolset | v5，生成 MSI |
| 代码签名 | EV 证书（硬件令牌），只在发版机器上 |

### Linux（M3）

| 工具 | 说明 |
|---|---|
| 运行时库 | 系统自带：`libpipewire-0.3.so.0`、xdg-desktop-portal + 桌面后端、logind；VA-API 要 `libva.so.2` / `libva-drm.so.2` + 显卡驱动，NVENC 要 NVIDIA 驱动。构建期不需要任何 -dev 包 |
| FFmpeg 共享库 | LGPL 构建，8.1，只开 vaapi / nvenc，放 `crates/host-linux/third_party/ffmpeg/`（不进 git，`crates/host-linux/packaging/fetch-ffmpeg.sh` 下载并校验 SHA-256） |
| openh264 | 真编码测试要么开 feature `openh264-source` 从源码编（要 C++ 编译器），要么用 `RS_OPENH264_PATH` 指向现成的 `libopenh264.so.7`；两者都没有时这些用例跳过并打印原因。发布包运行时加载 Cisco 官方二进制 |
| 桌面集成测试 | 第 2–4 层要的 PipeWire、wireplumber、dbus、Xvfb、sway、xdg-desktop-portal(-wlr) 解开到一个本地 sysroot 目录（不装进系统），`crates/host-linux/tests/desktop/run.sh` 起环境；测试 feature `desktop-it` |
| AppImage | `appimagetool`（发版机器上），`crates/host-linux/packaging/build-appimage.sh` |

### 测试用 tailnet

在 Headscale 上为开发单独准备：

| 节点 | tag | 用途 |
|---|---|---|
| 你的 Mac（开发机） | `tag:rs-host` | 跑 `rs-host-agent` |
| 测试 iPhone / iPad / Pixel | 无 tag，属于 `group:ops` 用户 | App |
| 一台 Linux 服务器 | `tag:server` | SSH 测试；开 Tailscale SSH |
| 一台 Linux 服务器 | `tag:server` | SSH 测试；不开 Tailscale SSH，走密钥 |

ACL 用设计文档 §4.3 那份。

## 统一入口：justfile

```make
default: check

# 提交前必跑：格式、静态检查、全部单测、许可证
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run --workspace
    cargo deny check
    cd app && pnpm -s typecheck && pnpm -s lint && pnpm -s test

# 改了 proto/ 之后
proto:
    cargo build -p rsp-core

# 被控端（开发机直接跑，权限授给终端）
host *ARGS:
    RUST_LOG=info,rs=debug cargo run --release -p rs-host-agent -- {{ARGS}}

# 合成画面的被控端（Linux / CI 可跑）
host-fake *ARGS:
    RUST_LOG=info cargo run -p rs-host-agent --features fake -- --platform fake {{ARGS}}

probe *ARGS:
    cargo run --release -p rs-probe -- {{ARGS}}

# App
mobile-ios:
    app/rust/build-ios.sh          # 产出 app/ios/RsMobile.xcframework + uniffi 绑定
mobile-android:
    app/rust/build-android.sh      # 产出 app/android/app/src/main/jniLibs/* + uniffi 绑定
ios: mobile-ios
    cd app && pnpm ios
android: mobile-android
    cd app && pnpm android

host-ui:
    cd host-ui && cargo tauri dev

fuzz TARGET SECS="60":
    cargo +nightly fuzz run {{TARGET}} -- -max_total_time={{SECS}}

# 集成测试（Linux，需要 CAP_NET_ADMIN 建网络命名空间），见 09 · 集成测试
e2e-fake FILTER="":
    cargo nextest run -p host-fake --test e2e {{FILTER}}

# App 端到端（Maestro，模拟器），见 09 · 端到端
e2e-app PLATFORM="ios":
    cd app && pnpm e2e:{{PLATFORM}}

# 弱网场景，见 09 · 弱网
netem PROFILE IFACE:
    tools/netem/linux.sh {{PROFILE}} {{IFACE}}

# 发版（只在发版机器上跑，需要签名密钥），见 09 · 签名与发布
# 参数原样转给脚本，例如 `just release-ios --dry-run`、`just release-ios --upload`
release-ios *ARGS:
    tools/release/ios.sh {{ARGS}}
release-mac *ARGS:
    tools/release/mac.sh {{ARGS}}
release-win *ARGS:
    tools/release/win.ps1 {{ARGS}}
release-android *ARGS:
    tools/release/android.sh {{ARGS}}

bump VERSION:
    tools/bump-version.sh {{VERSION}}
```

## 代码约定

### Rust

- 库 crate 用 `thiserror` 定义错误；只有 bin（`rs-host-agent`、`rs-probe`、host-linux 的 `remote-host-ctl`）用 `anyhow`。
- 测试以外不写 `unwrap()` / `expect()`，确实不可能失败的地方用 `expect("说明为什么不可能")`。
- `unsafe` 只允许出现在 `host-mac`、`host-win`、`host-linux`、`rs-mobile`、`rs-desktop`、`rsp-transport`（自定义 UDP socket）里，每处写 `// SAFETY:` 说明。
- FFI 函数统一前缀 `rs_`；每个导出函数用 `catch_unwind` 包住，panic 不能穿过 FFI 边界；回调在哪个线程调用、指针有效期写在头文件注释里（沿用 M0 `rsffi.h` 的写法）。
- 日志用 `tracing`，带结构化字段：`session`、`peer`、`path`、`host`。**不记录**剪贴板内容、键入的文本、文件内容、密码、私钥。
- 时间一律 unix 微秒 `u64`（与 M0 一致），只在显示时格式化。
- 异步运行时只用 tokio。平台回调线程到 tokio 用 `mpsc`，不在回调里 `block_on`。

### Swift / Kotlin

- 原生组件只做三件事：上屏、手势、调用 C ABI。业务状态放 Rust 或 JS。
- Swift 用 5 语言模式，打开严格并发检查的警告；回调里不阻塞主线程。
- Kotlin 用协程，解码线程和 UI 线程分开。

### TypeScript

- `strict: true`；ESLint + Prettier；页面组件不写业务逻辑，放到 `src/features/<域>/` 的 hooks 里。
- 所有用户可见文字走 `src/i18n/zh.ts`，方便以后加英文；文案以画布为准。

### Go（tailnet）

- `gofmt` + `go vet`；错误用 `fmt.Errorf("...: %w", err)` 包装。

### 提交与分支

- 一个人开发时可以直接提交到 `main`。
- 多人或多个 agent 并行时，每个任务一个分支 `t/<任务编号>-<短名>`（如 `t/A3-fec`），合并前 `just check` 必须通过。
- 提交说明以任务编号开头：`A3: 视频分片 Reed-Solomon 编解码`。
- 协议改动（`proto/`、`rsp-core`）必须同时更新 [03-protocol.md](03-protocol.md) 和测试向量。

### 版本号

- 单一来源：根 `Cargo.toml` 的 `workspace.package.version`。`just bump 0.2.3`（`tools/bump-version.sh`）同时改 `Cargo.toml` 和各 `Cargo.lock` 里的工作区包、`app/package.json`、iOS `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION`、`host-ui` 的 `package.json` / `tauri.conf.json` / `Cargo.toml` / `Cargo.lock`；`tools/bump-version.sh --check` 只检查不改。Android `versionName` / `versionCode` 在构建时由 Gradle 从 `Cargo.toml` 算出，不用改文件。
- 构建号：iOS / Android 用 CI 构建序号；Host 用 `git rev-list --count HEAD`。

### 密钥与敏感信息

- 签名证书、发布密钥、Headscale API Key、预授权密钥都不进仓库，也不写进文档示例。构建时由发版机器注入（沿用你们现有 RN App 的构建密钥托管方式）。
- 文档和测试里的地址只用示例段：tailnet `100.64.0.x`，公司内网 `10.30.x.x`，VPN 地址池 `10.99.x.x`，家里 `192.168.31.x`，域名 `hs.[你的域名]`、`[公司域名]`。
