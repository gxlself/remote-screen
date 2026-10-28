# Remote Screen app

React Native 0.88 (New Architecture) app for iOS, iPadOS and Android. Screens, navigation and settings are
TypeScript; the remote session view, gestures, video decoding and the terminal are native (Swift / Kotlin) and
talk to the Rust core (`crates/rs-mobile`) directly. Architecture notes: [docs/dev/07-app.md](../docs/dev/07-app.md).

## Install

Node ≥ 22.13 and pnpm 11 (`corepack enable`):

```sh
pnpm install --frozen-lockfile
```

`pnpm install` uses the checked-in `.npmrc` (`node-linker=hoisted`) that React Native needs. On Linux this is all
you need for TypeScript work — installing and the JS checks never invoke Cargo, CocoaPods or Xcode:

```sh
pnpm typecheck
pnpm lint
pnpm test
```

## iOS

Needs Xcode 26, CocoaPods ≥ 1.16 and the Rust iOS targets (installed from `rust-toolchain.toml`).

```sh
./rust/build-ios.sh
cd ios && pod install && cd ..
pnpm ios
```

`rust/build-ios.sh` builds `rs-mobile` for `aarch64-apple-ios` and `aarch64-apple-ios-sim`, creates the ignored
`ios/RsMobile.xcframework`, and regenerates the committed uniffi bindings. It builds Release by default; set
`RS_MOBILE_PROFILE=Debug` (or `CONFIGURATION=Debug`) for a debug archive. `just mobile-ios` runs the same script.

`scripts/build-ios.sh` builds Rust, optionally installs Pods, and builds the app without signing:

```sh
./scripts/build-ios.sh Debug iphonesimulator
./scripts/build-ios.sh Release iphoneos
```

If `xcodebuild -workspace` misbehaves in a restricted environment, `RS_XCODE_PROJECTS=1` builds the Pods and app
projects with a shared `BUILD_DIR` instead; add `RS_SKIP_PODS=1` when the Podfile didn't change. Products land in
`ios/build-<configuration>-<sdk>/Build/Products/`.

Debug builds use the bundle ID `ai.gxlself.remotescreen.dev` (display name "Remote Screen Dev") and need Metro
(`pnpm start`); Release uses `ai.gxlself.remotescreen` and embeds the JS bundle. To run on a device, pick your own
team under Signing & Capabilities.

### How the Rust build is wired into Xcode

- The `RsMobileBuild` run-script target is a dependency of the `RsCore` pod, so it runs before CocoaPods copies the
  XCFramework or compiles the generated glue. `rust/inputs.xcfilelist` / `rust/outputs.xcfilelist` track inputs
  and outputs; generation happens in a temporary directory and unchanged files keep their mtimes.
- Two committed relative symlinks keep CocoaPods globs inside the pod root:
  `native/core/cpp/generated → cpp/generated` and `native/core/ios/RsMobile.xcframework → ios/RsMobile.xcframework`.
- `opus` → `opusic-sys` builds libopus with CMake. The script pins each target's `CC` / `CXX` to the `xcrun`
  paths so shell and Xcode builds share one CMake cache.

## Android

Needs JDK 17, Android SDK 37, NDK 27.1 and `cargo-ndk`.

```sh
./rust/build-android.sh
pnpm android
```

`rust/build-android.sh` (or `just mobile-android`) builds `rs-mobile` for the ABIs listed in
`android/gradle.properties` (`reactNativeArchitectures`) into the native module's `jniLibs`. Pure Kotlin logic in
`android/nativelogic` has JVM unit tests: `just android-test`.

## Generated code

The uniffi bindings are committed: `src/native/generated/rs_mobile.ts`, `src/native/generated/rs_mobile-ffi.ts`,
`cpp/generated/rs_mobile.cpp` and `cpp/generated/rs_mobile.hpp`, plus the TurboModule adapters under
`native/core/`. After changing the `rs-mobile` uniffi surface, regenerate them on a Mac:

```sh
pnpm bindings
```

CI (`tools/ci/check-linux.sh --only bindings`) fails when they are stale.

## Native layer conventions

- Pure Swift logic lives in `ios/NativeLogic/Sources/NativeLogic/<feature>/` with tests in
  `ios/NativeLogic/Tests/NativeLogicTests/`; it depends only on Foundation, CryptoKit, CoreGraphics and Network and
  is also compiled into the app target. Native views and modules live in `ios/RemoteScreen/Native/<feature>/`.
- Pure Kotlin logic lives in `android/nativelogic`; Android views and modules in
  `android/app/src/main/java/ai/gxlself/remotescreen/rsnative/<feature>/`.
- Module specs are `src/native/specs/Native<Name>.ts` and always use the nullable `TurboModuleRegistry.get`;
  component specs are `<Name>NativeComponent.ts`. Business code imports specs only through the wrappers in
  `src/native/<feature>/`.
- `modulesProvider` fills its table with `NSClassFromString`, so a listed class that doesn't exist crashes at
  launch. Register new native classes in `package.json` and `src/native/devharness/registry.ts`.
- After adding native iOS files, run a build with `pod install` (without `RS_SKIP_PODS`):
  `scripts/configure-project.rb` updates `RemoteScreen.xcodeproj/project.pbxproj`. Commit that change; don't edit
  the project by hand.

## Launch arguments for testing

| Argument | Effect |
|---|---|
| `-autoConnect <host_id or ip>` | After onboarding and unlock, resolve the target (paired hosts first, then the local computer list) and open the session. Logs use the `RS-AUTOCONNECT` tag. |
| `-disconnectAfter <seconds>` | Close the session that long after streaming starts and log a latency `summary` |
| `-devHarness <name> -harnessArg <arg>` | Open a single native test page, e.g. `ping`, or `selftest host:port` |

```sh
xcrun simctl launch --console-pty booted ai.gxlself.remotescreen -autoConnect 100.64.0.7 -disconnectAfter 20
```

Stream the native markers in another terminal:

```sh
xcrun simctl spawn booted log stream --level info --predicate 'eventMessage BEGINSWITH "RS-"'
```

IPv6 addresses must be bracketed, e.g. `[fd7a:115c::7]:7400`.

## Release builds

Signing material is read from local, git-ignored files; see
[docs/dev/09-quality-release.md](../docs/dev/09-quality-release.md) and `just release-ios` / `just release-android`
with `--plan` or `--dry-run` first.
