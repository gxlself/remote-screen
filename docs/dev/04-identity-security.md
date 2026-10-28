# 04 · 身份、配对与安全

对应设计文档 §4.2、§4.7、§4.8、§08。本文定实现细节：密钥在哪、怎么配对、连接时怎么判、作废怎么做。

## 标识

| 名称 | 定义 | 例子 | 用在哪 |
|---|---|---|---|
| 指纹 `fp` | `SHA-256(SPKI DER)`，32 字节 | 界面：`7K2M 9QXA 4D1F 8ZPW` | TLS 校验、配对表主键 |
| `device_id` | `base32(fp[0..10])` 小写，16 字符 | `k3jq7x2m9p4tbw8a` | App 设备在 Host 里的 id |
| `host_id` | 同上，用 Host 证书的 fp | `h7v2…` | Host 在 App 里的 id |
| tailnet 节点 | WhoIs 返回的 `Node.StableID` 和名字 | `xl-iphone` | tailnet 路径的第二层校验 |
| 用户 | WhoIs 的 `UserProfile.LoginName` | `alice@` | 白名单、审计 |

`device_id` 只由公钥决定：同一台手机重装 App 会生成新密钥，因此是一台新设备，需要重新配对（与设计文档「作废不能撤销，找回后要重新配对」一致）。

App、host-ui 的指纹显示统一为：完整 SPKI SHA-256 的前 10 字节，用 Crockford base32（`0123456789ABCDEFGHJKMNPQRSTVWXYZ`）编码成 16 个大写字符，每 4 个一组用空格分隔。完整 32 字节仍用于鉴权，线上 fp 编码和小写 `device_id` / `host_id` 的算法不变；显示层统一实现待跟进。

## 密钥与证书

| 谁 | 密钥 | 存放 | 证书 |
|---|---|---|---|
| iOS App | P-256 设备密钥 | Secure Enclave，`kSecAttrTokenIDSecureEnclave`，访问控制 `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`，不加生物识别（Face ID 由 App 锁负责，避免每次握手弹 Face ID） | 首次启动用 `rcgen` 远程签名生成自签证书（10 年），DER 存 App 沙盒 |
| Android App | P-256 设备密钥 | AndroidKeyStore，`setIsStrongBoxBacked(true)`，不支持时退回 TEE；`setUserAuthenticationRequired(false)` | 同上 |
| Mac 电脑端（M3） | P-256 设备密钥 | Secure Enclave（CryptoKit `SecureEnclave.P256.Signing.PrivateKey`）；`dataRepresentation`（安全芯片加密的密钥句柄，换机无效）存数据目录 `device_key.se`（0600）。没有安全芯片的 Mac 退回软件 P-256，私钥存登录钥匙串（generic password，服务 `ai.gxlself.remotescreen.desktop`，账户 `device-key`） | 同 iOS App：首次启动远程签名生成自签证书（10 年），DER 存数据目录 `rs.db` |
| Windows 电脑端（M3） | P-256 设备密钥 | CNG `Microsoft Platform Crypto Provider`（TPM 2.0），按用户持久化、不可导出；没有 TPM 时退回 `Microsoft Software Key Storage Provider`（按用户，DPAPI 保护）；用的哪种写进数据目录 `device_key.json` | 同上 |
| iOS / Android | SSH 密钥（可选两种） | ① 安全芯片里的 P-256（`ecdsa-sha2-nistp256`，导不出）；② Ed25519 软件密钥，私钥用安全芯片里的密钥加密后存储 | — |
| Mac Host | P-256 主机密钥 | 登录钥匙串（`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`），启动时载入内存给 rustls | 自签，CN `h-<随机>`，20 年 |
| Windows Host | P-256 主机密钥 | `C:\ProgramData\RemoteHost\host.key`，DPAPI 机器级加密，ACL 只给 SYSTEM 和 Administrators | 同上 |
| 录制主密钥 | iOS 钥匙串 generic password（`ai.gxlself.remotescreen.recording` / `master-v1`，`AfterFirstUnlockThisDeviceOnly`）；Android Keystore AES-256-GCM（`rs.recording.master.v1`，有 StrongBox 时用 StrongBox） | 包裹每份录制的随机文件密钥；不导出，不进备份 | — |

**rustls 外部签名（App 侧）**：`rsp-transport` 定义

```rust
pub trait ExternalSigner: Send + Sync {
    /// 用设备私钥对 TLS 1.3 CertificateVerify 的待签数据做 ECDSA-P256-SHA256，返回 DER 编码签名。
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError>;
    fn spki_der(&self) -> Vec<u8>;
}
```

`rs-mobile` 通过 C ABI 把它桥接到原生：iOS `SecKeyCreateSignature(.ecdsaSignatureMessageX962SHA256)`，Android `Signature.getInstance("SHA256withECDSA")`。同一个签名器也给 SSH 用（`ecdsa-sha2-nistp256` 认证）。

## 连接时的鉴权

Host 在收到 `ClientHello` 后按下面的顺序判定，第一个命中的规则决定结果：

```
输入：本地监听地址、对端地址、对端证书 fp、ClientHello、配对表、信任日志、Host 策略

1  intent == PROBE                                    → 只回 ServerHello（精简），不判权限
2  intent == PAIR                                     → 进入配对流程（下一节）
3  路径 = 本地地址在 100.64.0.0/10 或 fd7a:115c:a1e0::/48 ? TAILNET : （来源在内网白名单网段 ? INTRANET/LAN : 丢弃）
4  fp 在作废名单                                      → DENIED 180「本机已被作废」
5  fp 不在配对表                                      → DENIED 181「这台手机还没和这台电脑配对」
6  TAILNET：WhoIs(对端地址)
     失败（tailscaled 不可用）                         → 按 INTRANET 规则处理，记警告
     用户不在 allowed_users                           → DENIED 130
     节点 StableID 既不是配对记录的节点，也不是该设备登记的第二节点（BIND_NODE） → 需要本机确认（记安全日志）
   INTRANET / LAN：
     Host 没打开内网连接，或来源网段不在名单            → 握手前已拒绝（监听层），见 06-host
     设备对该路径的权限 = deny                         → DENIED 131「这台手机不允许经内网连接」
7  Host 总开关「允许被远程控制」关                    → DENIED 132
8  会话已满（1 个控制者 + max_viewers 个观看者）        → DENIED 142
9  角色：设备权限 = view_only 或路径权限 = view_only 或 intent == VIEW → VIEWER
         已有控制者                                    → VIEWER（可 RequestControl）
         否则                                         → CONTROLLER
10 需要本机确认，任一成立：
     Host 设为「有人值守」
     设备权限 = CONTROL 且电脑前 60 秒内有人操作
     路径是 INTRANET / LAN 且「内网连接每次确认」开
     第 6 步标记了节点不符
   → PENDING_APPROVAL，30 秒
11 其他                                               → OK
```

### FILES 连接鉴权

`Intent FILES = 5` 用于文件页连接电脑浏览和传文件，不抢控制，也不进入「正在观看」。`host_core::auth::decide` 的规则如下（文件任务实现）：

1. 规则 1–7 与 CONTROL 相同。
2. 跳过规则 8 的观看人数上限。
3. 规则 9 换为：设备权限或路径权限为 `ViewOnly` → `PolicyDenied`；否则 `Role::Viewer`。FILES 永远是 Viewer，现有 `is_controller()` 挡住输入、剪贴板写入和会话动作。
4. 规则 10 照常计算审批原因，但不加 `LocalActivity`；有人在用电脑不影响传文件。
5. 可传文件的条件为 `ConnInfo.intent == Intent::Files && ConnInfo.can_control`；`can_control` 由 handshake 现有 `control_input` 推算。

FILES 不进 roster，不计 controller / viewer、不触发接管，不出现在 IPC `sessions` 和胶囊里。同一设备的新 FILES 连接以 `EndReason::Replaced` 替换旧 FILES 连接，不影响该设备的 CONTROL / VIEW 连接。`StartStream` 回 `ErrorInfo PolicyDenied`，不断开；`RequestControl` 忽略并记 debug。有传输时不断开；无传输且 10 分钟没有控制消息时发 `Goodbye IDLE_TIMEOUT`（`files::deadline` / `on_tick`）。

FILES 不写 `session_start` / `session_end`。每个传完的文件由 `audit_transfer` 按 [06 · 审计事件口径](06-host.md#审计事件) 写一条 `file`，`detail = {op, files, bytes}`，不记文件名。兼容握手必须要求 `FILE_BROWSE`，见 03 §11。

观看者上限来自 `[access] max_viewers`（默认 3）；名册已使用配置，鉴权入口仍固定为 3（实现待跟进）。控制台不属于当前登录用户时，在开始会话前以 `AuthResult(DENIED, error.code=190)` 拒绝；`ServerHello` 不增加控制台字段。普通锁屏不等于离开控制台；会话中转到登录窗口或其他用户时停止采集并发含错误码 190 的 `Goodbye`（控制台策略已有，握手和会话层接线实现待跟进，见 06）。

设备权限三档（配对时在 Host 上选，之后可在「已配对的设备」里改，画布 `HostDevices`、`HostConfirm`）：

| 档 | 含义 |
|---|---|
| 仅观看 | 永远是 VIEWER |
| 允许控制（默认） | 电脑前没人时直接连上；电脑前 60 秒内有人操作时先请对方确认 |
| 允许无人值守 | 永远直接连上（Host 设成「有人值守」时除外） |

不管哪一档，会话期间电脑上一直显示「正在被控制」胶囊（设计文档 §08 会话一行）。

### WhoIs 的实现

Host 通过 tailscaled 的 LocalAPI 查询：`GET /localapi/v0/whois?addr=<ip>:<port>`，返回 `Node`（含 `StableID`、`Name`、`Tags`）和 `UserProfile`（含 `LoginName`）。

| 平台 | LocalAPI 位置 |
|---|---|
| macOS，tailscaled 系统服务版（推荐，见 06-host） | Unix socket `/var/run/tailscaled.socket` |
| macOS，Tailscale App 版 | 本机 TCP 端口 + 令牌，读法参考 tailscale 源码 `safesocket/safesocket_darwin.go` |
| Windows | 命名管道 `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled` |
| 都不行 | 退回执行 `tailscale whois --json <ip>` |

结果按 IP 缓存 30 秒。路径是直连还是 DERP：查 `GET /localapi/v0/status` 里对应 peer 的 `CurAddr`（空则走中继）和 `Relay`（中继地域），写进 `PeerInfo` 和 `HostStats`。

## 配对

### 按地址 + 配对码

可直接拨号到用户提供的地址，不预设 Host 指纹，先接受 TLS 证书并取得 Host 的 SPKI 指纹，再用 Host 本机显示的六位配对码做现有的 SPAKE2。transcript 包含「SPAKE2 消息 ‖ device fp ‖ Host fp」，中间人替换证书会导致确认失败；配对码只显示在 Host 本机，是带外通道，所以不需要事先从别处拿到指纹。确认成功后才保存指纹与地址。使用电脑系统密码配对必须先确认 Host 指纹（密码会经 TLS 送到 Host）；原来的指纹来源是 Hub 目录，移除 Hub 后 App 没有这个入口，协议消息和 `PAIR_PASSWORD` 功能位保留。

### 二维码内容

```
rsp://pair?v=1&h=<host_id>&fp=<fp 的 base64url>&c=<6 位数字>&n=<主机名，URL 编码>&a=<地址1>,<地址2>
```

`a` 是 Host 当前可用的地址（tailnet 地址一定有；打开了内网连接才带内网地址）。二维码和 6 位码 5 分钟有效，一次性，Host 菜单里一次只有一个有效码。

### 流程

```
App                                                  Host（处于「显示配对二维码」状态）
 │ 拨号（03 §10.2）；扫码时校验 Host fp == 二维码 fp
 │── ClientHello(intent=PAIR) ─────────────────────►│  不查配对表；来源走内网时须打开「允许新设备经内网配对」
 │── PairStart(device_name, platform, has_qr) ─────►│
 │◄────────────────────────────── PairSpake(msg_H) ─│  SPAKE2：口令 = 6 位码，身份 A = "rsp-app:" + 设备 fp，B = "rsp-host:" + Host fp
 │── PairSpake(msg_A) ─────────────────────────────►│
 │       双方得到 K；K_confirm = HKDF(K, "rsp-pair-confirm")
 │── PairConfirm(HMAC(K_confirm, "A" ‖ transcript)) ►│  验证失败 → BAD_CODE，剩余次数 −1
 │◄──── PairConfirm(HMAC(K_confirm, "B" ‖ transcript))│  App 验证失败 → 判定为中间人，终止
 │◄──────────────────── PairResult(WAITING_LOCAL) ───│  Host 弹窗：「小明的 iPhone（alice@）请求配对」+ 选权限
 │◄──────────────────── PairResult(OK, host_id, name)│  30 秒内点允许；拒绝 → DENIED，超时同拒绝
```

- `transcript` = 双方 SPAKE2 消息 ‖ 设备 fp ‖ Host fp，绑定了 TLS 两端的身份，所以中间人换掉任何一端的证书都会让确认失败。
- 用 `spake2` crate（Ed25519 群）。
- 同一个码错 5 次立即作废，Host 菜单提示「配对码已失效」；所有来源 5 分钟内最多 20 次配对尝试。
- 成功后：Host 写配对表（记录 `paired_via`、WhoIs 用户和节点、选的权限）；App 保存 `{host_id, fp, name, 地址}`。
- 画布：`Pair`（扫码 / 输入码）、`PairWaiting`、`PairDone`、`HostPair`、`HostConfirm`。

### 其他配对方式（M2）

| 方式 | 做法 | 电脑前确认 |
|---|---|---|
| 电脑的系统账户密码 | Host 打开此功能后才可用。新设备经 TLS 发来账户名和密码，Host 用 macOS OpenDirectory / Windows `LogonUserW` 校验，通过即配对 | 不需要；成功后电脑上弹系统通知 |
| 配对文件 | Host 导出一次性 `.rspair` 文件（Host fp、地址、一次性口令，24 小时有效），IT 分发；App 导入后按二维码流程走，口令来自文件 | 不需要（导出文件就是同意） |

#### 系统账户密码

前提：App 已有这台 Host 的 fp（原来来自 Hub 目录；移除 Hub 后 App 没有入口，下面是协议与 Host 侧规则）；Host 的 `ServerHello.features` 含 `PAIR_PASSWORD`；Host 配置 `security.password_pairing` 打开。

```
App（Host fp 已固定）                                Host
 │── ClientHello(intent=PAIR) ─────────────────────►│  路径检查同 6 位码（内网须 allow_pairing）
 │◄──────────── ServerHello(features ∋ PAIR_PASSWORD)
 │── PairPassword(device_name, platform, account, password) ►│
 │      ① 路径不允许 → DISABLED_ON_PATH（握手阶段已有）
 │      ② 开关关 / 平台无校验器 → METHOD_DISABLED（不计失败）
 │      ③ 全局 5 分钟 20 次（与 6 位码共用）超限 → LOCKED, retry_after_s
 │      ④ 密码锁未到期 → LOCKED, retry_after_s
 │      ⑤ 长度不合规 / 账户不存在 / 不满足策略 / 密码错 → BAD_CODE, attempts_left（都算一次失败，回应不可区分）
 │      ⑥ 第 5 次失败 → LOCKED, retry_after_s = 900
 │      ⑦ 系统校验不可用（OpenDirectory 报错）→ DENIED（不计失败，写 warn）
 │      ⑧ 通过 → 写配对表（权限 = security.password_pairing_permission）→ OK
 │◄────────────────────────────── PairResult ──────│  失败时从收到 PairPassword 起至少 1 s 才回
```

| 项 | 规则 |
|---|---|
| 可用账户 | 管理员组成员，或当前控制台登录用户。先查账户再验密码；不满足也回 `BAD_CODE` 并计失败，不暴露账户是否存在 |
| 密码锁 | 连续失败 5 次锁 15 分钟；最后一次失败超过 15 分钟后计数清零；成功清零；锁到期清零。整机一把锁，不按账户分 |
| 锁状态 | `password_lock.json`（state_dir，0600）：`{"v":1,"failures":2,"last_failure_at_us":…,"locked_until_us":0}`，不含账户名；读坏了按「锁 15 分钟」处理 |
| 密码 | 只以 `Zeroizing<Vec<u8>>` 存在，校验后立即清零；不落盘、不进日志；`PairPassword` 的 `Debug` 打成 `<redacted>` |
| 账户名 | 失败时日志和审计不带（防止用户把密码填进账户栏）；成功时审计带短名 |
| 流程 | 不走 SPAKE2，不经 `WAITING_LOCAL`；成功后电脑上弹系统通知（`event.pairingResult`，`method = "password"`） |

#### 配对文件

文件内容：UTF-8 文本，一行 URI，末尾 `\n`，整个文件 ≤ 4096 字节；解析时允许去掉首尾空白（含 CRLF）。App 也接受同一 URI 作为深链或粘贴文本。

```
rsp://pairfile?v=1&h=<host_id>&fp=<fp>&f=<file_id>&s=<secret>&n=<主机名>&a=<地址1>,<地址2>&e=<过期时间>
```

| 键 | 内容 | 编码 / 限制 |
|---|---|---|
| `v` | 版本 | 固定 `1`，其他 → 版本错误 |
| `h` | host_id | 必须 = `id_from_fp(fp)` |
| `fp` | Host SPKI SHA-256 | base64url 无填充，32 字节（43 字符） |
| `f` | file_id | base64url 无填充，16 字节（22 字符） |
| `s` | 一次性口令 | base64url 无填充，32 字节（43 字符），Host 用 OS 随机数生成 |
| `n` | 主机名 | 百分号编码，解码后 ≤ 64 字符 |
| `a` | 地址 | 与二维码 `a` 同规则：1–8 个 IP 字面量，可带端口，逗号分隔 |
| `e` | 过期时间 | unix **秒**，十进制，= 导出时间 + 24 小时 |

- 生成时键按上表顺序；解析时不限顺序；重复键（含未知键）→ 错误；未知键忽略。与二维码规则一致。
- 文件名 `<主机名>-<yyyyMMdd-HHmm>.rspair`，时间取导出时刻的 UTC；主机名里的 `/ \ : * ? " < > |` 和控制字符换成 `-`，整体 ≤ 80 字符。MIME `application/x-rspair`；iOS UTI `ai.gxlself.remotescreen.pairfile`。
- SPAKE2 口令（字节顺序固定，共 64 字节）：`"rsp-pairfile-v1\0"`（16 字节）‖ file_id（16 字节）‖ secret（32 字节）。身份 A/B、`transcript`、`K_confirm` 与 6 位码相同。
- 流程：`PairStart` 带 `pair_file_id`，其余同[流程](#流程)；Host 确认成功后直接 `PairConfirm` + `PairResult(OK)`，不发 `WAITING_LOCAL`；权限取导出时选的档。

| Host 判定 | 结果 |
|---|---|
| 文件不存在 / 已用 / 已撤销 / 已过期 / 已作废，或 `pair_file_id` 不是 16 字节 | `EXPIRED` |
| SPAKE2 确认失败 | 该文件失败数 +1，`BAD_CODE`（`attempts_left` = 5 − 失败数）；第 5 次 → 文件作废，`EXPIRED` |
| 全局 5 分钟 20 次 | 照算，超限 `LOCKED` |

Host 存储 `pairfiles.json`（state_dir，0600，整文件原子替换）：

```json
{"v":1,"files":[{"file_id":"<b64url>","secret":"<b64url>","label":"IT 分发 10 月","permission":"control",
  "created_at_us":1792465134000000,"expires_at_us":1792551534000000,"state":"active","failures":0,
  "used_by":null,"used_at_us":null}]}
```

- `state`：`active` / `used` / `failed`（错 5 次）/ `revoked`（本机撤销）/ `expired`（读时按时间判）。
- 同时最多 50 个 `active`；终态记录保留 7 天后清掉。
- 口令存盘是有意的，与配对表同等保护；日志、IPC、审计里不出现 `secret`。

## 信任声明

信任声明是设备和 Host 之间签名的指令格式（作废、批准、要求确认、登记第二节点）。2026-10-24 移除 Hub 后没有下发来源：原来由 Hub 写信任日志并下发给各 Host，现在 Host 不会收到信任日志，也不会自己产生。格式、`rsp-core::trust` 的构造与验签、Host 的处理规则保留，以后接入新的来源时直接复用；只有设备签名的声明才会被执行。

Host 的 `AgentIdentity.user` 固定取本机 Tailscale 当前登录用户的登录名：LocalAPI `GET /localapi/v0/status` → `Self.UserID` → `User[UserID].LoginName`（Self 用户）。它与 `allowed_users` 为空时「只允许本机 Tailscale 登录的用户」采用同一个来源，不取系统账户名、tag 的所有者或远端连接用户。

文件 `proto/rsp/v1/trust.proto`：

```proto
message Statement {
  enum Kind { KIND_UNSPECIFIED = 0; REVOKE_DEVICE = 1; APPROVE_DEVICE = 2; REQUIRE_CONFIRM = 3; BIND_NODE = 4; }
  Kind   kind            = 1;
  string user            = 2;   // "alice@"
  bytes  subject_fp      = 3;   // 被作废 / 被批准 / 登记第二节点的设备
  bytes  subject_cert    = 4;   // 仅 APPROVE：新设备证书 DER
  string subject_name    = 5;
  uint64 issued_at_us    = 6;
  bytes  nonce           = 7;   // 16 字节随机
  string subject_node    = 8;   // 仅 BIND_NODE：设备第二个 tailnet 节点的 StableID
}
message SignedStatement {
  bytes statement  = 1;         // Statement 编码后的字节
  bytes signer_fp  = 2;         // 签名设备的 SPKI SHA-256 指纹，不能为空
  bytes signature  = 3;         // ECDSA-P256-SHA256，签 "rsp-trust-v1\0" ‖ statement
  reserved 4;                   // 原 hub_seq：序号改为 Host 本机日志的元数据，不上线
  reserved "hub_seq";
}
```

Host 处理规则：

| kind | 需要签名 | Host 动作 |
|---|---|---|
| REVOKE_DEVICE | 是，签名者必须是本机已配对、未作废、权限不低于「允许控制」的设备 | 从配对表删除；加入作废名单；断开它的会话（`Goodbye(REVOKED)`） |
| REQUIRE_CONFIRM | 是，签名者要求同上 | 该设备之后每次连接都要本机确认 |
| APPROVE_DEVICE | 是，签名者必须是本机已配对、未作废、权限不低于「允许控制」的设备 | 加入配对表，权限 = 签名者的档；字段要求见[APPROVE_DEVICE 字段](#approve_device-字段) |
| BIND_NODE | 是，签名者必须就是 subject 本身（signer_fp = subject_fp），且是本机已配对、未作废的设备（任何权限档） | 记下该设备的第二个节点 StableID；之后第 6 步比对时，配对记录的节点或这个节点都算相符；字段要求见[第二节点登记](#第二节点登记) |

按序号顺序执行。同一序列里，签名者在更早的序号上已被作废的，其声明无效。

日志序号由 Host 本机记录（不在线上传输，也不在签名范围内），必须连续，Host 用连续性判断是否漏了条目。`statement.user` 与本机用户比对只用于 REQUIRE_CONFIRM、APPROVE_DEVICE 和 BIND_NODE（不符则拒绝该条，日志继续）；REVOKE_DEVICE 不论属于哪个用户都执行：作废是全局安全事实，只增不减，对本机没配对过的 fp 也加入作废名单（防止该密钥以后再来配对）。

### APPROVE_DEVICE 字段

签名格式：ECDSA-P256-SHA256，DER，签 `"rsp-trust-v1\0"`（13 字节）‖ `Statement` 编码字节；`SignedStatement.statement` 存的就是被签的字节，不重新编码。

| 字段 | 要求 |
|---|---|
| `kind` | `APPROVE_DEVICE = 2` |
| `user` | 签名者的用户（WhoIs 登录名） |
| `subject_fp` | 32 字节，= SHA-256(新设备证书的 SPKI DER) |
| `subject_cert` | 新设备证书 DER；公钥只能是 P-256（未压缩点）或 Ed25519；DER 后面不能有多余字节 |
| `subject_name` | 去掉首尾空白后 1–64 个 Unicode 标量，不含控制字符 |
| `issued_at_us` | unix 微秒 |
| `nonce` | 16 字节随机 |
| `signer_fp` | 签名者（旧设备）SPKI SHA-256，32 字节，不能为空 |

- Host 规则同上表；另加一条：`subject_name` 不合规 → `InvalidStatement`。
- 「新设备连接前先问我」：签名者在本机「见过」名单里 → 新设备也记为见过，不弹；否则照常弹。
- rsp-core 公开 `trust::{valid_subject_name, subject_fingerprint, build_approve}`；向量 `crates/rsp-core/tests/vectors/trust/approve_flow.{json,bin}`。

### 第二节点登记

打开「App 内连接」（D42）后，同一台手机有两个 tailnet 节点：系统 Tailscale 的节点和 App 内节点，配对记录只记一个节点。设备可以用自己的密钥签一条 BIND_NODE 声明第二个节点，Host 照信任日志执行。原来的登记入口在 Hub，移除后 App 不再产生这类声明；没有声明时，用第二个节点连 Host 照常要本机确认（鉴权第 6 步）。格式和 Host 规则保留。

| 字段 | 要求 |
|---|---|
| `kind` | 4（BIND_NODE） |
| `user` | 设备所属用户 |
| `subject_fp` | 设备 SPKI SHA-256，32 字节；与 `signer_fp` 相同 |
| `subject_cert`、`subject_name` | 空 |
| `subject_node` | 第二个节点的 StableID，1–64 字节，只含 `A-Z a-z 0-9 . _ -` |
| `issued_at_us` | unix 微秒；Host 用它比较新旧 |
| `nonce` | 16 字节；与作废、批准共用防重放 |

Host 执行：
- 签名者是本机已配对、未作废的设备（同一批里更早序号已作废的不算；同一批里更早序号刚批准的，用批准里的证书验签）；`user` = 本机用户；验签通过；`subject_node` 合规、`subject_cert` 为空。不满足就拒绝这一条，日志继续。
- 每台设备只记一个第二节点，存 `bound_nodes.json`。`issued_at_us` 比已记的新才替换；相同且节点相同算重复，不报错；其他情况拒绝（`stale_binding`）。
- 设备被作废或从配对表删除时，它的第二节点一并删除。

已知限制：某台 Host 在声明发出**之后**才和这台设备配对，处理那条日志时还不认识签名者，会拒绝（`unknown_signer`）。

## 丢了手机：作废

对应设计文档 §8.1、画布 `LostDevice`。没有中心服务后，作废只在每台电脑本机做，不跨 Host：

1. 在每台电脑的 Remote Host 菜单「已配对的设备」里作废这台手机（画布 `HostDevices`）：本机立即删除配对、加入作废名单、断开它当前的会话（`Goodbye(REVOKED)`）。
2. 手机的 tailnet 节点由用户在 Headscale / Tailscale 管理端自行删除。
3. 服务器 `authorized_keys` 里带 `rs-device=<device_id>` 标记的公钥行由用户自行删除（App 不再保存清理用的维护密钥）。

App 没有作废别的设备的入口。

### 被作废的设备

- 再连任何 Host：`DENIED 180`，App 显示「本机已被作废」（设计文档 §9.7 的错误表）+「重新配对」。
- 重新配对 = App 删除旧设备密钥、生成新密钥，按首次配对走。

## 数据存放一览

| 数据 | 位置 | 保护 |
|---|---|---|
| App 设备私钥 | 安全芯片 | 不可导出 |
| App 设备证书、已配对 Host 列表、known_hosts | App 沙盒 `Application Support/RemoteScreen/core`，Rust 管理的 SQLite `rs.db` | iOS 数据保护 `CompleteUntilFirstUserAuthentication`，目录不进 iCloud / iTunes 备份（设备私钥在安全芯片里，恢复到新手机也用不了）；Android 应用私有目录 |
| App 设置 | MMKV | 不含秘密 |
| 电脑端设备证书、已配对 Host、日志、崩溃报告 | Mac `~/Library/Application Support/ai.gxlself.remotescreen.desktop/core/`；Windows `%LOCALAPPDATA%\ai.gxlself.remotescreen.desktop\core\`；结构同 App 的 RsCore 数据目录 | 0600 / 用户目录 ACL；不放漫游目录 |
| SSH 软件私钥、服务器密码（用户选择保存时） | iOS 钥匙串 / Android Keystore 加密的文件 | `ThisDeviceOnly`；内置 VNC / RDP 凭据：iOS 钥匙串 generic password（service `ai.gxlself.remotescreen.adapter`，account = `<协议>:<hostId 或 地址:端口>`）；Android Keystore AES-GCM 密钥加密后存 `files/adapter-secrets/`。RDP 证书指纹不是秘密，存 SQLite kv `adapter.cert.<同一键>`。iOS 删 App 不删钥匙串，所以重装后第一次启动按 service 清掉旧条目（RS-V4Y） |
| App 日志 | 沙盒 `logs/`，保留 7 天 | 导出时默认打码 |
| App 原生崩溃收件箱 | RsCore 数据目录的 `crashes/native/`；iOS 在 `Application Support/RemoteScreen/core/crashes/native/`，Android 在 `noBackupFilesDir/RemoteScreen/core/crashes/native/` | 每个采集器在写入新诊断后把收件箱裁到最近 10 个 `.json`。文件为 `{"v":1,"incidentAt":<unix 微秒>,"appVersion":…,"osVersion":…,"name":…,"message":…,"stack":…,"thread":…}`；iOS 文件名 `<incident>-metrickit-<index>.json`，Android 为 `<incident>-exitinfo-<pid>.json`。采集端只做字段提取和长度上限，不做 URL、路径、IP 脱敏；Core 导入时才按崩溃报告规则脱敏。Android 只选 7 天内的异常退出、每次最多 5 条。Core 启动时（`Core::new`，`init_logging` 默认开）按文件名取前 20 个导入；导入成功即删源文件，坏文件、超过 256 KiB 或超过 7 天的文件也删，写入失败的文件留到下次启动重试 |
| App 崩溃报告 | RsCore 数据目录的 `crashes/`（原生收件箱导入后也落到这里），最多 10 份，超过 7 天清理 | JS 崩溃和 Rust panic 在落盘前、原生崩溃在 Core 导入时对 `message`、`name`、`stack`、`osVersion`（以及 Rust panic 的 `location`、`thread`，原生崩溃的 `thread`、`appVersion`）脱敏：URL、主目录路径、引号文本、IP 地址替换或掩码，去控制字符并截断；报告只留在本机，不上传 |
| Host 主机私钥 | Mac 钥匙串 / Windows DPAPI 文件 | 见上表 |
| Host 配对表、第二节点 `bound_nodes.json`、信任日志、设置 | Mac `~/Library/Application Support/Remote Host/`（0600）/ Windows `C:\ProgramData\RemoteHost\`（ACL） | 文件权限 |
| Host 配对文件 `pairfiles.json`（含一次性口令） | 同上 state_dir | 0600，与配对表同等保护；口令不进日志、IPC、审计 |
| Host 密码锁 `password_lock.json` | 同上 state_dir | 0600；只有失败次数和时间，不含账户名和密码 |
| Host 日志 | Mac `~/Library/Logs/Remote Host/` / Windows `C:\ProgramData\RemoteHost\logs\`，保留 14 天 | 不含输入内容 |
| `<dataDir>/recordings/` | 加密的会话录制 | 按 `recording.retentionDays` 清理 |

### 内置 VNC / RDP 的传输安全

VNC 会话本身不加密（ARD 认证只加密凭据），所以入口优先用 tailnet 地址；不经 tailnet 时页面提示连接未加密。RDP 使用 TLS + NLA，证书首次使用即信任，指纹变化要用户确认。

## App 锁

画布 `AppLock`、`Onboard3`。

| 场景 | 默认 | 实现 |
|---|---|---|
| 打开 App / 从后台回来超过 5 分钟 | 开 | iOS `LAContext.evaluatePolicy(.deviceOwnerAuthentication)`；Android `BiometricPrompt`（允许设备密码兜底） |
| 进入远控前 | 关 | 同上 |
| 连接标记为「敏感」的主机 | 开（对被标记的） | 同上 |
| 查看或导出 SSH 私钥 | 始终 | 同上，不受设置影响 |

## 威胁与对策核对表

每条都要有对应的测试（[09](09-quality-release.md#安全测试)）。

| 威胁（设计文档 §08） | 对策 | 实现任务 |
|---|---|---|
| 公网扫描、同一局域网里的其他设备 | 默认只监听 tailnet 地址；ACL 只放行 7400 给指定用户组 | H11、运维 |
| 公司内网里的其他人扫到端口 | 内网监听默认关；限定网卡和来源网段；未配对设备握手即拒；不回识别信息 | H11、A8 |
| 冒充手机 | 设备密钥在安全芯片 + 双向证书；tailnet 路径再叠 WhoIs | A8、H3、P2 |
| DHCP 换址后连错电脑 | 指纹不符一律拦下（`RS-E150`） | A6、A8 |
| tailnet 里其他成员误连或越权 | WhoIs 用户白名单；按设备权限三档 | H3、H5 |
| Headscale 被攻破后伪造节点 | 指纹固定；信任声明里加权限需设备签名（D9） | A8、H2 |
| 被控端的人不知情 | 「正在被控制」胶囊常驻、一键断开；本机操作时暂停远端输入 | H6 |
| 被控端的人不知道自己被录制 | 录制时告知 Host，胶囊显示「录制中」，并写审计（方案 A） | M3 V5 |
| 离开后屏幕敞开 | 空闲超时；会话结束可自动锁屏 | H6 |
| 手机丢失 | App 锁；在每台电脑本机作废（`DENIED 180`） | P9 |
| 配对码被在线猜测 | SPAKE2；错 5 次作废；全局限速 | H4 |
| 更新包被替换 | Ed25519 + 平台代码签名双重校验（D13） | H16 |
| 日志泄露 | 不记录输入内容；导出时地址打码 | 全部 |
