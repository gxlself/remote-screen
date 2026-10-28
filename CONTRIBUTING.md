# Contributing to Remote Screen

Thanks for your interest! Bug reports, fixes, translations, docs and new platform work are all welcome.
Issues and pull requests can be written in English or Chinese.

## Before you start

- **Bugs:** open an issue with the template. Include the platforms on both ends (e.g. iPad iPadOS 18.1 →
  macOS 15.2 Screen Sharing), how you connect (LAN / VPN / tailnet / Host), and the error code shown on the
  disconnect screen (`RS-E1xx`), if any. The app can export its local logs from Settings.
- **Security issues:** don't open a public issue — see [SECURITY.md](SECURITY.md).
- **Larger changes** (new protocol messages, new platforms, architecture changes): open an issue first so we
  can agree on the approach. Design rationale lives in [`docs/dev/`](docs/dev/README.md), especially
  [01-decisions.md](docs/dev/01-decisions.md).

## Development setup

See [docs/dev/02-repo-env.md](docs/dev/02-repo-env.md) for toolchains per platform. In short:

| You work on | You need |
|---|---|
| Rust crates, Host on Linux | `rustup` (toolchain pinned in `rust-toolchain.toml`), `cargo-nextest`, `cargo-deny`, `just` |
| Mobile app (JS) | Node ≥ 22.13, pnpm 11 (`corepack enable`) |
| iOS / Mac Host / desktop on macOS | Xcode 26, CocoaPods ≥ 1.16 |
| Android | JDK 17, Android SDK 37, NDK 27.1, `cargo-ndk` |
| Embedded tailnet | Go 1.26 |
| Windows Host / driver | Visual Studio 2022 + WDK (driver only) |

## Checks

Every pull request must pass the Linux gate, which is what CI runs:

```bash
bash tools/ci/check-linux.sh
```

Run a subset while iterating, e.g. `--only fmt,clippy,tests` or `--only app`; `--list` shows every section.
macOS-only crates (`host-mac`, `rs-desktop` Mac glue) are covered by `tools/ci/check-mac.sh`.

## Code guidelines

These are the rules from [02-repo-env](docs/dev/02-repo-env.md#代码约定) that reviewers check most often:

- **Hot paths stay native.** Video frames, touch/pointer input and terminal bytes go between native code and
  Rust directly; JavaScript only handles screens, parameters and events.
- **Every user-visible failure has an error code** and a next step ([10-appendix](docs/dev/10-appendix.md)).
- **Rust:** library crates use `thiserror`, binaries use `anyhow`. No `unwrap()` outside tests.
  `unsafe` only in the platform/FFI crates, each block with a `// SAFETY:` comment. FFI exports are prefixed
  `rs_` and wrapped in `catch_unwind`. Logs use `tracing` with structured fields and **never** contain
  clipboard contents, typed text, file contents, passwords or keys. Async runtime is tokio only.
- **Protocol changes** (`proto/`, `crates/rsp-core`) update [03-protocol.md](docs/dev/03-protocol.md) and the
  test vectors in the same commit. Removed protobuf fields become `reserved`.
- **TypeScript:** `strict`, ESLint + Prettier; screens hold no business logic (put it in hooks under
  `src/features/<area>/`); all user-visible strings go through `src/i18n/`.
- **Swift / Kotlin:** native components only render, handle gestures and call the C ABI; state lives in Rust
  or JS. Don't block the main thread.
- **Generated code:** uniffi bindings in `app/src/native/generated/` and `app/cpp/generated/` are committed.
  If you change the `rs-mobile` uniffi surface, regenerate them on a Mac with `pnpm --dir app bindings`;
  the `bindings` check fails when they are stale.
- **Examples and tests** use documentation address ranges only: tailnet `100.64.0.x`, office `10.30.x.x`,
  VPN `10.99.x.x`, home `192.168.31.x`. Never commit real hosts, keys or credentials.

## Commits and pull requests

- Branch from `main`; keep pull requests focused on one change.
- Commit subjects are imperative and short (≤ 72 characters), e.g. `rs-vnc: Handle ZRLE tiles at the right edge`.
  Explain the *why* in the body when it isn't obvious.
- Add or update tests with behaviour changes. Bug fixes should come with a test that fails without the fix.
- By submitting a contribution you agree to license it under `MIT OR Apache-2.0`, as described in the
  [README](README.md#license).
