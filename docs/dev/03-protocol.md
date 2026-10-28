# 03 · RSP/1 协议规范

RSP（Remote Screen Protocol）是 App 和被控端之间唯一的协议。本文是实现依据：字段、字节序、时序、定时器都以这里为准。设计背景见设计文档 §06。

- 字节序：大端。
- 时间：unix 纪元微秒 `u64`，各自用本机时钟，靠 [时钟同步](#9-时钟同步) 换算。
- 坐标：相对于被推流的那块显示器，归一化到 0..1，原点左上。
- 版本：本文描述的是 **RSP/1 最终版**，在 0.3.0 冻结（D19）。0.2.x 期间可以不兼容地修改。

## 1. 传输

| 项 | QUIC（主路） | TCP（回退） |
|---|---|---|
| 端口 | UDP 7400 | TCP 7400 |
| 安全 | TLS 1.3，双向证书 | TLS 1.3，双向证书 |
| ALPN | `rsp/1` | `rsp/1` |
| 实现 | quinn 0.11 + rustls 0.23（ring） | tokio TcpStream + tokio-rustls |
| 多路复用 | QUIC 流 + DATAGRAM | 自定义记录格式，见 [1.3](#13-tcp-记录格式) |

### 1.1 TLS

- 只允许 TLS 1.3，套件用 rustls 默认（AES-128-GCM、ChaCha20-Poly1305）。
- **服务端证书**：Host 首次启动生成 P-256 密钥和自签证书。Subject CN 用随机串 `h-<8位随机>`，SAN 为 `rs-host`，不暴露主机名和产品名。有效期 20 年，密钥不变则指纹不变。
- **客户端证书**：App 的设备证书（D7），必填。Host 用自定义 `ClientCertVerifier`：接受自签证书，照常校验 TLS 的 CertificateVerify 签名，然后用 SPKI 指纹查配对表，见 [04](04-identity-security.md#连接时的鉴权)。
- **指纹**：`SHA-256(SubjectPublicKeyInfo DER)`，32 字节。App、host-ui 统一取前 10 字节，用 Crockford base32（字母表 `0123456789ABCDEFGHJKMNPQRSTVWXYZ`）编码为 16 个大写字符，每 4 个用空格分组，例如 `7K2M 9QXA 4D1F 8ZPW`。显示格式不改变完整指纹的校验和线上编码（界面统一实现待跟进）。
- 客户端不校验证书链和主机名，只比对指纹：已配对用保存的指纹，扫码配对用二维码里的指纹，6 位码配对见 [04](04-identity-security.md#配对)。

### 1.2 QUIC 参数

| 参数 | 值 | 说明 |
|---|---|---|
| `max_idle_timeout` | 10 s | 与 M0 一致 |
| `keep_alive_interval` | 会话中 2 s；预热连接 15 s | 公司防火墙 30–60 秒回收 UDP 映射（设计文档 §4.9） |
| `initial_mtu` | 1200 | |
| MTU 探测 | 开，上限 1452 | tailnet 接口 MTU 是 1280，实际会停在约 1252 |
| DATAGRAM 收发缓冲 | 收 4 MB，发 8 MB | 与 M0 一致 |
| 单向流上限 | 64 | 剪贴板、文件每项一条流 |
| 拥塞控制 | Cubic，初始窗口 240 000 字节；丢包判定 `packet_threshold` 128、`time_threshold` 2.0 | 两端（rsp-transport、rsp-client）一致。初始窗口让起播关键帧加 200 ms 媒体不必等 ACK 爬坡；放宽的丢包判定避免抖动重排被当成拥塞而压窗，真实丢包仍由 Cubic 退避。媒体窗口下限 84 KiB（`MEDIA_WINDOW`）：普通丢包退避后 cwnd 不低于 84 KiB，保证一个关键帧（约 63.5 KB）加 25% FEC 和包头能在一个 RTT 内发出；也低于实测 4 Mbps 链路约 100 KB 的 200 ms 排队预算。出现 persistent congestion 时不套这个下限，允许退到 QUIC 最小窗口（约 2×MTU），直到 Cubic 自己回升到 84 KiB 以上（GF35，`rsp-transport::quic::MediaCongestion`，两端一致）。应用层另有码率控制（§7）。真实网络效果待 M3 实测（RS-GF23） |

### 1.3 TCP 记录格式

TCP 回退时，一条 TLS 连接上传所有通道，每条记录：

```
channel u8 | length u32 | payload[length]
```

| channel | 对应 | payload |
|---|---|---|
| 0x00 | control | 一个 `Envelope`（不再带自己的长度前缀） |
| 0x02 | cursor | 一个 `CursorMsg` |
| 0x03 | clipboard | `item_id u32 | flags u8（bit0 = 最后一块） | 数据块` |
| 0x04 | file | `slot u32 BE | flags u8 | data`，data ≤ 16 KiB（`FILE_TCP_CHUNK`），见下文 |
| 0x10 | video | 视频头（见 §5，`data_shards=1`、`parity_shards=0`、`shard_len=0`）+ 整帧数据 |
| 0x11 | audio | 同音频数据报 |
| 0x12 | pointer | 同指针数据报 |

0x04 沿用 `channel u8 | len u32 | payload` 外层记录。`payload = slot u32 BE | flags u8 | data`：

- bit0 FIN：本 slot 正常结束，data 为空。
- bit1 RESET：发送方放弃本 slot，data 为 u32 BE reset 码（§8.2）。
- bit2 STOP：接收方让对端停发，data 为 u32 BE stop 码。其余位必须为 0，否则 Protocol 错误。
- slot 由打开方从 1 开始自增，两个方向各自计数。没有 STOP 位时指发送方自己打开的流；有 STOP 位时指收到方打开的流。
- `UniSend::reset(code)` 发 RESET；`UniRecv::stop(code)` 发 STOP 后丢弃后续数据。发送方收到 STOP 后，后续 `write_raw` 返回 `ConnError::Closed`（或已有的 stopped 错误），不再发数据。
- 文件数据块小，避免挡住控制记录；M2 不做优先级队列。TCP 实现由文件回退任务补齐。

**输入单独一条 TCP 连接**（设计文档 §4.9：鼠标不被视频拖慢）：同样 TLS + 双向证书，第一条记录是 channel 0x00 的 `AttachInput { session_token }`，之后每条记录 channel 0x01、payload 是一个输入事件。

TCP 模式的发送队列：

- 视频只保留「最新一帧」。写不出去时丢弃排队中的旧帧，并让编码器下一帧出关键帧（P 帧有依赖，丢了旧帧后面的就解不了）。强制关键帧最多每 500 ms 一次。
- 码率上限 6 Mbps，起步 4 Mbps。
- 每 30 秒在后台试一次 QUIC，通了就用会话令牌切过去（[§10](#10-多路径与回退)）。

TCP 接收侧使用有界数据报队列，队列满时丢弃并计数，不阻塞控制记录。

## 2. 通道

| 通道 | QUIC 载体 | 方向 | 可靠性 | 首字节 |
|---|---|---|---|---|
| control | 客户端打开的第一个双向流 | 双向 | 可靠有序 | 无（直接是长度前缀的 Envelope） |
| input | 单向流 | App → Host | 可靠有序 | `0x01` |
| cursor | 单向流 | Host → App | 可靠有序 | `0x02` |
| clipboard | 单向流，每项一条 | 双向 | 可靠 | `0x03` |
| file | 单向流，每个文件一条（M2） | 双向 | 可靠、可续传 | `0x04` |
| video | DATAGRAM | Host → App | 不可靠 + FEC | `0x10` |
| audio | DATAGRAM（M2） | Host → App | 不可靠 | `0x11` |
| pointer | DATAGRAM | 双向 | 只保留最新 | `0x12` |
| probe | DATAGRAM（诊断） | 双向 | 不可靠 | `0x13` |

单向流打开后先写 1 字节类型，之后按各通道定义发送：input / cursor 是「`u32` 长度 + 内容」的记录序列，clipboard / file 是带长度的头部后跟原始数据（§8）。收到不认识的流类型：直接 `stop_sending`，不断开连接。收到不认识的数据报类型：丢弃并计数。

## 3. 控制面消息

文件：`proto/rsp/v1/control.proto`。控制流上每条消息是 `u32 长度 + Envelope`，长度上限 1 MB（M0 的 `MAX_FRAME_LEN`）。

```proto
syntax = "proto3";
package rsp.v1;

message Envelope {
  oneof msg {
    ClientHello      client_hello      = 1;
    ServerHello      server_hello      = 2;
    AuthResult       auth_result       = 3;
    SessionState     session_state     = 4;
    StartStream      start_stream      = 5;
    StreamConfig     stream_config     = 6;
    KeyframeRequest  keyframe_request  = 7;
    Feedback         feedback          = 8;
    RateUpdate       rate_update       = 9;
    Ping             ping              = 10;
    Pong             pong              = 11;
    DisplayList      display_list      = 12;
    SelectDisplay    select_display    = 13;
    RequestControl   request_control   = 14;
    ControlRequest   control_request   = 15;
    ControlResponse  control_response  = 16;
    LocalActivity    local_activity    = 17;
    IdleWarning      idle_warning      = 18;
    PauseStream      pause_stream      = 19;
    ResumeStream     resume_stream     = 20;
    HostStats        host_stats        = 21;
    SessionAction    session_action    = 22;
    SetQuality       set_quality       = 23;
    AttachInput      attach_input      = 24;
    SetAudio         set_audio         = 25;
    Viewport         viewport          = 26;   // App → Host，M3 视口感知编码（03 §5.5）
    ViewportAck      viewport_ack      = 27;   // Host → App
    PairStart        pair_start        = 30;
    PairSpake        pair_spake        = 31;
    PairConfirm      pair_confirm      = 32;
    PairResult       pair_result       = 33;
    PairPassword     pair_password     = 34;   // 35–39 留给配对以后用
    ErrorInfo        error             = 40;
    Goodbye          goodbye           = 41;
    FileList     file_list     = 50;
    FileListing  file_listing  = 51;
    FileRequest  file_request  = 52;
    FileQuery    file_query    = 53;
    FileStatus   file_status   = 54;
    FileCancel   file_cancel   = 55;
    RecordingNotice  recording_notice  = 56;   // App → Host（M3 V5）
    CreateVirtualDisplay  create_virtual_display  = 60;
    VirtualDisplayResult  virtual_display_result  = 61;
    ReleaseVirtualDisplay release_virtual_display = 62;
  }
}

enum Platform  { PLATFORM_UNSPECIFIED = 0; IOS = 1; IPADOS = 2; ANDROID = 3; MACOS = 4; WINDOWS = 5; LINUX = 6; }
enum Intent    { INTENT_UNSPECIFIED = 0; CONTROL = 1; VIEW = 2; PAIR = 3; PROBE = 4; FILES = 5; }
enum PathKind  { PATH_UNSPECIFIED = 0; TAILNET_DIRECT = 1; TAILNET_DERP = 2; INTRANET = 3; LAN = 4; MANUAL = 5; }
enum Transport { TRANSPORT_UNSPECIFIED = 0; QUIC = 1; TCP = 2; }
enum Codec     { CODEC_UNSPECIFIED = 0; HEVC = 1; H264 = 2; }
enum Role      { ROLE_UNSPECIFIED = 0; CONTROLLER = 1; VIEWER = 2; }
enum Quality   { QUALITY_AUTO = 0; QUALITY_CLEAR = 1; QUALITY_SMOOTH = 2; QUALITY_ECO = 3; }

// ---------- 握手 ----------
message ClientHello {
  uint32    protocol          = 1;   // 固定 1
  string    app_version       = 2;   // "0.3.1"
  uint64    features          = 3;   // 支持的功能位，见 §11
  uint64    required_features = 4;   // 要求对方必须支持的功能位
  string    device_name       = 5;   // "小明的 iPhone"
  Platform  platform          = 6;
  Intent    intent            = 7;
  bytes     resume_token      = 8;   // 16 字节；空 = 新会话
  ClientCaps caps             = 9;
  PathKind  path              = 10;  // App 拨号时认为的路径
  Transport transport         = 11;
}
message ClientCaps {
  repeated Codec decode   = 1;       // 按偏好排序
  uint32 max_width        = 2;
  uint32 max_height       = 3;
  uint32 max_fps          = 4;       // iPad Pro 120
  uint32 screen_width_px  = 5;       // 手机屏幕物理像素，Host 用来选默认分辨率
  uint32 screen_height_px = 6;
  bool   audio_opus       = 7;
}
message ServerHello {
  uint32   protocol          = 1;
  string   host_version      = 2;    // "0.3.2"
  uint64   features          = 3;
  uint64   required_features = 4;
  string   host_id           = 5;    // 见 04 · 标识
  string   host_name         = 6;    // "Studio Mac mini"
  Platform os                = 7;
  string   os_version        = 8;    // "macOS 26.3"
  repeated Display displays  = 9;
  PeerInfo peer              = 10;   // Host 认出来的你
}
message PeerInfo {
  string   user        = 1;          // WhoIs 登录名 "alice@"；内网路径为空
  string   node        = 2;          // WhoIs 节点名 "xl-iphone"
  string   device_id   = 3;
  PathKind path        = 4;          // Host 视角：直连 / DERP / 内网 / 局域网
  string   derp_region = 5;          // "上海"，仅 DERP
}
message Display {
  uint32 id         = 1;             // 线上 id，1..=255，见下方说明
  string name       = 2;             // "内建显示器"、"DELL U2723QE"
  uint32 width_px   = 3;
  uint32 height_px  = 4;
  float  scale      = 5;             // 2.0 = Retina
  int32  x          = 6;             // 桌面坐标系中的位置（点）
  int32  y          = 7;
  uint32 refresh_hz = 8;
  bool   primary    = 9;
  uint32 rotation   = 10;            // 0 / 90 / 180 / 270
  bool   is_virtual = 11;            // Host 建的虚拟显示器（M3，03 §3「虚拟显示器」）
}
```

**显示器线上 id**：`Display.id`、`StartStream.display_id`、`SelectDisplay.display_id`、`StreamConfig.display_id`、视频头和指针数据报里的 `display_id` 都是同一个线上 id，取值 1..=255，由 Host 分配，与系统的显示器 id（如 Mac 的 `CGDirectDisplayID`）无关。同一个 Host 进程里，一块屏拔掉再插回保持原 id；1..=255 用完时才回收拔掉最久的那个。客户端请求里的 0 表示「主显示器」（`primary`，没有标记时取列表第一块）；Host 发出的消息里不出现 0。

```protobuf
message AuthResult {
  enum Status { STATUS_UNSPECIFIED = 0; OK = 1; PENDING_APPROVAL = 2; DENIED = 3; }
  Status    status             = 1;
  Role      role               = 2;
  bytes     session_token      = 3;  // OK 时下发，16 字节随机
  uint32    approval_timeout_s = 4;  // PENDING 时 = 30
  ErrorInfo error              = 5;  // DENIED 时的原因
  bool      resumed            = 6;
}

// ---------- 会话与多设备 ----------
message SessionState {
  Role your_role              = 1;
  Participant controller      = 2;
  repeated Participant viewers = 3;
  bool input_paused_local     = 4;   // 电脑前有人在用，远端输入暂停
}
message Participant {
  string   device_id   = 1;
  string   device_name = 2;
  string   user        = 3;
  PathKind path        = 4;
  uint64   since_us    = 5;
}
message RequestControl  {}
message ControlRequest  { Participant requester = 1; uint32 timeout_s = 2; }    // Host → 当前控制者
message ControlResponse { string requester_device_id = 1; bool allow = 2; }     // 当前控制者 → Host
message LocalActivity   { bool active = 1; }
message IdleWarning     { uint32 seconds_left = 1; bool lock_on_end = 2; }
message SessionAction {
  enum Kind { KIND_UNSPECIFIED = 0; LOCK_SCREEN = 1; CTRL_ALT_DEL = 2; LOCK_ON_END = 3; WAKE_DISPLAY = 4; }
  Kind kind   = 1;
  bool enable = 2;                    // 只对 LOCK_ON_END 有意义
}

// ---------- 视频 ----------
message DisplayList   { repeated Display displays = 1; }
message SelectDisplay { uint32 display_id = 1; }
message StartStream {
  uint32  display_id   = 1;
  Codec   codec        = 2;
  uint32  max_width    = 3;
  uint32  max_height   = 4;
  uint32  fps          = 5;
  uint32  bitrate_kbps = 6;           // 0 = Host 按路径选起始码率
  Quality quality      = 7;
}
message SetQuality { Quality quality = 1; }

// App → Host。会话中开关远端声音（§8.3）。
message SetAudio {
  bool enabled = 1;   // true：开始发 0x11；false：停止
}
// App → Host。App 开始 / 停止本地录制（M3 V5）。只是告知，Host 不能拒绝。
message RecordingNotice {
  bool active = 1;
}
// App → Host。请 Host 新建一块虚拟显示器（M3）。只在 Host 声明了 VIRTUAL_DISPLAY 时发送；只有控制者能建。
message CreateVirtualDisplay {
  uint32 request_id = 1;   // App 自增，原样回在 VirtualDisplayResult
  uint32 width_px   = 2;   // 期望像素尺寸，保持横竖；两者都必须非 0
  uint32 height_px  = 3;
  float  scale      = 4;   // 期望 HiDPI 倍率；0 = Host 选
  uint32 refresh_hz = 5;   // 0 = 60
  bool   select     = 6;   // true：建好后把本连接的推流切到新屏（等同于随后一条 SelectDisplay）
}

// Host → App。回应 CreateVirtualDisplay。
message VirtualDisplayResult {
  enum Status {
    STATUS_UNSPECIFIED = 0;
    OK          = 1;   // display 为新屏（或本会话已有的那块）
    UNSUPPORTED = 2;   // 这台 Host 现在造不了（私有 API 不在、驱动没装）
    NOT_ALLOWED = 3;   // 不是控制者
    LIMIT       = 4;   // 本会话已有一块正在建，或整机已满
    BAD_REQUEST = 5;   // 尺寸为 0、太小
    FAILED      = 6;   // 平台建失败，或 10 秒内没出现
  }
  uint32  request_id = 1;
  Status  status     = 2;
  Display display    = 3;   // 仅 OK：实际尺寸 / 倍率 / 刷新率，可能与请求不同
  string  message    = 4;   // 给日志看的英文
}

// App → Host。移除本会话建的虚拟显示器。不是本会话建的，Host 忽略。不回应；结果看随后的 DisplayList。
message ReleaseVirtualDisplay {
  uint32 display_id = 1;   // 线上 id
}

message StreamConfig {
  uint32 epoch             = 1;       // 每次参数变化 +1，视频头里带
  uint32 display_id        = 2;
  Codec  codec             = 3;
  uint32 width             = 4;
  uint32 height            = 5;
  uint32 fps               = 6;
  uint32 bitrate_kbps      = 7;
  float  cursor_x          = 8;
  float  cursor_y          = 9;
  bool   cursor_visible    = 10;
  PixelRect crop              = 11;  // 本 epoch 画面对应的显示器区域（原生像素）；缺省 = 整块显示器
  uint32    viewport_seq      = 12;  // 该裁剪依据的 Viewport.seq；未裁剪为 0
  uint32    display_width_px  = 13;  // 本 epoch 显示器原生像素（含旋转）；0.5+ 的 Host 总是填
  uint32    display_height_px = 14;
}
// 显示器原生像素矩形：左上角原点，x 向右、y 向下；与 Display.width_px/height_px 同一坐标系
message PixelRect {
  uint32 x = 1;
  uint32 y = 2;
  uint32 w = 3;
  uint32 h = 4;
}

// 本地放大后的可见区域（03 §5.5）。双方都声明 VIEWPORT 才发
message Viewport {
  uint32    seq            = 1;  // 每连接从 1 递增；Host 只处理比已处理的大的
  uint32    display_id     = 2;  // 与当前 StreamConfig.display_id 相同
  PixelRect rect           = 3;  // 可见区，向外对齐 16 px；缺省 = 整屏（未放大）
  bool      settled        = 4;  // 手势和动画都已停止 ≥ 250 ms
  uint32    view_width_px  = 5;  // rect 在手机屏上占的物理像素
  uint32    view_height_px = 6;
  bool      crop_ok        = 7;  // App 此刻能按 StreamConfig.crop 渲染
}

message ViewportAck {
  enum Mode { MODE_OFF = 0; ROI = 1; CROP = 2; }
  uint32 seq  = 1;
  Mode   mode = 2;
}
message KeyframeRequest {
  enum Reason { REASON_UNSPECIFIED = 0; LOSS = 1; DECODER_RESET = 2; RESUME = 3; BACKLOG = 4; }
  Reason reason          = 1;
  uint32 last_good_frame = 2;
}
message PauseStream  {}               // App 切后台：停止编码，连接保留
message ResumeStream {}               // 回前台：Host 立刻发关键帧

// ---------- 反馈与统计 ----------
message Feedback {                    // App → Host，每 200 ms
  uint32 epoch                   = 1;
  uint32 interval_ms             = 2;
  uint32 frames_received         = 3;
  uint32 frames_lost             = 4;  // FEC 也救不回的帧
  uint32 frames_recovered        = 5;  // 靠 FEC 救回的帧
  uint32 shards_received         = 6;
  uint32 shards_lost             = 7;
  uint32 recv_kbps               = 8;
  int32  delay_gradient_us_per_s = 9;  // 单程时延的斜率，§7
  uint32 owd_p50_us              = 10; // 采集→收齐
  uint32 decode_queue            = 11; // 待解码帧数
  uint32 decode_p50_us           = 12;
  uint32 render_fps              = 13;
  uint32 battery_pct             = 14;
  bool   low_power               = 15;
}
message RateUpdate {                  // Host → App，参数变化时
  uint32 target_kbps   = 1;
  uint32 fps           = 2;
  float  scale         = 3;           // 1.0 / 0.75 / 0.5
  uint32 fec_ratio_pct = 4;
  string reason        = 5;           // "delay" / "loss" / "path:derp" …，只用于日志和 HUD
}
message HostStats {                   // Host → App，每 1 s
  uint32   capture_fps    = 1;
  uint32   encode_p50_us  = 2;
  uint32   encode_p95_us  = 3;
  uint32   cpu_pct        = 4;
  PathKind path           = 5;
  string   derp_region    = 6;
  uint32   send_kbps      = 7;
}
message Ping { uint64 t1_us = 1; }
message Pong { uint64 t1_us = 1; uint64 t2_us = 2; }

// ---------- TCP 模式输入连接 ----------
message AttachInput { bytes session_token = 1; }

// ---------- 配对（04） ----------
message PairStart {
  string   device_name  = 1;
  Platform platform     = 2;
  bool     has_qr       = 3;
  bytes    pair_file_id = 4;   // 16 字节 = 配对文件流程（口令来自文件）；空 = 6 位码流程；其他长度 → Host 回 EXPIRED
}
message PairSpake   { bytes msg = 1; }              // SPAKE2 消息，双方各发一条
message PairConfirm { bytes mac = 1; }              // HMAC(K_confirm, transcript)
message PairResult {
  enum Status { STATUS_UNSPECIFIED = 0; OK = 1; WAITING_LOCAL = 2; DENIED = 3; BAD_CODE = 4;
                EXPIRED = 5; LOCKED = 6; DISABLED_ON_PATH = 7; METHOD_DISABLED = 8; }
  Status status        = 1;
  string host_id       = 2;
  string host_name     = 3;
  uint32 attempts_left = 4;
  uint32 retry_after_s = 5;    // 仅 LOCKED：距解锁的秒数（向上取整）；其他状态为 0
}
// 系统账户密码配对。只在 intent=PAIR、且 ServerHello.features 含 PAIR_PASSWORD 时由 App 发送，每个连接最多一次。
// METHOD_DISABLED：这种配对方式在这台 Host 上没打开（开关关着，或平台没有系统账户校验）。
message PairPassword {
  string   device_name = 1;
  Platform platform    = 2;
  string   account     = 3;    // 系统账户短名或全名，1–256 字节 UTF-8，不含控制字符
  bytes    password    = 4;    // 密码的 UTF-8 字节，1–1024 字节；只在 TLS 内传输，双方用完立即清零
}

// ---------- 错误与结束 ----------
message ErrorInfo {
  uint32 code          = 1;           // 101–199，见 10-appendix · 错误码
  string message       = 2;           // 给日志看的英文，界面文案由 App 按 code 取
  uint32 retry_after_s = 3;
  map<string, string> detail = 4;     // 如 {"host_version":"0.2.4","min":"0.3.0"}
}
message Goodbye {
  enum Reason {
    REASON_UNSPECIFIED = 0; USER_CLOSED = 1; HOST_CLOSED_BY_LOCAL_USER = 2; IDLE_TIMEOUT = 3;
    REPLACED = 4; REVOKED = 5; BACKGROUND = 6; HOST_SHUTDOWN = 7; HOST_UPDATING = 8; PERMISSION_LOST = 9;
  }
  Reason reason  = 1;
  string message = 2;
}
```

演进规则：字段编号只增不改；删字段用 `reserved`；新增枚举值时，旧代码按「未知」处理，不能崩。

配对消息补充（流程见 [04 · 其他配对方式](04-identity-security.md#其他配对方式m2)）：

| 项 | 规则 |
|---|---|
| `PairPassword` | 账户密码配对。不走 SPAKE2，不经 `WAITING_LOCAL`；Host 回一条 `PairResult` 后关闭。`Debug` / 日志里 `account`、`password` 一律打成 `<redacted>` |
| `PairStart.pair_file_id` | 配对文件流程。其余与二维码流程相同（SPAKE2 + 双向确认），确认成功后直接 `PairResult(OK)`，不发 `WAITING_LOCAL` |
| `METHOD_DISABLED` | 该方式在这台 Host 上没打开。不计失败次数；App 按「方式不可用」显示，不当协议错误 |
| `retry_after_s` | 只在 `LOCKED` 时非 0：距解锁的秒数，向上取整 |
| 配对连接的 `ServerHello` | 也填 `features`（功能位不算识别信息）；名字、OS、显示器仍不填 |
| 功能位不满足 | App 把 `PAIR_PASSWORD` / `PAIR_FILE` 并进 `required_features`；Host 没有该位 → 按 §11 规则 2 以 170 结束，不发 `PairPassword` / `pair_file_id` |

**SetAudio**（App → Host）：会话中开关远端声音。只在 Host 声明了 `AUDIO_OPUS` 时发送。Host 收到后覆盖握手时 `ClientCaps.audio_opus` 的选择，直到连接结束。客户端没声明 `AUDIO_OPUS` 时，Host 忽略这条消息。

**RecordingNotice**（App → Host，M3 V5，功能位 `RECORDING_NOTICE`）：App 开始或停止本地录制时告知 Host，只是告知，Host 不能拒绝，也不回应。只在 Host 声明了 `RECORDING_NOTICE` 时发送；没声明的旧 Host 上 App 仍可录制，但开始前提示「这台电脑不会显示录制提示」。
- 控制者和观看者都可以发。App 每收到一条 `StreamConfig` 都会重发 `active = true`，Host 对已在录的连接再收到 `true` 不做处理；没在录时收到 `false` 也不做处理。
- Host 收到 `active = true` 时把这个会话标为「录制中」（IPC `SessionSummary.recording`，「正在被控制」胶囊和会话列表随之刷新）；收到 `active = false` 或连接关闭时清掉标记，并写一条审计 `recording`（[06 · 审计事件口径](06-host.md#审计事件)）。
- 连接断开后用 `resume_token` 恢复时，旧连接关闭会结束一条审计，新连接上 App 重发 `active = true` 后开始新的一条：一次录制可能因为重连被拆成多条审计。
- Envelope 56 是录制组的。

**虚拟显示器**（M3，功能位 `VIRTUAL_DISPLAY`）

- `CreateVirtualDisplay`（App → Host）：只有控制者能发；Host 没声明 `VIRTUAL_DISPLAY` 时 App 不发。
  - Host 选模式：保持横竖，超出平台上限时等比缩小；宽高取平台要求的倍数；倍率和刷新率取平台支持的最近值，刷新率只有 30 / 60。
  - Host 等新屏出现在自己的显示器列表里以后，才先发 `DisplayList`（新屏 `is_virtual = true`），再发 `VirtualDisplayResult(OK)`。
  - `select = true` 且本连接已在推流时，随后发新的 `StreamConfig`（epoch + 1），切到新屏。
  - 10 秒内没出现回 `FAILED`。
- **归属**：虚拟屏属于建它的会话。
  - 每个会话最多一块；同一会话再请求回 `OK` 和已有的那块。
  - 整机最多两块，满了回 `LIMIT`。
  - 观看者回 `NOT_ALLOWED`。
- **生命周期**：
  - 断线进入 60 秒保留期（§4.5）时虚拟屏保留；保留期内恢复，还是同一块屏、同一个线上 id。
  - 会话真正结束（含保留期过期）时 Host 销毁它，随后的 `DisplayList` 里不再有它。正在看它的连接按「所选显示器被拔掉」处理，切到主显示器。
  - Host 退出时虚拟屏随进程消失。
- `ReleaseVirtualDisplay`（App → Host）：移除本会话建的虚拟屏；不是本会话建的忽略。不回应。
- `Display.is_virtual`：Host 建的虚拟屏为 true。`name` 由 Host 定为「虚拟屏（<设备名>）」。虚拟屏永远不是 `primary`。旧 App 忽略该字段，照普通显示器显示。

**Viewport / ViewportAck**（M3，[§5.5](#55-视口感知编码m3)）：App → Host 回报本地放大后的可见区域，Host 每处理一条回一条 `ViewportAck`。双方都声明 `VIEWPORT` 才发，否则 Host 忽略。`StreamConfig.crop` 只在客户端声明了 `VIEWPORT`、且最近一条 `Viewport.crop_ok = true` 时出现；`display_width_px/height_px` 由 0.5+ 的 Host 总是填。Envelope 26、27 是视口组的。

## 4. 时序

### 4.1 新会话（tailnet，已配对，允许直接控制）

```
App                                         Host
 │── QUIC + TLS（双向证书）─────────────────►│  SPKI 查配对表；tailnet 来源再做 WhoIs
 │── 打开控制流：ClientHello ───────────────►│  版本、功能位检查 → 鉴权 → 策略
 │◄────────────── ServerHello + AuthResult(OK, CONTROLLER, token)
 │◄────────────── SessionState
 │── StartStream(display, quality=AUTO) ───►│  按路径选起始码率
 │◄────────────── StreamConfig(epoch=1)
 │◄═══════════════ 视频数据报（先关键帧）
 │── 打开 input 流（0x01）──────────────────►│
 │◄────────────── 打开 cursor 流（0x02），先发当前光标形状
 │── Ping / Feedback（周期）───────────────►│
 │◄────────────── Pong / HostStats / RateUpdate
```

目标：点击到首帧 ≤ 1 s；有预热连接时 ≤ 300 ms（设计文档 §11.2）。

### 4.2 需要本机确认

`AuthResult(PENDING_APPROVAL, approval_timeout_s=30)` → Host 弹确认框（画布 `HostConfirm` / `WinConfirm`）→ 30 秒内：

- 允许：`AuthResult(OK, role)`，继续 4.1。
- 拒绝：`AuthResult(DENIED, error{code=140})`，Host 关闭连接。App 显示 `ErrDenied`，「再请求」有次数限制（每设备每 10 分钟 3 次）。
- 超时：同拒绝，`detail.reason = "timeout"`。

App 在等待期间显示 `PairWaiting` 同款的等待状态，可取消（发 `Goodbye(USER_CLOSED)`）。

### 4.3 有人已在控制（多设备，设计文档 §9.12）

- 新连接得到 `AuthResult(OK, VIEWER)` + `SessionState(controller=…)`，画面照常推。画布 `SessionShared` 的 `view` 状态。
- App 发 `RequestControl`：
  - 当前控制者仍在线且为同一用户的设备：Host 立即把控制权交给它，所有人收到新的 `SessionState`。原控制者变为观看并显示提示（`SessionShared` 的 `taken` 状态，Mac 端 `HostHandoff`）。
  - 不同用户：Host 发 `ControlRequest` 给当前控制者，并在电脑上弹窗。30 秒内控制者 `ControlResponse(allow)` 或电脑前有人点允许 → 交接；否则请求者保持观看，收到 `ErrorInfo(code=141)`。
- 控制者断开后的 60 秒保留期内，同一用户的新连接要接管也必须本机确认（30 秒）；不能套用在线控制者的立即交接规则。原设备用有效恢复令牌恢复按 §4.5 处理。保留期结束后，同用户设备可请求直接接管，仍受有人值守策略约束。
- 观看者上限取 Host 配置 `[access] max_viewers`，默认 3；达到上限后新增观看者返回 `DENIED(code=142)`。名册已按配置限制，鉴权入口仍写死 3（实现待跟进）。
- 观看者的 input 流、指针数据报一律丢弃（Host 侧强制，不依赖 App 自觉）。

### 4.4 预热与探测

- **预热**：设备页出现在线设备时，App 对最近用过的最多 4 台建立 QUIC + TLS，**不打开控制流**。Host 不记会话、不弹窗。用户点击后在这条连接上发 `ClientHello`。预热连接 60 秒没用就关。
- **探测**（诊断页 `Diagnostics`）：`ClientHello(intent=PROBE)` → Host 回 `ServerHello`（只填 protocol、host_version、host_id、peer）+ `Goodbye(USER_CLOSED)`。用于测每条路的握手耗时和身份，不涉及画面，不需要本机确认。

### 4.5 后台与恢复

1. App 切后台：先发 `ReleaseAll`，再发 `PauseStream`，Host 停止为这条连接编码，连接保留；App 暂停主路判死和备用路补建。
2. 30 秒内回前台：发 `ResumeStream`，Host 立刻出关键帧。
3. 超过 30 秒：App 发 `Goodbye(BACKGROUND)` 关闭连接。Host 把会话（角色、显示器、画质）保留 60 秒。本会话建的虚拟显示器同样保留。
4. `ClientHello.resume_token` 使用上次 `AuthResult.session_token`（16 字节）。旧连接仍存活时即可使用；需要恢复的旧连接结束后，令牌在 60 秒保留窗口内有效。令牌绑定同一设备身份和同一 Host，单次使用；成功时 Host 消耗旧令牌，返回 `AuthResult(OK, resumed=true, session_token=新令牌)`，继承角色、显示器、画质并替换仍存活的旧连接，跳过本机确认。
5. 恢复成功后 Host 发新的 `StreamConfig`，`epoch = 旧 epoch + 1`，随后直接出关键帧并重发当前光标形状。客户端清空旧 epoch 的重组槽和解码积压；TCP 专用输入连接必须用新令牌重新发送 `AttachInput`，旧令牌不能继续绑定输入（恢复 epoch 与输入重绑接线实现待跟进）。
6. 无效、过期或设备不匹配的令牌不算恢复：Host 走新会话鉴权，可能重新要求本机确认；`AuthResult.resumed=false` 时客户端按新会话处理。恢复尝试最终失败在客户端映射 `RS-E199`；Host 明确返回的拒绝原因仍保留用于诊断（客户端换路失败映射实现待跟进）。
7. Host 暂停兜底为 45 秒：未收到 `ResumeStream` 且连接仍在时，超时自动恢复推流并出关键帧（实现待跟进：当前闸门超时发 `CloseBackground`、按后台告别保留会话，需改为恢复）。收到 `ResumeStream` 或令牌恢复成功时都直接出关键帧。
8. 网络切换、TCP 升级 QUIC 都走上述令牌恢复，见 [§10](#10-多路径与回退)；客户端完整换路状态机与会话接线实现待跟进。

### 4.6 结束

- 用户断开：`Goodbye(USER_CLOSED)`，QUIC 应用关闭码 0。
- 电脑前的人点「断开」：Host 发 `Goodbye(HOST_CLOSED_BY_LOCAL_USER)`。
- 空闲：Host 在空闲超时前 60 秒发 `IdleWarning(60, lock_on_end)`（画布 `SessionIdle`），到时 `Goodbye(IDLE_TIMEOUT)`，按设置锁屏。
- 作废：Host 收到作废后立刻 `Goodbye(REVOKED)`，关闭码 6。
- 会话中途离开当前控制台（快速用户切换、转到登录窗口）：Host 停止采集并发 `Goodbye`，`error.code=190`，`detail.console=other_user|login_window`。连接前用 `AuthResult(DENIED, error.code=190)` 拒绝，不给 `ServerHello` 添加控制台字段。普通锁屏仍属于当前控制台，不单凭 `locked` 拒绝；策略已有，会话层停止采集、告别和握手接线实现待跟进。

QUIC 应用关闭码：

| 码 | 含义 |
|---|---|
| 0 | 正常结束 |
| 1 | 协议错误（消息解析失败、顺序错误） |
| 2 | 鉴权失败 |
| 3 | 被同一设备的新连接顶替 |
| 4 | Host 关闭 / 升级中 |
| 5 | 空闲超时 |
| 6 | 设备已作废 |

## 5. 视频分片与 FEC

### 5.1 数据报格式（类型 0x10）

```
偏移 长度 字段
 0    1   type            = 0x10
 1    1   flags           bit0 关键帧；bit1 静止补帧（高质量）；其余为 0
 2    1   display_id
 3    1   epoch           StreamConfig.epoch 的低 8 位
 4    4   frame_id        每个 epoch 从 0 开始递增
 8    2   shard_idx       0..data_shards+parity_shards-1；小于 data_shards 的是数据分片
10    2   data_shards     k
12    2   parity_shards   m
14    2   shard_len       每个分片的字节数（偶数）；TCP 模式为 0
16    4   frame_len       帧的真实字节数（最后一个数据分片要去掉补零）
20    8   capture_ts_us   这帧在 Host 屏幕上显示的时刻（Host 时钟）
28    …   分片内容        shard_len 字节
```

头部 28 字节（M0 是 18 字节，没有 FEC 和显示器字段）。帧内容是 Annex-B 格式的 HEVC 或 H.264 访问单元，与 M0 一致。

### 5.2 发送端

```
S = floor((conn.max_datagram_size() - 28) / 2) * 2      // 每帧开始时取一次，随 MTU 探测变化
k = ceil(frame_len / S)
r_eff = (r == 0) ? 0 : (关键帧 ? max(r, 0.25) : r)       // r 是当前冗余比例，§7；关键帧下限见下
m = (r_eff == 0) ? 0 : clamp(ceil(k * r_eff), 1, 64)
数据分片 i = frame[i*S .. min((i+1)*S, frame_len)]，最后一片补零到 S
冗余分片 = reed_solomon_simd::encode(k, m, 数据分片)
发送顺序：数据分片 0..k-1，然后冗余分片 0..m-1
```

- **关键帧冗余下限 0.25**（`flags.bit0 = 1` 的帧，含首帧、丢帧恢复帧、换 epoch 后的首帧）。关键帧丢了要再走一轮请求（至少 RTT 加 §5.3 的 300 ms 限流），而起播和刚开始丢包时 `loss_ewma` 还没跟上，`r` 停在 0.05：10% 随机丢包下 k=44 的关键帧整帧丢失概率约 70%，下限 0.25 时约 0.7%（k=85 时约 0.03%）。只多占关键帧的冗余，P 帧和静止补帧不变；`r == 0`（不发冗余）时关键帧也不发。编码器码率仍按 §7.2 的 `target / (1 + r)` 算，不为关键帧额外扣减。
- 帧过大（k > 4096）直接丢弃并让下一帧出关键帧，记错误日志（正常码率下关键帧也就 100–300 个分片）。
- 视频数据报发送失败（`SendDatagramError::TooLarge`）时，立刻重取 `max_datagram_size()` 重切这一帧。

### 5.3 接收端

- 每个 `(epoch, frame_id)` 一个缓冲槽，最多同时 8 个槽。epoch 比当前小的分片直接丢。
- 收到任意 k 个分片即可恢复：缺数据分片时用 `reed_solomon_simd::decode`，再截取 `frame_len` 字节。
- 只按 `frame_id` 递增交付，永不交付比上一个已交付更旧的帧。
- **判定丢帧**：比它新 2 帧的帧收齐即判丢；否则，最后一个有效分片到达后 `max(2 × 帧间隔, 50 ms)` 仍不足 k 个即判丢；接收方自身被饿住的时间不计入期限。
- 丢帧之后进入「等关键帧」：后面的非关键帧都丢掉，发 `KeyframeRequest(LOSS, last_good_frame)`；500 ms 内没等到关键帧就再发一次。
- 解码器未就绪或待解码超过 2 帧（M0 冷启动积压的问题）：清空积压，发 `KeyframeRequest(BACKLOG)`。
- Host 端关键帧请求限流：每 300 ms 最多响应一次，多余的合并。

### 5.4 静止补帧

画面 300 ms 没有变化时，Host 用更高质量重编最后一帧（VideoToolbox 临时调高 `Quality`；Windows 临时降低 QP），`flags.bit1 = 1`。这一帧不是关键帧，按普通 P 帧发。画面再动时恢复原参数。之后 1 秒内不再补帧。

### 5.5 视口感知编码（M3）

App 本地放大后，用 `Viewport` 告诉 Host 自己看得见显示器的哪一块；Host 提高这一块的画质，或只编码这一块。

**App**

- `rect` 用显示器原生像素（与 `Display.width_px/height_px`、`StreamConfig.display_width_px/height_px` 同一坐标系，左上角原点），向外对齐 16 px；可见区宽高都 ≥ 98% 时不带 `rect`（整屏）。
- 最多 10 次/秒；内容不变不发；手势和动画都停止 250 ms 后补一条 `settled = true`。
- `seq` 每连接从 1 递增。新连接（含恢复）后收到第一条 `StreamConfig` 时重报当前状态。
- `crop_ok`：此刻能按 `StreamConfig.crop` 渲染；画中画等不能时为 false。

**Host**

- 每条 seq 比已处理的大的 `Viewport` 回 `ViewportAck{seq, mode}`；seq 不大于已处理的直接丢。`display_id` 不是当前推流显示器的，只回 Ack 不处理。
- 放大倍数 z = sqrt(显示器面积 / rect 面积)。

| 模式 | 进入 | 退出 | 做法 |
|---|---|---|---|
| ROI | 编码器支持 ROI，z ≥ 1.25 | z < 1.15 | 可见区每边外扩 1/10、对齐 64 px 作焦点区，QP −6；可见区 ≤ 半屏时其余 +2。最多 5 次/秒更新。不换 epoch、不发关键帧 |
| CROP | 平台支持区域采集、`crop_ok`、`settled`、z ≥ 2 | z < 1.6、整屏、`crop_ok = false`，或手势中可见区越出裁剪区 | 可见区每边外扩 1/8、对齐 16 px 作裁剪区，只采这一块，按新尺寸重建编码器 |

- 裁剪输出尺寸：保持裁剪区宽高比；像素数不超过当前码控（§7.3、§7.4）给的整屏推流尺寸；不超过裁剪区原生像素；不超过手机上可见区占的物理像素；不超过 `StartStream.max_width/max_height`；偶数。
- 进入、换、退出裁剪都重建管线：epoch + 1，新 `StreamConfig` 带 `crop` 和 `viewport_seq`，第一帧是关键帧。两次裁剪变化（含进入）至少隔 1 s；进入或换裁剪后至少保持 300 ms 才因越界退出。
- 码率：两种模式都不改 §7 的目标码率。
- 换显示器、新连接、恢复：回到整屏，等 App 重报。§7.4 降档重建时保留裁剪区，按新尺寸重算输出。
- 静止补帧（§5.4）照常；ROI 生效时补帧那一帧用整帧补帧参数。

**App 渲染**

- `StreamConfig.crop` 缺省 = 整屏。有 crop 时 `src = crop / (display_width_px, display_height_px)`。
- 画面区域始终按整块显示器的宽高比算，解码画面画在其中 `src` 对应的那一块；指针、触控坐标照旧按整块显示器归一化（§6），与 crop 无关。
- 新几何在该 epoch 第一帧上屏时一起切换，之前继续显示旧帧。裁剪期间，裁剪区外用进入裁剪前最后一帧整屏画面垫底。

## 6. 输入、指针、光标

### 6.1 输入事件（input 流 0x01）

每条记录 `u32 长度 + 事件`，事件首字节是类型：

| 类型 | 名称 | 内容 | 说明 |
|---|---|---|---|
| 0x01 | MouseMove | `x f32, y f32` | 只在没有 DATAGRAM 时（TCP 模式）用；平时走指针数据报 |
| 0x02 | MouseButton | `button u8, down u8, x f32, y f32, clicks u8` | 0 左 1 右 2 中 3 后退 4 前进；自带坐标，Host 先移再按（M0 已实现） |
| 0x03 | Scroll | `dx f32, dy f32, phase u8, momentum u8, unit u8` | 自然滚动语义；`phase` 0 无 1 开始 2 变化 3 结束；`momentum` 同取值表示惯性阶段；`unit` 0 像素 1 行 |
| 0x04 | Key | `usage u16, down u8, mods u8` | USB HID 用法码（D22）；`mods` 是按下这一键后的修饰键状态 |
| 0x05 | Text | `len u16, utf8` | 已上屏的文字（输入法组合在手机上完成），单条 ≤ 4 KB |
| 0x06 | ReleaseAll | — | 释放所有按键、修饰键、活动滚动 / 惯性阶段及五个鼠标键（左、右、中、后退、前进）；App 切后台、断线前、切换控制者时发 |
| 0x07 | Touch | `id u8, phase u8, x f32, y f32, pressure f32` | M2，Windows 原生触控 |

`mods` 位：bit0 左 Ctrl，bit1 左 Shift，bit2 左 Alt/Option，bit3 左 Win/Command，bit4–7 对应右侧（HID 顺序）。

解码时坐标做 M0 的处理：非有限值拒绝，其余夹到 0..1。

Host 注入约定（macOS）：

- `Text` 注入前按 UTF-16 单元拆段，每段 ≤ 20，不能拆开代理对；每段用 flags 清零的 Unicode keyDown/keyUp（虚拟键码 0）。CRLF 作为一次 Return，单独 CR、LF、U+2028、U+2029 也转 Return，Tab 转 Tab；其余 C0、DEL、C1 控制字符丢弃，Backspace / Escape 必须发 `Key`。当前拆段器为保留完整扩展字素簇，单簇超过 20 时允许超长，与此硬上限不符（实现待跟进）。
- 修饰键变化用 `FlagsChanged`，普通键按 `mods` 先同步左右修饰键，再发 keyDown/keyUp。方向键隐含 `NumericPad | SecondaryFn`，F1–F20、Help、Home、End、PageUp、PageDown、ForwardDelete 隐含 `SecondaryFn`，数字小键盘隐含 `NumericPad`。Backspace（HID `0x2A`）映射 macOS `kVK_Delete (0x33)`，ForwardDelete（`0x4C`）映射 `0x75`。
- 滚动状态为 Idle / Scrolling / Momentum；手势 Begin → Change* → End 后可接惯性 Begin → Change* → End。丢 Begin 时补零位移 Begin，重入 Begin / 切换阶段时先收尾；无活动阶段的孤立 End 丢弃。手势和惯性同时非零、非有限位移均丢弃；普通滚轮的两个 phase 都为 0。300 ms 无新事件时补零位移 End；`ReleaseAll` 同样收尾并清空小数累积。惯性位移由客户端产生，Host 不生成惯性。
- 客户端键盘布局不进协议。Host 按自身键盘类型（ANSI / ISO / JIS）解析 HID，macOS 从 `KBGetLayoutType(LMGetKbdType())` 获取档案并设置注入的 keyboard type；未知布局退 ANSI，ISO 做位置互换。

**系统组合键**：⌘Tab、⌘空格、Alt+Tab 等会被手机系统拦截，由 App 工具条按钮直接发对应的 Key 序列；调度中心（Mission Control）默认 ⌃↑。地球键 / Fn 目前无法用 Keyboard page `0x07` 的 `Key` 事件表达，M2 再议。Ctrl+Alt+Del 走控制面 `SessionAction(CTRL_ALT_DEL)`（Windows 用 `SendSAS`）。

### 6.2 指针数据报（0x12）

```
type u8 = 0x12 | seq u32 | display u8 | flags u8（bit0 可见） | x f32 | y f32       共 15 字节
```

- App → Host：光标绝对位置。App 本地画光标（设计文档 §6.1），每次手指移动发一次，最高 120 Hz。
- Host → App：电脑前的人或程序移动了光标时，按 60 Hz 上报，App 把本地光标拉过去。
- 收端丢弃 `seq` 不大于上一个的包。

### 6.3 光标形状（cursor 流 0x02）

记录内容是 `proto/rsp/v1/streams.proto` 的 `CursorMsg`：

```proto
message CursorMsg {
  oneof msg {
    CursorShape shape = 1;   // 新形状：带图片
    uint64      use   = 2;   // 已发过的形状，按 id 复用
    bool        hidden = 3;  // true 隐藏（如视频全屏），false 恢复
  }
}
message CursorShape {
  uint64 id     = 1;         // 图片内容的 xxh3 哈希
  uint32 width  = 2;
  uint32 height = 3;
  uint32 hot_x  = 4;
  uint32 hot_y  = 5;
  float  scale  = 6;         // 图片像素 / 点
  bytes  png    = 7;
}
```

App 按 id 缓存最近 64 个形状。Host 只在形状变化时发送；会话开始和恢复时先发当前形状。

## 7. 码率控制

Host 端为每条连接维护一个控制器，输入是 App 每 200 ms 的 `Feedback`，输出是编码器目标码率、帧率、缩放比例和 FEC 冗余比例。

### 7.1 App 侧测量

- **单程时延**：`owd = 收齐时刻(本机) − (capture_ts_us − clock_offset)`。绝对值含时钟误差，只用斜率。
- **时延斜率**：最近 1 秒内每帧的 `(收齐时刻, owd)` 做线性回归，斜率单位 µs/s。持续为正说明队列在涨。
- **丢包**：`shards_lost` 按每帧收到的最大 `shard_idx` 推算。

### 7.2 控制规则

```
每收到一次 Feedback：
  loss      = shards_lost / (shards_received + shards_lost)           // FEC 之前
  loss_ewma = 0.8 * loss_ewma + 0.2 * loss
  过载 = (gradient > 5 ms/s 连续 2 次) 或 frames_lost > 0 或 decode_queue > 2
  空闲 = gradient < 1 ms/s 且 loss < 2% 持续 1 s

  过载：target *= 0.85（400 ms 内最多降一次）
  空闲：target *= 1.08（每秒最多升一次），且不超过 recv_kbps × 1.5
  其他：保持

  r = clamp(0.05 + 4 * loss_ewma, 0.05, 0.30)                        // FEC 冗余比例
  编码器码率 = target / (1 + r)
```

声音开启时，视频编码码率 = `max(500, (target − 128) / (1 + r))`，且不超过原 `encoder_kbps`。

### 7.3 按路径的起点和边界

| 路径 | 起始 | 下限 | 上限 | 最高帧率 | 最大分辨率 |
|---|---|---|---|---|---|
| 局域网 | 20 Mbps | 4 | 40 | 60（iPad Pro 120） | 原生 |
| 公司内网（UDP） | 4 Mbps | 2 | 10 | 先 30，稳定 10 s 后 60 | 1440p |
| tailnet 直连 | 8 Mbps | 2 | 15 | 60 | 1440p |
| DERP 中继 | 3 Mbps | 1.5 | 5 | 30 | 1080p |
| TCP 模式 | 4 Mbps | 2 | 6 | 30 | 1080p |

`StartStream.bitrate_kbps` 非 0 时作为起点，但仍受上限约束。

### 7.4 降档顺序

目标码率贴着下限超过 2 秒时逐级降档，每降一级发新的 `StreamConfig`（epoch + 1）和 `RateUpdate`：

| 画质档 | 第 1 级 | 第 2 级 | 第 3 级 |
|---|---|---|---|
| 自动 / 清晰 | 帧率降到 30 | 分辨率 × 0.75 | 分辨率 × 0.5 |
| 流畅 | 分辨率 × 0.75 | 分辨率 × 0.5 | 帧率降到 30 |
| 省流量 | 固定 30 fps、上限 4 Mbps、分辨率 × 0.75 起 | — | — |

码率超过下限 1.5 倍持续 5 秒，逐级恢复。电量低于 20%（`Feedback.low_power`）时，「自动」按「流畅」处理并把帧率上限降到 30（设计文档 §9.11）。

## 8. 剪贴板、文件、声音

### 8.1 剪贴板（0x03，每项一条流）

```proto
message ClipboardHeader {
  enum Kind { KIND_UNSPECIFIED = 0; TEXT = 1; PNG = 2; }
  Kind   kind   = 1;
  uint64 size   = 2;     // 文本 ≤ 1 MB，图片 ≤ 10 MB
  bytes  sha256 = 3;
}
```

流内容：`u32 长度 + ClipboardHeader`，后面是原始字节，结束即 FIN。

- Host → App：控制者在会话中，电脑剪贴板变化时自动推送（设置可关）。App 不自动写入手机剪贴板，而是在剪贴板面板（画布 `SessionClipboard`）里显示「电脑上复制了…」，点一下才写入。
- App → Host：只在用户点「发送到电脑」时发送（iOS 读剪贴板会弹系统提示，不做自动读取）。
- 观看者不收剪贴板。

### 8.2 文件（0x04，M2）

```proto
message FileHeader {
  string transfer_id = 1;    // 随机 128 位，32 个小写十六进制字符，续传沿用
  string name        = 2;
  uint64 size        = 3;
  uint64 offset      = 4;    // 续传起点
  bytes  sha256      = 5;    // 整个文件
  uint64 mtime_us    = 6;
  string dest_dir    = 7;    // 仅 App → Host 上传；"" = [files].dest_dir
}
```

`FileHeader` 属于 streams 消息。QUIC 每个文件一条单向流：`0x04 | header_len u32 | protobuf FileHeader | 文件[offset..size] | FIN`。文件体是连续原始字节，不给每块加 `u32` 前缀。`header_len` ≤ 4096；文件名非空、UTF-8 ≤ 1024 字节且不含 NUL；`dest_dir` ≤ 2048 字节且不含 NUL，是否绝对路径由 Host 判断；Host → App 恒为 ""，App 忽略它。`sha256` 必须 32 字节；单文件 ≤ 1 TiB（`1 << 40` 字节），`offset ≤ size`。Host 接收还受 `ReceiveConfig.max_file_bytes` 限制，默认同为 1 TiB。

续传与落盘：

- 接收目录内用 `.rs-<transfer_id>.part` 存文件体、`.rs-<transfer_id>.json` 存元数据（`transfer_id, name, size, sha256_hex, mtime_us`，name 已清洗，SHA-256 为十六进制文本）。恢复起点取本地已接收的连续文件长度；发送方的成功写入偏移不代表对端已落盘，不能用作接收方确认。
- `offset=0` 是重传，丢弃同 id 的旧半截内容。非零 offset 必须与半截文件的 id、size、整文件 sha256、mtime_us 一致，且本地长度 ≥ offset；多出的尾部截断到 offset，不足则 `ResumeMismatch { have }`。FIN 提前到达或传输中断保留半截文件及续传位置。
- 收齐后校验整个文件 SHA-256（包括续传前缀），通过后才分配最终文件名并原子改名；不覆盖同名文件，尝试附加编号。落盘文件名清洗路径分隔符、控制字符等并限制为 255 UTF-8 字节，防路径穿越。校验失败为 `Sha256Mismatch`，删除半截文件和元数据，不发布最终文件。平台默认目的地为 Mac `~/Downloads` / Windows 下载文件夹，App 文件页管理手机侧；目前引擎由调用方传入已存在的 `dest_dir`，平台默认目录接线尚未完成。
- 本地取消通过 `CancelFlag`；接收方删除半截文件和元数据，发送方返回 `Cancelled`。进度为 `transfer_id, direction, state, done, size, bytes_per_sec, fail`，`done` 是绝对文件偏移；每 100 ms 节流，状态变化与到达 size 时立即报告。该回调不是线上 `FileStatus`。

本地进度类型：`transfer_id: String`，`direction: Send|Receive`，`state: Queued|Running|Interrupted|Verifying|Completed|Failed|Cancelled`，`done/size/bytes_per_sec: u64`，`fail: Option<Io|Sha256Mismatch|ResumeMismatch|SourceChanged|Protocol|TooLarge>`。发送中断返回 `Interrupted { sent }`，接收中断返回 `Interrupted { transfer_id, received }`；sent / received 均为绝对文件位置。这些是库返回值，没有约定 JSON / protobuf 枚举值。

控制面文件消息（Envelope 50–55）：

```proto
// 文件错误。值名带 FILE_ERROR_ 前缀，prost 生成 FileError::NotFound 这类名字。
enum FileError {
  FILE_ERROR_NONE              = 0;
  FILE_ERROR_NOT_FOUND         = 1;
  FILE_ERROR_PERMISSION_DENIED = 2;   // 系统拒绝（含 macOS TCC）
  FILE_ERROR_OUTSIDE_ROOT      = 3;   // 路径（含软链解析后）不在浏览根内，或命中黑名单
  FILE_ERROR_NOT_A_DIR         = 4;
  FILE_ERROR_NOT_A_FILE        = 5;
  FILE_ERROR_DISABLED          = 6;   // [files] 关掉了这个方向或浏览
  FILE_ERROR_NOT_ALLOWED       = 7;   // 角色 / 意图 / 权限不允许（viewer、仅查看）
  FILE_ERROR_BUSY              = 8;   // 超过并发上限
  FILE_ERROR_IO                = 9;
  FILE_ERROR_NO_SPACE          = 10;
  FILE_ERROR_TOO_LARGE         = 11;
  FILE_ERROR_RESUME_MISMATCH   = 12;
  FILE_ERROR_SHA256_MISMATCH   = 13;
  FILE_ERROR_SOURCE_CHANGED    = 14;
  FILE_ERROR_PROTOCOL          = 15;
}

message FileEntry {
  enum Kind { KIND_UNSPECIFIED = 0; FILE = 1; DIR = 2; SYMLINK = 3; OTHER = 4; }
  string name        = 1;   // 单层名字，不含路径
  Kind   kind        = 2;   // 软链按目标类型报；目标不存在或越界报 SYMLINK
  uint64 size        = 3;   // 目录为 0
  uint64 mtime_us    = 4;   // 0 = 未知
  int32  child_count = 5;   // 目录的子项数；-1 = 没数（超时或不是目录）
}

// App → Host：列一个目录。
message FileList {
  uint32 request_id = 1;    // App 自增，原样回在 FileListing
  string path       = 2;    // Host 上的绝对路径；"" = 默认根（第一个浏览根）
  uint32 offset     = 3;    // 分页起点（排序后的下标）
}

// Host → App。
message FileListing {
  uint32 request_id           = 1;
  string path                 = 2;   // 规范化后的绝对路径
  string parent               = 3;   // "" = 已在根，不能再往上
  repeated FileEntry entries  = 4;
  uint32 next_offset          = 5;   // 0 = 没有下一页
  FileError error             = 6;   // 非 NONE 时 entries 为空
}

// App → Host：请 Host 把一个文件发过来（Host 新开 0x04 流）。
message FileRequest {
  uint32 request_id      = 1;
  string path            = 2;   // Host 上的绝对路径
  string transfer_id     = 3;   // 续传时沿用；"" = Host 新生成
  uint64 offset          = 4;   // App 本地已收到的连续长度
  uint64 expect_size     = 5;   // 续传时给出上次 FileHeader 的值；首次为 0
  uint64 expect_mtime_us = 6;
  bytes  expect_sha256   = 7;   // 空或 32 字节
}

// App → Host：问 Host 某个上传收到了多少（App 上传续传前用）。
message FileQuery {
  uint32 request_id  = 1;
  string transfer_id = 2;
}

// 双向。Host → App：回 FileQuery、FileRequest 失败、上传收完 / 失败。
// App → Host：下载在手机上校验落盘后发 COMPLETED（Host 只打日志）。
message FileStatus {
  enum State {
    STATE_UNSPECIFIED = 0;
    UNKNOWN   = 1;   // Host 没有这个 transfer_id 的半截文件
    PARTIAL   = 2;   // 有半截文件，received 为连续长度
    RECEIVING = 3;   // 正在接收
    COMPLETED = 4;
    FAILED    = 5;
    CANCELLED = 6;
  }
  uint32    request_id  = 1;   // 回应 FileQuery / FileRequest 时原样回；主动通知为 0
  string    transfer_id = 2;
  State     state       = 3;
  uint64    received    = 4;
  FileError error       = 5;
  string    saved_name  = 6;   // COMPLETED 上传时 Host 最终用的文件名（可能带编号）
}

// 双向：取消一个传输。收到方停掉对应流；接收方删半截文件。
message FileCancel {
  string transfer_id = 1;
}
```

App 上传续传前用 `FileQuery` 查 Host 的连续已接收长度；下载用 `FileRequest`，Host 新开 0x04 流。下载校验并落盘后，App 发 `FileStatus COMPLETED`，Host 只打日志。上传完成通知的 `saved_name` 是 Host 最终文件名，可能带编号。

0x04 流 stop / reset 码（常量由 `rsp_core::file_ctl` 提供）：

| 码 | 名字 | 用法 |
|---|---|---|
| 1 | `STREAM_REJECTED` | 校验失败、通道不可用（兼容现有代码） |
| 2 | `STREAM_PAUSED` | 用户暂停，接收方保留半截文件 |
| 3 | `STREAM_CANCELLED` | 用户取消，总是和 `FileCancel` 一起发 |
| 4 | `STREAM_BUSY` | 并发上限 |
| 5 | `STREAM_DISABLED` | `[files]` 关闭 |
| 6 | `STREAM_NOT_ALLOWED` | 角色 / 意图不允许 |

语义以控制消息为准，reset 码只帮日志。取消一定发 `FileCancel`；只看到流断开的一方按中断处理，保留半截文件。

控制与并发上限（`rsp_core::file_ctl`）：

- `FileListing` 每页 ≤ 1000 条且编码后 ≤ 256 KiB；超出则截断并给 `next_offset`。控制帧上限 `rsp_core::control::MAX_CONTROL_FRAME` 为 1 MiB。
- `FileEntry.name` ≤ 1024 字节；`FileList.path` / `FileRequest.path` ≤ 4096 字节；`FileHeader.dest_dir` ≤ 2048 字节。
- 每条连接同时接收 ≤ 4（现有 `MAX_CONCURRENT_RECEIVES`）、同时发送 ≤ 2；超出回 `FILE_ERROR_BUSY` / stop 码 4。
- 每条连接同时只处理 1 个 `FileList`，排队 ≤ 4，再多回 BUSY。

`[files]` 配置与默认浏览根见 [06 · 文件收发与配置](06-host.md#文件收发与配置)；此处规定接口，业务实现由后续文件任务接线。

### 8.3 声音（0x11，M2）

```
type u8 = 0x11 | seq u16 | capture_ts_us u64 | opus_frame
```
头 11 字节，大端。

**编码**
- Opus，48 kHz 双声道，`Application::Audio`；
- 每包正好一个 10 ms 帧（480 样本/声道）；
- 目标 96 kbps，受限 VBR，开带内 FEC（丢包率提示 5%）。
- `opus_frame` 1–1275 字节，不分片。超过路径最大数据报的包直接丢。

**字段**
- `capture_ts_us`：该帧首样本的采集时刻（Host unix 微秒，与视频同一时钟）。
- `seq`：
  - 每个连接独立，从 0 开始，u16 回绕；
  - 每次发送尝试 +1，发送失败也 +1；
  - 暂停 / 恢复、静音抑制都不重置，也不前进（不发就不加）。

**何时发送**（全部满足）
- 对方 `ClientHello.features` 含 `AUDIO_OPUS`；
- `ClientCaps.audio_opus` 为真，或之后最近一次 `SetAudio.enabled` 为真；
- 流在跑（`StartStream` 之后、没有 `PauseStream`）；
- 平台能采系统声音。
- 控制者和旁观者都发。

**其他发送规则**
- **静音抑制**：连续 200 ms 峰值低于 −70 dBFS 时停发，有声即恢复。恢复后 `seq` 接着上次，`capture_ts_us` 跳过静音时长。静音期间不发心跳。
- **TCP**：同样的字节走 0x11 记录通道（§1.3）。
- **码率**：声音开启时，Host 从视频编码码率里预留 128 kbps（§7）。
- **优先级**：声音不排在视频分片后面。QUIC 上：视频发送前看发送队列积压，超过 8 KiB 先等，最多 50 ms；**有声音的连接上**视频分片还要按估算速率 pacing——每发满 8 KiB 等 `(rtt + 3 × (rtt − min_rtt)) × 字节数 / cwnd`，等待时继续从编码队列预取（最多一个队列容量，不丢不乱序）。原因：本地队列水位看不到已在途、排在瓶颈链路上的字节，大窗口能把整个关键帧一次推进瓶颈队列，把随后的声音堵住。TCP 上声音记录优先于视频最新帧槽。Host 管线接线见 [06 · 推流管线](06-host.md#推流管线)。
- **测量口径**：`crates/host-fake/tests/e2e/audio.rs` 的 `audio_clean_latency`、`audio_two_percent_loss_no_pops` 断言首声 capture→receive p95 ≤ 150 ms（实测 83–104 ms）；模拟播放端 p95（106–143 ms）只记录、不断言。

**接收**
- 抖动缓冲 40–80 ms 自适应，初始 40 ms。
- 迟到包丢弃。
- 单个丢包：下一包已到就用它的带内 FEC 恢复，否则 PLC。
- 连续缺 3 帧后淡出为静音并重新缓冲。
- 某包的 `seq` 落在已播放范围、但 `capture_ts_us` 比按 `seq` 推算的时刻晚 50 ms 以上 → 视为静音或暂停后的新话段，从它重新起播。
- `seq` 跳变超过 1000 → 视为新流。
- 音画不做严格同步，声音只求不卡。

**为什么没有 epoch**：每包独立可解，没有跨包的解码依赖。连续性靠 `seq` + `capture_ts_us` 已够。

## 9. 时钟同步

沿用 M0 `rsp-core::clock`：App 每 1 秒发 `Ping{t1}`（前 5 秒每 200 ms 一次），Host 回 `Pong{t1, t2}`。`offset = t2 − (t1 + t4) / 2`，取最近 30 个样本里 RTT 最小的那个。HUD 的 RTT 取最近 5 个样本的中位数。

## 10. 多路径与回退

### 10.1 候选地址

App 拨号前为一台电脑收集候选（设计文档 §4.6、§4.11）：

| 来源 | 内容 | 路径类型 |
|---|---|---|
| 本机目录（已配对记录、tailnet 节点，以及 Tailscale / Headscale API 的设备列表） | tailnet 100.x 地址 | TAILNET_DIRECT（实际直连还是 DERP 由 Host 在 `PeerInfo` 里告诉 App） |
| 已配对记录 | 配对时登记的内网、局域网地址 | INTRANET 或 LAN（按「其他网络」里登记的网段判定，见 07-app） |
| mDNS `_rsp._udp` | 家里局域网地址 | LAN |
| 手动添加 | 主机名或 IP | MANUAL |
| 本地缓存 | 上次成功的地址 | 原类型，排最前 |

只尝试「可能可达」的候选：比如手机没连公司 VPN 且不在公司网段内，就不拨 10.30.x 的地址，而是直接判定该路「需要公司 VPN」。

### 10.2 拨号（Happy Eyeballs）

```
t = 0       按优先级对每个候选发起 QUIC 握手，相邻候选间隔 25 ms
            优先级：上次成功 > LAN > INTRANET > TAILNET > MANUAL
t = 250 ms  若还没有任何 QUIC 握手完成：对每个候选发起 TCP + TLS，同样间隔 25 ms
首个握手完成（TLS 完成且 Host 指纹匹配）后，再等 30 ms：
            这期间有更高优先级的路完成，就选它（延迟相近时优先内网）
选中后：    在它上面发 ClientHello
            保留一条不同类型的已握手连接做备用（不开控制流，keep-alive 15 s），其余关闭
总超时：    3 s（只有 DERP 可用时放宽到 5 s）
```

握手完成但指纹不匹配：这条路标记为「另一台电脑」，不参与选择；如果所有路都是这个结果，报 `RS-E150`。

### 10.3 拨号失败的归类

App 必须把失败归到画布上的某个断开页：

| 观察到的情况 | 错误码 | 画板 |
|---|---|---|
| 目录显示节点离线，且没有可用的内网 / 局域网路 | RS-E110 | `ErrOffline` |
| 节点在线，7400 端口 TCP 被拒绝 | RS-E120 | `ErrNoHost` |
| 节点在线，但 UDP 和 TCP 都超时 | RS-E130 | `ErrPolicy`（通常是 ACL 没放行） |
| 只有内网路，且本机没有公司 VPN 接口 | RS-E160 | `ErrVPN` |
| 有到内网网段的路由，但 UDP 和 TCP 都超时 | RS-E161 | `ErrPort` |
| 所有完成握手的路指纹都不对 | RS-E150 | `ErrFingerprint` |
| 已知的 Host 版本低于要求（拨号前就判定） | RS-E170 | `ErrVersion` |

握手成功后由 Host 返回的错误（`AuthResult.error` / `ErrorInfo`）见 [10-appendix](10-appendix.md#错误码)。

### 10.4 会话中换路

- 主路判死：从最后一次入站消息起静默 500 ms 开始探测，每 250 ms 发一次 Ping；累计静默 1.5 s 且存在未答 Ping 时判死。任何入站数据报或控制消息（包括 Pong、HostStats）都重置静默计时；传输层内部消费的 QUIC keep-alive 不算会话入站。若 Ping 始终无法发出，静默 3 s 兜底判死；后台 PauseStream 期间暂停此计时。
- 有备用连接：立即在备用连接上做会话恢复（4.5 第 4 步）；没有就重新执行 10.2。目标 ≤ 1 s 恢复画面（设计文档 §4.6），≤ 2 s 是验收线（§11.2）。
- 换路期间画面冻结在最后一帧，不黑屏；HUD 显示「正在换路」（画布 `SessionReconnect`）；期间的触摸丢弃，不回放。
- TCP 模式下每 30 秒试一次 QUIC；在旧连接仍存活时于 QUIC 上提交恢复令牌，收到 `resumed=true` 和新 `session_token` 后替换旧连接。恢复的 epoch、重组器重置、TCP 输入重绑、`resumed=false` 和错误码 199 按 §4.5；完整换路及恢复接线实现待跟进。
- Wi-Fi 与 5G 互切：tailnet 地址不变，QUIC 连接通常不中断；如果中断，按上面处理。内网路径地址变了，用 `endpoint.rebind()` 让 QUIC 迁移。

## 11. 版本与功能位

| 位 | 名称 | 含义 | 首个版本 |
|---|---|---|---|
| 0 | `FEC_RS` | 视频 Reed-Solomon | 0.3.0 |
| 1 | `CURSOR_SHAPE` | cursor 流 | 0.3.0 |
| 2 | `POINTER_DGRAM` | 指针数据报 | 0.3.0 |
| 3 | `CLIPBOARD_TEXT` | 文本剪贴板 | 0.3.0 |
| 4 | `CLIPBOARD_IMAGE` | 图片剪贴板 | 0.4.0 |
| 5 | `MULTI_DISPLAY` | 多显示器选择 | 0.3.0 |
| 6 | `KEY_HID` | HID 键盘事件 | 0.3.0 |
| 7 | `TEXT_INPUT` | Text 事件 | 0.3.0 |
| 8 | `TCP_TRANSPORT` | TCP 回退 | 0.3.0 |
| 9 | `RESUME` | 会话恢复 | 0.3.0 |
| 10 | `MULTI_VIEWER` | 观看者与接管 | 0.3.0 |
| 11 | `HQ_REFRESH` | 静止补帧 | 0.3.0 |
| 12 | `RATE_FEEDBACK` | Feedback / RateUpdate | 0.3.0 |
| 13 | `SESSION_ACTIONS` | 锁屏、结束后锁屏等 | 0.3.0 |
| 14 | `INTRANET_CONFIRM_EACH` | 「内网连接每次确认」 | 0.3.2 |
| 15 | `AUDIO_OPUS` | 声音；平台支持系统声音采集时才声明 | 0.4.0 |
| 16 | `FILE_TRANSFER` | 文件 | 0.4.0 |
| 17 | `TOUCH_NATIVE` | Windows 原生触控 | 0.4.0 |
| 18 | `CTRL_ALT_DEL` | SAS | 0.4.0 |
| 19 | `PAIR_PASSWORD` | Host 认识 `PairPassword`（与密码配对开关是否打开无关） | 0.4.0 |
| 20 | `PAIR_FILE` | Host 认识 `PairStart.pair_file_id` | 0.4.0 |
| 21 | `FILE_BROWSE` | 文件浏览、拉取、续传查询、取消、`FileHeader.dest_dir` 与 `Intent FILES`；隐含 `FILE_TRANSFER` | 0.4.0 |
| 22 | `VIRTUAL_DISPLAY` | 虚拟显示器；平台能造时才声明（Mac 私有 API 在、Windows 驱动已装） | 0.5.0 |
| 23 | `VIEWPORT` | 视口感知编码：Viewport / ViewportAck、StreamConfig.crop（§5.5） | 0.5.0 |
| 24 | `RECORDING_NOTICE` | App 本地录制告知（RecordingNotice） | 0.5.0 |

规则：

1. `protocol` 不等于 1：Host 回 `ErrorInfo(code=170 或 171)` 后关闭。
2. 一方的 `required_features` 不是另一方 `features` 的子集：Host 太旧回 170，App 太旧回 171，`detail` 带双方版本。
3. App 0.3.x 要求 Host ≥ 0.3.0：App **在拨号前**就用已知的 Host 版本（上次会话或配对时记下的）判断（画布 `ErrVersion` 显示「它装的 Host 0.2.4」靠的就是这个），握手时再查一次兜底。
4. 可选功能对方不支持时，相关开关置灰并注明「需要 Host 0.x.y」——App 内置上表的「功能位 → 首个版本」映射。

文件兼容规则：只有 `FILE_TRANSFER` 而没有 `FILE_BROWSE` 的 M1 Host，App 只允许「发到电脑」（上传到默认目录，不续传查询），不发上述 C1 控制消息。使用 `Intent FILES = 5` 时必须 `required_features |= FILE_BROWSE`：老 Host 会把未知 intent 当 `INTENT_UNSPECIFIED` 并走 CONTROL 规则，可能抢控制；要求功能位可让它在握手时直接拒绝。App 收到这种拒绝报 `files.unsupported`，提示升级电脑端。Envelope 25 是声音组的 `SetAudio`，34 是配对组的 `PairPassword`，35–39 留给配对组。

## 12. 测试向量

`crates/rsp-core/tests/vectors/` 下每种消息至少一个二进制样本和一个 JSON 描述：

- 每种 `Envelope` 变体（含所有枚举的非默认值）。
- 视频头：普通帧、关键帧、TCP 模式（`shard_len=0`）。
- FEC：固定内容的 3 帧（k = 1 / 7 / 120，m = 1 / 2 / 12），删掉任意 m 个分片后能还原。
- 每种输入事件、指针数据报、CursorMsg、ClipboardHeader。
- 控制面向量：`control/viewport`、`control/viewport_ack`、`control_enums/viewport_ack_{roi,crop}`、`control/recording_notice`。

所有解码器（Envelope、视频头、输入、指针、TCP 记录、CursorMsg）都有 cargo-fuzz 目标，见 [09](09-quality-release.md#模糊测试)。
