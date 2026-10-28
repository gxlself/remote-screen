# 弱网注入

场景表来自 `docs/dev/09-quality-release.md`，实际参数在 `profiles.toml`。所有参数按单向配置；Linux 的 `tc` 作用在指定网卡的出方向，macOS 对进入和离开本机的流量使用两个 dummynet pipe。

## Linux 软路由

```sh
sudo tools/netem/linux.sh weak wlan0
sudo tools/netem/linux.sh handover wlan0       # 前台循环，Ctrl-C 恢复
sudo tools/netem/linux.sh bw-step wlan0        # 前台循环，Ctrl-C 恢复
sudo tools/netem/linux.sh show wlan0
sudo tools/netem/linux.sh off wlan0            # 可重复执行
tools/netem/linux.sh --dry-run weak wlan0      # 只打印将执行的命令
```

Linux 软路由的简要步骤：

1. 用 hostapd 把一块无线网卡（例如 `wlan0`）设为热点；手机连接该 SSID。需要 NAT/转发时同时配置 `ip_forward` 和防火墙转发规则。
2. 确认手机与 Host 的 tailnet 连接确实经过这块网卡；tailnet 流量不会被排除，也会一起受到弱网影响。
3. 在软路由上执行上面的 `linux.sh`。`udp-blocked` 优先创建独立的 `nft` 表 `inet rs_netem`，环境没有 `nft` 时改用带 `rs-netem` 注释的 `iptables` 规则；`off` 会清理两种标记。

## macOS

```sh
sudo tools/netem/mac.sh weak
sudo tools/netem/mac.sh --dry-run weak
sudo tools/netem/mac.sh --ports 7400,41641 weak
sudo tools/netem/mac.sh show
sudo tools/netem/mac.sh off
```

脚本用 `dnctl pipe` 和 PF 锚点 `com.apple/rs-netem`，默认匹配 TCP/UDP `7400` 以及 WireGuard UDP `41641`；`--ports` 会覆盖 TCP 和 UDP 的匹配端口。`udp-blocked` 使用 `pf` 的 UDP block，`handover` 和 `bw-step` 在前台循环，按 Ctrl-C 会清理锚点和 pipe。

### 为什么锚点必须叫 `com.apple/rs-netem`

macOS 自带的 `/etc/pf.conf` 只引用 `com.apple/*` 这一组锚点：

```
scrub-anchor "com.apple/*"
nat-anchor "com.apple/*"
rdr-anchor "com.apple/*"
dummynet-anchor "com.apple/*"
anchor "com.apple/*"
load anchor "com.apple" from "/etc/pf.anchors/com.apple"
```

主规则集里没有 `rs-netem`，所以把规则加载到顶层锚点 `rs-netem` 只是把规则存进了 PF，**永远不会被评估**，弱网注入看起来成功、实际无效。改成 `com.apple/rs-netem` 后，这个子锚点被 `com.apple/*` 通配引用，`dummynet` 规则和 `udp-blocked` 的 `block` 规则都会真正生效，同时不需要修改系统 `pf.conf`（也不该改：系统升级会覆盖它，而且会影响本机其他 PF 使用者）。

### PF 引用令牌

`pfctl -E` 会输出一个引用令牌（`Token : <数字>`），脚本把它写到 `${TMPDIR:-/tmp}/rs-netem.pf-token`。`off`（以及 `handover` / `bw-step` 退出时的清理）用 `pfctl -X <token>` 只释放脚本自己拿到的这一个引用，而不用 `pfctl -d`——后者会把别人（例如系统服务或同事）开启的 PF 一起关掉。令牌文件不存在时 `off` 会跳过释放，因此可以重复执行；下一次 `pfctl -E` 之前也会先释放遗留令牌，避免中断的运行泄漏引用。

`show` 打印锚点里的 `rules` 和 `dummynet` 规则以及 `dnctl list`，用来确认规则确实挂在会被评估的锚点上。

## 注意事项

- `profiles.toml` 的参数是单向的。Linux 的 `tc qdisc` 主要限制离开指定网卡的流量；若需要双向限制，请在链路两端或命名空间对端另行注入。
- macOS dummynet 没有抖动参数，`jitter_ms` 会被忽略并打印提示；延迟、丢包率、带宽仍按场景设置。
- macOS 规则只对本机实际经过 PF 的流量有效，通常只对物理网卡上的连接生效。WireGuard 的 UDP 端口默认已包含，tailnet 流量需确认实际使用的端口，否则用 `--ports` 调整。
- 只在本机测试 `--dry-run`；脚本不应在共享开发机或生产环境真实执行 `tc`、`iptables`、`nft`、`dnctl` 或 `pfctl`。
- 运行需要 root 权限，且 Linux 注入需要存在 `ip`、`tc` 和可用的防火墙工具。

## 自测

```sh
bash tools/netem/test.sh
```

自测只运行两个脚本的 `--dry-run`，覆盖全部 9 个场景、清理路径和错误路径；最后输出 `ALL OK`。
