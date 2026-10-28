# Security policy

Remote Screen gives full control of a computer to whoever holds a paired device, so we take security reports
seriously.

## Reporting a vulnerability

Please **do not** open a public issue. Report privately through GitHub:
[Security → Report a vulnerability](https://github.com/gxlself/remote-screen/security/advisories/new).

Include what you found, the affected component and version (app, Host, desktop controller, protocol), steps to
reproduce, and the impact you expect. We aim to acknowledge reports within 3 working days and to agree on a
disclosure date with you once a fix is available.

## Scope

In scope:

- The RSP/1 protocol and its implementation: pairing, device certificates, trust statements, session
  authentication and permissions (`crates/rsp-*`, `crates/host-*`, `proto/`).
- The mobile app and desktop controller: key storage, credential handling for VNC/RDP/SSH, recording
  encryption, app lock.
- The Host agent and its local IPC (`crates/rs-host-agent`, `host-ui/`).

Out of scope: vulnerabilities in macOS Screen Sharing, Windows Remote Desktop, OpenSSH, Tailscale/Headscale or
other third-party software themselves (report those upstream), and attacks that require an already
compromised or unlocked device.

## Supported versions

Fixes go into the latest release and `main`. The security model is described in
[docs/dev/04-identity-security.md](docs/dev/04-identity-security.md).
