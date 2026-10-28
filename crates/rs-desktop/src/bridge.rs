//! RsCore 门面（07 · 电脑端）：构造 Core、注入签名器、事件转发、外壳 JSON 调用白名单。
//!
//! 前端走 App 的 `createRsCore`：方法名、参数顺序同 `RsMobileBindings`，返回值是 rs-mobile
//! 现成的 JSON 字符串包装函数的结果，错误形状同 `CoreErrorLike`。

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Instant,
};

use rs_mobile::{
    rscore::{
        api_diag, api_identity, api_pairing, api_sessions,
        dto_core::{CoreConfig, DevicePlatform},
        error::CoreError,
        runtime::{self, Core, CoreOptions},
    },
    uniffi_api::EventSink,
};
use rsp_transport::identity::ExternalSigner;
use serde_json::Value;

/// `EventForward::typed` 的参数；外壳 crate 不直接依赖 rs-mobile，实现 `EventForward` 时从这里引用。
pub use rs_mobile::rscore::dto_core::CoreEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopConfig {
    pub data_dir: PathBuf,
    pub app_version: String,
    pub platform: DevicePlatform,
}

/// Windows 目标 → Windows，其他 → Macos。
pub fn default_platform() -> DevicePlatform {
    if cfg!(windows) {
        DevicePlatform::Windows
    } else {
        DevicePlatform::Macos
    }
}

/// `<app_local_data_dir>/core`。
pub fn core_dir(app_local_data: &Path) -> PathBuf {
    app_local_data.join("core")
}

/// RsCore 事件的去处。回调在 RsCore 的 `rs-events` 线程上按序执行，实现不得阻塞
/// （转给外壳事件或别的线程后立即返回），否则后面所有事件都会等它。
pub trait EventForward: Send + Sync + 'static {
    /// 原始 JSON，原样转给前端。
    fn raw(&self, seq: u64, payload: &str);
    /// 解析后的事件；解析失败的不调用。
    fn typed(&self, event: &CoreEvent);
}

/// 序列化后与 App `CoreErrorLike` 相同。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallError {
    pub tag: String,
    pub inner: CallErrorInner,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallErrorInner {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_json: Option<String>,
}

impl CallError {
    fn new(tag: &str, message: String) -> Self {
        Self {
            tag: tag.to_owned(),
            inner: CallErrorInner {
                message,
                status: None,
                hub_code: None,
                current_json: None,
            },
        }
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::new("Unsupported", msg.into())
    }

    /// 消息里只有方法名和原因，不带参数值。
    pub fn bad_args(name: &str, why: &str) -> Self {
        Self::new("Internal", format!("bad args for {name}: {why}"))
    }
}

impl From<CoreError> for CallError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Network { message } => Self::new("Network", message),
            CoreError::Http {
                status,
                hub_code,
                message,
            } => {
                let mut error = Self::new("Http", message);
                error.inner.status = Some(status);
                error.inner.hub_code = hub_code;
                error
            }
            CoreError::Conflict {
                message,
                current_json,
            } => {
                let mut error = Self::new("Conflict", message);
                error.inner.current_json = current_json;
                error
            }
            CoreError::Unsupported { message } => Self::new("Unsupported", message),
            CoreError::Cancelled { message } => Self::new("Cancelled", message),
            CoreError::Internal { message } => Self::new("Internal", message),
        }
    }
}

/// 前端可调的方法名（`RsMobileBindings` 的子集）。会话只给主窗口用的管理类调用：
/// 开会话走外壳命令，按键走原生窗口。
pub const CALLS: &[&str] = &[
    "coreInit",
    "coreShutdown",
    "identityEnsure",
    "pairingParseQr",
    "pairingStart",
    "pairingLoadFile",
    "pairingCancel",
    "hostsPaired",
    "hostsForget",
    "sessionsPrewarm",
    "sessionsList",
    "sessionsClose",
    "sessionsSetQuality",
    "sessionsSelectDisplay",
    "sessionsRequestControl",
    "sessionsRespondControl",
    "sessionsAction",
    "sessionsCombo",
    "diagProbe",
    "diagProbeAddress",
    "networkStatus",
    "settingsSet",
    "logsExport",
    "logsWrite",
];

/// 电脑端唯一的 RsCore 入口。
pub struct Bridge {
    core: Arc<Core>,
    subscriptions: Mutex<Vec<u64>>,
}

impl Bridge {
    /// `Core::new` → `set_signer(signer)` → `install_core`。data_dir 不存在就建（0700）。
    /// 已有装好的 core 时报错，不替换。
    pub fn start(
        cfg: DesktopConfig,
        signer: Option<Arc<dyn ExternalSigner>>,
        opts: CoreOptions,
    ) -> Result<Arc<Bridge>, CallError> {
        if runtime::core().is_ok() {
            return Err(CoreError::internal("core already started").into());
        }
        create_private_dir(&cfg.data_dir).map_err(CoreError::from)?;
        let data_dir = cfg
            .data_dir
            .to_str()
            .ok_or_else(|| CoreError::internal("data dir is not valid UTF-8"))?
            .to_owned();
        let core = Core::new(
            CoreConfig {
                files_dir: None,
                data_dir,
                app_version: cfg.app_version,
                platform: cfg.platform,
            },
            opts,
        )?;
        core.set_signer(signer);
        runtime::install_core(core.clone());
        Ok(Arc::new(Bridge {
            core,
            subscriptions: Mutex::new(Vec::new()),
        }))
    }

    pub fn core(&self) -> Arc<Core> {
        self.core.clone()
    }

    pub fn subscribe(&self, fwd: Arc<dyn EventForward>) -> u64 {
        let id = self.core.events().subscribe(Arc::new(Forwarder {
            fwd,
            unparsed: Mutex::new(HashSet::new()),
        }));
        self.subscriptions().push(id);
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        let mut subscriptions = self.subscriptions();
        let Some(index) = subscriptions.iter().position(|&own| own == id) else {
            return;
        };
        subscriptions.swap_remove(index);
        drop(subscriptions);
        self.core.events().unsubscribe(id);
    }

    /// 见 [`call`]。
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, CallError> {
        call(name, args).await
    }

    /// 退订、`Core::shutdown`、`take_core`。可重复调用。
    pub fn shutdown(&self) {
        let ids = std::mem::take(&mut *self.subscriptions());
        for id in ids {
            self.core.events().unsubscribe(id);
        }
        self.core.shutdown();
        if runtime::core().is_ok_and(|installed| Arc::ptr_eq(&installed, &self.core)) {
            drop(runtime::take_core());
        }
    }

    fn subscriptions(&self) -> MutexGuard<'_, Vec<u64>> {
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

struct Forwarder {
    fwd: Arc<dyn EventForward>,
    /// 解析失败过的事件 `type`，每种只 warn 一次。
    unparsed: Mutex<HashSet<String>>,
}

impl EventSink for Forwarder {
    fn on_event(&self, seq: u64, payload: String) {
        self.fwd.raw(seq, &payload);
        match serde_json::from_str::<CoreEvent>(&payload) {
            Ok(event) => self.fwd.typed(&event),
            Err(_) => {
                let kind = serde_json::from_str::<Value>(&payload)
                    .ok()
                    .and_then(|event| event.get("type")?.as_str().map(str::to_owned))
                    .unwrap_or_default();
                // 解析错误的文字可能带事件里的值，不记。
                let first = self
                    .unparsed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(kind.clone());
                if first {
                    tracing::warn!(event_type = %kind, "RsCore event not parsed; raw only");
                }
            }
        }
    }
}

/// 按 `RsMobileBindings` 的方法名调 rs-mobile 的 JSON 字符串包装函数（作用于已装好的 core）。
///
/// `args` 是按 `RsMobileBindings` 参数顺序的 JSON 数组：整数可以是 JSON 数字或十进制字符串
/// （前端的 bigint 转成字符串传），`Option<T>` 参数为 null 或缺省时是 None。
/// 返回：String 结果 → JSON 字符串值（不解析，前端自己 parse）；u64 → 十进制字符串；
/// 无返回值 → null；Option<String> → 字符串或 null。
/// 日志只记方法名、耗时和是否成功，不记参数和返回值。
pub async fn call(name: &str, args: Value) -> Result<Value, CallError> {
    let started = Instant::now();
    let result = dispatch(name, args).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if result.is_ok() {
        tracing::debug!(call = name, elapsed_ms, ok = true, "rscore call");
    } else {
        tracing::info!(call = name, elapsed_ms, ok = false, "rscore call");
    }
    result
}

async fn dispatch(name: &str, args: Value) -> Result<Value, CallError> {
    if !CALLS.contains(&name) {
        return Err(not_available(name));
    }
    let mut a = Args::new(name, args)?;
    match name {
        // Core 由外壳启动和关闭。
        "coreInit" => {
            a.text()?;
            Ok(Value::Null)
        }
        "coreShutdown" => {
            a.none()?;
            Ok(Value::Null)
        }

        "identityEnsure" => reply(api_identity::identity_ensure(a.text()?).await),

        "pairingParseQr" => Ok(api_pairing::pairing_parse_qr(a.text()?).into_value()),
        "pairingStart" => reply(api_pairing::pairing_start(a.text()?).await),
        "pairingLoadFile" => reply(api_pairing::pairing_load_file(a.text()?).await),
        "pairingCancel" => {
            a.none()?;
            api_pairing::pairing_cancel();
            Ok(Value::Null)
        }
        "hostsPaired" => {
            a.none()?;
            reply(api_pairing::hosts_paired().await)
        }
        "hostsForget" => reply(api_pairing::hosts_forget(a.text()?).await),

        "sessionsPrewarm" => {
            a.count(1, 1)?;
            reply(api_sessions::sessions_prewarm(a.strings(0)?))
        }
        "sessionsList" => {
            a.none()?;
            reply(api_sessions::sessions_list())
        }
        "sessionsClose" => {
            a.count(1, 1)?;
            reply(api_sessions::sessions_close(a.uint(0)?))
        }
        "sessionsSetQuality" => {
            a.count(2, 2)?;
            reply(api_sessions::sessions_set_quality(a.uint(0)?, a.string(1)?))
        }
        "sessionsSelectDisplay" => {
            a.count(2, 2)?;
            reply(api_sessions::sessions_select_display(a.uint(0)?, a.u32(1)?))
        }
        "sessionsRequestControl" => {
            a.count(1, 1)?;
            reply(api_sessions::sessions_request_control(a.uint(0)?))
        }
        "sessionsRespondControl" => {
            a.count(3, 3)?;
            reply(api_sessions::sessions_respond_control(
                a.uint(0)?,
                a.string(1)?,
                a.bool(2)?,
            ))
        }
        "sessionsAction" => {
            a.count(2, 3)?;
            reply(api_sessions::sessions_action(
                a.uint(0)?,
                a.string(1)?,
                a.opt_bool(2)?,
            ))
        }
        "sessionsCombo" => {
            a.count(2, 2)?;
            reply(api_sessions::sessions_combo(a.uint(0)?, a.string(1)?))
        }

        "diagProbe" => reply(api_diag::diag_probe(a.text()?).await),
        "diagProbeAddress" => {
            a.count(1, 2)?;
            reply(api_diag::diag_probe_address(a.string(0)?, a.opt_u16(1)?).await)
        }
        "networkStatus" => {
            a.none()?;
            reply(api_diag::network_status().await)
        }
        "settingsSet" => {
            a.count(2, 2)?;
            reply(api_diag::settings_set(a.string(0)?, a.string(1)?))
        }
        "logsExport" => {
            a.count(1, 1)?;
            reply(runtime::logs_export(a.bool(0)?).await)
        }
        "logsWrite" => {
            a.count(3, 3)?;
            runtime::logs_write(a.string(0)?, a.string(1)?, a.string(2)?);
            Ok(Value::Null)
        }

        _ => Err(not_available(name)),
    }
}

fn not_available(name: &str) -> CallError {
    CallError::unsupported(format!("not available on desktop: {name}"))
}

trait IntoValue {
    fn into_value(self) -> Value;
}

impl IntoValue for () {
    fn into_value(self) -> Value {
        Value::Null
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl IntoValue for Option<String> {
    fn into_value(self) -> Value {
        self.map_or(Value::Null, Value::String)
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }
}

fn reply<T: IntoValue>(result: Result<T, CoreError>) -> Result<Value, CallError> {
    Ok(result?.into_value())
}

/// 按位置取参数。取出的值被移走（口令只在参数数组里留一份）。
struct Args<'a> {
    name: &'a str,
    values: Vec<Value>,
}

impl<'a> Args<'a> {
    fn new(name: &'a str, args: Value) -> Result<Self, CallError> {
        match args {
            Value::Array(values) => Ok(Self { name, values }),
            _ => Err(CallError::bad_args(name, "args must be an array")),
        }
    }

    fn count(&self, min: usize, max: usize) -> Result<(), CallError> {
        let got = self.values.len();
        if (min..=max).contains(&got) {
            return Ok(());
        }
        let want = if min == max {
            min.to_string()
        } else {
            format!("{min} to {max}")
        };
        Err(CallError::bad_args(
            self.name,
            &format!("expected {want} arguments, got {got}"),
        ))
    }

    fn none(&self) -> Result<(), CallError> {
        self.count(0, 0)
    }

    /// 只有一个字符串参数。
    fn text(&mut self) -> Result<String, CallError> {
        self.count(1, 1)?;
        self.string(0)
    }

    fn bad(&self, index: usize, want: &str) -> CallError {
        CallError::bad_args(self.name, &format!("args[{index}] must be {want}"))
    }

    fn take(&mut self, index: usize) -> Value {
        self.values
            .get_mut(index)
            .map(Value::take)
            .unwrap_or(Value::Null)
    }

    fn string(&mut self, index: usize) -> Result<String, CallError> {
        match self.take(index) {
            Value::String(value) => Ok(value),
            _ => Err(self.bad(index, "a string")),
        }
    }

    fn strings(&mut self, index: usize) -> Result<Vec<String>, CallError> {
        let items = match self.take(index) {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(value) => Some(value),
                    _ => None,
                })
                .collect(),
            _ => None,
        };
        items.ok_or_else(|| self.bad(index, "an array of strings"))
    }

    fn bool(&mut self, index: usize) -> Result<bool, CallError> {
        match self.take(index) {
            Value::Bool(value) => Ok(value),
            _ => Err(self.bad(index, "a boolean")),
        }
    }

    fn opt_bool(&mut self, index: usize) -> Result<Option<bool>, CallError> {
        match self.take(index) {
            Value::Null => Ok(None),
            Value::Bool(value) => Ok(Some(value)),
            _ => Err(self.bad(index, "a boolean or null")),
        }
    }

    /// JSON 数字或十进制字符串。
    fn uint(&mut self, index: usize) -> Result<u64, CallError> {
        let parsed = match self.take(index) {
            Value::Number(number) => number.as_u64(),
            Value::String(text) if is_decimal(&text) => text.parse().ok(),
            _ => None,
        };
        parsed.ok_or_else(|| self.bad(index, "an unsigned integer"))
    }

    fn u32(&mut self, index: usize) -> Result<u32, CallError> {
        u32::try_from(self.uint(index)?).map_err(|_| self.bad(index, "a 32-bit unsigned integer"))
    }

    fn opt_u16(&mut self, index: usize) -> Result<Option<u16>, CallError> {
        if matches!(self.values.get(index), None | Some(Value::Null)) {
            return Ok(None);
        }
        u16::try_from(self.uint(index)?)
            .map(Some)
            .map_err(|_| self.bad(index, "a 16-bit unsigned integer or null"))
    }
}

fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}
