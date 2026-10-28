# M0 延迟测量

对照项目早期设计方案（未公开）§01 体验指标里的端到端延迟：**画面变化到手机上屏**，局域网直连 ≤ 40 ms，同城 5G 直连 ≤ 70 ms。

结论先行：

- 管线内各段已实测。主机屏幕出帧到客户端收齐 p50 11.5 ms，到 iOS 解码完成 p50 15.5 ms。加上真实 Wi-Fi 和手机上屏，局域网 p50 估算在 25–35 ms，落在 40 ms 目标内。
- **真机 glass-to-glass 尚未测**：需要真机签名 + 240fps 拍摄，表格留在 §3，等真机跑完填。
- 稳定性还不够：冷启动前几秒和主机唤醒显示器后，有 100–500 ms 的长尾（见 §2.4）。
- 本轮测试时屏幕处于锁屏状态，画面变化率只有 ~34 fps，所以**还没有在全动态画面下验证 1080p60 稳定 60 帧**。

## 1. 测量方法

### 1.1 管线内计时（自动，本次已做）

每帧在视频数据报头里带 `capture_ts_us`，取值为 ScreenCaptureKit 的 `SCStreamFrameInfoDisplayTime`，也就是这帧在主机屏幕上显示的时刻（mach 时间换算成 unix µs）。客户端用 control 流上的 Ping/Pong 做 NTP 式对时：取 RTT 最小的样本，offset = t2 − (t1+t4)/2。再把采集时刻换算到本机时钟。

| 段 | 在哪里量 | 含义 |
|---|---|---|
| capture→encoded | `rs-host` 日志，每 2 s 一行 | 主机显示该帧 → VideoToolbox 吐出码流（含 SCK 交付 + 编码） |
| capture→receive | `rs-probe` 汇总 | 主机显示 → 客户端收齐全部分片、重组完成 |
| capture→decoded | iOS App HUD / `[rs-stats]` 日志 / 会话结束时的 `session end` 汇总 | 主机显示 → `VTDecompressionSession` 输出图像（随后立刻 enqueue 到 `AVSampleBufferDisplayLayer`） |
| queue / decode | App 会话汇总 | 收齐 → 开始解码的等待；纯解码耗时 |

不在管线计时内的部分：手机上屏（enqueue → 面板发光，60Hz 屏 ≈ 4–16 ms，ProMotion ≈ 4–8 ms）。这部分只能靠 §1.2 的拍摄法。

### 1.2 Glass-to-glass（真机，待做）

1. Mac 上打开毫秒计时器，全屏或放大到主显示器中央，例如浏览器打开 <https://www.estopwatch.net/> 或任意 ms 级计时页。
2. iPhone 运行 Demo 连上 Mac，画面里能看到同一个计时器。手机和 Mac 屏幕并排、同一高度。
3. 另一台手机用 240fps 慢动作拍两块屏，拍 10 s 以上。
4. 逐帧回放，随机抽 ≥ 20 帧，记录「Mac 读数 − 手机读数」。取 p50 / p95 / 最大值。
5. 场景分三组：静态计时器、滚动网页、拖动窗口。网络分两组：同一 Wi-Fi；手机关 Wi-Fi 走 5G（经 Tailscale，直连而非 DERP，可用 `tailscale status` 确认 `direct`）。

240fps 下每帧 4.2 ms，读数误差约 ±4 ms；计时器本身受 Mac 刷新率限制（60Hz → 16.7 ms 一跳），所以要多取样再看分布，不要看单次。

## 2. 本次实测结果（2026-10-20）

### 2.1 环境

- 主机：MacBook Air，Apple M5，32 GB，macOS 26.3.2。合盖，外接两台 2560×1440@60Hz 显示器，采集主显示器并缩放到 1920×1080（或 1280×720）。HEVC Main，低延迟码控生效（`low-latency RC: yes`），无 B 帧（ffprobe `has_b_frames=0`），12 Mbps。
- 网络：客户端连主机的 tailnet 地址 `100.64.0.6:7400`，**客户端和主机在同一台 Mac 上**。数据走本机协议栈，没经过真实 Wi-Fi，Ping RTT 0.3–1 ms。
- 客户端：
  - `rs-probe`：桌面 Rust 客户端，只重组不解码。
  - iOS Demo：跑在 iPhone 17 Pro 模拟器（iOS 26.5）里，与主机共享同一颗 M5。
- 画面：测试时 Mac 处于锁屏，画面是锁屏界面，每秒约 33–35 帧变化。主机只在画面变化时编码，所以平均帧率是 ~34 fps，不是 60。
- 每次新会话开始，若显示器在睡眠，主机会先唤醒显示器。

### 2.2 数字

| 场景 | 时长 / 样本 | 帧率 · 码率 | 丢包 | capture→encoded（主机） | capture→receive | capture→decoded |
|---|---|---|---|---|---|---|
| A. rs-probe 1080p | 30 s / 979 帧 | 34.5 fps · 9.6 Mbps | 0（帧号无缺口） | avg 9.0–9.7 ms，2 s 窗口内 max 11–23 ms，个别 37 ms | **p50 11.5 · p95 17.4 · p99 57.5** · max 104 ms | — |
| B. rs-probe 720p | 20 s / 627 帧 | 35.2 fps · 5.8 Mbps | 0 | — | **p50 8.5 · p95 12.2 · p99 15.4** · max 29 ms | — |
| C. iOS 模拟器 1080p | 32 s / 1031 帧 | 33–35 fps · 11 Mbps | 0，解码失败 0 | 稳态 avg 8 ms | — | 整场 **p50 15.5** · p95 99.6 · p99 347；稳态 2 s 窗口 p50 14–17 · p95 16–28 |
| C 的分段 | | | | | | queue p50 0.1 · p95 1.7 · p99 143；**decode p50 4.0** · p95 11.3 · p99 20.1 |

其他：

- **首帧**：显示器已亮时，从发起连接到收到首帧 158–563 ms，满足 ≤ 0.8 s 的目标。显示器睡眠时要先唤醒，耗时约 6.5 s：唤醒约 1 s，ScreenCaptureKit 列出显示器又等了约 4.5 s。
- **画面静止时带宽**：只在画面变化时编码。静止时仅有关键帧请求触发的重编码，以及 QUIC keepalive。
- **峰值帧率**：主机唤醒显示器时的锁屏动画期间，有 2 s 窗口跑到 61–63 fps、14.7–18.8 Mbps，说明采集到编码的链路能跟上 60 fps。但这个窗口同时也是长尾最严重的时候（见 §2.4），所以还不能说「1080p60 稳定」。
- **输入**：
  - `rs-probe --move-to 0.5,0.5`：主机光标移到屏幕中心再还原，独立进程每 250 ms 采样确认。
  - iOS Demo `-selfTestMove YES`：走手势同一条代码路径（`moveCursor` → `rs_client_mouse_move` → input 流 → `CGEventPost`），光标沿路径移到中心再回到原位。
  - 本轮没有做自动点击，以免误点用户桌面。点击、拖拽和滚动的注入代码已写好，待真机手测。

### 2.3 对照方案预算

| 段 | 方案预算（局域网） | 本次实测 | 备注 |
|---|---|---|---|
| 采集 + 编码 | 8 + 3–6 ms | capture→encoded 稳态 avg 8–9 ms | 起点是「主机屏幕显示该帧」，不含方案里「平均等半帧」的那部分 |
| 分片 + 网络 + 重组 | 1 + 1–25 + 1 ms | encoded→receive ≈ 2–3 ms（11.5 − 9） | 本机回环，**不含真实 Wi-Fi** |
| 解码 | 2–5 ms | 模拟器 p50 4.0 ms | 模拟器用的是 Mac 的解码器，真机 A 系列硬解应相当或更快 |
| 上屏 | 4–8 ms | 未测 | 只能靠拍摄 |
| **合计** | **≈ 27 ms（目标 40）** | 管线内 p50 15.5 ms | 估算真机局域网 p50 ≈ 15.5 + Wi-Fi 2–8 + 上屏 4–16 ≈ **22–40 ms**，同城 5G 再加 10–25 ms 单程 ≈ **35–65 ms**。均为估算，待 §3 实测确认 |

### 2.4 已知长尾与原因

- **冷启动积压**：iOS 端创建 `VTDecompressionSession` 时（模拟器里约 300 ms），帧在通道里排队。HEVC 的 P 帧不能跳，只能按顺序解完，于是 queue p99 达到 143 ms。这是 C 场景 p95/p99 偏高的主因，2 s 预热之后基本消失。
  - 改进：解码器未就绪或积压超过 2 帧时，丢弃积压并请求关键帧（M1）。
- **主机侧突刺**：刚唤醒显示器后的头几秒，以及模拟器、Xcode 和主机同时抢同一颗 M5 时，capture→encoded 偶发 50–200 ms，RTT 偶发 90–200 ms。
  - 跨两台机器测时会少很多，需要在真机测试中复核。
  - 若仍存在，查 SCK `queueDepth` 和编码回调线程的调度。
- **无 FEC**：本次是本机回环，零丢包，所以 FEC 缺失没有暴露。真实 Wi-Fi 下丢一个分片就要等关键帧，见 §4。

## 3. 真机 glass-to-glass 记录表（待填）

| 日期 | 手机 / 系统 | 网络（Wi-Fi / 5G，direct?） | 分辨率 · 码率 | 场景 | 样本数 | p50 | p95 | max | 目标 | 达标? |
|---|---|---|---|---|---|---|---|---|---|---|
| | | Wi-Fi，direct | 1080p · 12 Mbps | 静态计时器 | | | | | ≤ 40 ms | |
| | | Wi-Fi，direct | 1080p · 12 Mbps | 滚动网页 | | | | | ≤ 40 ms | |
| | | Wi-Fi，direct | 1080p · 12 Mbps | 拖动窗口 | | | | | ≤ 40 ms | |
| | | 5G，direct | 1080p · 12 Mbps | 静态计时器 | | | | | ≤ 70 ms | |
| | | 5G，direct | 720p · 8 Mbps | 滚动网页 | | | | | ≤ 70 ms | |

同时记下 App HUD 上的 `decode p50/p95` 和 `rtt`，可以把拍摄结果拆成「管线内 + 上屏」两段。

## 4. M0 的已知限制

- 视频数据报**没有 FEC**，丢分片即丢帧，丢帧后等关键帧。真实无线网络上会出现卡顿，FEC 放在 M1。
- 没有接 tailscaled LocalAPI 的 WhoIs 鉴权。**tailnet 里任何设备都能连上并控制这台 Mac**，所以 `rs-host` 只在测试时临时开。
- 主机证书每次启动重新生成，客户端默认不校验（`rs-probe --pin` 可选校验）。还没有配对和指纹持久化。
- 只支持 HEVC，只采主显示器，不传光标形状（客户端画通用箭头），没有声音、剪贴板、键盘输入。
- 一次只服务一个客户端，新连接会顶掉旧连接。

## 5. 复现命令

```bash
cargo build --release -p rs-host -p rs-probe
```

```bash
RUST_LOG=info ./target/release/rs-host
```

```bash
./target/release/rs-probe --seconds 30 --dump /tmp/m0.h265
```

```bash
ffprobe -v error -show_entries stream=codec_name,width,height,has_b_frames -count_frames -show_entries stream=nb_read_frames /tmp/m0.h265
```

iOS 模拟器定时测量（需先按 README 构建 App）：

```bash
xcrun simctl launch --console-pty <模拟器UDID> ai.gxlself.remotescreen.demo -autoConnect 100.64.0.6 -disconnectAfter 30
```

会话结束时输出 `[rs] session end (...)`，这一行就是整场汇总。
