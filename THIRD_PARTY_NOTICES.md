# Third-party notices

Remote Screen is licensed under `MIT OR Apache-2.0` (see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE)). The following third-party material is **bundled in this repository** and
keeps its original license. Each directory carries the full license text.

| Path | Origin | License |
|---|---|---|
| `app/ios/vendor/SwiftTerm/` | [SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) by Miguel de Icaza and contributors (see `VENDOR.md` for the pinned revision and local changes) | MIT — [`LICENSE`](app/ios/vendor/SwiftTerm/LICENSE) |
| `tailnet/internal/testcontrol/` | Test control server copied from [tailscale/tailscale](https://github.com/tailscale/tailscale) v1.94.2 | BSD-3-Clause — [`LICENSE`](tailnet/internal/testcontrol/LICENSE) |
| `drivers/win-iddcx/` | Adapted from Microsoft's [Windows-driver-samples](https://github.com/microsoft/Windows-driver-samples) `video/IndirectDisplay/IddSampleDriver` | MIT — see [`README.md`](drivers/win-iddcx/README.md) |
| `app/ios/RemoteScreen/Fonts/`, `app/android/app/src/main/assets/fonts/` | [Geist and Geist Mono](https://github.com/vercel/geist-font) by Vercel | SIL Open Font License 1.1 — `OFL-1.1.txt` in each directory |

## Dependencies

Rust, Go and JavaScript dependencies are fetched by their package managers and are not vendored here. The
Rust dependency licenses are checked by [`cargo-deny`](deny.toml) against a permissive allowlist (no GPL,
LGPL or AGPL). Notable dependencies include [IronRDP](https://github.com/Devolutions/IronRDP) (MIT/Apache-2.0,
pinned to a git revision), [quinn](https://github.com/quinn-rs/quinn), [rustls](https://github.com/rustls/rustls),
[russh](https://github.com/Eugeny/russh), [uniffi](https://github.com/mozilla/uniffi-rs),
[Tauri](https://tauri.app), [React Native](https://reactnative.dev) and
[tsnet](https://pkg.go.dev/tailscale.com/tsnet) (BSD-3-Clause).

## H.264 software encoding (Linux Host)

The Linux Host can encode with [OpenH264](https://www.openh264.org). Release packages do not include it; at
run time the Host loads Cisco's prebuilt `libopenh264.so.7`, which Cisco distributes under its own binary
license and which covers the H.264 patent royalties for that binary. Building OpenH264 from source (cargo
feature `openh264-source`) gives you a library that is **not** covered by Cisco's patent license.
