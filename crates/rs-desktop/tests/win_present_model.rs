use rs_desktop::{
    pointer::Viewport,
    win_present_model::{
        target_rect, PresentPacer, PresentWhat, ResetBudget, ViewCache, OUTPUT_COLOR_SPACE,
        OUTPUT_LEGACY, STREAM_COLOR_SPACE, STREAM_LEGACY,
    },
};

#[test]
fn target_rect_matches_viewport() {
    for (client_w, client_h, video_w, video_h) in [
        (1920, 1080, 1920, 1080),
        (1600, 1200, 1920, 1080),
        (1280, 1024, 2560, 1440),
        (800, 1200, 1920, 1080),
    ] {
        let vp = Viewport {
            view_w: f64::from(client_w),
            view_h: f64::from(client_h),
            video_w,
            video_h,
        };
        let (left, top, width, height) = vp.content_rect();
        assert_eq!(
            target_rect(client_w, client_h, video_w, video_h),
            rounded_rect(left, top, width, height)
        );
    }
}

#[test]
fn target_rect_degenerate() {
    assert_eq!(
        target_rect(800, 600, 0, 360),
        rs_desktop::win_present_model::RectPx {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        }
    );
    assert_eq!(
        target_rect(800, 600, 640, 0),
        rs_desktop::win_present_model::RectPx {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        }
    );
    assert_eq!(
        target_rect(0, 0, 640, 360),
        rs_desktop::win_present_model::RectPx {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    );
}

#[test]
fn color_space_mirrors_host() {
    assert_eq!(
        STREAM_COLOR_SPACE,
        host_win::vpmf_model::VP_OUTPUT_COLOR_SPACE
    );
    assert_eq!(
        OUTPUT_COLOR_SPACE,
        host_win::vpmf_model::VP_INPUT_COLOR_SPACE
    );
    assert_eq!(STREAM_LEGACY, host_win::vpmf_model::VP_OUTPUT_LEGACY);
    assert_eq!(OUTPUT_LEGACY, host_win::vpmf_model::VP_INPUT_LEGACY);
}

#[test]
fn pacer_waits_first() {
    let mut pacer = PresentPacer::new();
    assert!(pacer.wants_waitable());
    assert_eq!(pacer.on_decoded(1), None);
    assert_eq!(pacer.next(), None);
    pacer.on_swapchain_ready();
    assert_eq!(pacer.next(), Some(PresentWhat::Pending(1)));
    assert!(pacer.wants_waitable());
}

#[test]
fn pacer_supersedes() {
    let mut pacer = PresentPacer::new();
    assert_eq!(pacer.on_decoded(1), None);
    assert_eq!(pacer.on_decoded(2), Some(1));
    pacer.on_swapchain_ready();
    assert_eq!(pacer.next(), Some(PresentWhat::Pending(2)));
}

#[test]
fn pacer_redraw() {
    let mut pacer = PresentPacer::new();
    pacer.on_decoded(1);
    pacer.on_swapchain_ready();
    assert_eq!(pacer.next(), Some(PresentWhat::Pending(1)));
    pacer.request_redraw();
    pacer.on_swapchain_ready();
    assert_eq!(pacer.next(), Some(PresentWhat::Redraw));
    pacer.request_redraw();
    pacer.on_decoded(2);
    pacer.on_swapchain_ready();
    assert_eq!(pacer.next(), Some(PresentWhat::Pending(2)));
    assert_eq!(pacer.next(), None);
}

#[test]
fn view_cache_bounded() {
    let mut cache = ViewCache::new(32);
    let mut calls = 0;
    for key in 0..33 {
        cache
            .get_or_insert_with(key, || {
                calls += 1;
                Ok::<_, ()>(key * 2)
            })
            .unwrap();
    }
    assert_eq!(cache.len(), 32);
    assert!(cache
        .get_or_insert_with(0, || {
            calls += 1;
            Ok::<_, ()>(0)
        })
        .is_ok());
    assert_eq!(calls, 34);
    cache.clear();
    assert_eq!(cache.len(), 0);
}

#[test]
fn reset_budget() {
    let mut budget = ResetBudget::new();
    assert!(budget.try_take(0));
    assert!(budget.try_take(1_000_000));
    assert!(budget.try_take(2_000_000));
    assert!(!budget.try_take(3_000_000));
    assert!(budget.try_take(10_500_000));
}

fn rounded_rect(
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> rs_desktop::win_present_model::RectPx {
    rs_desktop::win_present_model::RectPx {
        left: left.round() as u32,
        top: top.round() as u32,
        right: (left + width).round() as u32,
        bottom: (top + height).round() as u32,
    }
}
