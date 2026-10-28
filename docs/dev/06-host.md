# 06 · 被控端（Host）

对应设计文档 §05。M0 的 `host/` 是一个命令行程序：只采主显示器、一次一个客户端、无鉴权。本文是产品形态的设计。

## 组成

| 部分 | crate / 目录 | 进程 | 职责 |
|---|---|---|---|
| 平台无关核心 | `crates/host-core` | 在 agent 里 | 监听、鉴权、配对、会话管理、推流管线、更新、本地 IPC |
| Mac 平台层 | `crates/host-mac` | 在 agent 里 | 采集、编码、注入、光标、剪贴板、电源、锁屏、本机操作检测 |
| Windows 平台层 | `crates/host-win` | 服务 + 会话进程 | 同上，外加服务、桌面切换、防火墙 |
| 测试平台层 | `crates/host-fake` | 在 agent 里 | 合成画面（带帧号和时间戳的测试图），记录收到的输入 |
| 常驻进程 | `crates/rs-host-agent` | Mac：LaunchAgent；Windows：服务 | 装配以上部分 |
| 界面 | `host-ui/`（Tauri 2） | 用户会话里 | 菜单栏 / 托盘、设置、向导、确认弹窗、被控胶囊、更新窗口 |

## host-core

```
host-core/src/
├─ config.rs        # config.toml 读写、默认值、热更新通知
├─ identity.rs      # 主机密钥与证书（经 Platform::keystore 存取），host_id、fp
├─ pairing.rs       # 配对表、配对码、SPAKE2 服务端、限速
├─ pwpair.rs        # 账户密码配对：密码锁与校验流程（M2）
├─ pairfile_store.rs # 配对文件 pairfiles.json（M2）
├─ sysauth.rs       # SystemAuthenticator：系统账户密码校验接口（M2）
├─ trust.rs         # 信任日志：验签、执行（没有下发来源，见 04 · 信任声明）
├─ auth.rs          # 04 · 连接时的鉴权，输出 Decision
├─ whois.rs         # tailscaled LocalAPI 客户端 + CLI 兜底 + 缓存
├─ listen.rs        # QUIC / TCP 监听、绑定策略、来源过滤、mDNS 广播
├─ session/
│  ├─ manager.rs    # 一台电脑的全部连接：控制者、观看者、接管、恢复令牌
│  ├─ conn.rs       # 单条连接：控制流状态机、输入路由、光标流、剪贴板
│  └─ approval.rs   # 本机确认（经 IPC 让界面弹窗），30 秒超时
├─ pipeline/
│  ├─ capture.rs    # 每块显示器一个采集源，扇出给多个编码器
│  ├─ encoder.rs    # 编码器包装：码率 / 帧率 / 缩放 / 强制关键帧 / 静止补帧
│  ├─ packetizer.rs # 分片 + FEC + 数据报发送 / TCP 记录
│  └─ ratectl.rs    # 03 §7 码率控制（逻辑在 rsp-core::ratectl，这里接线）
├─ input.rs         # 控制者的输入 → Platform::inject；本机操作时暂停
├─ update.rs        # 自动更新（D13）
├─ ipc.rs           # 本地 JSON-RPC 服务端（给 host-ui）
├─ audit.rs         # 本地审计队列，离线时先存
└─ logs.rs          # tracing 输出、14 天滚动、导出打码
```

### 平台接口

```rust
pub trait Platform: Send + Sync + 'static {
    fn info(&self) -> HostInfo;                                   // 名称、系统版本、机型
    fn displays(&self) -> Vec<DisplayInfo>;
    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>);  // 插拔、分辨率变化

    fn capture(&self, display: u32, cfg: CaptureConfig, sink: FrameSink) -> Result<Box<dyn CaptureHandle>>;
    fn audio_capture_supported(&self) -> bool { false }
    fn capture_audio(&self, cfg: AudioCaptureConfig, sink: AudioChunkSink) -> Result<Box<dyn CaptureHandle>> {
        let _ = (cfg, sink);
        Err(PlatformError::Unsupported)
    }
    fn capture_region_supported(&self) -> bool { false }                    // M3 视口裁剪
    fn capture_region(&self, display: u32, rect: PixelRect, cfg: CaptureConfig, sink: FrameSink)
        -> Result<Box<dyn CaptureHandle>> { let _ = (display, rect, cfg, sink); Err(PlatformError::Unsupported) }
    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>>;   // 硬件编码器

    fn inject(&self, ev: InputEvent, display: &DisplayInfo);
    fn release_all(&self);
    fn session_action(&self, kind: SessionActionKind) -> Result<()>;   // 锁屏、Ctrl+Alt+Del、唤醒显示器

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>);      // 形状 + 位置
    fn watch_local_activity(&self, tx: mpsc::Sender<bool>);      // 电脑前有人操作（排除我们注入的事件）
    fn clipboard_get(&self) -> Option<ClipboardItem>;
    fn clipboard_set(&self, item: ClipboardItem) -> Result<()>;
    fn watch_clipboard(&self, tx: mpsc::Sender<()>);

    fn power_keep_awake(&self, on: bool);
    fn console_state(&self) -> ConsoleState;                     // 已登录 / 锁屏 / 登录窗口
    fn screen_sharing_enabled(&self) -> Option<bool> { None }     // 默认无法判断
    fn filevault_enabled(&self) -> Option<bool> { None }          // 默认无法判断
    fn permissions(&self) -> Permissions;                        // 屏幕录制、辅助功能
    fn request_permission(&self, p: PermissionKind);
    fn nics(&self) -> Vec<Nic>;                                  // 名称、地址、网段、MAC、是否 tailnet
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()>;
    fn keystore(&self) -> &dyn KeyStore;                         // 主机密钥存取
    fn system_authenticator(&self) -> Option<Arc<dyn SystemAuthenticator>> { None }  // None = 不支持账户密码配对
    fn virtual_displays(&self) -> Option<Arc<dyn VirtualDisplayBackend>> { None }  // M3 虚拟显示器；None = 不支持
}

pub trait Encoder: Send {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()>;  // 异步输出到 sink
    fn set_rate(&mut self, kbps: u32, fps: u32);
    fn force_keyframe(&mut self);
    fn set_quality_boost(&mut self, on: bool);                   // 静止补帧
    fn roi_supported(&self) -> bool { false }
    fn set_roi(&mut self, regions: &[RoiRegion]) -> Result<()> { let _ = regions; Err(PlatformError::Unsupported) }
}
```

虚拟显示器后端 `host_core::vdisplay::VirtualDisplayBackend`（`create` / `destroy` 可能阻塞数秒，host-core 放进 `spawn_blocking`；`caps` / `cleanup_stale` 须在百毫秒内返回，Host 启动时同步调用）：

- Host 启动时调一次 `virtual_displays()` 并缓存，接着调 `cleanup_stale()`、`caps()`。`caps()` 为 `Some` 才声明 `VIRTUAL_DISPLAY`。
- 平台只负责造屏和销毁。新屏照常出现在 `displays()` / `watch_displays()` 里，host-core 认领它：Mac 按 `CGDirectDisplayID` 认领，Windows 按 EDID 名字认领。
- 采集、编码、注入都走普通显示器的路径。
- 模式选择、排列、生命周期是纯逻辑：`crates/host-core/src/vdisplay/{mode,arrange,manager}.rs`。

`CapturedFrame` 持有平台原生图像句柄（Mac `CVPixelBuffer` / IOSurface，Windows `ID3D11Texture2D`），只在平台层内部解开，`host-core` 不碰像素，保证零拷贝。

平台抽象的两项诊断查询正式签名为 `fn screen_sharing_enabled(&self) -> Option<bool>` 和 `fn filevault_enabled(&self) -> Option<bool>`。`Some(true)` / `Some(false)` 分别表示已开启 / 未开启，`None` 表示无法判断；trait 均提供默认实现返回 `None`，平台实现可覆盖，诊断页对 `None` 显示「未知」（trait 与平台查询、诊断展示实现待跟进）。FileVault 是否开启与当前是否因 FileVault 未解锁而无法登录是两个不同的状态。

系统账户校验（`host_core::sysauth`，账户密码配对用，见 [04 · 系统账户密码](04-identity-security.md#系统账户密码)）：

```rust
pub struct SystemAccount { pub short_name: String, pub full_name: String, pub is_admin: bool, pub is_console_user: bool }
pub enum SysAuthError { WrongPassword, Unavailable(String) }
pub trait SystemAuthenticator: Send + Sync {
    fn lookup(&self, account: &str) -> Result<Option<SystemAccount>, SysAuthError>;  // 短名或全名；找不到 → Ok(None)
    fn verify(&self, short_name: &str, password: &[u8]) -> Result<(), SysAuthError>; // 错 → WrongPassword
}
```

两个方法都可能阻塞，调用方放进 `spawn_blocking`。实现方不得记录或保存密码。Mac 用 OpenDirectory（见 [macOS · 其他](#其他)），Windows 用 `LogonUserW`（W 轨道）；其他平台返回 `None`。

系统声音使用 `AudioCaptureConfig { sample_rate: u32, channels: u8, exclude_self: bool }`，M2 只用 48 kHz、双声道、排除本进程。`AudioChunk { capture_ts_us: u64, channels: u8, frames: u32, samples: Vec<f32> }` 的时间是块首样本的 unix 微秒，与视频同一时钟；`frames` 为每声道样本数，`samples` 是交错 f32（[-1, 1]），长度等于 frames × channels。`AudioChunkSink` 为 `tokio::sync::mpsc::Sender<AudioChunk>`。

- 平台用 `try_send` 交块，满了就丢，绝不阻塞采集线程。
- 采样率必须正好等于请求值，做不到返回 `Unsupported`。不在 host-core 里重采样。
- drop handle 后 100 ms 内停止采集并释放 sink（通道随之关闭）。
- 与视频采集互相独立：视频重建、换屏不影响声音。
- 权限不足返回 `PermissionDenied`。macOS 上「屏幕与系统录音」一个授权同时管画面和声音。
- Host 只在 `platform.audio_capture_supported()` 为 true 时，在 `ServerHello.features` 里带 `AUDIO_OPUS`；默认实现返回 false。
- Host 不加 `[audio]` 配置开关，控制者和旁观者都能收声音（[03 §8.3](03-protocol.md#83-声音0x11m2)）。

### 推流管线

```
           ┌─────────────┐   每块显示器一个采集源
           │ capture(d1) │──┬──► encoder(连接 A，1440p60 12 Mbps) ─► packetizer(A) ─► QUIC A
           └─────────────┘  └──► encoder(连接 B，观看者，720p30 3 Mbps) ─► packetizer(B) ─► QUIC B
```

- 采集源按显示器共享；编码器按连接独立（设计文档 §9.12「每个连接各自一路码流」），所以控制者的码率不被观看者拖累。观看者上限为 `[access] max_viewers`（`u8`，默认 3）；名册已用配置，鉴权入口仍写死 3（实现待跟进）。
- 采集只在画面变化时出帧（SCK、DXGI 都如此），静止时带宽接近 0（设计文档 §01）。
- 编码输出回调在平台线程上，经 `mpsc::channel(4)` 交给 tokio；采集帧同样走 `channel(4)`。通道满时**丢掉新到的这一帧**，并尽快要一个关键帧：Mac 置 `skip_until_key` 并调用 `rs_encoder_keyframe`，关键帧送达前继续丢非关键帧；Windows 见「Windows · 进程」的管道背压；host-fake 让下一帧直接编成关键帧。采集帧满了只丢不补（Windows 服务侧计入 `dropped_frames`）。
- packetizer 每帧取一次 `max_datagram_size()`，按 [03 §5](03-protocol.md#5-视频分片与-fec) 切片。
- 有声音的连接上（发声音的任务存活期间登记 `AudioPriority`），QUIC 视频分片不再一次性塞给 quinn，而是按估算速率 pacing：每发满 8 KiB 等一下，间隔 = `(rtt + 3 × (rtt − min_rtt)) × 字节数 / cwnd`；等待期间继续从编码队列预取，最多再收一个队列容量的帧，不丢帧、不乱序。只看本地发送队列水位不够——它看不到已经在途、排在瓶颈链路上的字节，大窗口能把整个关键帧一次推进瓶颈队列，把声音堵在后面。实现：`crates/host-core/src/pipeline/backlog.rs`（AU2G）；规则与测量口径见 [03 §8.3](03-protocol.md#83-声音0x11m2)。
- 视口感知编码（[03 §5.5](03-protocol.md#55-视口感知编码m3)）：`handlers/viewport.rs` 收 `Viewport`，状态机在 `pipeline/viewport.rs`，尺寸与 ROI 换算在 `pipeline/viewport_geom.rs`。裁剪用 `capture_region` 重建本连接的管线；ROI 用 `Encoder::set_roi`，不重建。

### 会话管理

```
连接状态：Handshaking → Authenticating → (PendingApproval) → Active(Controller|Viewer) → Paused → Closed
                                                                       ▲      │
                                                       resume_token ───┘      └─► Lingering(60 s，保留角色/显示器/画质)
```

- 同一台设备的新连接顶替旧连接（关闭码 3），继承角色。
- 控制者断开：保留角色、显示器、画质 60 秒给原设备恢复；保留期内同一用户的新连接申请接管也必须本机确认（30 秒），不能按「同用户立即交接」处理。保留期结束后同用户观看者可 `RequestControl` 直接取得控制，仍受有人值守策略约束；当前控制者在线时，同用户接管才立即交接（03 §4.3）。
- 恢复令牌是 16 字节的 `session_token`，旧连接仍存活时可消费，旧连接进入保留期后 60 秒内有效，成功即单次消费并换新；TCP 输入连接用新令牌重新 `AttachInput`。恢复推流发 `StreamConfig(epoch+1)`，客户端重置重组器，Host 直接出关键帧及当前光标。`resumed=false` 按新会话鉴权，客户端恢复最终失败映射 199（完整恢复 / 换路、epoch 与输入重绑接线实现待跟进）。
- `PauseStream` 停止当前连接编码，`ResumeStream` 立即出关键帧；暂停 45 秒未恢复且连接仍在时自动恢复推流（实现待跟进：当前超时动作是 `CloseBackground`）。
- 电脑前有人操作（`watch_local_activity` = true）：远端输入暂停，发 `SessionState(input_paused_local=true)`；无操作 3 秒后恢复（画布 `SessionLocalUse`）。
- 空闲：控制者 30 分钟（可配）没有输入，提前 60 秒发 `IdleWarning`，到时结束；设置了「结束后锁屏」就锁屏（画布 `SessionIdle`）。
- 会话开始、结束、角色变化都写审计（事件和字段见下文[审计事件](#审计事件)；后台恢复不拆会话，保留期过期时补记结束），并通过 IPC 通知界面刷新胶囊和菜单。
- 虚拟显示器归属建它的会话，随会话真正结束而销毁。Host 每 250 ms 检查一次：归属会话不在名册里、或保留期已过的，就销毁（03 §3「虚拟显示器」）。

### 本机身份与审计

Host 不连接任何中心服务（2026-10-24 移除 Hub，见 [01 · D51](01-decisions.md#d51--移除-hub2026-10-24)）。连接、配对和鉴权都在本机完成：经 tailnet 或已配置的内网地址直连，用 6 位配对码或配对文件配对，按本机配对表、权限与本机确认策略鉴权；WhoIs 用来解析本机 Tailscale 登录用户。需要停用某台手机时，在 Host 本机移除它（IPC `devices.revoke` 只在本机执行）。连接记录留在本机，Host 由用户手动更新，SSH 使用设备公钥登录。

旧 `config.toml` 里的 `[hub]` 节不再读取，加载时只告警、照常启动；`--notify-uninstall` 作为隐藏的空操作参数保留（见「agent 命令行」）。

`AgentIdentity.user` 为本机 Tailscale 当前登录用户的登录名，读取 LocalAPI `/localapi/v0/status` 的 Self 用户（`Self.UserID` 关联 `User[UserID].LoginName`）。`[access] allowed_users=[]` 时「只允许本机 Tailscale 登录的用户」也使用此来源。不能改用系统账户或 tag 所有者。信任日志里的 REQUIRE_CONFIRM / APPROVE_DEVICE 声明仍与此身份比对，REVOKE_DEVICE 全局执行（04 · 信任声明）。

审计先写本地队列（`audit.rs`），经 IPC `audit.list` 给 host-ui 显示。每条记录带 `ts_us`（原始 unix 微秒，u64）和 `seq`（u32）：`seq` 由队列在 push 时分配，同一 `ts_us` 内从 0 递增，不同 event 共用该计数，持久化时保留原值。重启后允许对同一微秒重新从 0 开始，因为 `ts_us` 不会回退。秒级显示 `ts = floor(ts_us / 1_000_000)`，不能从秒级 `ts` 反推原微秒。

### 审计事件

每条审计记录的字段：`ts`、`ts_us`、`seq`、`user`、`device_id`、`host_id`、`server`、`event`、`path`（`tailnet_direct` / `tailnet_derp` / `intranet` / `lan`）、`duration_s`、`detail`（JSON，不含内容数据）。

**事件口径**（D40）：
- `session_start` / `session_end`：一个会话一对。`detail = {role: "controller" | "viewer"}`，`path` 取会话的路径。
  - App 切后台（`Goodbye BACKGROUND`）或连接断开进入 60 秒保留期，不算结束。
  - 保留期内用 `resume_token` 恢复（`resumed=true`），不写新的 `session_start`。
  - 旧连接因恢复被 `Replaced` 关闭时，也不写 `session_end`。
  - 真正结束时写一条 `session_end`：正常断开、保留期过期、本机断开、作废、同设备新会话顶掉。`duration_s` 从会话开始算到最后一个连接离开，不含保留期。`ts` 为写入时刻。
- `role_change`：控制者和观看者互换时，每个角色变了的参与者各一条。`detail = {from, to}`，取值 `controller` / `viewer`；`duration_s` 为 null。
- `file`：每个传完的文件一条。`detail = {op, files, bytes}`：`op` 为 `"upload"`（手机传到电脑）或 `"download"`（电脑传到手机），`files` 固定为 1，`bytes` 为字节数；`duration_s` 为传输秒数。不记文件名、路径和内容；中断、取消的传输不记。
- `recording`（M3 V5）：App 在会话中本地录制（`RecordingNotice`，[03 · 控制面消息](03-protocol.md#3-控制面消息)），一次录制一条，在停止或会话结束时写；`duration_s` 为录制时长，`detail = {role}`，`role` 取录制时的 `controller` / `viewer`。连接断开后恢复时，一次录制可能被拆成多条。
- Host 在会话进行中或保留期内退出时，下次启动补写这次会话的 `session_end`：保留期内退出的按离开时刻算时长，进行中退出的按最后一次存活记录算（每 60 秒记一次，最多少算约 1 分钟）；`ts` 为补写时刻，`detail` 多 `recovered: true` 和 `ended_at`（会话实际结束时刻，unix 微秒：保留期内退出的为离开时刻，进行中退出的为最后一次存活记录时刻；旧版 Host 补写的记录没有这个字段，按 `ts` 显示）。断电等未落盘的情况仍可能缺失。

配对与信任事件（`event` 字段）：

| event | 时机 | detail |
|---|---|---|
| `pairing.password` | 账户密码配对成功 | `{"result":"ok","account":"<短名>"}` |
| `pairing.denied` | 已有；密码 / 文件配对失败也用它 | `{"method":"password"\|"file","status":<PairResult.status>}`，**不带账户名** |
| `pairing.file` | 配对文件配对成功 | `{"file_id":"…"}`，不带口令 |
| `trust.approved` | 应用 APPROVE_DEVICE 成功 | `{"seq":n,"signer_device_id":"…"}`，`device_id` 列 = 新设备 |
| `trust.rejected` | 信任日志某条被拒（含 SEC-10） | `{"seq":n,"kind":"approve"\|…,"reason":"<RejectReason 的 snake_case>"}`，同时 `warn!(event = "trust.rejected", …)` |

### 文件收发与配置

文件通道的头、大小上限、续传和错误见 [03 §8.2](03-protocol.md#82-文件0x04m2)。Host 引擎以 64 KiB 缓冲读写；接收参数为 `ReceiveConfig { dest_dir: PathBuf, max_file_bytes: u64, buf_size: usize }`，默认空目录、1 TiB、64 KiB，调用方必须指定已存在的目标目录。先写 `.rs-<id>.part` 和 `.rs-<id>.json`，中断保留，取消删除，整文件校验通过才改为最终名称；发送端校验源文件是否变化。

`[files]` 配置契约（业务接线由文件任务完成）：

| 键 | 默认 | 含义 |
|---|---|---|
| `enabled` | true | 总开关 |
| `from_phone` | true | 允许手机发文件到电脑 |
| `to_phone` | true | 允许手机从电脑取文件（`FileRequest`） |
| `browse` | true | 允许 `FileList` |
| `dest_dir` | "" | `HostServices.downloads_dir`，Mac `~/Downloads`，Windows 下载 Known Folder |
| `browse_roots` | [] | 浏览根目录；为空或所有配置根均无法规范化时，`BrowsePolicy` 回退到用户主目录（`HOME`） |
| `max_file_mb` | 0 | 不另设上限，仍受 `MAX_FILE_BYTES` 1 TiB 限制 |
| `partial_max_age_days` | 7 | 启动时清理更旧的 `.rs-*.part` |

`~` 开头的路径按用户主目录展开（`expand_home`）。默认禁区是 `~/Library`、`~/.ssh`、`~/.gnupg`、`~/.Trash` 和 `state_dir`；即使把浏览根设为主目录仍然生效，软链解析后也要校验浏览根与禁区。macOS 引导时不预先申请下载、桌面、文稿的 TCC 权限；浏览到时再由系统弹框。接收校验并原子改名后调用 `Platform::on_file_received(path, device_name)`，macOS 标记 quarantine，其他平台默认不处理。

Windows 服务以 SYSTEM 运行时不使用 SYSTEM 的个人目录：`WinUserDirs` 取活动控制台会话的用户令牌，用 `SHGetKnownFolderPath` 解析该用户的 Downloads 和 Profile，并规范化路径。可写性不是以 SYSTEM 身份试写，而是把该用户令牌复制成识别级模拟令牌（不做线程模拟），对目录的安全描述符 DACL 执行 `AccessCheck`，请求 `FILE_ADD_FILE`（`crates/host-win/src/user_dirs.rs:145-163、428-509`）。文件接收的默认目录是该用户的「下载」；没有活动控制台用户、Known Folder 解析失败、路径被禁用或不可写时，回退到 `data_dir\received`。`pairing.exportFile` 未指定 `dir` 时同样使用该用户的「下载」，解析不到用户目录则写 `state_dir\exports`；显式目录也先规范化、排除禁区，再用同一模拟令牌对 DACL 做 `AccessCheck`（`crates/host-win/src/user_dirs.rs:220-235`）。

Host 主动推文件为 **M3**；M2 由 App 发 `FileRequest` 拉取。FILES 不写会话开始 / 结束审计；文件审计由 `audit_transfer` 按上文[审计事件](#审计事件)的口径产生，不记录文件名。

### 监听与来源过滤

| 监听 | 地址 | 默认 | 说明 |
|---|---|---|---|
| tailnet | 本机所有 100.64.0.0/10 和 fd7a:115c:a1e0::/48 地址 | 开 | 地址变化（tailscaled 重启）时重新绑定 |
| 内网 / 局域网 | 设置里选中的网卡地址 | 关 | 打开后只接受来源在白名单网段内的连接（设计文档 §4.8） |

- QUIC：quinn 0.11 的 `Endpoint::accept()` 先拿到 `Incoming`，检查 `remote_address()`：来源不在白名单就 `incoming.ignore()`（不回任何包，不暴露这是 Remote Host）。
- TCP：accept 后检查来源，不在白名单立即关闭，不发 TLS ServerHello。
- 未配对设备经内网来：TLS 完成后在 `ClientHello` 阶段拒绝；「允许新设备经内网配对」默认关。
- mDNS：只在设置里标为「家」的网络上广播 `_rsp._udp`，TXT 记录只有 `id=<host_id>`，不含主机名。

### 配对

6 位码 / 二维码流程见 [04 · 配对](04-identity-security.md#配对)；M2 的三种方式见 [04 · 其他配对方式](04-identity-security.md#其他配对方式m2)。Host 侧要点：

| 项 | 规则 |
|---|---|
| 功能位 | 认识 `PairPassword` 宣告 `PAIR_PASSWORD`，认识 `pair_file_id` 宣告 `PAIR_FILE`（03 §11）；配对连接的 `ServerHello` 也填 `features` |
| 全局限额 | 所有方式共用「5 分钟 20 次」 |
| 密码锁 | `pwpair.rs`；连续失败 5 次锁 15 分钟，整机一把锁；状态存 `password_lock.json`（0600），读坏了按「锁 15 分钟」 |
| 密码处理 | 收到后立刻移进 `Zeroizing<Vec<u8>>`，校验后清零；失败回应至少 1 s；失败日志和审计不带账户名 |
| 配对文件 | `pairfile_store.rs` 管 `pairfiles.json`（0600，原子替换）；最多 50 个 active；导出时 Host 自己写文件（缺省：Mac `~/Downloads`，其他平台 `<state_dir>/exports`），文件 0600 |
| 成功通知 | 任何方式配对成功都发 `event.pairingResult`（带 `method`），host-ui 弹系统通知；应用 APPROVE 成功也发（`method = "approve"`） |

## macOS

### 包结构与进程

```
Remote Host.app/
├─ Contents/MacOS/Remote Host                     # Tauri 界面（菜单栏）
├─ Contents/MacOS/rs-host-agent                   # 常驻进程（Rust）
├─ Contents/Library/LaunchAgents/
│     ai.gxlself.remotehost.agent.plist            # 由 SMAppService.agent(plistName:) 注册
└─ Contents/Resources/…
```

- 界面首次启动时注册 LaunchAgent（`SMAppService`，macOS 13+）。agent 在用户登录后由 launchd 启动，崩溃自动拉起（`KeepAlive`）。
- 屏幕录制、辅助功能权限由 agent 申请，系统设置里显示为「Remote Host」（D11）。
- 界面退出不影响 agent；agent 需要弹窗时如果界面没在运行，用 `open -a "Remote Host" --args --prompt` 拉起。
- Bundle ID 沿用 M0 的前缀 `ai.gxlself`：界面 `ai.gxlself.remotehost`，agent `ai.gxlself.remotehost.agent`。

### 采集与编码

沿用 M0 `shim.m`，扩展：

| 项 | 做法 |
|---|---|
| 虚拟显示器 | 私有 `CGVirtualDisplay`：<br>- 运行时用 `NSClassFromString` 探测，不直接引用类符号；<br>- 所有调用在一个串行队列上；<br>- 2 倍时 `hiDPI = 1`，模式按点给；<br>- 建好后用 `CGConfigureDisplayOrigin` 放到最右侧、与主屏上沿对齐；<br>- 进程退出时由 WindowServer 回收，所以没有残留；<br>- 屏幕录制权限照旧，新屏不另要权限（`vdisplay.m`） |
| 多显示器 | `SCShareableContent` 列出所有 `SCDisplay`，每块一个 `SCStream`；按需启动（有人看哪块才采哪块） |
| 声音 | 单独一路只要声音的 `SCStream`，`capturesAudio = YES`、`excludesCurrentProcessAudio = YES`、48 kHz、双声道 |
| 参数 | `pixelFormat = NV12`、`showsCursor = false`、`queueDepth = 3`、`minimumFrameInterval = 1/fps` |
| 缩放 | `SCStreamConfiguration.width/height` 直接输出目标分辨率，不另做缩放 |
| 编码 | VideoToolbox HEVC，`RealTime`、`AllowFrameReordering = false`、低延迟码控（`kVTVideoEncoderSpecification_EnableLowLatencyRateControl`），M0 已验证 |
| 码率调整 | 运行中改 `AverageBitRate` 和 `DataRateLimits`（1 秒窗口 1.5 倍），不重建会话 |
| 分辨率 / 帧率变化 | 重建 `VTCompressionSession` 和 SCK 配置（`updateConfiguration`），epoch + 1 |
| H.264 兜底 | 设备不支持 HEVC 硬编时（老 Intel Mac） |
| 静止补帧 | 保留最后一个 `CVPixelBuffer`；300 ms 无新帧时临时把 `Quality` 调到 0.9 再编一次 |
| 视口裁剪 | `capture_region` 用 `SCStreamConfiguration.sourceRect`（点 = 像素 / scale），每次换裁剪重建 SCStream；VideoToolbox 无 ROI |
| 权限每月确认 | macOS 15+ 采集权限会定期弹确认；申请 `com.apple.developer.persistent-content-capture` 授权（需要向 Apple 申请），拿到之前在菜单里提示「需要在 Mac 上点一次允许」 |

### 输入注入

| 事件 | 做法 |
|---|---|
| 鼠标 | `CGEventCreateMouseEvent` + `CGEventPost(kCGHIDEventTap)`；坐标 = 显示器 `bounds` 内的点；按下前先移动（M0 已实现）；`kCGMouseEventClickState` 设 clicks |
| 滚动 | `CGEventCreateScrollWheelEvent2`，按 ScrollGesture 的 Idle / Scrolling / Momentum 状态设置 phase；补齐丢失的 Begin / End，300 ms 无事件自动用零位移 End 收尾；惯性由客户端提供，ReleaseAll 清空阶段和小数累积（03 §6.1） |
| 按键 | HID → macOS 虚拟键码；修饰键通过 `FlagsChanged` 同步，普通键附加隐含 flags：方向键 NumericPad + SecondaryFn，功能 / 导航键 SecondaryFn，数字小键盘 NumericPad；Backspace `0x2A → 0x33`，ForwardDelete `0x4C → 0x75` |
| 键盘类型 | 客户端布局不进协议；Host 用 `KBGetLayoutType(LMGetKbdType())` 检测 ANSI / ISO / JIS，未知时 ANSI，按自身类型映射并设置事件 keyboard type；ISO 做位置互换 |
| 文本 | `CGEventKeyboardSetUnicodeString`，每段 ≤ 20 个 UTF-16 单元、不拆代理对，keyDown/keyUp 的 vk=0、flags=0；CRLF / CR / LF / U+2028 / U+2029 转 Return，Tab 转 Tab，其余 C0 / DEL / C1 丢弃。当前单个超长字素簇可突破 20 单元（实现待跟进） |
| ReleaseAll | 松开已记录普通键和左右修饰键、结束滚动及惯性、释放五个鼠标键；断线 / 换控制者 / 切后台时必须执行 |
| 系统组合键 | 调度中心默认 ⌃↑；地球键 / Fn 目前 Key 无法表达，M2 再议；完整输入语义见 03 §6.1 |
| 锁屏 | 注入 ⌃⌘Q |
| 标记 | 所有注入事件设置 `kCGEventSourceUserData = 0x5253`（"RS"），用来区分本机操作 |

### 本机操作检测

`CGEventTapCreate(kCGSessionEventTap, kCGTapOptionListenOnly, 鼠标和键盘事件)`，`kCGEventSourceUserData != 0x5253` 的事件算本机操作。需要辅助功能权限（已有）。

### 光标

- 形状：每 33 ms 读 `NSCursor.currentSystem` 的图片，按 PNG 字节的 xxh3 判断是否变化；变化才发。
- 位置：`CGEventGetLocation(CGEventCreate(NULL))`，变化时经指针数据报上报（03 §6.2）。

### 其他

| 项 | 做法 |
|---|---|
| 剪贴板 | `NSPasteboard.general.changeCount` 每 300 ms 轮询；只读写文本和 PNG |
| 防休眠 / 唤醒 | 会话期间 `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep)`；开始推流时 `IOPMAssertionDeclareUserActivity` 唤醒显示器（M0 已实现） |
| 控制台状态 | `CGSessionCopyCurrentDictionary()` 查询当前控制台。连接前为登录窗口 / 其他用户时 `AuthResult(DENIED, error.code=190)`；会话中途离开时停止采集并发含错误码 190 的 Goodbye，detail.console 为 `login_window` / `other_user`，不扩充 ServerHello；普通 Locked 仍允许，Unknown 暂允许并记日志。策略默认每 2 s 轮询，会话停止 / 告别和握手接线实现待跟进 |
| 系统账户校验 | `sysauth.rs` + `sysauth.m`：OpenDirectory 按短名或全名查记录（`ODRecord`），`verifyPassword` 校验；管理员 = `admin` 组成员，控制台用户取当前登录用户。不缓存、不记录密码；OpenDirectory 报错 → `Unavailable` |
| 系统屏幕共享兜底 | 检测 `launchctl print system/com.apple.screensharing` 是否启用；启用时 App 的「用系统屏幕共享登录」可用（设计文档 §5.3） |
| tailscaled | 向导里检测 Tailscale 的安装形态；无人值守的 Mac 建议改用系统服务版 tailscaled（开机即上线，登录窗口时 tailnet 仍在） |

## Windows

### 进程

```
RemoteHostService（SYSTEM，自动启动）
 ├─ 网络：QUIC / TCP 监听、鉴权、配对、会话管理、更新     ← host-core 全部在这里
 ├─ 管道 \\.\pipe\RemoteHost.session.<会话号>  ◄──►  rs-host-session.exe（SYSTEM 令牌，放在活动控制台会话里）
 │                                                     采集（DXGI）、编码（FFmpeg/MF）、注入、光标、剪贴板、本机操作检测
 └─ 管道 \\.\pipe\RemoteHost.ui  ◄──►  RemoteHost.exe（Tauri 托盘，每个登录用户一份，开机由 HKLM\…\Run 启动）
```

- 会话进程的启动：`WTSGetActiveConsoleSessionId` → 复制服务自己的令牌 → `SetTokenInformation(TokenSessionId)` → `CreateProcessAsUserW`，桌面 `winsta0\default`。控制台会话变化（登录、注销、切换用户、RDP 接管）时重建。
- 桌面切换：会话进程每帧前检查 `OpenInputDesktop`，切到 `Winlogon`（登录界面、UAC 安全桌面）时 `SetThreadDesktop` 并重建 DXGI 复制。因为是 SYSTEM 令牌，安全桌面也能采、能注入（设计文档 §05「UAC 弹窗和登录界面也能看见、能操作」）。
- 帧走管道：会话进程把编码后的码流和光标送给服务，服务发网络；输入反方向。1080p60 约 15 Mbps，管道带宽足够。
- 管道背压：服务收到的新编码帧塞不进 host-core 的通道时丢掉这一帧，把该路编码器标成「等关键帧」并只发一次 `Ctl::EncoderKeyframe`；之后丢弃非关键帧，直到有关键帧送达。
- 会话进程断开（崩溃、控制台会话变化重建）时，服务保留 host-core 一侧的采集、编码、声音登记，所有编码器标成「等关键帧」；新的会话进程连上后按原参数重新登记，并对每路编码器各请求一次关键帧，host-core 侧的通道不重建（`crates/host-win/src/proxy_media.rs`，W1D）。
- 剪贴板只在 `default` 桌面可用；在安全桌面时剪贴板功能置灰。

### 采集与编码

| 项 | 做法 |
|---|---|
| 采集 | DXGI Desktop Duplication，每个输出一个 `IDXGIOutputDuplication`；`AcquireNextFrame(16 ms)`；`DXGI_ERROR_ACCESS_LOST` 时（桌面切换、分辨率变化）重建 |
| 光标 | `GetFramePointerShape` 直接拿形状（单色 / 彩色 / 掩码三种格式都要转成 RGBA PNG），位置用 `PointerPosition` |
| 编码 | D12：FFmpeg `hevc_nvenc` / `hevc_amf` / `hevc_qsv`，`hw_frames_ctx` 用 D3D11，纹理直接喂；参数：无 B 帧、低延迟预设（NVENC `p1` + `ull`）、CBR、GOP 无限 + 按需关键帧 |
| 选择顺序 | 按 `IDXGIAdapter` 厂商 ID 选（NVIDIA 0x10DE、AMD 0x1002、Intel 0x8086）；失败依次试 Media Foundation 硬件 MFT、openh264 |
| 颜色转换 | BGRA → NV12 用 D3D11 Video Processor（GPU 上做） |
| 视口裁剪与 ROI | `capture_region` 不改采集；区域随 `CaptureStart` 传到会话进程，编码时 VideoProcessor 以区域为源矩形。QSV 用 `AV_FRAME_DATA_REGIONS_OF_INTEREST` 多区域；NVENC / AMF 第一版不支持 ROI |

### 输入

| 事件 | 做法 |
|---|---|
| 鼠标 | `SendInput` + `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`，坐标换算到虚拟桌面 0..65535 |
| 按键 | HID → 扫描码，`KEYEVENTF_SCANCODE`（扩展键加 `KEYEVENTF_EXTENDEDKEY`） |
| 文本 | `KEYEVENTF_UNICODE` |
| 触控（M2） | `InjectSyntheticPointerInput`（`PT_TOUCH`） |
| Ctrl+Alt+Del | `SendSAS(FALSE)`；安装器设置策略 `SoftwareSASGeneration`（允许服务生成，取值见「打包」） |
| 锁屏 | `LockWorkStation` |
| 标记 | `dwExtraInfo = 0x5253`；本机操作检测用低级钩子，`LLMHF_INJECTED` / `LLKHF_INJECTED` 且 `dwExtraInfo == 0x5253` 的忽略 |

### 虚拟显示器（M3）

- 驱动：`drivers/win-iddcx/`，UMDF 2 + IddCx，硬件 ID `Root\RsVirtualDisplay`。
  - 设备接口 GUID `{13037E20-EC6D-42CD-8293-42F53DA517A6}`，只允许 SYSTEM 和 Administrators 打开。
  - IOCTL 协议见 `drivers/win-iddcx/include/RsIddIoctl.h`，与 `crates/host-win/src/vdisplay_ioctl.rs` 逐字段对应：`GET_INFO` / `ADD` / `REMOVE` / `REMOVE_ALL` / `PING`。
- 驱动只造显示器：模式和 EDID 都由服务在 `ADD` 里给出，EDID 序列号 = 显示器编号，名字 = `RS Virtual NN`。
  - 交换链只取帧、立即释放，不处理画面内容。
  - 画面由会话进程照常用 DXGI Desktop Duplication 采。
- 租约：服务每 1 秒 `PING`；驱动 5 秒没收到任何 IOCTL 就移除全部虚拟屏。服务崩溃、停止、升级时虚拟屏会在 5 秒内消失；服务启动时再 `REMOVE_ALL` 一次。
- 认领：会话进程枚举显示器时，名字优先用 `DisplayConfigGetDeviceInfo` 取的 EDID 名（真实显示器也因此显示为 `DELL U2723QE` 这类名字，而不是「通用即插即用监视器」）。服务按名字 `RS Virtual NN` 认领。
- 位置用系统默认（通常在最右侧），本版不调整。
- 签名与安装见 `drivers/win-iddcx/README.md`。正式发布需要 attestation 签名（09）。

### 防火墙

用 `INetFwPolicy2`（COM）管理规则，规则名统一前缀 `Remote Host`：

| 规则名 | 协议 / 端口 | 本地地址 | 远程地址 | 配置文件 | 谁建、何时建 |
|---|---|---|---|---|---|
| `Remote Host (tailnet) UDP`、`Remote Host (tailnet) TCP` | UDP、TCP 各一条，入站，监听端口（默认 7400） | `100.64.0.0/10`、`fd7a:115c:a1e0::/48` | 同左 | 全部 | MSI 安装时建；服务每次启动检查，缺了或内容不对就补回 |
| `Remote Host (内网) UDP`、`Remote Host (内网) TCP` | 同上 | 选中网卡上的内网 / 局域网地址（不含 tailnet 地址） | 设置里的来源网段（`allowed_cidrs`，按网段截断后排序） | 仅「域」「专用」 | 服务按设置同步 |

- 四条规则的分组都是 `Remote Host`，程序限定为服务的 exe。
- 同步时机：服务启动时先补 tailnet 规则、再同步内网规则；网卡地址变化（地址监听）或内网设置变化时重新同步内网规则。同步先删后加，某条失败时继续处理其余规则并记下第一个错误。COM 调用是阻塞的，服务在 `spawn_blocking` 里调用（`crates/host-win/src/firewall.rs`）。
- 以下三种情况不建内网规则，已有的内网规则会被删掉：没选内网网卡（`disabled`）、没设来源网段（`no_sources`）、选中的网卡上没有可用地址（`no_local_addresses`）。
- 组策略锁定：`INetFwPolicy2::LocalPolicyModifyState` 不是 `NET_FW_MODIFY_STATE_OK` 就算锁定（`policy_locked`），这时本机加不了规则，只能交给 IT。
- `network.firewallStatus` 的取值：`enabled` = 「域」或「专用」配置文件的防火墙开着；`active` = 内网规则不需要（上面三种跳过情况）**或**每条期望的内网规则恰好存在一条且内容一致——跳过也算 active。`detail` 含：`profiles`（domain / private / public 是否开启）、`current_profiles`（当前网络属于哪些配置文件）、`policy_locked`、`tailnet`（`ok` / `missing`）、`intranet`（每条 `{name, state}`，state 为 `ok` / `missing` / `mismatch` / `duplicate`）、`skipped`、`error`、`rules`（期望规则的完整内容）。
- `network.itNote` 的文本固定为以下几行（`firewall_model::it_note`）：
  1. `Remote Host 使用 UDP 和 TCP <端口> 端口。`
  2. `请允许来源网段 <网段列表> 访问本机。`（没设来源网段时为 `还没有设置来源网段。`）
  3. `tailnet 地址：<地址，或「无」>；局域网地址：<地址（网卡名）、…，或「无」>。`
  4. 锁定时 `这台电脑的防火墙由组策略管理，本机程序加不了规则，请在组策略里添加下面的入站规则（仅「域」「专用」网络）：`；未锁定时 `如需手动添加，入站规则如下（仅「域」「专用」网络）：`
  5. 之后每条内网规则一行 `netsh advfirewall firewall add rule name="…" dir=in action=allow protocol=… localport=… localip=… remoteip=… profile=…`；不需要规则时改为一行 `（内网连接未打开或没有可用的网卡地址，暂不需要规则）`

内网规则在设置里打开时才添加。添加失败（组策略锁定）时，设置页显示「防火墙规则被策略拦住」并给出规则内容（画布 `WinNetwork`），同时提供「复制给 IT 的说明」（设计文档 §4.13）。

## Linux（M3）

### 进程
systemd **user** 单元 `remote-host.service`（`ExecStart=~/.local/lib/remote-host/current/usr/bin/rs-host --platform linux`，`WantedBy=graphical-session.target`），随图形会话起停；另有 `remote-host-notify.service` 跑 `remote-host-ctl notify`（桌面通知审批）。没有 system 服务，不支持登录界面。

### 采集与编码
| 项 | 做法 |
|---|---|
| Wayland 采集 | xdg-desktop-portal：一个 RemoteDesktop 会话里带 ScreenCast（`SelectDevices` 里 `persist_mode=2` + `restore_token`，token 存 `<state_dir>/portal-restore.json`）；`OpenPipeWireRemote` 的 fd 连 PipeWire，只协商 SHM 缓冲（BGRx / BGRA / RGBx / RGBA），每帧拷进帧池；RemoteDesktop 不可用（wlroots）时退回纯 ScreenCast |
| X11 采集 | XShm `GetImage`（memfd 段），XDamage 判脏；RandR `GetMonitors` 给显示器 |
| 光标 | portal `cursor_mode=METADATA`（`Cursor` + `Bitmap` 元数据），不支持时 EMBEDDED；X11 用 XFixes |
| 编码 | FFmpeg 8.1 LGPL 动态库：Intel / AMD → `hevc_vaapi`（`sws_scale` BGR0→NV12 后上传）；NVIDIA → `hevc_nvenc`（直接喂 BGR0）；兜底 openh264。参数同 Windows：无 B 帧、CBR、GOP 无限 + 按需关键帧 |
| 选择顺序 | 按 `/sys/class/drm/renderD*/device/vendor`（0x10DE / 0x8086 / 0x1002）选；环境变量 `RS_VIDEO_ENCODER=vaapi / nvenc / openh264` 可强制（与 Windows 同名） |

### 输入
| 事件 | 做法 |
|---|---|
| Wayland | portal RemoteDesktop：`NotifyKeyboardKeycode`（HID → evdev）、`NotifyKeyboardKeysym`（文本；当前布局里没有的字符会丢）、`NotifyPointerMotionAbsolute`（流坐标）、`NotifyPointerButton`、`NotifyPointerAxis`（平滑，取反）/ `NotifyPointerAxisDiscrete`（行） |
| X11 | XTest `FakeInput`，键码 = evdev + 8；文本按 keysym 找键码，找不到时临时改一个空闲键码的映射 |
| 本机操作检测 | X11：XI2 raw 事件，排除 XTEST 设备；Wayland：只看光标元数据位置是否偏离最后一次注入位置（检测不到键盘） |
| 锁屏 / 唤醒 | logind `Session.Lock`；唤醒 = 注入 0 位移移动（X11 另加 DPMS On）；Ctrl+Alt+Del 不支持 |

### 其他
- 主机密钥：`<state_dir>/keys/`，目录 0700、文件 0600。`state_dir` = `$XDG_STATE_HOME/remote-host`。
- 控制台状态：logind（本会话 `Active` / `LockedHint`，seat0 当前会话的 `Class` / 用户）。
- 剪贴板：X11 CLIPBOARD 选区（文本、PNG）；Wayland 下经 XWayland 的 CLIPBOARD，没有 XWayland 时不支持。
- 声音：PipeWire 默认输出设备 monitor，F32 48 kHz 双声道。
- 打包：AppImage，`remote-host-ctl install` 解包到 `~/.local/lib/remote-host/app-<版本>/`、`current` 软链指向它并写 systemd 单元；自动更新解包新版本到同级目录后原子切换 `current`，`systemctl --user --no-block restart remote-host.service`，自检失败切回旧目录。平台校验只查 AppImage 结构（ELF + type-2 魔数 + 架构），真实性靠 Ed25519 清单签名。下载用系统 `curl`。

## host-ui（Tauri 2）

界面只是 agent 的一个客户端，所有状态从 IPC 来。

| 窗口 / 元素 | 画板 | 说明 |
|---|---|---|
| 菜单栏面板（Mac） | `HostMenu`、`HostHandoff` | 在线状态、当前会话（控制中 / 观看中，可单独断开、改为仅观看）、开关、已配对设备、连接记录 |
| 托盘面板（Windows） | `WinTray` | 同上 |
| 「正在被控制」胶囊 | `HostMenu`、`HostUpdate` 顶部 | 置顶、透明、可拖动；显示控制者和时长，「断开」按钮；会话期间常驻 |
| 本机确认弹窗 | `HostConfirm`、`WinConfirm` | 30 秒倒计时；允许 / 拒绝；配对时多一个权限选择 |
| 配对二维码 | `HostPair` | 二维码 + 6 位数字 + 倒计时 |
| 已配对的设备 | `HostDevices` | 每台设备的权限三档、按路径权限、最后连接、作废 |
| 首次设置向导 | `HostSetup`（Mac）、`WinFirstRun`（Windows） | Mac：tailnet → 屏幕录制 → 辅助功能 → 配对；Windows：tailnet → 系统服务 → 内网连接 → 配对 |
| 网络与访问设置 | `WinNetwork` | 内网连接开关、网卡、来源网段、「我会从公司 VPN 连」、防火墙状态、复制给 IT |
| 软件更新 | `HostUpdate` | 版本、更新说明、「会话结束后安装」、频道、签名已校验 |
| 系统通知 | `HostHandoff` | 控制权交接、新设备配对成功（按 `event.pairingResult.method` 区分：配对码 / 配对文件 / 账户密码 / 已配对设备批准）、作废执行 |
| 设置 ›「配对方式」（M2） | — | 「允许用这台电脑的账户密码配对」开关与权限档；配对文件：导出（选权限、备注）、列表（状态、用过的设备）、撤销；导出后显示文件路径和「在 Finder 中显示」 |

菜单开关「新设备连接前先问我」对应 `[security] confirm_new_devices: bool`，默认 true（配置、IPC 和界面接线实现待跟进）；它不替代配对流程必须本机允许的要求。

`[security]` 配置（`settings.get/set` 用同名 JSON 键）：

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `security.confirm_new_devices` | bool | `true` | 新设备连接前先问我 |
| `security.password_pairing` | bool | `false` | 允许用这台电脑的系统账户密码配对（M2） |
| `security.password_pairing_permission` | `"view_only"` / `"control"` / `"unattended"` | `"control"` | 账户密码配对得到的权限（M2） |

host-ui 的 Host / 设备指纹显示统一引用 [04 · 标识](04-identity-security.md#标识)（显示层实现待跟进）。

### 本地 IPC（JSON-RPC 2.0，按行分隔）

Mac：Unix socket `~/Library/Application Support/Remote Host/agent.sock`（0600）。Windows：命名管道 `\\.\pipe\RemoteHost.ui`（ACL：SYSTEM + 交互用户）。

| 方法 | 参数 → 返回 |
|---|---|
| `status.get` | 无参数 → `{online: bool, tailnet_addresses: string[], version: string, permissions: object, listening: bool, sessions: SessionSummary[]}` |
| `sessions.list` | 无参数 → `{sessions: SessionSummary[]}` |
| `sessions.disconnect` / `sessions.setViewOnly` | `{session_id: string}` → `{}`；后者当前只支持改为观看 |
| `pairing.start` | 无参数 → `{qr_url: string, code: string, expires_at: u64}`（unix 微秒） |
| `pairing.cancel` | `{code?: string\|null}` → `{}` |
| `devices.list` | 无参数 → `{devices: DeviceSummary[]}` |
| `devices.update` | `{device_id: string, permission: Permission, paths: string[]}` → `{}` |
| `devices.revoke` | `{device_id: string}` → `{}` |
| `settings.get` / `settings.set` | 无参数 / 完整设置的部分更新 → 生效后的完整设置；新增 `[security] confirm_new_devices` 默认 true（实现待跟进） |
| `network.listNics` | 无参数 → `{nics: unknown[]}`，网卡元素尚无正式 TS 字段类型 |
| `network.firewallStatus` / `network.itNote` | 无参数 → `{enabled: bool, active: bool, detail: unknown}` / `{text: string}` |
| `approval.pending` | 无参数 → `{"requests": [<与 event.approvalRequested 的 params 相同的对象>]}`；没有待处理请求时返回 `{"requests": []}`。host-ui 被 `--prompt` 拉起或 IPC 重连后补拿，按 request_id 与实时通知合并；不重复处理已关闭请求（实现待跟进） |
| `approval.respond` | `{request_id: string, allow: bool, permission?: Permission\|null}` → `{}` |
| `update.status` | 无参数 → `{state: string, version: string\|null, notes: string\|null}` |
| `update.installNow` / `update.setChannel` | 无参数 / `{channel: "stable"\|"beta"}` → `{}` |
| `pairing.exportFile` | `{permission?: Permission, label?: string（≤ 64 字符）, dir?: string（绝对路径）}` → `{file_id: string, path: string, file_name: string, expires_at: u64}`；业务错误 `too_many_files` / `io` / `invalid_params`。Host 自己把文件写到 `dir`（缺省见「配对」），不经 IPC 返回文件内容和口令（M2） |
| `pairing.listFiles` | 无参数 → `{files: [{file_id, label: string\|null, permission, created_at: u64, expires_at: u64, state: "active"\|"used"\|"failed"\|"revoked"\|"expired", used_by_device_id: string\|null, used_at: u64\|null}]}`（M2） |
| `pairing.revokeFile` | `{file_id: string}` → `{}`；业务错误 `not_found`（M2） |
| `logs.export` | `{mask: bool}` → `{path: string}`（zip 路径） |
| `audit.list` | `{limit?: number\|null, before_us?: u64\|null}` → `{entries: unknown[]}` |

方法名表在 `host_core::ipc::METHOD_NAMES`；`crates/host-core/tests/ipc/` 里每个方法一对样例（`<method>.request.json` / `.response.json`），契约测试比较「样例方法集合 == 方法名表」，加方法时三处一起改。

FILES 连接不进 roster，不出现在 `sessions` 事件里，`ipc::SessionRole` 不用改。连接审批 `event.approvalRequested` 的 payload 追加 `"intent": "control" | "view" | "files"`，host-ui 据此显示「请求传文件」；现有 pairing / takeover 审批不要求该字段。

`Permission = "view_only" | "control" | "unattended"`；IPC 使用 snake_case。`SessionSummary` 的正式字段为 `{session_id: string, device_id: string, device_name: string, role: "viewer"|"controller", connected_at_us: u64, recording: bool}`（`recording`：M3 V5，该会话的 App 正在本地录制，来自 RecordingNotice；旧 agent 不发，读端缺省 false）；`DeviceSummary` 为 `{device_id: string, name: string, permission: Permission, paths: string[], last_connected_at_us: u64|null}`。所有 `_us` 时间为 unix 微秒。

host-ui 扩展以 `host-ui/src/ipc/types.ts` 当前已使用的线名和可空性为准；字段使用 snake_case，方法名保留上表已有拼写。上表的 `StatusGetResult`、`SessionSummary`、`DeviceSummary` 与动作参数已按该文件登记。以下能力是正式需求（实现待跟进），但本次核对的 types.ts 尚无对应扩展签名：

| IPC / 类型 | 要补的字段或动作 | 当前契约边界 |
|---|---|---|
| `status.get` | 主机名 | 当前只有 version / 地址，无主机名字段 |
| `SessionSummary` | 会话路径、延迟、显示器 | 当前只有身份、角色和连接时间，无上述字段 |
| `DeviceSummary` | 设备平台、完整指纹、配对时间 | 当前只有权限、路径和最后连接时间；确认请求中的 platform / device_fp 不能代替设备列表字段 |
| `sessions.disconnect` | 「断开并锁屏」 | 当前参数只有 session_id，没有锁屏选项 |
| `sessions.setViewOnly` | 观看改回控制 | 当前是单向降为观看；恢复控制必须走角色仲裁，不能绕过本机确认 |
| agent → host-ui | 作废事件及设备标识 | 当前只有作废方法，没有专用通知 |

上表的缺失项在 `types.ts` 里还没有类型定义，这里不预先约定字段名、可空性、动作参数或作废事件名；补类型时同步改 IPC 两端。当前 `sessions.disconnect` / `sessions.setViewOnly` 的参数只有非空 `session_id: string`，不能据此声称支持锁屏选项或恢复控制；`approval.pending` 的 `requests` 包装已经确定，需补入 types.ts（实现待跟进）。

agent → 界面的通知：`event.approvalRequested`、`event.approvalClosed`、`event.sessionsChanged`、`event.pairingResult`、`event.updateState`、`event.permissionsChanged`、`event.handoff`。

现有载荷：`event.sessionsChanged {sessions?: SessionSummary[]}`，`event.pairingResult {status: number, host_id: string, host_name: string, attempts_left: number, device_fp: string, method: "code"|"file"|"password"|"approve", device_name: string, account?: string}`（`method` / `device_name` 为 M2 追加，旧字段不变；`account` 只在 `method="password"` 且成功时带，值为短名），`event.updateState {state?: string}`，`event.permissionsChanged` 为权限 object，`event.handoff` 至少允许 `{session_id?: string}`；不能假设 handoff 已携带会话路径等扩展字段。

本机确认弹窗的两个通知：

| 通知 | 载荷 |
|---|---|
| `event.approvalRequested` | `{request_id, kind: "connection"\|"pairing"\|"takeover", device_name, device_id, platform, whois_login?, path?, reasons: [..], timeout_s: 30, permission_choice: bool, device_fp?}`；`permission_choice` 只在配对时为 true（本机选权限档） |
| `event.approvalClosed` | `{request_id, kind, outcome: "allowed"\|"denied"\|"timeout"\|"cancelled"}`；弹窗被回答、超时或对方断开时发，界面据此关掉对应弹窗 |

同一时间可以有多个弹窗；任意一个弹窗打开期间，所有远端输入一律不注入（09 SEC-13）。

## 自动更新

当前没有更新源：Host 由用户手动安装新版本（2026-10-24 移除 Hub，Hub 原是唯一的更新源）。本地更新流水线保留（下载与校验、平台签名校验、会话结束后安装、自检回滚），等以后接入新的更新源时使用；`update.channel` 配置项照常接受。

D13。agent 里的 `update.rs`：

```
来源待定（当前没有）：
  1 取到清单 {version, url, sha256, size, signature, notes}
  2 用编进 agent 的 Ed25519 公钥验清单签名；不通过 → 丢弃，写日志
  3 下载到临时目录，校验 sha256
  4 平台签名校验：
      Mac      解压前先查 zip 条目：只允许 `Remote Host.app/…` 和 ditto 生成的 `__MACOSX/Remote Host.app/…` 元数据；
               拒绝 `..`、绝对路径、其他顶层目录、指出 bundle 的符号链接；用 ditto 解到 0700 的临时目录后再复查一遍符号链接
               SecStaticCodeCheckValidity（严格校验 + 嵌套代码 + RestrictSymlinks + RestrictToAppLike），要求：
               标识符与当前 App 相同、Developer ID 证书链（Developer ID CA + Developer ID Application 叶子证书）、Team ID 与当前 App 相同；
               开发证书或 ad-hoc 签名的包一律拒绝，当前 App 本身没有 Team ID 时不启用自动更新
      Windows  WinVerifyTrust，要求签名证书主题与当前相同
  5 有活动会话、等待注册的连接或 60 秒保留期内的 lingering 会话 → 状态 waiting_session_end（画布 HostUpdate 的 waiting）；只有 `active_sessions() == 0` 才继续。保留期内的 lingering 仍算未结束，过期条目不再阻塞（名册定时器可稍后清理）
     「现在安装」→ 先 Goodbye(HOST_UPDATING) 断开所有会话
  6 安装：
      Mac      解开新的 Remote Host.app 到临时位置 → 原子替换 /Applications/Remote Host.app → launchctl kickstart 重启 agent
      Windows  服务以 SYSTEM 运行 msiexec /i new.msi /qn，MSI 负责停服务、替换、启动
  7 新版本启动后 60 秒内自检（监听正常、能采集一帧）
     自检通过 → 本地 update.state=ready
     失败 → 回滚到备份的旧版本，本地 update.state=rolled_back，并写回滚审计
```

更新状态为 `idle | waiting_session_end | downloading | verifying | installing | restarting | ready | rolled_back | failed`。`ready` 专指新版本启动并自检通过；本地安装进行中不得上报 `ready`，本地 `update.status` 现有 `ready` 是安装前准备完成，两者不可混用。

频道：稳定版 / 测试版；默认稳定版自动安装，可改为「只提醒」。

## 打包

| 平台 | 产物 | 要点 |
|---|---|---|
| Mac | `RemoteHost-<版本>.dmg`（universal） | Developer ID 签名；Hardened Runtime；entitlements：网络服务端 / 客户端、`com.apple.security.automation.apple-events`（锁屏注入不需要，按需）；`notarytool` 公证 + `stapler`；卸载用 `tools/release/mac/uninstall.sh`：注销 LaunchAgent 后删 app、数据和钥匙串 |
| Windows | `RemoteHostSetup-<版本>.exe`（WiX Burn 引导器）+ `RemoteHost-<版本>.msi` | EV 代码签名；MSI 安装服务、托盘自启、tailnet 防火墙规则、`SoftwareSASGeneration` 策略；支持 `msiexec /qn PAIRFILE=<路径>` 静默部署（设计文档 §5.1）；卸载时删服务、规则、配对表 |

### Windows 安装包要点

- **静默部署参数**（MSI 公共属性，Setup.exe 原样透传）：`PAIRFILE`（必须是绝对路径）、`PAIRFILE_PERMISSION`（`view_only` / `control` / `unattended`；给了 `PAIRFILE` 没给它时默认 `control`）、`PAIRFILE_LABEL`（备注，可空）。MSI 只校验绝对路径和权限枚举，然后把三个值写进 `HKLM\SOFTWARE\Remote Host\Deploy`（64 位视图，值名 `PairFile`、`PairFilePermission`、`PairFileLabel`），**不读、不校验配对文件本身**；由服务消费。
- **批量导出**：服务读取上述注册表值和键的最后写入时间。`PAIRFILE`（大小写不敏感）以 `.rspair` 结尾时按文件处理，使用其父目录并保留文件名；否则按目录处理，由服务生成文件名。指纹包含三个注册表值和键写入时间，写入 `deploy_pairfile.json`；相同指纹只导出一次，任一值或写入时间变化才再次导出。导出文件 ACL 只授予 SYSTEM 和 Administrators；配对文件有效期为 24 小时。
- **卸载**：删服务、全部防火墙规则（两条 tailnet 规则随 MSI 组件删除；分组 `Remote Host` 里其余规则即内网规则，由卸载时的清理动作 `RsCleanupFirewall` 删除，见 `RemoteHost.wxs:253-266`）、`paired.json`、`pairfiles.json`、`password_lock.json`；作废记录、密钥、配置、日志默认保留，`REMOVEDATA=1` 时整个数据目录一起删。大版本升级不删数据、不要求重启，允许装旧版本用于回滚。
- **`SoftwareSASGeneration`**：原来没有或为 0 → 设为 1；原来为 2 → 设为 3；为 1 或 3 不动。写入 1 和写入 3 的两个 MSI 组件都是 `Permanent`，**卸载时不还原**。
- **UpgradeCode**：`tools/release/win/Product.wxi` 里的 `UpgradeCode`（MSI）和 `BundleUpgradeCode`（Setup.exe）永远不改，改了就无法原地升级。（文档里不抄 GUID 值。）
- **WebView2**：Setup.exe 先装 WebView2 运行时（已装则跳过），该包 `Vital="no"`，装失败不阻断后面的 MSI；MSI 包 `Vital="yes"`。运行时是 `Permanent`，卸载 Remote Host 不会卸载它。
- **版本号**：MSI 和 `RemoteHostSetup-<版本>.exe` 的版本取根 `Cargo.toml`（`tools/release/win.ps1`）；`host-ui/src-tauri/tauri.conf.json` 的 `version` 不参与 Windows 打包。托盘 `status.get.version` 来自 `rs-host-agent` 的 `env!("CARGO_PKG_VERSION")`，该包继承 workspace 版本，因此以 MSI 版本为准且同源。

## 日志与崩溃

- 日志：`tracing` 按天滚动，保留 14 天；导出时 IP、主机名、用户名替换为 `10.30.•.•` 这类掩码（画布 `Diagnostics` 的写法）。
- 崩溃：Mac 用 panic hook + 系统崩溃报告（`~/Library/Logs/DiagnosticReports/rs-host-agent*.ips`）；Windows 用 `SetUnhandledExceptionFilter` + `MiniDumpWriteDump`。崩溃文件只留在本机，不上传。

## agent 命令行

```
rs-host-agent [--config <路径>] [--platform mac|win|fake] [--bind <ip>]... [--view-only]
              [--log <级别>] [--print-pair-code] [--dump <文件.h265>]
              [--fail <错误码>]          # 只在 --platform fake 下可用：每次鉴权都返回这个错误码，给 App 端到端测试用
```

`--notify-uninstall` 是旧版本留下的参数：现在接受后直接退出，不出现在帮助里，有一条测试，免得旧的卸载脚本报错。

`--platform fake` 可以在 Linux 上跑完整的协议栈（CI 集成测试用）。`--print-pair-code` 在终端打印配对码，没有界面也能配对。
