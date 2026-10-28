# App Store 上架资料 · 简体中文（主语言）

App Store Connect（ASC）里逐项照填。字数按 ASC 的计法（一个汉字算一个字符）。

| 项 | 值 |
|---|---|
| Bundle ID | `ai.gxlself.remotescreen` |
| Apple ID（ASC App ID） | `6826963518`，App 里「去更新 App」打开 `itms-apps://apps.apple.com/app/id6826963518` |
| 团队 | 个人开发者账号，Team ID `W8L8ZJ3N2P` |
| 主语言 | 简体中文 |
| 价格 | 免费，无 App 内购买 |
| 版本 | 1.0.0（构建号 `1000000 + N`，见下方「构建号」） |
| 版权 | `2026 Xiaolong Gao` |
| 支持网址 | `https://gxlself.github.io/remote-screen-site/support.html` |
| 营销网址 | `https://gxlself.github.io/remote-screen-site/` |
| 隐私政策网址 | `https://gxlself.github.io/remote-screen-site/privacy.html` |

## 构建号

`CFBundleVersion` 由 `tools/release/ios/common.sh` 的 `bundle_version` 按
`主 × 1000000 + 次 × 10000 + 修订 × 100 + N` 算出，1.0.0 就是 `1000000 + N`。
工程里的 `CURRENT_PROJECT_VERSION = 1000000` 是 `tools/bump-version.sh` 按 N = 0
写进去的默认值（原来的 10000 就是 0.1.0 的 N = 0）。`configure-project.rb` 不改它。

## 1.0 不含 App 内 tailnet

`tools/release/ios.sh` 默认按 `RS_TAILNET_IOS=off` 出包，日志写「This build does not include
the in-app tailnet」，`release-summary.json` 里 `"tailnet": "off"`、`"tailnetIncluded": false`。
App 不建立 VPN、不带组网库；App 里检测到没有 tailnet 时，引导页、设置、首页都不出现「App 内连接 /
Headscale」入口。在外面连回电脑靠用户自己装的 Tailscale App 或公司 VPN；填 Tailscale API 令牌读设备
列表的功能保留（只是 HTTPS 请求，不组网）。以后要带上：`--with-tailnet`。

每次上传：`tools/release/ios.sh --upload --build N`，N 从 1 开始、每次 +1（0–99），
脚本用命令行参数覆盖 `CURRENT_PROJECT_VERSION`，不用改工程文件。ASC 要求同一版本号下
构建号严格递增；被拒或重传都要换一个新的 N。改版本号（`just bump 1.0.1`）后 N 从 0 重新算。

## App 名称（≤ 30）

```
远屏 Remote Screen
```

## 副标题（≤ 30）

```
用手机和 iPad 远控你的电脑
```

## 宣传文本（≤ 170）

```
打开 Mac 自带的屏幕共享或 Windows 远程桌面，就能用 iPhone 和 iPad 直接连上，不用在电脑上装任何东西。同一 Wi-Fi 里直接连，人在外面可以经你自己的 Tailscale 网络连回去。
```

## 描述（≤ 4000）

```
远屏用 iPhone 和 iPad 远程控制你自己的电脑。

不用装东西就能连
• Mac：打开「系统设置 › 通用 › 共享 › 屏幕共享」，远屏会在同一网络里自动找到它，用 Mac 的账户登录即可。
• Windows：打开「设置 › 系统 › 远程桌面」，在远屏里按地址添加，用 Windows 账户登录。
• 也可以扫描局域网，或者直接输入 IP 地址和主机名添加电脑。

在外面也能连回家
• 手机上装了 Tailscale 并登录后，远屏可以连接你 tailnet 里的电脑。
• 连着公司 VPN 时，也能经公司内网连回办公室的电脑。
• 连接只在你的设备和你的电脑之间进行，远屏没有自己的中转服务器，也不需要注册账号。

为触屏设计的操作
• 触控板模式和触屏模式，双指滚动、捏合缩放，双指轻点就是右键。
• 支持外接键盘、鼠标、触控板和 Apple Pencil；常用快捷键一键发送。
• 多显示器之间切换，按屏幕大小自动调整画面。

iPad
• 横竖屏都能用，支持分屏、侧拉和台前调度。
• 可以把不同的电脑放进不同的窗口，同时开着。

安装可选的 Remote Host 后还能
• 更低延迟、更高画质的画面，以及电脑的声音。
• 在手机和电脑之间传文件。
• 画中画：切到别的 App 时，电脑画面留在小窗里。
• 录制远控过程，录像加密保存在本机，开始录制时对方能看到提示。

SSH 终端
• 连接服务器或电脑的 SSH，支持密钥登录、跳板机和 Tailscale SSH。
• 密钥只保存在本机钥匙串里。

隐私与安全
• 可以用 Face ID 或密码锁住 App，进入远控前再验证一次。
• 远屏不收集个人数据，没有广告，没有统计和追踪 SDK。
• 日志和崩溃记录只保存在本机，只有你主动导出时才会离开设备。

使用前请确认：要控制的电脑由你本人管理，或者已经得到它的管理者授权。
```

## 关键词（≤ 100 字符，逗号分隔，不含 App 名里的词）

```
远程桌面,远程控制,屏幕共享,VNC,RDP,SSH,终端,局域网,文件传输,画中画,外接键盘,鼠标,多显示器,远程办公
```

不放第三方商标（Tailscale、Headscale、Windows 等）和别家 App 名，避免 2.3.7 被拒。共 60 个字符。如果 ASC 提示超长（按字节算是 136），删掉末尾的「多显示器,远程办公」再试。不放「内网穿透」这类组网说法：1.0 不含 App 内组网。

## 类别

- 主要类别：**效率**
- 次要类别：**工具**

## 年龄分级问卷

按 ASC 现行问卷（2025 年起的新版）逐项回答，结果应为 **4+**。

| 问题 | 答案 |
|---|---|
| 家长控制（App 内有家长控制功能） | 否 |
| 年龄验证（App 内有年龄验证） | 否 |
| 卡通或幻想暴力 | 无 |
| 写实暴力 | 无 |
| 长时间的写实暴力或血腥内容 | 无 |
| 性内容或裸露 | 无 |
| 露骨的性内容和裸露 | 无 |
| 粗俗或低俗幽默 | 无 |
| 成人或性暗示主题 | 无 |
| 恐怖或惊悚主题 | 无 |
| 酒精、烟草或药物的使用及相关内容 | 无 |
| 医疗或治疗信息 | 无 |
| 模拟赌博 | 无 |
| 竞赛 | 无 |
| 赌博（真钱） | 否 |
| 不受限制的网页访问（App 内置浏览器） | 否。远屏只显示用户自己电脑的画面，没有内置浏览器 |
| 用户生成内容 / 社交 | 否 |
| 用户之间的消息或聊天 | 否 |
| 广告 | 否 |

## App 隐私（营养标签）

结论：**「不收集数据」（Data Not Collected）**，「用于追踪」也是否。

依据：

| 网络请求 | 去向 | 是否算 ASC 意义上的「收集」 |
|---|---|---|
| 远控、文件、声音、SSH、VNC、RDP | 用户自己添加的电脑和服务器 | 否：去向由用户控制，开发者拿不到 |
| Bonjour 发现、局域网扫描 | 本地网络 | 否：不离开局域网 |
| Tailscale API（`api.tailscale.com`），只有用户填了自己的 API 令牌才会请求 | Tailscale | 否：用用户自己的令牌读用户自己的设备列表，只为完成这次请求 |
| 崩溃和日志 | 只存在本机 | 否：不上传，用户主动导出才离开设备 |
| 统计、广告、追踪 SDK | 没有 | — |

ASC 问卷照下面填：

- 「你或你的第三方合作伙伴是否从此 App 收集数据？」→ **否**。
- 隐私政策网址：`https://gxlself.github.io/remote-screen-site/privacy.html`。

## 出口合规

- `Info.plist` 已写 `ITSAppUsesNonExemptEncryption = false`，上传后 ASC 不再逐个构建询问。
- App 只用公开标准的加密：TLS 1.3 和 QUIC（rustls / ring）、SSH（russh）、VNC 与 Apple 远程桌面认证（DES、DH + AES-128、MD5）、RDP（NTLM / CredSSP over TLS）、配对（SPAKE2、ECDSA P-256、HKDF / HMAC-SHA-256）、录像加密（AES-256-GCM），全部来自公开的算法和开源库，没有自研算法。设备密钥在安全芯片（Secure Enclave）里生成。
- 如果 ASC 仍弹出加密问卷：选「使用标准加密算法」并确认符合 Category 5 Part 2 的大众市场豁免。向法国分发需要另行申报，见交付报告的「需要用户决定」。

## 审核备注（App Review Information › Notes）

建议直接粘贴英文版（见 `docs/store/en-US.md`），审核员多数读英文。中文版：

```
远屏是一个远程桌面客户端，用来控制用户自己的 Mac 或 Windows 电脑。不需要注册账号，也不需要登录。我们无法提供测试电脑，但任何一台打开了屏幕共享的 Mac 都可以用来测试：

1. 在 Mac 上打开「系统设置 › 通用 › 共享」，打开「屏幕共享」。点旁边的 ⓘ，在「允许访问」里选「所有用户」，或者加入你要登录的账户。
2. 让 iPhone 或 iPad 和这台 Mac 连到同一个 Wi-Fi。
3. 打开远屏。引导页第一步保持「使用手机上的 Tailscale」，直接点「继续」（不需要安装 Tailscale）；后面两步都可以直接继续。iOS 询问本地网络权限时点「允许」。
4. 首页点「添加电脑」，「同一网络里的 Mac」下会出现这台 Mac。点它，输入 Mac 的用户名和密码，就能看到并控制 Mac 的屏幕。
   如果没有自动出现，点「按地址添加」，输入 Mac 的 IP 地址（在 Mac 的「系统设置 › Wi-Fi › 详细信息」里能看到）。

Windows 同理：打开「设置 › 系统 › 远程桌面」，在远屏里按 IP 地址添加，用 Windows 账户登录。

后台模式说明：Info.plist 的 UIBackgroundModes 只有 audio。它是系统画中画（AVPictureInPictureController，播放远程电脑画面）所需的后台模式。远屏不播放静音音频，不用它在后台保活；App 进入后台时会停止播放远程声音，后台连接最多保留系统给的短时间（beginBackgroundTask），之后自动暂停。

Tailscale 是可选的：只有用户想在外面连回自己的电脑时才需要，基本功能不依赖它。远屏本身不建立 VPN，也不含组网功能：在外面连回电脑要用户自己装 Tailscale App 或连公司 VPN。远屏没有开发者自己的服务器，不收集任何数据。
```

## 截图要求（提醒）

- iPhone 6.9 英寸（1320 × 2868 或 2868 × 1320）至少 1 张，最多 10 张。
- iPad 13 英寸（2064 × 2752 或 2752 × 2064）至少 1 张：App 支持 iPad，必须提供。
- 截图里电脑画面只用你自己的测试账户，不要出现别人的姓名、邮件或公司内网信息。
