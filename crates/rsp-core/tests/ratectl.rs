//! 03 §7 码率控制的验收测试。
//!
//! 每条规则一个用例，阈值直接引用 `ratectl` 里的常量：规格改了测试跟着改。
//! 所有时间都是测试自己造的 unix 微秒，控制器不读系统时钟。

use rsp_core::fec;
use rsp_core::pb::{Feedback, Quality};
use rsp_core::ratectl::{
    bounds_for, Controller, Decision, Path, DEGRADED_FPS, ECO_FPS, ECO_MAX_KBPS, FLOOR_DWELL_US,
    IDLE_DWELL_US, IDLE_INTERVAL_US, LOW_POWER_MAX_FPS, OVERLOAD_FACTOR, OVERLOAD_INTERVAL_US,
    RECOVER_DWELL_US, RECOVER_FACTOR, RECV_HEADROOM,
};

const MS: u64 = 1_000;
const SECOND: u64 = 1_000_000;
/// 协议约定 Feedback 每 200 ms 一次。
const FEEDBACK_INTERVAL_US: u64 = 200 * MS;
const T0: u64 = 1_700_000_000_000_000;
const NATIVE: (u32, u32) = (2560, 1440);

/// 控制器 + 一个可以往前推的时钟 + 当前反馈的链路参数。
struct Rig {
    controller: Controller,
    now_us: u64,
    recv_kbps: u32,
    low_power: bool,
}

impl Rig {
    fn new(path: Path, quality: Quality, start_kbps: u32) -> Self {
        Self::with_native(path, quality, start_kbps, NATIVE)
    }

    fn with_native(path: Path, quality: Quality, start_kbps: u32, native: (u32, u32)) -> Self {
        Self {
            controller: Controller::new(path, quality, start_kbps, native, T0),
            now_us: T0,
            recv_kbps: 40_000,
            low_power: false,
        }
    }

    /// 一条「链路很闲」的反馈：梯度 0、不丢包、不排队。
    fn idle_feedback(&self) -> Feedback {
        Feedback {
            recv_kbps: self.recv_kbps,
            interval_ms: 200,
            low_power: self.low_power,
            ..Feedback::default()
        }
    }

    /// 一条触发过载（`frames_lost > 0`）的反馈。
    fn overload_feedback(&self) -> Feedback {
        Feedback {
            frames_lost: 1,
            recv_kbps: self.recv_kbps,
            interval_ms: 200,
            low_power: self.low_power,
            ..Feedback::default()
        }
    }

    fn feedback(&mut self, fb: &Feedback) -> Decision {
        self.feedback_after(FEEDBACK_INTERVAL_US, fb)
    }

    fn feedback_after(&mut self, dt_us: u64, fb: &Feedback) -> Decision {
        self.now_us += dt_us;
        self.controller.on_feedback(fb, self.now_us)
    }

    fn idle(&mut self) -> Decision {
        let fb = self.idle_feedback();
        self.feedback(&fb)
    }

    fn overload(&mut self) -> Decision {
        let fb = self.overload_feedback();
        self.feedback(&fb)
    }

    fn tick(&mut self, dt_us: u64) -> Decision {
        self.now_us += dt_us;
        self.controller.tick(self.now_us)
    }

    /// 用连续过载把目标码率压到下限。
    fn drive_to_floor(&mut self) {
        let min_kbps = self.controller.bounds().min_kbps;
        for _ in 0..64 {
            if self.overload().target_kbps <= min_kbps {
                return;
            }
        }
        panic!("target never reached the floor");
    }

    /// 贴在下限上继续过载，直到降档到 `level`。
    fn drive_to_level(&mut self, level: u8) -> Decision {
        for _ in 0..64 {
            let decision = self.overload();
            if decision.level == level {
                return decision;
            }
        }
        panic!("never reached level {level}");
    }

    /// 空闲上调，直到目标码率超过 `min × RECOVER_FACTOR`。
    fn drive_above_recovery_threshold(&mut self) -> u64 {
        let threshold = (self.controller.bounds().min_kbps as f32 * RECOVER_FACTOR) as u32;
        for _ in 0..80 {
            if self.controller.target_kbps() > threshold {
                return self.now_us;
            }
            self.idle();
        }
        panic!("idle never lifted the target above 1.5 × min");
    }
}

#[test]
fn bounds_follow_the_path_table() {
    let lan = bounds_for(Path::Lan, false);
    assert_eq!(
        (lan.start_kbps, lan.min_kbps, lan.max_kbps, lan.max_fps),
        (20_000, 4_000, 40_000, 60)
    );
    assert_eq!(lan.max_height, None);

    let lan_high_refresh = bounds_for(Path::Lan, true);
    assert_eq!(lan_high_refresh.max_fps, 120, "LAN 上 iPad Pro 120");
    assert_eq!(lan_high_refresh.start_kbps, lan.start_kbps);
    assert_eq!(lan_high_refresh.max_kbps, lan.max_kbps);

    let intranet = bounds_for(Path::Intranet, false);
    assert_eq!(
        (
            intranet.start_kbps,
            intranet.min_kbps,
            intranet.max_kbps,
            intranet.max_fps
        ),
        (4_000, 2_000, 10_000, 30)
    );
    assert_eq!(intranet.max_height, Some(1440));

    let tailnet = bounds_for(Path::TailnetDirect, false);
    assert_eq!(
        (
            tailnet.start_kbps,
            tailnet.min_kbps,
            tailnet.max_kbps,
            tailnet.max_fps
        ),
        (8_000, 2_000, 15_000, 60)
    );
    assert_eq!(tailnet.max_height, Some(1440));

    let derp = bounds_for(Path::Derp, false);
    assert_eq!(
        (derp.start_kbps, derp.min_kbps, derp.max_kbps, derp.max_fps),
        (3_000, 1_500, 5_000, 30)
    );
    assert_eq!(derp.max_height, Some(1080));

    let tcp = bounds_for(Path::Tcp, false);
    assert_eq!(
        (tcp.start_kbps, tcp.min_kbps, tcp.max_kbps, tcp.max_fps),
        (4_000, 2_000, 6_000, 30)
    );
    assert_eq!(tcp.max_height, Some(1080));

    // 高刷客户端只影响局域网。
    assert_eq!(bounds_for(Path::Derp, true), derp);
    assert_eq!(bounds_for(Path::Tcp, true), tcp);
}

#[test]
fn start_bitrate_defaults_to_the_path_and_is_clamped() {
    // 0 = 按路径起点。
    let rig = Rig::new(Path::TailnetDirect, Quality::Auto, 0);
    assert_eq!(rig.controller.target_kbps(), 8_000);

    // 低于下限被夹到下限。
    let mut low = Rig::new(Path::Tcp, Quality::Auto, 500);
    assert_eq!(low.controller.target_kbps(), 2_000);
    assert_eq!(low.idle().target_kbps, 2_000);

    // 高于上限被夹到上限。
    let mut high = Rig::new(Path::Derp, Quality::Auto, 99_000);
    assert_eq!(high.controller.target_kbps(), 5_000);
    assert_eq!(high.idle().target_kbps, 5_000);

    // 区间内原样使用。
    let mut inside = Rig::new(Path::Lan, Quality::Auto, 12_345);
    assert_eq!(inside.idle().target_kbps, 12_345);
}

#[test]
fn high_refresh_client_gets_120fps_on_lan() {
    let plain = Controller::new(Path::Lan, Quality::Auto, 0, NATIVE, T0);
    assert_eq!(plain.bounds().max_fps, 60);

    let high = Controller::new_with_high_refresh(Path::Lan, Quality::Auto, 0, NATIVE, true, T0);
    assert_eq!(high.bounds().max_fps, 120);
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.controller = high;
    assert_eq!(rig.idle().fps, 120);

    // 其他路径不受影响。
    let derp = Controller::new_with_high_refresh(Path::Derp, Quality::Auto, 0, NATIVE, true, T0);
    assert_eq!(derp.bounds().max_fps, 30);
}

#[test]
fn loss_ewma_uses_0_8_0_2_and_feeds_the_fec_ratio() {
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 20_000);
    let lossy = Feedback {
        shards_received: 90,
        shards_lost: 10,
        recv_kbps: 20_000,
        interval_ms: 200,
        ..Feedback::default()
    };

    let first = rig.feedback(&lossy);
    assert!((rig.controller.loss_ewma() - 0.02).abs() < 1e-6);
    assert!((first.fec_ratio - fec::redundancy_ratio(0.02)).abs() < 1e-6);
    assert!((first.fec_ratio - 0.13).abs() < 1e-6);

    let second = rig.feedback(&lossy);
    assert!((rig.controller.loss_ewma() - 0.036).abs() < 1e-6);
    assert!((second.fec_ratio - fec::redundancy_ratio(0.036)).abs() < 1e-6);

    // 收敛到 10% 丢包（EWMA 渐近逼近，容差 1e-4），r 封顶 0.30。
    for _ in 0..80 {
        rig.feedback(&lossy);
    }
    assert!((rig.controller.loss_ewma() - 0.1).abs() < 1e-4);
    let capped = rig.idle();
    assert!((capped.fec_ratio - 0.30).abs() < 1e-6);

    // 编码器码率 = target / (1 + r)。
    let expected = (capped.target_kbps as f32 / (1.0 + capped.fec_ratio)).round() as u32;
    assert_eq!(capped.encoder_kbps, expected);
    assert!(capped.encoder_kbps < capped.target_kbps);
}

#[test]
fn overload_by_gradient_needs_two_consecutive_samples() {
    let rising = Feedback {
        delay_gradient_us_per_s: 6_000,
        recv_kbps: 20_000,
        interval_ms: 200,
        ..Feedback::default()
    };
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 20_000);

    // 第一次只是计数，不动码率。
    assert_eq!(rig.feedback(&rising).target_kbps, 20_000);
    // 连续第二次触发过载 ×0.85。
    let second = rig.feedback(&rising);
    assert_eq!(
        second.target_kbps,
        (20_000.0_f32 * OVERLOAD_FACTOR).round() as u32
    );
    assert_eq!(
        second.rate_update.as_ref().map(|u| u.reason.as_str()),
        Some("delay")
    );

    // 中间夹一次低梯度，连续计数清零。
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 20_000);
    rig.feedback(&rising);
    rig.feedback(&Feedback {
        delay_gradient_us_per_s: 0,
        ..Feedback::default()
    });
    assert_eq!(rig.feedback(&rising).target_kbps, 20_000);
    // 再连续一次才降。
    assert_eq!(
        rig.feedback(&rising).target_kbps,
        (20_000.0_f32 * OVERLOAD_FACTOR).round() as u32
    );
}

#[test]
fn overload_by_frames_lost_and_decode_queue() {
    let mut lost = Rig::new(Path::Lan, Quality::Auto, 20_000);
    let decision = lost.overload();
    assert_eq!(decision.target_kbps, 17_000);
    assert_eq!(
        decision.rate_update.as_ref().map(|u| u.reason.as_str()),
        Some("loss")
    );

    let mut queued = Rig::new(Path::Lan, Quality::Auto, 20_000);
    let deep = Feedback {
        decode_queue: 3,
        recv_kbps: 20_000,
        interval_ms: 200,
        ..Feedback::default()
    };
    let decision = queued.feedback(&deep);
    assert_eq!(decision.target_kbps, 17_000);
    assert_eq!(
        decision.rate_update.as_ref().map(|u| u.reason.as_str()),
        Some("queue")
    );

    // decode_queue == 2 不算过载。
    let mut shallow = Rig::new(Path::Lan, Quality::Auto, 20_000);
    let ok = Feedback {
        decode_queue: 2,
        recv_kbps: 20_000,
        interval_ms: 200,
        ..Feedback::default()
    };
    assert_eq!(shallow.feedback(&ok).target_kbps, 20_000);
}

#[test]
fn overload_is_rate_limited_to_one_decrease_per_400ms() {
    assert_eq!(OVERLOAD_INTERVAL_US, 400 * MS);
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 20_000);
    assert_eq!(rig.overload().target_kbps, 17_000);

    // 400 ms 内的第二次过载不再降。
    let blocked = rig.overload();
    assert_eq!(blocked.target_kbps, 17_000);
    assert!(blocked.rate_update.is_none());

    // 满 400 ms 之后可以再降。
    let allowed = rig.feedback_after(400 * MS, &rig.overload_feedback());
    assert_eq!(
        allowed.target_kbps,
        (17_000.0_f32 * OVERLOAD_FACTOR).round() as u32
    );
}

#[test]
fn idle_needs_one_second_and_is_rate_limited_to_one_increase_per_second() {
    assert_eq!(IDLE_DWELL_US, SECOND);
    assert_eq!(IDLE_INTERVAL_US, SECOND);
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 8_000);

    // 前 1 s 只是计时（第一条反馈起算）。
    for _ in 0..(IDLE_DWELL_US / FEEDBACK_INTERVAL_US) {
        assert_eq!(rig.idle().target_kbps, 8_000);
    }
    // 满 1 s 升一次 ×1.08。
    let raised = rig.idle();
    assert_eq!(raised.target_kbps, (8_000.0_f32 * 1.08).round() as u32);
    assert_eq!(
        raised.rate_update.as_ref().map(|u| u.reason.as_str()),
        Some("idle")
    );

    // 1 s 内不再升。
    let blocked = rig.idle();
    assert_eq!(blocked.target_kbps, raised.target_kbps);
    assert!(blocked.rate_update.is_none());

    // 满 1 s 再升。
    let next = rig.feedback_after(IDLE_INTERVAL_US, &rig.idle_feedback());
    assert_eq!(
        next.target_kbps,
        (raised.target_kbps as f32 * 1.08).round() as u32
    );
}

#[test]
fn idle_increase_is_capped_by_recv_headroom() {
    // DERP：起点 2 Mbps、上限 5 Mbps；接收端只报 2 Mbps → 天花板 3 Mbps。
    let mut rig = Rig::new(Path::Derp, Quality::Auto, 2_000);
    rig.recv_kbps = 2_000;
    let cap = (2_000.0_f32 * RECV_HEADROOM) as u32;
    assert_eq!(cap, 3_000);

    let mut last = 0;
    for _ in 0..100 {
        last = rig.idle().target_kbps;
        assert!(last <= cap, "target {last} must not exceed recv_kbps × 1.5");
    }
    assert_eq!(last, cap, "target must stop at recv_kbps × 1.5");
}

#[test]
fn auto_and_clear_degrade_fps_then_resolution() {
    for quality in [Quality::Auto, Quality::Clear] {
        let mut rig = Rig::new(Path::Lan, quality, 0);
        let first = rig.idle();
        assert_eq!((first.fps, first.scale, first.level), (60, 1.0, 0));
        // 第一条决策就是 epoch 0 的建流配置。
        assert!(first.config_changed);
        assert!(first.rate_update.is_some());

        rig.drive_to_floor();
        // 贴下限 2 s → 第 1 级：帧率 30。
        let level1 = rig.drive_to_level(1);
        assert_eq!((level1.fps, level1.scale), (DEGRADED_FPS, 1.0));
        assert!(level1.config_changed);
        assert!(level1.rate_update.is_some());
        assert_eq!(rig.controller.stream_size(), (2560, 1440));

        // 再 2 s → 第 2 级：分辨率 ×0.75。
        let level2 = rig.drive_to_level(2);
        assert_eq!((level2.fps, level2.scale), (DEGRADED_FPS, 0.75));
        assert!(level2.config_changed);
        assert_eq!(rig.controller.stream_size(), (1920, 1080));

        // 再 2 s → 第 3 级：分辨率 ×0.5。
        let level3 = rig.drive_to_level(3);
        assert_eq!((level3.fps, level3.scale), (DEGRADED_FPS, 0.5));
        assert!(level3.config_changed);
        assert_eq!(rig.controller.stream_size(), (1280, 720));

        // 已经到底，不再降。
        assert_eq!(rig.drive_to_level(3).level, 3);
        assert_eq!(FLOOR_DWELL_US, 2 * SECOND);
    }
}

#[test]
fn smooth_degrades_resolution_then_fps() {
    let mut rig = Rig::new(Path::Lan, Quality::Smooth, 0);
    assert_eq!(rig.idle().fps, 60);

    rig.drive_to_floor();
    let level1 = rig.drive_to_level(1);
    assert_eq!((level1.fps, level1.scale), (60, 0.75));
    let level2 = rig.drive_to_level(2);
    assert_eq!((level2.fps, level2.scale), (60, 0.5));
    let level3 = rig.drive_to_level(3);
    assert_eq!((level3.fps, level3.scale), (DEGRADED_FPS, 0.5));
    assert!(level3.config_changed);
    assert!(level3.rate_update.is_some());
}

#[test]
fn eco_is_fixed_30fps_capped_at_4mbps_and_scales_at_level_one() {
    let mut rig = Rig::new(Path::Lan, Quality::Eco, 0);
    let first = rig.idle();
    assert_eq!(first.fps, ECO_FPS);
    assert_eq!(first.scale, 1.0);
    assert_eq!(
        first.target_kbps, ECO_MAX_KBPS,
        "LAN 起点 20 Mbps 被 4 Mbps 上限夹住"
    );

    rig.drive_to_floor();
    let level1 = rig.drive_to_level(1);
    assert_eq!((level1.fps, level1.scale), (ECO_FPS, 0.75));
    // 省流量只有第 1 级。
    for _ in 0..40 {
        assert!(rig.overload().level <= 1);
    }
    assert!(rig.controller.target_kbps() <= ECO_MAX_KBPS);
}

#[test]
fn recovery_steps_up_after_5s_above_1_5x_floor() {
    assert_eq!(RECOVER_DWELL_US, 5 * SECOND);
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.idle();
    rig.drive_to_floor();
    assert_eq!(rig.drive_to_level(3).level, 3);

    // 空闲把码率抬过下限 1.5 倍（4 Mbps × 1.5 = 6 Mbps）。
    let above_at = rig.drive_above_recovery_threshold();
    assert_eq!(rig.controller.level(), 3, "刚过阈值还不恢复");

    // 未满 5 s 不恢复。
    let mut decision = rig.idle();
    while rig.now_us < above_at + RECOVER_DWELL_US {
        assert_eq!(decision.level, 3);
        assert!(!decision.config_changed);
        decision = rig.idle();
    }
    // 满 5 s 逐级恢复一级。
    assert_eq!(decision.level, 2);
    assert_eq!((decision.fps, decision.scale), (DEGRADED_FPS, 0.75));
    assert!(decision.config_changed);
    assert!(decision.rate_update.is_some());
    assert_eq!(
        decision.rate_update.as_ref().map(|u| u.reason.as_str()),
        Some("recover")
    );
}

#[test]
fn intranet_raises_the_fps_cap_after_10s_stable() {
    let mut rig = Rig::new(Path::Intranet, Quality::Auto, 0);
    assert_eq!(rig.idle().fps, 30, "公司内网先 30 fps");

    // 稳定 9.8 s 还是 30。
    for _ in 0..48 {
        assert_eq!(rig.idle().fps, 30);
    }
    // 满 10 s 升到 60。
    let boosted = rig.idle();
    assert_eq!(boosted.fps, 60);
    assert!(boosted.config_changed);
    assert!(boosted.rate_update.is_some());
    assert_eq!(rig.now_us - T0, 10 * SECOND);

    // 之后保持 60。
    assert_eq!(rig.idle().fps, 60);
}

#[test]
fn low_power_treats_auto_as_smooth_and_caps_30fps() {
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.low_power = true;
    let first = rig.idle();
    assert_eq!(first.fps, LOW_POWER_MAX_FPS);
    assert!(first.config_changed);

    rig.drive_to_floor();
    // 「自动」按「流畅」降档：先分辨率，再帧率。
    let level1 = rig.drive_to_level(1);
    assert_eq!((level1.fps, level1.scale), (LOW_POWER_MAX_FPS, 0.75));
    let level2 = rig.drive_to_level(2);
    assert_eq!((level2.fps, level2.scale), (LOW_POWER_MAX_FPS, 0.5));
    for _ in 0..20 {
        assert!(rig.overload().fps <= LOW_POWER_MAX_FPS);
    }

    // 电量恢复后回到「自动」的降档顺序（帧率先降）。
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.low_power = true;
    assert_eq!(rig.idle().fps, LOW_POWER_MAX_FPS);
    rig.low_power = false;
    assert_eq!(rig.idle().fps, 60);
    rig.drive_to_floor();
    let level1 = rig.drive_to_level(1);
    assert_eq!((level1.fps, level1.scale), (DEGRADED_FPS, 1.0));
}

#[test]
fn set_quality_reconfigures_and_clamps_the_target() {
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    assert_eq!(rig.idle().fps, 60);

    let now_us = rig.now_us;
    let eco = rig.controller.set_quality(Quality::Eco, now_us);
    assert_eq!(eco.fps, ECO_FPS);
    assert_eq!(eco.target_kbps, ECO_MAX_KBPS);
    assert!(eco.config_changed);
    assert!(eco.rate_update.is_some());

    let clear = rig.controller.set_quality(Quality::Clear, now_us);
    assert_eq!(clear.fps, 60);
    assert_eq!(clear.target_kbps, ECO_MAX_KBPS, "切档不凭空涨码率");
    assert!(clear.config_changed);
    assert!(clear.rate_update.is_some());
}

#[test]
fn unchanged_parameters_do_not_emit_rate_updates() {
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 20_000);
    // 梯度在 1 ms/s 和 5 ms/s 之间：既不是空闲也不是过载。
    let steady = Feedback {
        delay_gradient_us_per_s: 2_000,
        recv_kbps: 20_000,
        interval_ms: 200,
        ..Feedback::default()
    };

    let first = rig.feedback(&steady);
    assert!(first.rate_update.is_some(), "第一条决策要发初始参数");
    assert!(first.config_changed, "第一条决策是建流配置");

    for _ in 0..10 {
        let decision = rig.feedback(&steady);
        assert_eq!(decision.target_kbps, first.target_kbps);
        assert!(!decision.config_changed);
        assert!(decision.rate_update.is_none(), "参数没变就不刷屏");
    }
}

#[test]
fn tick_advances_dwell_timers_without_feedback() {
    // 贴下限 2 s 靠 tick 走完（期间没有 Feedback）。
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.drive_to_floor();
    assert_eq!(rig.controller.level(), 0);
    let degraded = rig.tick(FLOOR_DWELL_US);
    assert_eq!(degraded.level, 1);
    assert_eq!(degraded.fps, DEGRADED_FPS);
    assert!(degraded.config_changed);
    assert!(degraded.rate_update.is_some());

    // 恢复计时同样靠 tick 推进。
    let mut rig = Rig::new(Path::Lan, Quality::Auto, 0);
    rig.drive_to_floor();
    assert_eq!(rig.drive_to_level(3).level, 3);
    rig.drive_above_recovery_threshold();
    assert_eq!(rig.controller.level(), 3);
    let recovered = rig.tick(RECOVER_DWELL_US);
    assert_eq!(recovered.level, 2);
    assert!(recovered.config_changed);
    assert!(recovered.rate_update.is_some());
}

/// 瓶颈链路：有限缓冲的 drop-tail 队列。
///
/// 发送速率超过容量时队列增长；队列满之后多出来的字节被丢掉，丢包率 =
/// 溢出 / 到达。排队时延 = 队列字节数 / 容量，它的斜率就是 §7.1 的
/// `delay_gradient_us_per_s`。缓冲取 50 ms 的容量（浅缓冲，比 `weak` 场景的
/// 100 ms 单程时延还小），保证过载在 1–2 个 Feedback 周期内就被梯度或丢包
/// 发现，不会出现「队列慢慢涨、梯度刚好压在阈值下面」的假稳态。
struct Bottleneck {
    capacity_kbps: f64,
    buffer_bytes: f64,
    queue_bytes: f64,
    previous_delay_ms: f64,
}

/// 瓶颈缓冲深度（毫秒 × 容量）。
const BUFFER_MS: f64 = 50.0;

impl Bottleneck {
    fn new(capacity_kbps: f64) -> Self {
        Self {
            capacity_kbps,
            buffer_bytes: capacity_kbps * 125.0 * BUFFER_MS / 1_000.0,
            queue_bytes: 0.0,
            previous_delay_ms: 0.0,
        }
    }

    /// 推进一个 200 ms 的 Feedback 周期。
    fn step(&mut self, send_kbps: f64) -> Feedback {
        const DT_S: f64 = 0.2;
        const SHARDS_TOTAL: u32 = 100;
        /// 溢出不小于到达的 2% 才算丢帧（`frames_lost > 0` 触发过载）。
        const FRAME_LOSS_THRESHOLD: f64 = 0.02;

        let arrived_bytes = send_kbps * 125.0 * DT_S;
        let service_bytes = self.capacity_kbps * 125.0 * DT_S;
        self.queue_bytes += arrived_bytes;
        self.queue_bytes -= self.queue_bytes.min(service_bytes);
        let overflow_bytes = (self.queue_bytes - self.buffer_bytes).max(0.0);
        self.queue_bytes -= overflow_bytes;

        let delay_ms = self.queue_bytes * 8.0 / self.capacity_kbps;
        let gradient_us_per_s = ((delay_ms - self.previous_delay_ms) / DT_S * 1_000.0) as i32;
        self.previous_delay_ms = delay_ms;

        let loss = if arrived_bytes > 0.0 {
            overflow_bytes / arrived_bytes
        } else {
            0.0
        };
        let shards_lost = (f64::from(SHARDS_TOTAL) * loss).round() as u32;
        Feedback {
            interval_ms: 200,
            frames_lost: u32::from(loss >= FRAME_LOSS_THRESHOLD),
            shards_received: SHARDS_TOTAL - shards_lost,
            shards_lost,
            recv_kbps: send_kbps.min(self.capacity_kbps).round() as u32,
            delay_gradient_us_per_s: gradient_us_per_s,
            ..Feedback::default()
        }
    }
}

const CONVERGENCE_SECONDS: u64 = 60;
const WINDOW_SECONDS: u64 = 20;
const STEPS_PER_SECOND: u64 = 5;
/// 最后 20 s 里相邻 Decision 变化方向的翻转次数上限。
///
/// 规格里的 ×1.08 / ×0.85 本身就不对称（一升一降净 −8.2%），所以稳态是
/// 「缓慢爬升 → 撞到瓶颈 → 回退」的锯齿，而不是一条直线：这是 §7.2 的预期
/// 行为，不是振荡。一次锯齿至少需要 `IDLE_DWELL_US`（1 s 空闲驻留）+ 一次
/// Feedback 的过载检测（200 ms）= 1.2 s，20 s 内最多 `2 × floor(20/1.2) = 32`
/// 次方向翻转。三个瓶颈场景实测 9 / 1 / 14 次，取 24 作为上限：既能容纳
/// 模型差异，又能挡住「每个 Feedback 都翻一次」（那会是约 100 次）的坏实现。
const MAX_DIRECTION_FLIPS: usize = 24;

fn simulate(path: Path, start_kbps: u32, capacity_kbps: f64) -> Vec<Decision> {
    let mut rig = Rig::new(path, Quality::Auto, start_kbps);
    let mut link = Bottleneck::new(capacity_kbps);
    let mut decisions = Vec::new();
    for _ in 0..(CONVERGENCE_SECONDS * STEPS_PER_SECOND) {
        let feedback = link.step(f64::from(rig.controller.target_kbps()));
        decisions.push(rig.feedback(&feedback));
    }
    decisions
}

fn assert_converged(decisions: &[Decision], capacity_kbps: f64) {
    let window = &decisions[decisions.len() - (WINDOW_SECONDS * STEPS_PER_SECOND) as usize..];
    let min = window.iter().map(|d| d.target_kbps).min().unwrap_or(0);
    let max = window.iter().map(|d| d.target_kbps).max().unwrap_or(0);
    assert!(
        f64::from(min) >= 0.7 * capacity_kbps,
        "target dipped to {min} kbps, below 0.7 × {capacity_kbps}"
    );
    assert!(
        f64::from(max) <= 1.1 * capacity_kbps,
        "target peaked at {max} kbps, above 1.1 × {capacity_kbps}"
    );
    assert!(
        window.iter().all(|d| d.level == 0),
        "容量在上下限之间，不该降档"
    );

    let mut flips = 0usize;
    let mut previous = 0i64;
    for pair in window.windows(2) {
        let delta = i64::from(pair[1].target_kbps) - i64::from(pair[0].target_kbps);
        if delta == 0 {
            continue;
        }
        let direction = delta.signum();
        if previous != 0 && direction != previous {
            flips += 1;
        }
        previous = direction;
    }
    assert!(
        flips <= MAX_DIRECTION_FLIPS,
        "direction flips {flips} > {MAX_DIRECTION_FLIPS} (min {min}, max {max})"
    );
}

#[test]
fn converges_on_a_3_mbps_bottleneck() {
    // TCP 回退：下限 2 Mbps、上限 6 Mbps，3 Mbps 的瓶颈落在区间内。
    let decisions = simulate(Path::Tcp, 4_000, 3_000.0);
    assert_converged(&decisions, 3_000.0);
}

#[test]
fn converges_on_an_8_mbps_bottleneck() {
    let decisions = simulate(Path::Lan, 0, 8_000.0);
    assert_converged(&decisions, 8_000.0);
}

#[test]
fn converges_on_a_20_mbps_bottleneck() {
    let decisions = simulate(Path::Lan, 0, 20_000.0);
    assert_converged(&decisions, 20_000.0);
}
