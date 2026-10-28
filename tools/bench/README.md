# 对照基准操作手册（M3 V9）

## 1. 这是什么

这套工具量的是 **Host 页面出帧 → 控制端客户端窗口合成上屏**。Host 全屏显示带机器时间码的 `tools/bench/page/bench.html`；控制端截取客户端窗口并解出时间码，再扣掉两台机器的钟差。

| 指标 | 含义 |
|---|---|
| 延迟 p50 / p95 / p99 / max / mean | 时间码从 Host 页面出现到控制端窗口可见 |
| 显示帧率、最长卡顿、>100 ms / >500 ms 卡顿 | 控制端实际看到的流畅度 |
| 带宽 | Host 网卡计数器的收发速率，扣掉会话前空闲基线 |
| Host CPU / GPU | 被测进程占用；GPU 另记 VideoEncode |
| SSIM | 只有 `static` 场景，比较采集帧与确定性参考图 |

**不能**直接对照 [09 · 延迟测量](../../docs/dev/09-quality-release.md) 与 [m0-latency 的 §01 手机目标](../../docs/m0-latency.md)（≤ 40 ms）：这里不含控制端面板出光。本产品第一版用 iOS 模拟器，还多一层 Simulator 合成，所以只能做同口径横向比较与版本回归。方法和场景见下文，指标由采集与汇总脚本生成。

## 2. 拓扑

```text
Windows 测试机（Host）               Mac 构建机（控制端）              Linux 开发机（调度）
192.168.31.20                       192.168.31.30                    本仓库 worktree
├─ RemoteHostService / RustDesk     ├─ 采集 rs-bench（SCK + TCC）      ├─ 交叉编译 rs-bench.exe
│  / Sunshine（只跑一个）            ├─ iOS 模拟器 App（本产品）         ├─ bench.py 编排与报告
├─ Edge 全屏 bench.html             ├─ Safari（local 基线）            ├─ selftest / report / judge
├─ 采样 sample.ps1                  ├─ RustDesk / Moonlight 客户端     └─ 单测与门禁
└─ SSH 服务（bench-win 别名）        ├─ netem mac.sh（sudo -n）
                                     └─ Aqua 会话（gui-run.sh）
         ▲                                      ▲
         └──────────── 局域网（最好有线）───────┘
```

Mac 通过 SSH 控制 Windows；它自己运行采集、被测客户端和弱网整形。Linux 只做编译、静态检查与报告，不跑 GUI。

## 3. 一次性准备

### 3.1 Mac 构建机

1. **构建可采集的二进制。** 怎么做：在仓库根目录执行 `cargo build --release -p rs-bench`。怎么验证：`target/release/rs-bench clock --help` 退出 0，且 `ls -l target/release/rs-bench` 存在；再跑 `target/release/rs-bench capture --help`，若显示 `not implemented yet (RS-V9J)`，说明本分支还没有采集循环，先等 V9J 合入，不能实跑 selftest。
2. **用固定签名身份签名。** 怎么做：先 `security find-identity -v -p codesigning` 找到本机已有的、固定的代码签名身份（不要用 `-` 的 ad-hoc 身份），再执行 `codesign --force --sign <身份> target/release/rs-bench`；重编后重复一次。怎么验证：`codesign -dv --verbose=2 target/release/rs-bench` 两次重编后 `Identifier` 与授权记录仍对应。原因：ad-hoc 签名的 cdhash 每次重编都变，macOS 会把它当成新程序，屏幕录制授权随之失效。
3. **授屏幕录制。** 怎么做：先跑一次 `bench.py status`，它会用 Aqua 会话执行 `rs-bench capture --check` 并触发系统授权；按提示到「系统设置 › 隐私与安全性 › 屏幕录制」勾选 `rs-bench`，然后重启进程。怎么验证：`bench.py status` 的 `controller.capture` 为 `ok`；V9J 合入后也可用 `rs-bench capture --list-windows` 列出窗口。
4. **授 AppleScript 自动化。** 怎么做：第一次跑 `selftest` 时在弹窗允许 `osascript` 控制 Safari；若没弹，到「系统设置 › 隐私与安全性 › 自动化」手动勾选。怎么验证：`/usr/bin/osascript -e 'tell application "Safari" to get version'` 不返回 `-1743`（未授权）。这一步授权给 `/usr/bin/osascript`，不对 `rs-bench` 授予自动化权限。
5. **写 lab.json。** 怎么做：`mkdir -p ~/.config/rs-bench && cp tools/bench/lab.example.json ~/.config/rs-bench/lab.json`，再按本机改字段：`controller.addr` 填 `192.168.31.30`，`controller.rs_bench` 保持 `target/release/rs-bench`，`host.ssh` 填 `bench-win`，`host.addr` 填 `192.168.31.20`，`host.bench_dir` 填 `C:\\rs-bench`；`sim_device` 与第 8 项完全一致。字段说明见契约 I5。怎么验证：`python3 -c 'import json; json.load(open("/Users/<你的账号>/.config/rs-bench/lab.json"))'` 无异常。
6. **配 SSH 别名并验证免交互。** 怎么做：在 `~/.ssh/config` 写 `Host bench-win`、`HostName 192.168.31.20`、`User <Windows 控制台账号>`、`IdentityFile <私钥路径>`（若用默认密钥可省略），然后 `chmod 600 ~/.ssh/config`。怎么验证：`ssh -T -o BatchMode=yes bench-win hostname` 直接返回主机名、不提示口令或 yes/no；`scp -q -o BatchMode=yes <本地文件> bench-win:C:/rs-bench/` 也能成功。
7. **配 netem 免密 sudo。** 怎么做：用 `sudo visudo -f /etc/sudoers.d/rs-bench` 新建文件，只放下面一行（把 `<账号>` 和 `<仓库绝对路径>` 换成实际值；`<账号>` 用 `whoami` 查，路径用 `pwd` 得）：

   ```sudoers
   <账号> ALL=(root) NOPASSWD: /<仓库绝对路径>/tools/netem/mac.sh
   ```

   不要用通配符、不要放行整个仓库或 `sudo`。怎么验证：`sudo -n /<仓库绝对路径>/tools/netem/mac.sh --dry-run weak` 退出 0，且 `sudo -n -l` 只多这一条。
8. **准备模拟器与 App。** 怎么做：用 `xcrun simctl list devices available` 选定设备（例如 `iPhone 17 Pro`），把它写进 `lab.json` 的 `sim_device`；用 Xcode 构建 Debug 包并安装到该模拟器（或把 `.app` 路径写进 `controller.sim_app`），启动 App 与 Host 手动配对一次，完成引导、把 App 锁关掉。App 的 bundle id 是 `ai.gxlself.remotescreen.dev`。怎么验证：`xcrun simctl list devices booted` 能看到 `sim_device`；`bench.py status` 中 `remotescreen` 两列都是 `ok`，并且 `xcrun simctl get_app_container <UDID> ai.gxlself.remotescreen.dev app` 能打印路径。

### 3.2 Windows 测试机

1. **安装 OpenSSH 服务端。** 怎么做：以管理员运行 `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`，再 `Start-Service sshd` 并 `Set-Service sshd -StartupType Automatic`。怎么验证：本机 `ssh -V` 有输出；从 Mac 执行第 6 项的免交互命令成功。SSH 登录账号必须与控制台登录账号相同——`schtasks /it` 只能投进本账号的交互会话。
2. **保持控制台会话可采集。** 怎么做：由用户在系统设置里关闭锁屏与睡眠，保持控制台登录；不要用脚本改系统设置。怎么验证：远端运行 `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\rs-bench\host\bench.ps1 session`，返回 `"logged_in":true`、`"locked":false`、`"rdp":false`。
3. **接显示器或 HDMI 假负载。** 怎么做：接真实显示器或假负载，在显示设置里设 1920×1080、60 Hz；不要靠 RDP 维持会话。怎么验证：`bench.ps1 hostinfo` 的 `display` 为 `1920x1080@60`；`bench.py status` 的 `host.session` 为 `ok`。
4. **部署 Host 脚本与 rs-bench.exe。** 怎么做：先在 Linux 上 `heavy wincargo build --release -p rs-bench`，得到 `target/x86_64-pc-windows-gnu/release/rs-bench.exe`；若链接报 `unable to find library -lgcc` / `-lgcc_eh`，说明共享 llvm-mingw 缺 GCC 运行库，要由维护者修工具链，不能自己往共享目录写文件。得到 exe 后在 Mac 上于仓库根目录执行 `just bench deploy --lab ~/.config/rs-bench/lab.json --exe <exe 路径>`。怎么验证：`bench.ps1 env` 返回 `"rs_bench_exe":true`、`"page":true`、`"edge"` 非空；`bench.ps1 hostinfo` 正常。
5. **跑 Host 自检。** 怎么做：在 Windows 上执行 `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\rs-bench\host\test-bench.ps1`（以控制台账号、在交互会话运行，不要在 RDP 会话里跑）。怎么验证：最后一行是 `PASS <n> / FAIL 0`，退出码 0；`PASS` 至少覆盖 `hostinfo`、`env`、`session`、`page`、`sample`。

### 3.3 第三方（必须先征得用户同意）

**安装 RustDesk（AGPL-3.0）与 Sunshine/Moonlight（GPL-3.0）前，必须先向用户说明用途、系统服务、残留与卸载方式并取得明确同意。** 版本、许可证、下载地址和端口表见 `tools/bench/thirdparty.json` 与契约「第三方软件」小节；下面只写操作步骤。

| 软件 | 角色 | 版本 | 许可证 |
|---|---|---|---|
| RustDesk | Windows Host / Mac 客户端 | 1.5.0 | AGPL-3.0 |
| Sunshine | Windows Host | v2026.914.233613 | GPL-3.0 |
| Moonlight（moonlight-qt） | Mac 客户端 | 6.2.0 | GPL-3.0 |

上表是便于阅读的摘要；安装包文件名、URL 与校验值以 `tools/bench/thirdparty.json` 为准，整形端口见契约。

1. **下载并核对。** 怎么做：按 `thirdparty.json` 的 `url` 用官方 GitHub Release 下载，`shasum -a 256 <文件>` 得到校验值，与发布页或上游校验值核对后填入对应条目的 `sha256`；`sha256` 为 `null` 时安装会直接拒绝。怎么验证：`python3 -c 'import json; d=json.load(open("tools/bench/thirdparty.json")); [print(k, bool(v["sha256"])) for k,v in d["items"].items()]'` 四行都是 `True`。
2. **安装与状态。** 怎么做：`just bench install rustdesk --yes`（或 `moonlight`），脚本会缓存安装包、装到 Windows 服务与 Mac 的 `~/Applications/rs-bench/`；不带 `--yes` 只打印将要安装的版本与许可证。怎么验证：`just bench status` 中对应产品两列为 `ok`；安装前会自动保存 `~/.local/state/rs-bench/inventory/<id>-before.json`。
3. **卸载与残留比对。** 怎么做：`just bench uninstall rustdesk --yes`；它会卸载后重新拍清单，和安装前比服务、程序、防火墙规则、驱动（Mac 是 `~/Library` 下的 Preferences / Application Support / Caches / LaunchAgents）。怎么验证：退出码 0 表示无残留；退出码 3 时查看 `~/.local/state/rs-bench/inventory/rustdesk-leftover.json`，逐项人工清理。`moonlight` 同理。
4. **RustDesk 不向公网注册。** 默认把 `custom-rendezvous-server` 设为 `127.0.0.1`，只走 IP 直连；如果安装后 status 显示服务已启动但客户端连不上，先确认这条配置没有被恢复成默认值，不要把测试机信息发到公网 ID 服务器。
5. **Moonlight 配对。** 自动配对失败时，在 Sunshine 网页管理页读出 PIN，按 Moonlight 版本要求手工输入完成一次配对；配对成功后再跑 `run`。第三方命令行以 V9N / V9O 合入后的实测结果为准，本文不臆造参数。

## 4. 日常使用

先确认环境：`just bench status --json` 应全部 `ok`；任何一项失败先按第 7 节处理。`just bench` 后面接的是 `bench.py` 参数，路径都在 Mac 仓库根目录执行。

| 命令 | 用途与常用参数 |
|---|---|
| `just bench status` | 检查采集、netem、Host 会话、页面、Edge 与各产品安装状态；`--json` 便于脚本判断 |
| `just bench selftest` | 只跑本机基线（Safari + 截自己的窗口），默认 10 秒；`--seconds 30` 控制时长 |
| `just bench run --lab ~/.config/rs-bench/lab.json --products local,remotescreen --scenes static --networks lan` | 先跑小矩阵；`--seconds`、`--repeats`、`--matrix` 可覆盖默认矩阵，`--dry-run` 只打印格子 |
| `just bench run --lab ~/.config/rs-bench/lab.json` | 跑默认全矩阵；`--baseline <上一版 summary.json>` 同时生成判定结果 |
| `just bench report <summary.json> --baseline <基线> --out report.md` | 从已有 summary 重新生成 Markdown，不重跑 |
| `just bench judge <summary.json> --baseline <基线> --json verdict.json` | 只做回归判定；退出码 1 表示有 regression/fail，2 表示输入错误 |
| `just bench deploy --lab ~/.config/rs-bench/lab.json --exe <rs-bench.exe>` | 把页面、Host 脚本和 exe 部署到 `lab.json` 的 `host.bench_dir` |
| `just bench install <id> --yes` | 安装 `rustdesk` 或 `moonlight`；不带 `--yes` 只预览 |
| `just bench uninstall <id> --yes` | 卸载并做安装前后清单比对 |
| `just bench-test` | 跑 Python 单测、页面 node 测试和 `rs-bench` 单元测试；改文档后也可用它确认没碰坏工具 |

第一次接手按这个顺序跑：`just bench status` → `just bench selftest` → `just bench run --lab ~/.config/rs-bench/lab.json --products local,remotescreen --scenes static --networks lan` → 按需 `install` 第三方 → `just bench run` 全矩阵。每一步都先确认退出码与 summary 状态，再进入下一步。

默认矩阵是 `tools/bench/matrix.json`：本机基线 4 格（4 个场景 × `lan`）＋ 3 个产品 × 4 场景 × 3 网络 = 40 格，每格默认 60 秒采集、约 100 秒含准备与清理，**一整轮约 70 分钟**；`--repeats` 增加时按倍数估算。

## 5. 结果

```text
target/bench/<版本>/<UTC 时间戳>/
├─ summary.json              # 整轮汇总，I3
├─ report.md                 # 人读报告与判定摘要
└─ <run_id>/
   ├─ result.json            # 单格指标与状态，I3
   ├─ frames.jsonl           # 采集帧与时间码，I1
   ├─ clock.json             # 会话前后对时，I2
   ├─ host_stats.jsonl       # Host 采样，I4
   ├─ frames/*.pgm           # static 场景保存的 SSIM 帧
   └─ run.log                # 单格编排与清理日志
```

`result.json` 由 `benchlib/analyze.py` 根据 `frames.jsonl`、`clock.json` 和 `host_stats.jsonl` 汇总；字段、单位、百分位数与排序以分析脚本为准，不要从日志里猜指标。

| 状态 | 判定 |
|---|---|
| `ok` | 该格完整跑完；延迟有样本，`n >= 300`，解码失败率 ≤ 5%，无效延迟占比 ≤ 1% |
| `invalid` | 跑完了但延迟为 null、样本不足 300、`decode_fail_ratio > 0.05`，或 `invalid_latency / max(1, distinct_codes) > 0.01`；原因在 `error` |
| `failed` | 编排某一步失败（会话、安装、采集、网络整形等），原因在 `error`；cleanup 失败也会写进日志 |

**归档。** 把本轮 `summary.json` 放到 `docs/dev/latency/<版本>/bench/summary.json`，把 `report.md` 的内容并入 `docs/dev/latency/<版本>.md`。不要提交 `target/`、`.pgm` 或日志；下一版用上一版归档的 `summary.json` 作基线。

**判定。** 例如上一版是 `0.5.0`、本轮 `0.6.0`：

```sh
just bench judge target/bench/0.6.0/<时间戳>/summary.json \
  --baseline docs/dev/latency/0.5.0/bench/summary.json \
  --json target/bench/0.6.0/<时间戳>/verdict.json
```

默认阈值在 `tools/bench/thresholds.json`：本产品同格子相对上一版比较，p50 超 15% 且超 3 ms、p95 超 20% 且超 5 ms、帧率掉 10%、最长卡顿超 50% 且超 100 ms、SSIM 掉 0.02 等；`weak` 下最长卡顿超 500 ms 直接 fail。规则由阈值 JSON 定义。要调整时改阈值 JSON 并连同结果一起评审，别改判定代码；只对本轮临时试算可用 `--thresholds <文件>`。

## 6. 已知限制

- **`mac.sh` 不加抖动。** macOS dummynet 只应用延迟、丢包和带宽，结果统一记 `jitter_applied: false`；`weak` 的抖动与 09 的定义不完全一致。
- **模拟器偏差。** 本产品走 iOS 模拟器，多一层合成，且默认竖屏会把 16:9 画面缩小；延迟与 SSIM 对本产品不利，不能拿它的绝对值去比手机。
- **带宽是 Host 网卡计数器。** Windows 更新、同步盘或其他测试流量会混进来；只看扣掉空闲基线后的相对变化。
- **Sunshine 要控制台会话。** 锁屏、注销或 RDP 会话会让它采不到画面；必须接显示器/假负载并保持控制台登录。
- **录屏授权可能反复询问。** macOS 15 起系统可能定期要求再次确认；每次确认后用 `capture --list-windows` 复验，不要把它当成脚本故障。
- **第三方命令以实测为准。** RustDesk / Moonlight / Sunshine 的 CLI 尚未在本仓库验证，V9N、V9O 合入后按实测补全本节与第 3.3 节。

## 7. 故障排查

| 现象 | 原因 | 处理 |
|---|---|---|
| `capture: calib-timeout` | 客户端窗口被遮挡、Host 页面没起来，或客户端没连上 | 看该格 `run.log` 与 Host `bench.ps1 session`；把窗口置前、重跑 `page-start`、确认 App/客户端已连上后再采集 |
| `capture: no-window` | `lab.json` 的 bundle id、窗口标题或模拟器设备名不对 | V9J 合入后运行 `target/release/rs-bench capture --list-windows`，按输出的 app/title 修正适配器或 `sim_device`（当前分支该子命令仍是桩） |
| `netem unavailable` | `mac.sh` 的 sudoers 没配好，或 `controller.netem` 是 `none` | 用第 3.1 节第 7 项复验 `sudo -n ... mac.sh --dry-run weak`；成功后再跑 `weak`/`wifi-good` |
| `host console session not available` | Windows 锁屏、注销，或当前是 RDP 会话 | 回到控制台登录、解锁屏幕，关闭 RDP；`bench.ps1 session` 应显示 `logged_in:true`、`locked:false`、`rdp:false` |
| `bench.ps1 … bad output` | 有程序或 profile 往 stdout 写了日志/提示，破坏了单行 JSON | 用 `-NoProfile` 重跑该命令，检查 `$PROFILE`、`Write-Host` 与自定义产品脚本；stdout 只能留一行 JSON |
| `clock probe failed` | SSH 别名不可用、`rs-bench.exe` 不存在，或 Host 的 `clock serve` 起不来 | 复验 `ssh -T -o BatchMode=yes bench-win hostname`；查 `bench.ps1 env` 的 `rs_bench_exe`；必要时重新 `deploy --exe` |
| `edge did not start` | 控制台会话未登录/已锁，或 Edge 不在默认安装路径 | 先修会话，再确认 `bench.ps1 env` 的 `edge` 非空；仍失败则手工启动一次 Edge 排除策略拦截 |
| 重编后采集报无权限 | 签名身份变了，TCC 把 `rs-bench` 当成新程序 | 按第 3.1 节第 2 项用同一固定身份重签；到「屏幕录制」删旧条目后重新授权 |
| Moonlight 黑屏 | 正在用 RDP 会话，或显卡不支持硬件编码导致 Sunshine 无输出 | 切回控制台会话；在 Sunshine 日志确认编码器，必要时换支持硬件编码的显卡/驱动 |
| 延迟出现负值或很大 | 对时失败或两次探针漂移，Host 与控制端钟差估错 | 查看该格 `clock.json` 的 `before`/`after` 与 `spread_us`；重跑 `run`，确认 SSH 链路稳定且两台机器时间同步正常 |

排查后重跑最小集合（例如 `--products local --scenes static --networks lan`），确认 `status` 恢复再跑全矩阵；不要直接删掉失败格来“美化”报告。
