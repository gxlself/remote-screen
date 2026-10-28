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

# 每晚 capture→receive 回归；本机按 netem → faultnet → none 顺序选整形方式
latency SECS="60" OUT="target/latency/latest.json":
    tools/latency/run.sh "{{SECS}}" "{{OUT}}"

# 对照基准（M3 V9）：在控制端 Mac 上跑，见 tools/bench/README.md
bench *ARGS:
    python3 tools/bench/bench.py {{ARGS}}

# 对照基准的单测（Linux / Mac 都能跑）
bench-test:
    python3 -m unittest discover -s tools/bench/tests -t tools/bench
    node --test tools/bench/page/test/*.test.js
    cargo nextest run -p rs-bench

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

# 电脑端控制端（M3 V6），见 07 · 电脑端
desktop:
    cd desktop && pnpm tauri dev

fuzz TARGET SECS="60":
    cargo +nightly fuzz run {{TARGET}} -- -max_total_time={{SECS}}

# rsp-core 行覆盖率门禁（≥ 85%），见 09 · 覆盖率
coverage:
    cargo llvm-cov nextest -p rsp-core --summary-only --fail-under-lines 85

# Android 纯逻辑 JUnit（nativelogic，只要 JDK 17）
android-test:
    cd app/android && ./gradlew -p nativelogic test

# 构建 tailnet 宿主库和测试网助手
tailnet-host:
    bash tailnet/scripts/build-host.sh

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
# 参数原样转给脚本，例如 `just release-ios --dry-run`、`just release-ios --upload`；
# 不带参数就是真签名打包（不上传），先用 `--plan` / `--dry-run` 看步骤。
release-ios *ARGS:
    tools/release/ios.sh {{ARGS}}
release-mac *ARGS:
    tools/release/mac.sh {{ARGS}}
release-win *ARGS:
    tools/release/win.ps1 {{ARGS}}
release-android *ARGS:
    tools/release/android.sh {{ARGS}}
release-desktop-mac *ARGS:
    desktop/release/mac.sh {{ARGS}}
release-desktop-win *ARGS:
    desktop/release/win.ps1 {{ARGS}}

bump VERSION:
    tools/bump-version.sh {{VERSION}}
