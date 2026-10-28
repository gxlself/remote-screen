use host_win::vpmf_model::*;
use rsp_core::pb::Codec;

#[test]
fn geometry_and_rotation() {
    assert_eq!(even_down(7), 6);
    assert_eq!(even_down(8), 8);
    assert_eq!(rotated_size(1920, 1080, 0), (1920, 1080));
    assert_eq!(rotated_size(1920, 1080, 90), (1080, 1920));
    assert_eq!(rotated_size(1920, 1080, 180), (1920, 1080));
    assert_eq!(rotated_size(1920, 1080, 270), (1080, 1920));
    assert_eq!(rotated_size(1920, 1080, 45), (1920, 1080));

    assert_eq!(
        fit_rect(1920, 1080, 1920, 1080),
        RectU {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080
        }
    );
    assert_eq!(
        fit_rect(2560, 1440, 1920, 1080),
        RectU {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080
        }
    );
    assert_eq!(
        fit_rect(1920, 1200, 1920, 1080),
        RectU {
            left: 96,
            top: 0,
            right: 1824,
            bottom: 1080
        }
    );
    assert_eq!(
        fit_rect(1080, 1920, 1920, 1080),
        RectU {
            left: 656,
            top: 0,
            right: 1262,
            bottom: 1080
        }
    );
    assert_eq!(
        fit_rect(2560, 1080, 1920, 1080),
        RectU {
            left: 0,
            top: 134,
            right: 1920,
            bottom: 944
        }
    );
    assert_eq!(
        fit_rect(0, 1080, 1920, 1080),
        RectU {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080
        }
    );

    assert_eq!(vp_rotation(0), VpRotation::Identity);
    assert_eq!(vp_rotation(90), VpRotation::R90);
    assert_eq!(vp_rotation(180), VpRotation::R180);
    assert_eq!(vp_rotation(270), VpRotation::R270);
    assert_eq!(vp_rotation(360), VpRotation::Identity);
    assert_eq!(vp_rotation_value(VpRotation::Identity), 0);
    assert_eq!(vp_rotation_value(VpRotation::R90), 1);
    assert_eq!(vp_rotation_value(VpRotation::R180), 2);
    assert_eq!(vp_rotation_value(VpRotation::R270), 3);
}

#[test]
fn colors_and_view_cache() {
    assert_eq!(legacy_color_space(true, true, 1), 0x14);
    assert_eq!(legacy_color_space(true, true, 0), 0x04);
    assert_eq!(legacy_color_space(false, true, 1), 0x16);
    assert_eq!(legacy_color_space(true, false, 2), 0x20);
    assert_eq!(VP_INPUT_LEGACY, 0x04);
    assert_eq!(VP_OUTPUT_LEGACY, 0x14);
    assert_eq!(VP_OUTPUT_COLOR_SPACE, 8);

    let mut cache = ViewCache::<u32, u32>::new(2);
    assert_eq!(cache.get(1, || Ok::<_, &str>(10)), Ok(&10));
    assert_eq!(
        cache.get(1, || -> Result<u32, &str> { panic!("cache miss") }),
        Ok(&10)
    );
    assert_eq!(cache.get(2, || Ok::<_, &str>(20)), Ok(&20));
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get(3, || Ok::<_, &str>(30)), Ok(&30));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(1, || Ok::<_, &str>(11)), Ok(&11));
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get(4, || Err::<u32, _>("x")), Err("x"));
    assert_eq!(cache.len(), 0);
}

#[test]
fn mf_values_and_types() {
    assert_eq!(luid_parts(0x0000_0001_0000_ABCD), (0xABCD, 1));
    assert_eq!(
        luid_blob(0x0000_0001_0000_ABCD),
        [0xCD, 0xAB, 0, 0, 1, 0, 0, 0]
    );
    assert_eq!(luid_blob(-1), [0xFF; 8]);
    assert_eq!(mf_vendor_attr(0x10DE), "VEN_10DE");
    assert_eq!(mf_vendor_attr(0x8086), "VEN_8086");
    assert_eq!(mf_vendor_attr(0x1002), "VEN_1002");
    assert_eq!(mf_pack(1920, 1080), 8_246_337_209_400);
    assert_eq!(mf_pack(60, 1), 257_698_037_761);
    assert_eq!(hns_per_frame(60), 166_667);
    assert_eq!(hns_per_frame(30), 333_333);
    assert_eq!(hns_per_frame(1), 10_000_000);
    assert_eq!(sample_time(3, 60), 500_000);
    assert_eq!(sample_time(1, 60), 166_667);
    assert_eq!(sample_time(0, 60), 0);
    assert_eq!(pts_from_sample_time(166_667, 60), 1);
    assert_eq!(pts_from_sample_time(500_000, 60), 3);
    assert_eq!(pts_from_sample_time(499_000, 60), 3);
    for fps in [24, 30, 60, 120, 144] {
        for pts in 0..=1000 {
            assert_eq!(pts_from_sample_time(sample_time(pts, fps), fps), pts);
        }
    }

    assert_eq!(
        mf_codecapi(10_000_000, 60),
        vec![
            MfProp {
                api: MfApi::LowLatencyMode,
                value: MfValue::Bool(true),
                required: true
            },
            MfProp {
                api: MfApi::RateControlMode,
                value: MfValue::U32(0),
                required: true
            },
            MfProp {
                api: MfApi::MeanBitRate,
                value: MfValue::U32(10_000_000),
                required: true
            },
            MfProp {
                api: MfApi::BufferSize,
                value: MfValue::U32(166_667),
                required: false
            },
            MfProp {
                api: MfApi::GopSize,
                value: MfValue::U32(MF_GOP_INFINITE),
                required: false
            },
            MfProp {
                api: MfApi::BPictureCount,
                value: MfValue::U32(0),
                required: false
            },
            MfProp {
                api: MfApi::QualityVsSpeed,
                value: MfValue::U32(0),
                required: false
            },
        ]
    );
    assert_eq!(
        mf_codecapi(5_000_000_000, 60)[2].value,
        MfValue::U32(u32::MAX)
    );
    assert_eq!(
        mf_codecapi(5_000_000_000, 60)[3].value,
        MfValue::U32(83_333_334)
    );
    assert_eq!(
        mf_rate_props(3_000_000, 60),
        vec![
            MfProp {
                api: MfApi::MeanBitRate,
                value: MfValue::U32(3_000_000),
                required: true
            },
            MfProp {
                api: MfApi::BufferSize,
                value: MfValue::U32(50_000),
                required: false
            },
        ]
    );
    assert_eq!(
        mf_force_key(),
        MfProp {
            api: MfApi::ForceKeyFrame,
            value: MfValue::U32(1),
            required: true
        }
    );
    assert_eq!(mf_profile(Codec::H264), Some(100));
    assert_eq!(mf_profile(Codec::Hevc), Some(1));
    assert_eq!(mf_profile(Codec::Unspecified), None);

    assert_eq!(
        mf_type_attrs(Codec::H264, 1920, 1080, 60, 10_000_000, true),
        vec![
            MfAttr::FrameSize(8_246_337_209_400),
            MfAttr::FrameRate(257_698_037_761),
            MfAttr::AvgBitrate(10_000_000),
            MfAttr::Interlace(2),
            MfAttr::Profile(100),
            MfAttr::YuvMatrix(1),
            MfAttr::NominalRange(2),
            MfAttr::Primaries(2),
            MfAttr::Transfer(5),
        ]
    );
    assert_eq!(
        mf_type_attrs(Codec::Hevc, 1920, 1080, 60, 10_000_000, true)[4],
        MfAttr::Profile(1)
    );
    assert_eq!(
        mf_type_attrs(Codec::H264, 1920, 1080, 60, 10_000_000, false),
        vec![
            MfAttr::FrameSize(8_246_337_209_400),
            MfAttr::FrameRate(257_698_037_761),
            MfAttr::Interlace(2),
            MfAttr::YuvMatrix(1),
            MfAttr::NominalRange(2),
        ]
    );
}

#[test]
fn mf_flow_and_surface_ring() {
    assert_eq!(mf_event_kind(601), MfEventKind::NeedInput);
    assert_eq!(mf_event_kind(602), MfEventKind::HaveOutput);
    assert_eq!(mf_event_kind(603), MfEventKind::DrainComplete);
    assert_eq!(mf_event_kind(1), MfEventKind::Error);
    assert_eq!(mf_event_kind(604), MfEventKind::Other);
    assert_eq!(mf_event_kind(0), MfEventKind::Other);

    let mut flow = MfFlow::new();
    assert!(!flow.take_input());
    flow.on_event(MfEventKind::NeedInput);
    flow.on_event(MfEventKind::NeedInput);
    assert!(flow.take_input());
    assert!(flow.take_input());
    assert!(!flow.take_input());
    flow.on_event(MfEventKind::HaveOutput);
    assert!(flow.take_output());
    assert!(!flow.take_output());
    flow.on_event(MfEventKind::Other);
    flow.on_event(MfEventKind::Error);
    flow.on_event(MfEventKind::NeedInput);
    flow.on_event(MfEventKind::NeedInput);
    flow.reset();
    assert!(!flow.take_input());

    let mut ring = SurfaceRing::new(3);
    assert_eq!(ring.acquire(), Some(0));
    assert_eq!(ring.acquire(), Some(0));
    ring.submitted(0, 10);
    assert_eq!(ring.acquire(), Some(1));
    ring.submitted(1, 11);
    assert_eq!(ring.acquire(), Some(2));
    ring.submitted(2, 12);
    assert_eq!(ring.acquire(), None);
    assert_eq!(ring.busy_count(), 3);
    ring.released_through(10);
    assert_eq!(ring.busy_count(), 2);
    assert_eq!(ring.acquire(), Some(0));
    ring.submitted(0, 13);
    ring.released_through(12);
    assert_eq!(ring.busy_count(), 1);
    assert_eq!(ring.acquire(), Some(1));
    ring.reset();
    assert_eq!(ring.busy_count(), 0);
    assert_eq!(ring.acquire(), Some(0));
}
