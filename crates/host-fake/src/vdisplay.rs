//! 假虚拟显示器后端（M3 · V1）：只改 FakePlatform 的显示器列表，测试可注入错误、卡住或延迟出现。
//!
//! `FakeVirtualDisplays` 持有 `FakePlatform`，而 `FakePlatform` 的状态又持有这个后端，形成
//! 引用循环。它只用于测试，进程退出时由操作系统回收，可以接受。

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};

use host_core::{
    platform::{DisplayInfo, Platform},
    vdisplay::{VdCaps, VdCreated, VdError, VdMode, VirtualDisplayBackend},
};

use crate::FakePlatform;

/// 平台显示器 id = PLATFORM_ID_BASE + token。
pub const PLATFORM_ID_BASE: u32 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appear {
    /// create 返回前就加进显示器列表（Mac 的行为）。
    Immediately,
    /// create 返回后不加，等测试调 show_pending()。
    Manual,
    /// 永远不出现（用于测出现超时）。
    Never,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FakeVdOptions {
    /// None：caps() 返回 None，表示「不支持」。
    pub caps: Option<VdCaps>,
    /// true：VdCreated 只给 match_name（"RS Virtual NN"），不给 platform_id，模拟 Windows。
    pub identify_by_name: bool,
    pub appear: Appear,
    /// cleanup_stale() 的返回值。
    pub stale: usize,
}

impl Default for FakeVdOptions {
    fn default() -> Self {
        Self {
            caps: Some(mac_like_caps()),
            identify_by_name: false,
            appear: Appear::Immediately,
            stale: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VdCall {
    Caps,
    Create { mode: VdMode, label: String },
    Destroy { token: u64 },
    CleanupStale,
}

pub fn mac_like_caps() -> VdCaps {
    VdCaps {
        max_long_px: 7680,
        max_short_px: 4320,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        scales: vec![1.0, 2.0],
        align_px: 4,
        max_displays: 2,
    }
}

struct Screen {
    display: DisplayInfo,
    appeared: bool,
    vanished: bool,
}

struct Inner {
    next_token: u64,
    fail_next: Option<VdError>,
    hold_creates: bool,
    waiting_creates: usize,
    live: BTreeMap<u64, Screen>,
    destroyed: Vec<u64>,
    calls: Vec<VdCall>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            next_token: 1,
            fail_next: None,
            hold_creates: false,
            waiting_creates: 0,
            live: BTreeMap::new(),
            destroyed: Vec::new(),
            calls: Vec::new(),
        }
    }
}

pub struct FakeVirtualDisplays {
    platform: FakePlatform,
    opts: FakeVdOptions,
    inner: Mutex<Inner>,
    creates_released: Condvar,
}

impl FakeVirtualDisplays {
    pub fn new(platform: FakePlatform, opts: FakeVdOptions) -> Arc<Self> {
        Arc::new(Self {
            platform,
            opts,
            inner: Mutex::new(Inner::default()),
            creates_released: Condvar::new(),
        })
    }

    /// 便捷：new() 后立即 platform.set_virtual_displays(Some(..))。必须在 Host 启动前调用。
    pub fn install(platform: &FakePlatform, opts: FakeVdOptions) -> Arc<Self> {
        let backend = Self::new(platform.clone(), opts);
        platform.set_virtual_displays(Some(backend.clone()));
        backend
    }

    /// 下一次 create 返回这个错误（只生效一次）。
    pub fn fail_next(&self, err: VdError) {
        self.lock_inner().fail_next = Some(err);
    }

    /// true：之后的 create 卡住，直到 release_creates()。
    pub fn hold_creates(&self, on: bool) {
        self.lock_inner().hold_creates = on;
        if !on {
            self.creates_released.notify_all();
        }
    }

    /// 放行所有卡住的 create（并关掉 hold）。
    pub fn release_creates(&self) {
        self.lock_inner().hold_creates = false;
        self.creates_released.notify_all();
    }

    /// 当前卡在 create 里的调用数（测试用它等「已进入 create」）。
    pub fn waiting_creates(&self) -> usize {
        self.lock_inner().waiting_creates
    }

    /// Appear::Manual 下把所有已建但未出现的屏加进列表。
    pub fn show_pending(&self) {
        if self.opts.appear != Appear::Manual {
            return;
        }

        let pending = {
            let mut inner = self.lock_inner();
            let mut pending = Vec::new();
            for screen in inner.live.values_mut() {
                if !screen.appeared && !screen.vanished {
                    screen.appeared = true;
                    pending.push(screen.display.clone());
                }
            }
            pending
        };

        if pending.is_empty() {
            return;
        }
        let mut displays = self.platform.displays();
        for display in pending {
            push_unique(&mut displays, display);
        }
        self.platform.set_displays(displays);
    }

    /// 从列表里拿掉这块屏，但不算 destroy（模拟意外消失）。
    pub fn vanish(&self, token: u64) {
        let display = {
            let mut inner = self.lock_inner();
            let Some(screen) = inner.live.get_mut(&token) else {
                return;
            };
            screen.vanished = true;
            screen.display.clone()
        };

        let mut displays = self.platform.displays();
        remove_id(&mut displays, display.id);
        self.platform.set_displays(displays);
    }

    /// 把 vanish 掉的屏加回去。
    pub fn reappear(&self, token: u64) {
        let display = {
            let mut inner = self.lock_inner();
            let Some(screen) = inner.live.get_mut(&token) else {
                return;
            };
            if !screen.vanished {
                return;
            }
            screen.vanished = false;
            screen.appeared = true;
            screen.display.clone()
        };

        let mut displays = self.platform.displays();
        push_unique(&mut displays, display);
        self.platform.set_displays(displays);
    }

    /// 还没被 destroy 的 token，升序。
    pub fn live(&self) -> Vec<u64> {
        self.lock_inner().live.keys().copied().collect()
    }

    pub fn calls(&self) -> Vec<VdCall> {
        self.lock_inner().calls.clone()
    }

    pub fn create_count(&self) -> usize {
        self.lock_inner()
            .calls
            .iter()
            .filter(|call| matches!(call, VdCall::Create { .. }))
            .count()
    }

    pub fn destroyed(&self) -> Vec<u64> {
        self.lock_inner().destroyed.clone()
    }

    pub fn display_of(&self, token: u64) -> Option<DisplayInfo> {
        self.lock_inner()
            .live
            .get(&token)
            .map(|screen| screen.display.clone())
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn finish_create(&self, mode: VdMode, label: &str) -> Result<VdCreated, VdError> {
        let (token, platform_id, display, appear_now) = {
            let mut inner = self.lock_inner();
            let Some(caps) = &self.opts.caps else {
                return Err(VdError::Unsupported("fake: no caps".into()));
            };
            if inner.live.len() >= caps.max_displays as usize {
                return Err(VdError::Limit);
            }
            let token = inner.next_token;
            inner.next_token = inner.next_token.saturating_add(1);

            let platform_id = PLATFORM_ID_BASE + token as u32;
            let name = if self.opts.identify_by_name {
                format!("RS Virtual {token:02}")
            } else {
                label.to_owned()
            };
            let display = DisplayInfo {
                id: platform_id,
                name,
                width_px: mode.width_px,
                height_px: mode.height_px,
                scale: mode.scale,
                x: 10_000 * token as i32,
                y: 0,
                refresh_hz: mode.refresh_hz,
                primary: false,
                rotation: 0,
            };
            let appear_now = self.opts.appear == Appear::Immediately;
            inner.live.insert(
                token,
                Screen {
                    display: display.clone(),
                    appeared: appear_now,
                    vanished: false,
                },
            );
            (token, platform_id, display, appear_now)
        };

        if appear_now {
            let mut displays = self.platform.displays();
            push_unique(&mut displays, display);
            self.platform.set_displays(displays);
        }

        Ok(VdCreated {
            token,
            platform_id: (!self.opts.identify_by_name).then_some(platform_id),
            match_name: self
                .opts
                .identify_by_name
                .then(|| format!("RS Virtual {token:02}")),
            mode,
        })
    }
}

impl VirtualDisplayBackend for FakeVirtualDisplays {
    fn caps(&self) -> Option<VdCaps> {
        self.lock_inner().calls.push(VdCall::Caps);
        self.opts.caps.clone()
    }

    fn create(&self, mode: VdMode, label: &str) -> Result<VdCreated, VdError> {
        {
            let mut inner = self.lock_inner();
            inner.calls.push(VdCall::Create {
                mode,
                label: label.to_owned(),
            });
            if let Some(error) = inner.fail_next.take() {
                return Err(error);
            }

            let Some(caps) = &self.opts.caps else {
                return Err(VdError::Unsupported("fake: no caps".into()));
            };
            if inner.live.len() >= caps.max_displays as usize {
                return Err(VdError::Limit);
            }

            inner.waiting_creates += 1;
            while inner.hold_creates {
                inner = self
                    .creates_released
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            inner.waiting_creates -= 1;
        }

        self.finish_create(mode, label)
    }

    fn destroy(&self, token: u64) -> Result<(), VdError> {
        self.lock_inner().calls.push(VdCall::Destroy { token });

        let display = {
            let mut inner = self.lock_inner();
            let Some(screen) = inner.live.remove(&token) else {
                return Ok(());
            };
            inner.destroyed.push(token);
            screen.display
        };

        let mut displays = self.platform.displays();
        remove_id(&mut displays, display.id);
        self.platform.set_displays(displays);
        Ok(())
    }

    fn cleanup_stale(&self) -> usize {
        self.lock_inner().calls.push(VdCall::CleanupStale);
        self.opts.stale
    }
}

fn push_unique(displays: &mut Vec<DisplayInfo>, display: DisplayInfo) {
    if !displays.iter().any(|existing| existing.id == display.id) {
        displays.push(display);
    }
}

fn remove_id(displays: &mut Vec<DisplayInfo>, id: u32) {
    displays.retain(|display| display.id != id);
}
