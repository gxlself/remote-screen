# S1 · 安全芯片密钥做 rustls 客户端证书

> 验证用的代码（原 `spikes/` 目录）没有随开源版本发布，这里只保留结论和当时的做法，下面的命令不能直接运行。

## 结论

本 spike 验证了 D7 的 Rust/Swift 接线：`rsp-transport::ExternalSigner` 可以用外部 P-256 签名器生成 App 自签证书，并在 QUIC 的 TLS 1.3 双向认证中完成 `CertificateVerify`；服务端取到的客户端 SPKI 指纹与客户端返回的指纹一致。200 次签名的 p95 远低于 10 ms，文件持久化后的重启指纹不变，删除文件后指纹改变。

**维护者复测（2026-10-20，同一台 Apple Silicon Mac，在沙箱之外运行同一个 CLI）：Mac 上的安全芯片路径通过。** 实现者报告的 `SecureEnclave.isAvailable == false` 是沙箱（seatbelt）挡住了对安全芯片的访问，不是机器不支持。D7 结论改为 **macOS 安全芯片通过；iPhone 真机项在 P2 复核**。模拟器仍走 `ThisDeviceOnly` Keychain 软件密钥退路。

## 做法

- `spikes/s1-se-tls/Cargo.toml` 是独立 Cargo 包，带空 `[workspace]`，从 `spikes/s1-se-tls/rust` 执行 `cargo build` / `cargo test` 不会加入主 workspace。Rust 依赖 `../../crates/rsp-transport`，没有改 `rsp-transport` 或主 `Cargo.lock`。
- `s1-server` 使用 `rsp_transport::tls::server_config`，默认监听 `127.0.0.1:0`，要求客户端证书。握手后读取 `S1\n`，打印客户端 SPKI 指纹，回复 `OK\n` 并退出。
- `s1_client` staticlib 导出 `s1_handshake` 和 C 回调类型。Swift 只把 SPKI DER 交给 Rust，私钥签名通过回调返回 DER ECDSA P-256 签名。回调在 `tokio::task::spawn_blocking` 中执行，避免占用 QUIC worker。
- Swift 同一份 `S1SecureSigner.swift` 同时用于 macOS CLI 和 iOS App。Secure Enclave 的 `dataRepresentation` 存在状态目录；iOS 无 Secure Enclave（模拟器）时使用 `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` Keychain。SPKI 是固定的 P-256 SubjectPublicKeyInfo DER，指纹为其 SHA-256。

## 环境

| 项目 | 值 |
|---|---|
| 机器 | MacBook Air (Mac17,3)，Apple M5，32 GB |
| macOS | 26.3.2 |
| Xcode | 26.5 (17F42) |
| Swift | 6.3.2 |
| Rust | rustc/cargo 1.96.0 |
| 架构 | arm64 |
| Secure Enclave | `SecureEnclave.isAvailable == false`；未获得安全芯片密钥 |

尝试对 macOS CLI 做临时 ad-hoc 签名（`codesign -s - --force /tmp/s1-client`）后再次运行，`SecureEnclave.isAvailable` 仍为 `false`，所以没有把软件密钥结果误报成安全芯片结果。

## 数字

测试命令使用独立的临时状态目录。以下三次运行均启动一个新的 `s1-server` 进程；第二次使用同一个状态目录模拟重启，第三次删除 `p256-key.bin` 后重新生成。

| 场景 | 签名 p50 (ms) | 签名 p95 (ms) | 签名 max (ms) | 握手总耗时 (ms) | 客户端 SPKI 指纹 |
|---|---:|---:|---:|---:|---|
| 首次（冷启动） | 0.083 | 0.087 | 0.120 | 4.071 | `b203a4d6d8f0ec14dfb9252c2bd556a66c83cb00ee6665c658d4e84a233990ee` |
| 重启后（保留密钥） | 0.082 | 0.219 | 0.324 | 4.190 | `b203a4d6d8f0ec14dfb9252c2bd556a66c83cb00ee6665c658d4e84a233990ee` |
| 删除密钥后 | 0.077 | 0.081 | 0.104 | 3.975 | `bb9983d9c300641c33adfc42790c88534d93f0928e07c6602fd987aa435a61aa` |

三次运行的签名 p95 都小于 10 ms；前两次指纹完全相同，删除密钥后指纹变化。最后一次输出中，Swift、Rust client 和 server 的指纹分别为同一个 `bb99…61aa` 值，三者一致。服务端同时打印了自己的 Host SPKI 指纹，客户端使用 `ServerPin::Any` 完成 pairing-style spike；生产路径仍应由 A8 提供固定 Host 指纹。

## 复现

Rust 单测包含一个 `p256` 软件密钥回调，会启动 loopback QUIC server/client，断言两端指纹一致：

```sh
cd spikes/s1-se-tls/rust
cargo build
cargo test
```

macOS CLI 需要先生成静态库并直接链接 Swift 源码：

```sh
cd spikes/s1-se-tls
cargo build
mkdir -p /private/tmp/s1-clang-module-cache
CLANG_MODULE_CACHE_PATH=/private/tmp/s1-clang-module-cache \
  swiftc -O -parse-as-library \
  -import-objc-header apple/S1SecureSigner.h -I apple \
  -L target/debug -ls1_client -framework CryptoKit -framework Security \
  -o /tmp/s1-client apple/Sources/S1SecureSigner.swift apple/CLI/main.swift
./target/debug/s1-server > /tmp/s1-server.log 2>&1 &
# 从 /tmp/s1-server.log 读取 S1_SERVER_ADDR=127.0.0.1:<port>
S1_STATE_DIR="$PWD/.state" /tmp/s1-client 127.0.0.1:<port>
```

状态持久化检查可以在两次 CLI 进程之间删除文件：

```sh
rm -f spikes/s1-se-tls/.state/p256-key.bin
```

这只删除 spike 自己的测试密钥；`.state/` 已加入 `.gitignore`。

## iOS 构建与待维护者验证

XcodeGen 工程文件是生成物，源码在 `spikes/s1-se-tls/apple/`。模拟器和真机目标都已在本机编译通过：

```sh
cd spikes/s1-se-tls/apple
xcodegen generate
xcodebuild -project S1SecureTLS.xcodeproj -scheme S1SecureTLS \
  -sdk iphonesimulator -configuration Release \
  -derivedDataPath /private/tmp/S1SecureTLSDerivedData \
  build CODE_SIGNING_ALLOWED=NO
xcodebuild -project S1SecureTLS.xcodeproj -scheme S1SecureTLS \
  -sdk iphoneos -configuration Release \
  -derivedDataPath /private/tmp/S1SecureTLSDeviceDerivedData \
  build CODE_SIGNING_ALLOWED=NO
```

真机需要维护者提供签名团队和设备，并在有一台 Mac Host 的情况下再跑一次：

```sh
# Mac 上：地址按实际 tailnet 地址填写，示例使用文档保留地址
cd spikes/s1-se-tls
./target/debug/s1-server --addr 100.64.0.2:0

# 另一终端：签名编译（把 TEAMID 换成维护者团队）
cd spikes/s1-se-tls/apple
xcodebuild -project S1SecureTLS.xcodeproj -scheme S1SecureTLS \
  -destination 'generic/platform=iOS' -configuration Release \
  -derivedDataPath /private/tmp/S1SecureTLSDeviceSignedData \
  DEVELOPMENT_TEAM=TEAMID CODE_SIGNING_ALLOWED=YES build
xcrun devicectl list devices
xcrun devicectl device install app --device <UDID> \
  "/private/tmp/S1SecureTLSDeviceSignedData/Build/Products/Release-iphoneos/S1SecureTLS.app"
xcrun devicectl device process launch --device <UDID> ai.gxlself.s1-secure-tls
```

安装后在 App 输入 `s1-server` 打印的 `<host>:<port>`，点击「握手」。验收看三项输出：后端必须显示 `Secure Enclave`，App 显示的指纹必须等于 server 的 `S1_CLIENT_FP`，连续 200 次签名的 p95 必须不超过 10 ms；退出并重新打开 App 后指纹应保持不变。模拟器预期显示软件 Keychain 退路，不能据此判定安全芯片通过。

## 对 01 · D7 的影响

保留 D7 的设计：私钥不进入 Rust，`ExternalSigner` 同时服务 TLS 和 SSH，指纹继续定义为 SPKI DER 的 SHA-256。当前结果足以证明 A8 的 rustls 远程签名接口和 TLS 1.3 双向认证可行；D7 的最终状态要等真机 Secure Enclave 运行结果。若真机签名 p95 超过 10 ms，按 08 的退路把签名调用保持在 `spawn_blocking`；若设备 API 无法恢复密钥，则按 `ThisDeviceOnly` 软件 Keychain 退路降级并在 01 记录安全性降低。

契约变更请求：无。

## 维护者实测（沙箱外）

同一台 Mac（Apple Silicon，macOS 26），按上文「macOS CLI」的命令构建，在 codex 沙箱之外运行，状态目录放在会话临时目录。连续 3 次独立进程（每次新起 `s1-server`）：

| 次 | 后端 | 签名 p95 | 握手 | 指纹一致 |
|---|---|---|---|---|
| 1（首次生成密钥） | Secure Enclave | 4.94 ms（p50 4.64，max 11.7） | 31.5 ms | 是 |
| 2（重启） | Secure Enclave | 4.80 ms | 20.2 ms | 是 |
| 3（重启） | Secure Enclave | 4.90 ms | 21.3 ms | 是 |

- 三次的 `S1_SPKI_FP` 完全相同，持久化有效；状态文件是安全芯片密钥的封装数据（284 字节），不是私钥明文。
- 签名 p95 ≈ 5 ms，满足 ≤ 10 ms；比软件密钥慢一个数量级，但签名只在握手时发生，单次握手 20–30 ms 可以接受。回调保持在 `spawn_blocking` 里。
- 结论：D7 在 macOS 安全芯片上成立；P2 在 iPhone 真机上复核同样三项（后端显示 Secure Enclave、指纹与 server 一致、p95 ≤ 10 ms、重开 App 指纹不变）。
- 以后让实现者在沙箱里做安全芯片相关验证时，会遇到同样的 `isAvailable == false`；这类实测由维护者在沙箱外做。

