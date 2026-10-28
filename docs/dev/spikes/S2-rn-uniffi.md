> 已转正为 `app/`。
> 验证用的代码（原 `spikes/` 目录）没有随开源版本发布，这里只保留结论和当时的做法，下面的命令不能直接运行。

# S2 · React Native 新架构、UniFFI 与原生视频

2026-10-20。工程原在 `spikes/s2-rn/`。

**结论：四条标准中三条通过；视频链路在模拟器上跑通（收帧、解码、0 失败），但 decode p50 比同条件下的 M0 慢约 2.5 ms，超出 ≤ 1 ms，转到 P3 在真机上复测和排查（见文末「维护者实测」）。D1 / D2 不需要改。**
已产出 Debug / Release 模拟器应用，以及无签名 arm64 真机 Release 应用。
维护者已在增补前的 Release 上验证 JS 自检 pass：ping 0.48 ms、slow 305 ms、
10 个事件、停止后 0 个。本文将该运行结果与本地构建证据区分记录。
按维护者补充要求，本轮没有再执行
`simctl` / `react-native run-ios`，也没有启动 Host。

## 四条通过标准

| 标准（08 · S2） | 结论 | 证据 / 数字 |
|---|---|---|
| JS 同步、异步、事件正常 | 完成：维护者已在模拟器 Release 验证 pass | ping 0.48 ms、slow 305 ms、10 个事件、停止后 0 个。页面显示 JSON，并通过 Fabric prop 额外转到原生 `NSLog("S2-SELFTEST %@", json)`；新日志通路待维护者运行确认。 |
| 一个静态库供 UniFFI 和 Swift 使用、无重复符号 | 完成 | 两个 iOS Rust target 编译成功；Debug、Release 的实际链接均成功；最终 Debug dylib 中 `rs_client_connect`、`rs_client_close`、UniFFI `ping` 各定义 1 次。 |
| 原生组件出画面，decode p50 与 M0 差 ≤ 1 ms | 收帧解码通过；**decode 差值未达标**（模拟器） | 同一模拟器、同一 Host、同一提交：S2 decode p50 **5.5–5.9 ms**（p95 7.6–7.7），M0 Demo **3.3 ms**（p95 5.2，n=1877）；两者 `VideoDecoder.swift` 与计时点一致。转 P3 真机复测，见文末。 |
| Release 包体积增量 | 完成（首次对照实验） | arm64 模拟器：完整包 25.247 MiB，空模块基线 20.904 MiB，增量 **4.343 MiB（4,554,380 字节）**。自动连接 / 日志增补后完整包为 25.268 MiB，未重跑空基线。 |

原卡要求的 `-workspace` 构建入口仍受当前环境限制，见下文；替代的两工程构建已
实际编译并链接 `.app`。这项入口检查需维护者在沙箱外补跑。

## 版本与环境

| 项目 | 实际版本 |
|---|---|
| React Native / React | 0.88.0 / 19.3.0 |
| React Native Community CLI | 20.2.0（通过 latest 初始化） |
| uniffi-bindgen-react-native / @ubjs/core | 0.31.0-6 / 0.31.0-6，本地 npm 依赖 |
| Rust UniFFI | 0.31.2，独立 Cargo.lock 锁定 |
| Tokio / thiserror | 1.54.0 / 2.0.21，独立 Cargo.lock 锁定 |
| Node / npm | 22.22.2 / 10.9.7 |
| Xcode / iOS SDK | 26.5（17F42）/ 26.5 |
| Homebrew Rust / rustup stable | 1.96.0 / 1.99.1 |
| CocoaPods | 1.17.0，已有 `/opt/homebrew/bin/pod` |
| 最低 iOS / 模拟器架构 | 17.0 / arm64 |

RN 的新架构保持开启：Podfile 设置 `RCT_NEW_ARCH_ENABLED=1`，Info.plist 的
`RCTNewArchEnabled=true`；视频组件直接继承 `RCTViewComponentView`，注册生成的
Fabric descriptor，没有使用旧架构 interop 代替。

## 实现与 P1 起点

- `rust/` 是独立 Cargo workspace（空 `[workspace]`），包含自己的 Cargo.lock。
  `s2-rn-ffi` 输出 staticlib / rlib，以 `../../../crates/rs-mobile` 为 path 依赖。
  根 Cargo.toml、Cargo.lock、`crates/**`、`tools/**`、现有契约均未修改。
- Rust 导出同步 `ping`、基于 Tokio 的异步 `slow(ms)`，以及
  `EventSink.on_event(seq: u64, payload: String)`。`start_events` 立即发 seq=0，
  随后每次回调完成后等待 500 ms；重复订阅替换旧任务，`stop_events` 取消任务。
- `s2_mobile_link_anchor` 引用 `rs_mobile::rs_now_us`，使依赖进入同一个
  `libs2_rn_ffi.a`；原生端直接 include 现有 `rsffi.h`，不再单独链接
  `librs_mobile.a`。每个平台只有一个 Rust archive，xcframework 只封装平台切片。
- `native/core/` 是本地 npm 模块，生成 `S2Core` TurboModule / JSI 绑定。
  `scripts/generate-bindings.sh` 可重建绑定和 xcframework；大型生成目录不提交。
- `RsVideoView` 是直接实现的 Fabric 组件。Swift surface 使用
  `AVSampleBufferDisplayLayer`，通过 M0 C ABI 连接。`RemoteClient.swift` 和
  `VideoDecoder.swift` 从 M0 复制，仅增加体积基线所需条件编译；未改 M0 原文件。
  帧回调、VideoToolbox 解码和 enqueue 全在原生侧，JS 接收每 2 秒统计及状态变化。
- 首页启动时自检，也可重跑。结果、事件列表和视频统计在界面可见；Host / port
  输入框默认 `127.0.0.1:7400`，有 Connect / Disconnect。失败也显示 JSON，
  不依赖控制台才能判定结果。Android 保留 RN 模板，未验证。
- 无交互启动参数 `-autoConnect 127.0.0.1:7400` 由 SceneDelegate 从
  `ProcessInfo.processInfo.arguments` 读取，经 `initialProperties.autoConnect`
  传给 JS。首次挂载直接启用视频并填充 Host / port；不带参数仍等待手动连接。
- `S2-VIDEO` 由原生 `NSLog` 每 2 秒输出，并在状态变化时立即输出。JSON 包含
  `received`、`decoded`、`failures`、`decodeP50Ms`、`decodeP95Ms`、`sampleCount`、
  `width`、`height`、`fps`、`status`、`error`。连接失败、远端关闭、主动断开和
  视图销毁均输出 `status="error"` 与原因；无有效解码样本时 p50 / p95 均为 -1。
  自检 JSON 通过 `RsVideoView.selfTestJSON` 转到 ObjC++ 的 `NSLog`，无额外原生模块。

这里沿用 M0 的 `rs_client_connect`，没有提前实现 07 · App 中的
`SessionId` / `rs_session_attach` 或完整 `RsCore` 产品 API。P1 应按现有契约迁移，
本 spike 只提供集成起点；无契约变更请求。

## 构建与交付

下面路径均相对仓库根；bundle id 统一为 **`ai.gxlself.remotescreen.s2`**。

| 构建 | `.app` 路径 | JS |
|---|---|---|
| Debug 模拟器（完整） | `spikes/s2-rn/ios/build/Build/Products/Debug-iphonesimulator/S2RN.app` | 需 Metro |
| Release 模拟器（完整） | `spikes/s2-rn/ios/build-release-full/Build/Products/Release-iphonesimulator/S2RN.app` | 已内嵌 `main.jsbundle` |
| Release 模拟器（空模块） | `spikes/s2-rn/ios/build-release-baseline/Build/Products/Release-iphonesimulator/S2RN.app` | 已内嵌，仅体积实验 |
| Release 真机（完整，无签名） | `spikes/s2-rn/ios/build-release-full-device/Build/Products/Release-iphoneos/S2RN.app` | 已内嵌，不能直接安装到真机 |

自动连接 / 系统日志增补后，只重新构建了上表的 **Release 模拟器完整包**，覆盖原路径。
Debug、空基线及真机包保留首次构建版本，不包含本次增补。

可复现准备步骤：

```sh
cd spikes/s2-rn
npm ci
npm run bindings
bash scripts/build-ios.sh Debug full
```

脚本使用已有 CocoaPods，不需全局安装；pod install 后自动运行幂等的
`scripts/configure-project.rb`。生成绑定先于 Pods；否则 Pods 找不到 xcframework。

### 当前环境的 workspace 入口限制

标准命令在加载 workspace 时退出，尚未进入编译：

```sh
# 从仓库根执行；待维护者在沙箱外补验。
xcodebuild -workspace spikes/s2-rn/ios/S2RN.xcworkspace -scheme S2RN \
  -sdk iphonesimulator -configuration Debug \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath spikes/s2-rn/ios/build CODE_SIGNING_ALLOWED=NO build
```

实际诊断包括：

```text
CoreSimulatorService ... Code=61 "Connection refused"
xcodebuild: error: '.../ios/S2RN.xcworkspace' is not a workspace file.
```

相对路径、绝对路径以及最小临时 workspace 都被拒绝，`-project` 可以加载。
两种诊断同时出现，尚不能确认两者的因果关系。没有修改开发者的 Simulator 服务或配置。

本次通过的替代命令（从 `spikes/s2-rn` 执行）为：

```sh
xcodebuild -project ios/Pods/Pods.xcodeproj -scheme Pods-S2RN \
  -sdk iphonesimulator -configuration Debug \
  -destination 'generic/platform=iOS Simulator' -derivedDataPath ios/build \
  -jobs 4 BUILD_DIR="$PWD/ios/build/Build/Products" CODE_SIGNING_ALLOWED=NO build
xcodebuild -project ios/S2RN.xcodeproj -scheme S2RN \
  -sdk iphonesimulator -configuration Debug \
  -destination 'generic/platform=iOS Simulator' -derivedDataPath ios/build \
  -jobs 4 BUILD_DIR="$PWD/ios/build/Build/Products" CODE_SIGNING_ALLOWED=NO build
```

两次输出均为 `** BUILD SUCCEEDED **`。必须显式共用 `BUILD_DIR`，否则两个工程
默认产品路径不同，App 找不到 Pods 的头文件和库。脚本封装了这个入口：

```sh
S2_XCODE_PROJECTS=1 bash scripts/build-ios.sh Debug full
S2_XCODE_PROJECTS=1 bash scripts/build-ios.sh Release full
S2_XCODE_PROJECTS=1 bash scripts/build-ios.sh Release baseline
S2_XCODE_PROJECTS=1 bash scripts/build-ios.sh Release full iphoneos
```

Debug、基线和真机构建实际使用了该脚本，Pods 和 App 均构建成功。最终重新执行
了 Debug full，保持完整模块与 Swift 配置；如单独重跑基线实验，结束后执行普通
pod install 恢复：

```sh
/opt/homebrew/bin/pod install --project-directory=ios
```

### 同库与最终链接证据

```sh
xcrun nm -gU rust/target/aarch64-apple-ios-sim/release/libs2_rn_ffi.a \
  | rg ' _(rs_client_connect|rs_client_close|uniffi_s2_rn_ffi_fn_func_ping)$'
xcrun nm -gU ios/build/Build/Products/Debug-iphonesimulator/S2RN.app/S2RN.debug.dylib \
  | rg ' _(rs_client_connect|rs_client_close|rs_now_us|uniffi_s2_rn_ffi_fn_func_ping)$'
```

最终 Debug dylib 的实测输出（每项 1 行）：

```text
00000000000c17a4 T _rs_client_close
00000000000c1a74 T _rs_client_connect
00000000000c24d8 T _rs_now_us
00000000000a3bb8 T _uniffi_s2_rn_ffi_fn_func_ping
```

Debug 链接日志包含 `Ld .../S2RN.app/S2RN.debug.dylib`、`Ld .../S2RN.app/S2RN`，
最后 `** BUILD SUCCEEDED **`。Release 同样成功，最终 executable 也包含上述符号。
Xcode Debug 将原生实现放在 `.debug.dylib`，只检查小型主 executable 会漏判。
链接器仅提示重复的 `-lc++` 链接参数，没有 duplicate symbols 错误。

### Release 体积口径

由 `scripts/app-size.py` 递归累计 `.app` 内普通文件逻辑字节数，跳过 symlink；
1 MiB = 1,048,576 字节。不是压缩 IPA、App Store 下载量或 archive 的大小。
下表是自动连接 / 系统日志增补前的首次对照实验：

| 配置（均为 arm64 Release） | `.app` 字节 | MiB | 主 executable 字节 |
|---|---:|---:|---:|
| iphonesimulator 完整 | 26,473,550 | 25.247 | 5,000,328 |
| iphonesimulator 空 TurboModule / 空 surface | 21,919,170 | 20.904 | 445,944 |
| 完整减基线 | **4,554,380** | **4.343** | 4,554,384 |
| iphoneos 完整（无签名） | 26,287,676 | 25.070 | 4,993,568 |

模拟器 `du -sk` 分别为 26,212 / 21,764 KiB，文件系统分配块口径与上表不同。
基线保留相同 JS 源码、RN / Hermes、UniFFI 支持 Pod 和 Fabric 类名；用空的
`S2Core` TurboModule 替换生成绑定，去掉 Rust archive 和原生解码实现。
因此增量包括 Rust、生成的 C++ 绑定和视频实现，不能理解为纯 Rust 代码字节。
两次 Hermes bundle 内嵌了不同输出路径，基线 bundle 比完整包多 4 字节；这解释了
`.app` 增量与 executable 增量的 4 字节差。真机没有另构建空基线，不推导真机增量。

基线 executable 的 `nm -gU ... | rg '_(rs_client_|uniffi_s2_rn_ffi_)'`
无输出（rg exit 1），确认 Rust 接口没有残留。

本次增补后的完整 Release 模拟器包为 **26,495,445 字节（25.268 MiB）**，
executable 为 5,021,624 字节，确认 `main.jsbundle` 已内嵌。只按要求重建完整包，
未重跑基线，因此保留首次 4.343 MiB 对照结论，避免混用两版源码重新计算增量。
构建命令仍为 `S2_XCODE_PROJECTS=1 bash scripts/build-ios.sh Release full`，
Pods 和 App 两次均输出 `** BUILD SUCCEEDED **`。

## 其他验证

| 命令 | 结果 |
|---|---|
| `cargo build --manifest-path rust/Cargo.toml` | 成功（本机 Rust 1.96） |
| `cargo test --manifest-path rust/Cargo.toml --locked` | `1 passed; 0 failed`，0.54 s；覆盖异步定时、事件顺序、替换和停止释放 |
| `cargo fmt --manifest-path rust/Cargo.toml -- --check` | 通过 |
| `npm run typecheck` | 通过 |
| `npm run lint` | 通过，无错误或警告 |
| `bash -n scripts/generate-bindings.sh scripts/build-ios.sh` | 通过 |
| 仓库根 `cargo check --workspace` | 通过，`Finished dev profile ... in 30.44s` |

构建日志保存在本机 `/private/tmp/rs-s2-*.log`，不提交生成日志或产物。
关键日志为 `debug-{pods,app}-build`、`debug-final`、`release-full-{pods,app}`、`release-baseline`、
`release-device`、`release-automation`、`rust-test`、`workspace-check`；均带 `rs-s2-` 前缀与 `.log` 后缀。

## 遇到的坑与限制

1. 仅执行 `rustup run stable cargo` 仍可能从 PATH 取到 Homebrew rustc，从而报
   iOS target 缺 std。脚本显式用 `rustup which --toolchain stable rustc` 设置
   `RUSTC`，并移除从 macOS 构建继承的 SDKROOT / CFLAGS 等，使用 iOS 17 部署目标。
2. UBRN 本地 package 的 `repository` 要用 `{type,url}` 对象，字符串会使生成器 panic。
   `turboModule.name` 必须与 RN codegen 的 `S2CoreSpec` 一致；实际模块名是
   `S2Core`。名称配错会生成自包含头文件。`generate jsi turbo-module` 没有 `--ios`
   参数；正常复现直接使用 bindings 脚本即可。
3. 生成的外部回调会等待 JS 线程。`stop_events` 不能同步 join 正在回调的任务，
   否则 JS 等 Rust、Rust 等 JS；这里使用停止标记与 abort，允许最多一个已排队事件。
   自检还观察停止后 750 ms。此实现验证单个订阅，P1 应接入正式事件生命周期契约。
4. 开发者 CocoaPods 默认缓存目录经 symlink 指到沙箱外，首次安装写缓存受阻。
   Podfile 将缓存显式放在工程 `ios/build/pod-cache`，没有修改用户配置。
5. RN 0.88 的 codegen 类型从 `react-native` 根导出；旧的深层类型路径不可用。
   ObjC++ 引入 `S2RN-Swift.h` 前还需先引入 RN factory delegate 头，供模板
   `SceneDelegate` 的父类声明使用。
6. 沙箱内 ibtool 报 iOS 26.5 platform 未安装，虽然 SDK 编译可以执行。去掉模板
   launch storyboard 的构建资源引用，使用标准 `UILaunchScreen={}`，不依赖 ibtool。
   没有更改 Xcode 安装或 Simulator runtime。
7. Metro 关闭 Watchman 并限制 4 个 worker，避免依赖用户级 Watchman 服务。
   构建日志仍有 RN / Pods 的 nullability、deprecated API 等上游警告。

## 维护者补验（在可访问 Simulator 的终端）

先补跑上述 `-workspace` Debug 命令。Debug 启动 Metro：

```sh
cd spikes/s2-rn
npm start -- --host 127.0.0.1
```

另开终端，从仓库根执行（用 `simctl list` 中实际 iPhone UDID 替换示例变量）：

```sh
xcrun simctl list devices available
S2_SIMULATOR_UDID='<available-iPhone-UDID>'
xcrun simctl boot "$S2_SIMULATOR_UDID"
xcrun simctl bootstatus "$S2_SIMULATOR_UDID" -b
xcrun simctl install "$S2_SIMULATOR_UDID" \
  spikes/s2-rn/ios/build/Build/Products/Debug-iphonesimulator/S2RN.app
xcrun simctl launch --console "$S2_SIMULATOR_UDID" ai.gxlself.remotescreen.s2
```

视频补验优先使用新 Release 完整包，无需 Metro。先启动下文的本机 Host，安装
应用，再启动日志抓取，最后带参数启动应用；不要安装体积基线替代完整版。
已运行的应用须先 terminate，确保新参数通过冷启动生效：

```sh
xcrun simctl install "$S2_SIMULATOR_UDID" \
  spikes/s2-rn/ios/build-release-full/Build/Products/Release-iphonesimulator/S2RN.app
# 仅当应用已运行时执行：
xcrun simctl terminate "$S2_SIMULATOR_UDID" ai.gxlself.remotescreen.s2
```

在另一终端开始抓系统日志（同样设置实际 UDID），无需点击页面或打开 DevTools：

```sh
xcrun simctl spawn "$S2_SIMULATOR_UDID" log stream --style compact --level debug \
  --predicate 'eventMessage CONTAINS "S2-"'
```

然后在原终端执行（即 `xcrun simctl launch <udid> ai.gxlself.remotescreen.s2 -autoConnect 127.0.0.1:7400`）：

```sh
xcrun simctl launch "$S2_SIMULATOR_UDID" ai.gxlself.remotescreen.s2 \
  -autoConnect 127.0.0.1:7400
```

页面立即连接；每 2 秒应有原生 `S2-VIDEO` JSON。运行约 6 秒应有一行原生
`S2-SELFTEST`，不再依赖 Release 的 console.log 进入系统日志。
维护者已测得旧 Release 自检 pass（ping 0.48 ms、slow 305 ms、10 个事件、停止后 0 个）。
新包复验预期 `status=pass`、`ping=pong`、
`slowMs≥280`、连续 seq 从 0 开始、5 秒收到 9–12 个事件、停止后额外事件 ≤1。
后者是判定阈值，不是新包的实测数字。

视频验证从仓库根启动本机 Host，显式绑定 loopback，避免依赖 tailnet：

```sh
cargo run -p rs-host-agent -- --platform mac --bind 127.0.0.1 --view-only
# 无屏幕录制权限时改用此命令，只验证连接和收帧：
cargo run -p rs-host-agent -- --platform fake --bind 127.0.0.1 --view-only
```

二者择一运行，不要同时占用端口。使用上述 launch 参数自动连接；也可不带参数，
在页面用默认 `127.0.0.1:7400` 点击 Connect。
mac Host 要有屏幕录制权限；fake 码流不是可解码的视频，`received>0` 只能证明
传输链路，不能给出有效 decode p50 或画面结论。停止 Host 用它所在终端的 Ctrl-C。

解码比较：使用同一台 Mac、同一个模拟器型号 / OS、相同 Host、1920 最大宽度、
60 fps、12 Mbps，分别运行 S2 与 M0 至少 60 秒，排除前 2 秒启动期。
S2 页面 / `S2-VIDEO` 的 `decodeP50Ms` 来自 `rs_now_us` 的解码起止差，
不包含 renderer.enqueue；另有 `decodeP95Ms`、`sampleCount` 与当前 `width` / `height`。
无样本时 p50 / p95 均为 -1；出错时保留统计并立即输出 `status="error"` 与 `error`。
M0 应取 `StreamStats.sessionSummary()` 的 **`decode p50`**，不要取 HUD 的
capture→decoded 数字。两者百分位算法和预热口径相同。

M0 可用已有工程构建流程，或从仓库根执行：

```sh
cd tools/ios-demo
xcodegen
xcodebuild -project RemoteScreen.xcodeproj -scheme RemoteScreen \
  -sdk iphonesimulator -configuration Release \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath /tmp/rs-s2-m0 CODE_SIGNING_ALLOWED=NO build
xcrun simctl install "$S2_SIMULATOR_UDID" \
  /tmp/rs-s2-m0/Build/Products/Release-iphonesimulator/RemoteScreen.app
xcrun simctl launch --console "$S2_SIMULATOR_UDID" ai.gxlself.remotescreen.demo \
  -autoConnect 127.0.0.1 -maxWidth 1920 -disconnectAfter 60
```

将 S2 p50、M0 p50、样本数与差值补回本文；验收阈值为绝对差 ≤1 ms。
同时检查重复 Connect / Disconnect、重新运行自检、JS reload 后没有重复订阅。

## 对 D1 / D2 的影响

D1：当前稳定版 RN 新架构能容纳直接实现的 Fabric 视频组件，完成 iOS 编译链接；
是否出画面与延迟达标仍需上述运行结果，暂不触发 interop 或纯原生退路。

D2：同一个 Rust 静态库同时供生成的 TurboModule 与 Swift C ABI 使用已得到最终
链接证据；维护者也已运行确认 JS sync / async / 外部回调通过。这部分可作为 P1 起点。
若后续复测发现绑定层问题，按现有 D2 退路用手写 TurboModule 经 C ABI 传 JSON；
不因当前沙箱的 workspace / Simulator 限制提前改契约或宣布技术方案失败。

## 维护者实测（2026-10-20，模拟器）

环境：iPhone 17 Pro 模拟器（iOS 26.5），Mac 上 `rs-host --platform mac --bind 127.0.0.1 --view-only`（本分支同一提交构建），1920×1080，Host 实际约 30 fps、11 Mbps。

| 项 | S2（Release，`-autoConnect 127.0.0.1:7400`） | M0 Demo（Release，`-autoConnect 127.0.0.1`） |
|---|---|---|
| JS 自检 | pass：ping 0.29–0.48 ms、slow 305 ms、10 个事件（均隔 502 ms）、停止后 0 个 | — |
| 收帧 / 解码 / 失败 | 1754 / 1754 / 0 | 1877 帧，失败 0 |
| decode p50 / p95 | 5.5–5.9 / 7.6–7.7 ms（跳过前 2 s） | 3.3 / 5.2 ms（全程） |
| 机器负载（load avg） | 5.8–8.7 | 4.2–8.7 |

- 两个 App 的 `VideoDecoder.swift` 相同（S2 只多了 `#if !S2_BASELINE`），计时都从 `decoder.decode` 调用前到输出回调，帧都在 Rust 回调线程上送进解码器。差异不在解码器代码本身。
- 怀疑方向（P3 排查）：RN 进程里其他线程的竞争；视频视图在本次测试中位于页面首屏之外（层不在可见区域）；回调线程 QoS。
- 模拟器的解码走 Mac 的解码器，绝对值不代表真机（见 m0-latency）。**P3 验收要在真机上对比 M0 Demo 与 RN 版的 decode p50，差值 ≤ 1 ms**；超出时按上面的方向排查。
