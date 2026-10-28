# Roadmap

Where the project stands and what's next. Items marked **help wanted** are good places to start; open an issue
before picking up a large one so we can agree on the approach.

## Today

- **Native protocols first.** The app connects to macOS Screen Sharing (VNC/ARD), Windows Remote Desktop (RDP via
  IronRDP) and SSH without installing anything on the computer. LAN discovery uses Bonjour for Macs and an
  on-demand scan for Windows.
- **Remote Host** (macOS, Windows, Linux) adds the RSP/1 protocol: hardware encoding, FEC, adaptive bitrate,
  multipath with session recovery, sound, file transfer, virtual displays, viewport-aware encoding and encrypted
  recording.
- **Clients:** iOS / iPadOS (1.0 submitted to the App Store), Android (build from source), desktop controller for
  macOS and Windows (build from source).
- **Networking:** direct LAN, company VPN, or your own Tailscale/Headscale tailnet; an embedded tailnet node is
  available but off by default.

## Next

- **Host self-update.** The updater pipeline (signed manifest, platform code-signature check, install after the
  session ends, self-test and rollback) exists, but has had no update source since the Hub was removed. Plan:
  publish Ed25519-signed manifests with GitHub Releases. See [D13](dev/01-decisions.md#d13--自研更新器).
- **Signed public releases** of the Mac/Windows Host, the Linux AppImage and the desktop controller on GitHub
  Releases.
- **English UI** — **help wanted.** All strings already go through `app/src/i18n/`, `host-ui/src/i18n/` and
  `desktop/` i18n modules; only Chinese exists today.
- **Android release** on a store or as signed APKs.
- **Real-device latency numbers.** Glass-to-glass measurements on phones over Wi-Fi and 5G
  ([m0-latency.md](m0-latency.md) §3) — **help wanted** if you have a high-speed camera.

## Later / ideas

- Linux Host: input injection on wlroots compositors, login-screen access, clipboard without XWayland
  ([D47](dev/01-decisions.md#d47--linux-被控第一版m3)).
- Automatic removal of SSH public keys installed for a revoked device (manual today).

## Not planned

- A hosted relay or account service. Connections stay device-to-computer.
- Windows Home edition as a target computer (it has no Remote Desktop server).
