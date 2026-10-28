# App Store listing · English (U.S.) localization

Optional English localization in App Store Connect. The primary language is
Simplified Chinese (`docs/store/zh-Hans.md`); the app UI is Chinese only, so
the description says so. The review notes below are the version to paste into
App Review Information, since reviewers usually read English.

| Field | Value |
|---|---|
| Bundle ID | `ai.gxlself.remotescreen` |
| Apple ID | `6826963518` |
| Copyright | `2026 Xiaolong Gao` |
| Support URL | `https://gxlself.github.io/remote-screen-site/support.html` |
| Marketing URL | `https://gxlself.github.io/remote-screen-site/` |
| Privacy Policy URL | `https://gxlself.github.io/remote-screen-site/privacy.html` |

## Name (≤ 30)

```
Remote Screen - 远屏
```

## Subtitle (≤ 30)

```
Control your computers
```

## Promotional text (≤ 170)

```
Turn on Screen Sharing on your Mac or Remote Desktop on your Windows PC, and control it from your iPhone or iPad. Nothing to install on the computer.
```

## Description (≤ 4000)

```
Remote Screen lets you control your own computers from iPhone and iPad. The app interface is in Simplified Chinese.

Nothing to install
• Mac: turn on System Settings › General › Sharing › Screen Sharing. Remote Screen finds the Mac on the same network; sign in with your Mac account.
• Windows: turn on Settings › System › Remote Desktop, add the PC by address, and sign in with your Windows account.
• You can also scan the local network or add a computer by IP address or host name.

Reach home from anywhere
• With Tailscale installed and signed in on the phone, Remote Screen connects to the computers in your tailnet.
• On your company VPN, reach the computers in your office network too.
• Connections go straight between your devices and your computers. There is no relay server of ours and no account to create.

Built for touch
• Trackpad and touch modes, two-finger scroll, pinch to zoom, two-finger tap for right click.
• Hardware keyboard, mouse, trackpad and Apple Pencil support, with one-tap shortcuts.
• Switch between displays; the picture fits your screen.

iPad
• Portrait and landscape, Split View, Slide Over and Stage Manager.
• Open different computers in different windows side by side.

With the optional Remote Host installed on the computer
• Lower latency, higher quality and the computer's sound.
• Send files between the phone and the computer.
• Picture in Picture keeps the computer on screen while you use other apps.
• Record sessions; recordings are encrypted on the device, and the other side sees a notice when recording starts.

SSH terminal
• SSH to servers and computers with keys, jump hosts or Tailscale SSH.
• Keys stay in the device keychain.

Privacy and security
• Lock the app with Face ID or your passcode, and verify again before controlling a computer.
• No personal data collected, no ads, no analytics or tracking SDKs.
• Logs and crash records stay on the device until you export them yourself.

Only control computers you manage or have permission to use.
```

## Keywords (≤ 100 characters)

```
remote desktop,remote control,screen sharing,vnc,rdp,ssh,terminal,lan,file transfer,keyboard,mouse
```

No third-party trademarks or other app names (guideline 2.3.7). 98 characters.

## App Review notes

```
Remote Screen is a remote desktop client for controlling the user's own Mac or Windows computer. No account or sign-in is required. We cannot provide a test computer, but any Mac with Screen Sharing turned on works:

1. On the Mac, open System Settings › General › Sharing and turn on Screen Sharing. Click the (i) button next to it and set "Allow access for" to "All users", or add the account you will sign in with.
2. Put the iPhone or iPad on the same Wi-Fi network as the Mac.
3. Open Remote Screen. On the first onboarding page keep "使用手机上的 Tailscale" selected and tap "继续" (Continue); Tailscale does not need to be installed. The next two pages can be skipped with "继续" / "开始使用". Allow Local Network access when iOS asks.
4. On the home screen tap "添加电脑" (Add Computer). The Mac appears under "同一网络里的 Mac" (Macs on this network). Tap it and enter the Mac's user name and password to see and control the Mac's screen.
   If it does not appear, tap "按地址添加" (Add by address) and enter the Mac's IP address, shown in System Settings › Wi-Fi › Details on the Mac.

A Windows PC works the same way: turn on Settings › System › Remote Desktop, add the PC by IP address in Remote Screen, and sign in with the Windows account.

Background mode: UIBackgroundModes contains only "audio". It is the background mode required for system Picture in Picture (AVPictureInPictureController showing the remote computer's screen). The app does not play silent audio and does not use it to stay alive in the background. Remote audio stops when the app goes to the background; the connection is kept only for the short time granted by beginBackgroundTask and then pauses.

Tailscale is optional and only needed to reach the user's own computers from outside the local network; the basic features do not depend on it. The app does not set up a VPN and contains no networking of its own: to reach a computer from outside, the user installs the Tailscale app or connects to their company VPN. The app has no developer-run server and collects no data.
```

## App Privacy

"Data Not Collected"; not used for tracking. See the table in
`docs/store/zh-Hans.md` for the reasoning behind each network request.

## Age rating

All questionnaire items "None" / "No" (no in-app web browser, no user-generated
content, no messaging, no ads). Result: 4+.
