use host_core::config::VideoEncoder;
use host_core::encode_model::*;
use host_core::encoder_pick::{EncoderKind, VENDOR_NVIDIA};
use rsp_core::pb::Codec;

fn ms(n: u64) -> u64 {
    n * 1_000
}

fn s(n: u64) -> u64 {
    n * 1_000_000
}

fn f(force_key: bool, refine: bool) -> JobFlags {
    JobFlags { force_key, refine }
}

#[test]
fn backend_and_validation() {
    for b in [
        Backend::FfNvenc,
        Backend::FfAmf,
        Backend::FfQsv,
        Backend::MfHw,
    ] {
        assert_eq!(Backend::from_kind(b.kind()), Some(b));
    }
    assert_eq!(Backend::from_kind(EncoderKind::OpenH264), None);
    assert_eq!(Backend::FfNvenc.name(), "nvenc");
    assert!(Backend::FfNvenc.is_ffmpeg());
    assert!(!Backend::MfHw.is_ffmpeg());

    assert_eq!(
        validate(Codec::Hevc, 1920, 1080, 60, 10_000),
        Ok(EncParams {
            codec: Codec::Hevc,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 10_000_000,
        })
    );
    assert_eq!(
        validate(Codec::H264, 1921, 1081, 60, 10_000).unwrap().width,
        1920
    );
    assert_eq!(
        validate(Codec::Unspecified, 1920, 1080, 60, 1),
        Err(ParamError::Codec)
    );
    assert_eq!(
        validate(Codec::Hevc, 127, 1080, 60, 1),
        Err(ParamError::Size {
            width: 127,
            height: 1080
        })
    );
    assert_eq!(
        validate(Codec::H264, 4098, 2160, 60, 1),
        Err(ParamError::Size {
            width: 4098,
            height: 2160
        })
    );
    assert_eq!(
        validate(Codec::Hevc, 8194, 4320, 60, 1),
        Err(ParamError::Size {
            width: 8194,
            height: 4320
        })
    );
    assert_eq!(
        validate(Codec::Hevc, 1920, 1080, 0, 1),
        Err(ParamError::Fps(0))
    );
    assert_eq!(
        validate(Codec::Hevc, 1920, 1080, 60, 0),
        Err(ParamError::Bitrate(0))
    );
    assert_eq!(
        validate(Codec::Hevc, 1920, 1080, 60, 50)
            .unwrap()
            .bitrate_bps,
        100_000
    );
    assert_eq!(
        validate(Codec::Hevc, 1920, 1080, 60, 500_000)
            .unwrap()
            .bitrate_bps,
        400_000_000
    );
}

#[test]
fn ffmpeg_tables_and_rates() {
    let p = EncParams {
        codec: Codec::Hevc,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_bps: 10_000_000,
    };
    assert_eq!(
        ffmpeg_codec_name(Backend::FfNvenc, Codec::Hevc),
        Some("hevc_nvenc")
    );
    assert_eq!(
        ffmpeg_codec_name(Backend::FfQsv, Codec::H264),
        Some("h264_qsv")
    );
    assert_eq!(ffmpeg_codec_name(Backend::MfHw, Codec::Hevc), None);
    assert_eq!(hw_pixel_format(Backend::FfNvenc), Some("d3d11"));
    assert_eq!(hw_pixel_format(Backend::FfQsv), Some("qsv"));
    assert_eq!(hw_pixel_format(Backend::MfHw), None);
    let opts = ffmpeg_options(Backend::FfNvenc, &p);
    assert_eq!(opts.len(), 21);
    assert_eq!(
        (&opts[0].name, &opts[0].value, opts[0].required),
        (&"pixel_format", &"d3d11".to_owned(), true)
    );
    assert_eq!(
        (&opts[5].name, &opts[5].value),
        (&"bufsize", &"166667".to_owned())
    );
    assert_eq!(
        (&opts[19].name, &opts[19].value),
        (&"forced-idr", &"1".to_owned())
    );
    assert_eq!(
        ffmpeg_options(
            Backend::FfQsv,
            &EncParams {
                codec: Codec::H264,
                width: 1280,
                height: 720,
                fps: 30,
                bitrate_bps: 5_000_000
            }
        )
        .len(),
        22
    );
    assert_eq!(ffmpeg_options(Backend::MfHw, &p), Vec::new());
    assert_eq!(ffmpeg_options_retry(Backend::FfNvenc, &p), None);
    assert_eq!(
        ffmpeg_rate_options(Backend::FfNvenc, 3_000_000, 60)[2].value,
        "50000"
    );
    assert_eq!(
        ffmpeg_rate_options(Backend::FfQsv, 3_000_000, 60)[0].value,
        "2999999"
    );
    assert_eq!(effective_bps(10_000, 60, 60), 10_000_000);
    assert_eq!(effective_bps(10_000, 30, 60), 20_000_000);
    assert_eq!(effective_bps(10_000, 0, 60), 10_000_000);
    assert_eq!(vbv_bits(10_000_000, 60), 166_667);
}

#[test]
fn rate_hysteresis() {
    let mut r = RateApplier::new(RateCost::Idr, 10_000_000);
    assert_eq!(r.request(ms(100), 8_000_000), Some(8_000_000));
    assert_eq!(r.request(ms(300), 6_800_000), None);
    assert_eq!(r.deadline(), Some(ms(600)));
    assert_eq!(r.poll(599_999), None);
    assert_eq!(r.poll(ms(600)), Some(6_800_000));
    assert_eq!(r.request(ms(700), 3_000_000), Some(3_000_000));
    assert_eq!(r.request(ms(800), 3_100_000), None);
    assert_eq!(r.deadline(), Some(s(5) + ms(800)));
    assert_eq!(r.request(s(1), 3_200_000), None);
    assert_eq!(r.deadline(), Some(5_800_000));
    assert_eq!(r.poll(5_799_999), None);
    assert_eq!(r.poll(5_800_000), Some(3_200_000));
    assert_eq!(r.request(s(6), 4_000_000), None);
    assert_eq!(r.deadline(), Some(7_800_000));
    assert_eq!(r.poll(7_800_000), Some(4_000_000));
    assert_eq!(r.request(s(8), 4_000_000), None);
    assert_eq!(r.deadline(), None);
    assert_eq!(r.request(8_100_000, 3_900_000), None);
    assert_eq!(r.deadline(), Some(13_100_000));
    assert_eq!(r.request(8_200_000, 4_000_000), None);
    assert_eq!(r.deadline(), None);
    assert_eq!(r.current(), 4_000_000);

    let mut free = RateApplier::new(RateCost::Free, 10_000_000);
    assert_eq!(free.request(0, 10_100_000), None);
    assert_eq!(free.request(1, 10_200_000), Some(10_200_000));
    assert_eq!(free.request(2, 5_000_000), Some(5_000_000));
    let mut reopen = RateApplier::new(RateCost::Reopen, 10_000_000);
    assert_eq!(reopen.request(0, 8_000_000), Some(8_000_000));
    assert_eq!(reopen.request(ms(500), 7_000_000), None);
    assert_eq!(reopen.deadline(), Some(s(1)));
    assert_eq!(reopen.poll(s(1)), Some(7_000_000));
    reopen.reset(RateCost::Idr, 5_000_000);
    assert_eq!(reopen.request(1, 4_000_000), Some(4_000_000));
}

#[test]
fn key_gate_mailbox_pts() {
    let mut key = KeyTracker::new();
    assert!(!key.pending());
    key.request(1_000);
    assert_eq!(key.deadline(), Some(61_000));
    key.request(5_000);
    assert!(!key.poll(60_999));
    assert!(key.take_for_frame());
    assert!(!key.take_for_frame());
    key.request(ms(100));
    assert!(key.poll(ms(160)));

    let mut gate = OutputGate::new();
    assert!(!gate.admit(false));
    assert!(gate.admit(true));
    assert!(gate.admit(false));
    gate.on_sink_drop();
    assert!(!gate.admit(false));
    assert!(gate.admit(true));

    let mut box1 = Mailbox::new();
    assert_eq!(box1.put(1, f(false, false)), None);
    assert_eq!(box1.put(2, f(true, false)), Some(1));
    assert_eq!(box1.put(3, f(false, false)), Some(2));
    assert_eq!(box1.take(), Some((3, f(true, false))));
    assert_eq!(box1.put(6, f(false, false)), None);
    assert_eq!(box1.put(7, f(false, true)), Some(6));
    assert_eq!(box1.take(), Some((7, f(false, false))));

    let mut pts = PtsMap::new();
    assert_eq!(
        pts.next_pts(FrameMeta {
            capture_ts_us: 100,
            refine: false,
            submit_us: 101
        }),
        0
    );
    assert_eq!(
        pts.next_pts(FrameMeta {
            capture_ts_us: 200,
            refine: false,
            submit_us: 201
        }),
        1
    );
    assert_eq!(pts.take(1).unwrap().capture_ts_us, 200);
    assert_eq!(pts.take(5), None);
    pts.clear();
    for i in 0..40 {
        assert_eq!(
            pts.next_pts(FrameMeta {
                capture_ts_us: 1000 + i,
                refine: false,
                submit_us: 1001 + i
            }),
            i as i64 + 2
        );
    }
    assert_eq!(pts.len(), 32);
}

#[test]
fn nal_selection_failure_and_stats() {
    let sc = [0, 0, 0, 1];
    let vps = [0x40, 1, 0xaa];
    let sps = [0x42, 1, 0xbb];
    let pps = [0x44, 1, 0xcc];
    let idr = [0x26, 1, 0xdd];
    let p = [0x02, 1, 0xee];
    let mut au = Vec::new();
    for n in [&vps[..], &sps[..], &pps[..], &idr[..]] {
        au.extend(sc);
        au.extend(n);
    }
    assert_eq!(nal_type(Codec::Hevc, &idr), Some(19));
    assert!(au_is_key(Codec::Hevc, &au));
    assert!(!au_is_key(Codec::Hevc, &[0, 0, 0, 1, p[0], p[1], p[2]]));
    let mut cache = ParamSetCache::new(Codec::Hevc);
    assert_eq!(cache.fix(&au, true), au);
    let mut only_idr = Vec::from(sc);
    only_idr.extend(idr);
    let fixed = cache.fix(&only_idr, true);
    assert_eq!(&fixed[..12], &au[..12]);

    let mut probes = ProbeTable::default();
    let luid = 0x1_0000_0001;
    probes.record(ProbeEntry {
        luid,
        vendor_id: VENDOR_NVIDIA,
        has_output: true,
        backend: Backend::FfNvenc,
        codec: Codec::Hevc,
        result: ProbeResult::Ok,
    });
    assert_eq!(probes.codec_supported(Codec::Hevc), Some(true));
    assert_eq!(
        plan(
            VENDOR_NVIDIA,
            luid,
            &VideoEncoder::Auto,
            Codec::Hevc,
            true,
            &probes
        ),
        vec![Backend::FfNvenc, Backend::MfHw]
    );
    assert_eq!(
        probe_plan(VENDOR_NVIDIA),
        vec![Backend::FfNvenc, Backend::MfHw]
    );
    assert_eq!(reopen_backoff(0), 0);
    assert_eq!(reopen_backoff(5), 8_000_000);
    let mut fail = FailureState::new();
    assert!(!fail.should_report(0));
    fail.on_failure(s(1));
    assert!(fail.should_report(s(1)));
    assert!(!fail.should_report(s(2)));
    assert!(fail.should_report(s(6)));
    fail.on_device_change();
    assert_eq!(fail.retry_at(), Some(0));
    fail.on_success();
    assert!(!fail.failed());
    let mut lat = LatencyWindow::new();
    for i in 1..=100 {
        lat.push(i);
    }
    assert_eq!(lat.percentile(50), Some(50));
    assert_eq!(lat.percentile(95), Some(95));
    assert_eq!(parse_encoder_pref(" NVENC "), Some(VideoEncoder::Nvenc));
    assert_eq!(
        source_change(
            None,
            SourceKey {
                device_gen: 1,
                width: 1,
                height: 1,
                rotation: 0
            }
        ),
        SourceChange::Reopen
    );
    assert_eq!(
        refine_action(RefineMode::RoiDeltaQp(-8), true, false, true, 1),
        RefineAction::Roi(-8, 51)
    );
}
