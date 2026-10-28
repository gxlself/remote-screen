# Glass-to-glass 延迟测量工具

本目录是 [09 · 测试、发布与运维 §延迟测量](../../docs/dev/09-quality-release.md#延迟测量)
里真机 glass-to-glass 测量用的自包含工具，测量方法沿用
[m0-latency §1.2](../../docs/m0-latency.md#12-glass-to-glass真机待做)。

| 文件 | 用途 |
|---|---|
| `timer.html` | 在被控屏上全屏显示的毫秒计时器、逐帧翻转色块和滚动条纹 |
| `summarize.py` | 从拍摄抽样 CSV 计算样本数、p50、p95、max、均值 |
| `test_summarize.py` | `summarize.py` 的标准库单元测试 |
| `run.sh` | 起 host-fake、选择弱网整形并运行 `rs-probe --json` |
| `regress.py` | 与最近 N 天、相同 shaping 的 p95 中位数比较 |
| `test_regress.py` | `regress.py` 的标准库单元测试 |
| `faultnet-proxy/` | 无特权 loopback 延迟代理（faultnet 场景的启动器） |

## 拍摄步骤

1. 在被控 Mac 上用浏览器打开 `tools/latency/timer.html`，按 `F` 进入全屏。
   页面没有网络请求，也可以直接以 `file://` 打开。用 `python3 -m http.server`
   从本目录起本地服务也可以；不要让页面访问外网。
2. iPhone 运行 Remote Screen，连接并查看这台 Mac。手机和 Mac 两块屏并排、
   尽量同一高度，保证一台相机能同时拍清两块屏。
3. 用另一台手机以 **240 fps** 慢动作拍摄 **至少 10 秒**。相机固定，避免
   自动曝光或自动对焦在大色块上反复调整。需要测滚动场景时，按一次 `S`
   打开页面下半部分的匀速条纹；再按一次 `S` 回到静止计时器。
4. 逐帧回放，随机抽取 **至少 20 帧**。对每一帧读两块屏的 `ss.mmm` 值，
   不要只取连续几帧，也不要用单次读数代表整场延迟。
5. 把抽样写进 CSV（格式见下），运行 `summarize.py`，把结果填进
   m0-latency §3 的表格。每个场景单独一份 CSV，并记录日期、设备、系统、
   网络、分辨率/码率、场景和目标值。

`timer.html` 每帧翻转色块并显示 `00`–`99` 的帧号，用来核对视频里看到的是
哪一帧；画面的刷新率读数来自最近约 1 秒的 `requestAnimationFrame` 次数。
数值由 `performance.timeOrigin + performance.now()` 换算成墙钟毫秒，精度受
浏览器时钟和显示器刷新节奏限制。
240 fps 视频每帧间隔约 4.2 ms，逐帧读数本身约有 ±4 ms 的采样误差，所以应
使用至少 20 个抽样看分布，而不是比较单帧。

## CSV 格式

UTF-8、逗号分隔，第一行是固定的两列表头，后面每行一个抽样：

```csv
host_ms,phone_ms
59880,40
12123,12170
...
```

脚本计算 `latency = phone_ms - host_ms`；差值为负数时加
`60000`，处理分钟回绕。输入可带小数，统计保留输入精度，输出最多三位小数并
去掉末尾的 0。允许省略表头，但推荐保留。空文件、只有表头、缺列、负数或
非有限数都会报错并以退出码 2 结束。

## 汇总命令

```bash
cd tools/latency
python3 summarize.py samples.csv
python3 summarize.py samples.csv --md
python3 summarize.py < samples.csv
```

`--md` 只输出一行，可贴进 m0-latency §3 的“样本数 / p50 / p95 / max”四列：

```text
| 21 | 38 ms | 57 ms | 91 ms |
```

百分位使用**最近秩法**：排序后有 `n` 个样本，`p` 百分位取第
`ceil(p / 100 * n)` 个（1-based），例如 20 个样本的 p95 是第 19 个，
4 个样本的 p95 是第 4 个（也就是最大值）。均值为算术平均，四舍五入到
0.1 ms 显示，不参与 p50/p95/max 的最近秩计算。

## 测试

```bash
cd tools/latency
python3 -m unittest -q
```

测试覆盖分钟回绕、空输入报错、p95 最近秩取法、Markdown 输出和坏 CSV 的
错误码。

## 每晚回归

### 命令

```bash
just latency 60
just latency 20 target/latency/smoke.json   # 本机冒烟
```

`just latency` 调用 `tools/latency/run.sh SECS OUT`。脚本先构建
`rs-host`（fake 平台）、`rs-probe` 和 faultnet 启动器，再在 loopback 的
`RS_TEST_PORT_BASE..+RS_TEST_PORT_SPAN` 范围内选两个空闲端口：一个给 host-fake，
一个给延迟代理。host-fake 用 `--dev-allow-unpaired` 启动，探针使用
`--identity target/latency/identity`；退出 trap 只停止本脚本记录的 PID。

整形方式按顺序选择，并写入 JSON 的 `shaping`：

1. 能建立用户/网络命名空间且 `tc` 可用时，在隔离的 `lo` 上应用
   `tools/netem/linux.sh wifi-good`，值为 `netem-wifi-good`（2 ms ± 1 ms、
   100 Mbit）。普通非 root 开发机上 `unshare` 通常失败，因此不会改共享网卡。
2. 否则启动 `tools/latency/faultnet-proxy`，经进程内 faultnet 代理施加两个
   方向的 2 ms ± 1 ms、100 Mbit，值为
   `faultnet-delay-2ms-jitter-1ms-100mbit`。
3. 代理也无法启动时直接连接 host-fake，值为 `none`。

脚本的日志与 host 临时状态在 `$TMPDIR/rs-latency-run/`，采样 JSON 的行内字段
由 probe 产生；脚本只追加 `shaping` 与 unix 微秒 `timestamp_us`。
`--json <路径|->` 的 schema 1 格式：

```json
{"schema":1,"seconds":60,"frames":3500,"capture_to_receive_ms":{"p50":6.2,"p95":12.4,"p99":25.0,"max":80.0},"fps":59.8,"bitrate_kbps":12000.0,"codec":"hevc","addr":"127.0.0.1:33401","git_rev":"<sha>|null","shaping":"faultnet-delay-2ms-jitter-1ms-100mbit","timestamp_us":1792507200000000}
```

`capture_to_receive_ms` 的单位是毫秒，其余延迟采样由 probe 排除会话开始后的
第一秒。`git_rev` 来自 `RS_GIT_REV`，未设置时为 `null`；probe 的文本汇总仍
写到 stdout，`--json -` 会在汇总后额外输出一行 JSON。如果采样期间没有任何
可用于延迟统计的帧，probe 不写 JSON 并以非 0 退出；`run.sh` 也会删除旧的
`OUT`，避免把上一次结果误当成当次结果。

### History 格式与判定

CI 的 history 目录是 `target/ci-cache/latency-history/`，cache key 固定为
`latency-history`。每次成功生成 JSON 后，`regress.py` 将 current 原样复制为
`latency-YYYYmmddTHHMMSSZ.json`（UTC，由 `timestamp_us` 换算）；若同一秒已
存在，则追加 `-1`、`-2` 等后缀。读取时优先用 JSON 内的 `timestamp` 或
`timestamp_us`，其次用文件名中的时间戳；current 文件本身没有时间信息时，
可用文件 mtime 作为回退。

```bash
python3 tools/latency/regress.py \
  --history target/ci-cache/latency-history \
  --current target/latency/latest.json \
  --days 7 --threshold 0.20
```

只取 `shaping` 与 current 相同、且时间落在 current 之前最近 7 天内的历史样本；
过期样本、其他 shaping 的样本和坏 JSON 都忽略。history 少于 2 个时（含当次
共少于 3 个样本）打印 `insufficient history` 并以 0 退出。达到 3 个样本后，
取 history 的 p95 中位数（current 不参与中位数），current p95 大于
`中位数 × (1 + threshold)` 时打印 `LATENCY REGRESSION …` 并以 1 退出。
CI 作业设置了 `allow_failure: true`，因此退化只告警、不挡合并；JSON artifacts
保留 7 天。

回归脚本的测试（仓库根目录或本目录运行均可）：

```bash
python3 -m unittest tools/latency/test_regress.py
cd tools/latency && python3 -m unittest -q test_regress.py
```
