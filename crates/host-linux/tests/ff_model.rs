use std::collections::HashSet;
use std::path::{Path, PathBuf};

use host_core::encode_model::{self, Backend};
use host_linux::ff_model::*;
use rsp_core::pb::Codec;

fn node(path: &str, vendor: u32, driver: Option<&str>) -> GpuNode {
    GpuNode {
        dev_path: PathBuf::from(path),
        vendor,
        driver: driver.map(str::to_owned),
    }
}

fn backends(candidates: &[Candidate]) -> Vec<LinuxBackend> {
    candidates
        .iter()
        .map(|candidate| candidate.backend)
        .collect()
}

#[test]
fn backend_props() {
    assert_eq!(LinuxBackend::Vaapi.name(), "vaapi");
    assert_eq!(LinuxBackend::Nvenc.name(), "nvenc");
    assert_eq!(LinuxBackend::OpenH264.name(), "openh264");
    assert!(LinuxBackend::Vaapi.is_ffmpeg());
    assert!(LinuxBackend::Nvenc.is_ffmpeg());
    assert!(!LinuxBackend::OpenH264.is_ffmpeg());
    assert_eq!(LinuxBackend::Vaapi.rate_cost(), RateCost::Reopen);
    assert_eq!(LinuxBackend::Nvenc.rate_cost(), RateCost::Idr);
    assert_eq!(LinuxBackend::OpenH264.rate_cost(), RateCost::Reopen);

    for backend in [LinuxBackend::Vaapi, LinuxBackend::Nvenc] {
        assert!(backend.supports(Codec::Hevc));
        assert!(backend.supports(Codec::H264));
        assert!(!backend.supports(Codec::Unspecified));
    }
    assert!(LinuxBackend::OpenH264.supports(Codec::H264));
    assert!(!LinuxBackend::OpenH264.supports(Codec::Hevc));
    assert!(!LinuxBackend::OpenH264.supports(Codec::Unspecified));
}

#[test]
fn pref_parse() {
    assert_eq!(parse_pref(" VAAPI "), Some(LinuxEncoderPref::Vaapi));
    assert_eq!(parse_pref("openh264"), Some(LinuxEncoderPref::OpenH264));
    assert_eq!(parse_pref("qsv"), None);
    assert_eq!(parse_pref("mf"), None);
    assert_eq!(parse_pref(""), None);
    assert_eq!(ENCODER_ENV, "RS_VIDEO_ENCODER");
}

#[test]
fn auto_order() {
    let intel = node("/dev/dri/renderD128", VENDOR_INTEL, Some("i915"));
    let nv = node("/dev/dri/renderD129", VENDOR_NVIDIA, Some("nvidia"));
    let nouveau = node("/dev/dri/renderD129", VENDOR_NVIDIA, Some("nouveau"));
    let amd = node("/dev/dri/renderD130", VENDOR_AMD, None);

    let hevc = candidates(
        &[intel.clone(), nv.clone()],
        LinuxEncoderPref::Auto,
        Codec::Hevc,
        true,
    );
    assert_eq!(backends(&hevc), [LinuxBackend::Nvenc, LinuxBackend::Vaapi]);
    assert_eq!(hevc[1].render_node, Some(intel.dev_path.clone()));

    let h264 = candidates(
        &[intel.clone(), nv],
        LinuxEncoderPref::Auto,
        Codec::H264,
        true,
    );
    assert_eq!(
        backends(&h264),
        [
            LinuxBackend::Nvenc,
            LinuxBackend::Vaapi,
            LinuxBackend::OpenH264
        ]
    );

    assert_eq!(
        backends(&candidates(
            &[nouveau],
            LinuxEncoderPref::Auto,
            Codec::H264,
            true,
        )),
        [LinuxBackend::OpenH264]
    );
    let amd_candidates = candidates(
        std::slice::from_ref(&amd),
        LinuxEncoderPref::Auto,
        Codec::Hevc,
        true,
    );
    assert_eq!(amd_candidates[0].backend, LinuxBackend::Vaapi);
    assert_eq!(amd_candidates[0].render_node, Some(amd.dev_path));
    assert_eq!(
        backends(&candidates(&[], LinuxEncoderPref::Auto, Codec::H264, false)),
        [LinuxBackend::OpenH264]
    );
    assert!(candidates(&[], LinuxEncoderPref::Auto, Codec::Hevc, true).is_empty());
    assert_eq!(
        backends(&candidates(
            &[intel],
            LinuxEncoderPref::Auto,
            Codec::Unspecified,
            true,
        )),
        [LinuxBackend::Vaapi]
    );
}

#[test]
fn forced_order() {
    let intel = node("/dev/dri/renderD128", VENDOR_INTEL, Some("i915"));
    let nv = node("/dev/dri/renderD129", VENDOR_NVIDIA, Some("nvidia"));
    let nodes = [intel.clone(), nv];
    assert_eq!(
        backends(&candidates(
            &nodes,
            LinuxEncoderPref::Vaapi,
            Codec::H264,
            true,
        )),
        [
            LinuxBackend::Vaapi,
            LinuxBackend::Nvenc,
            LinuxBackend::OpenH264
        ]
    );
    assert_eq!(
        backends(&candidates(
            &nodes,
            LinuxEncoderPref::OpenH264,
            Codec::H264,
            true,
        )),
        [
            LinuxBackend::OpenH264,
            LinuxBackend::Nvenc,
            LinuxBackend::Vaapi
        ]
    );
    assert_eq!(
        backends(&candidates(
            std::slice::from_ref(&intel),
            LinuxEncoderPref::Nvenc,
            Codec::H264,
            true,
        )),
        [LinuxBackend::Vaapi, LinuxBackend::OpenH264]
    );
    assert_eq!(
        backends(&candidates(
            &nodes,
            LinuxEncoderPref::OpenH264,
            Codec::Hevc,
            true,
        )),
        [LinuxBackend::Nvenc, LinuxBackend::Vaapi]
    );
}

#[test]
fn codec_names_and_formats() {
    assert_eq!(
        ffmpeg_codec_name(LinuxBackend::Vaapi, Codec::Hevc),
        Some("hevc_vaapi")
    );
    assert_eq!(
        ffmpeg_codec_name(LinuxBackend::Vaapi, Codec::H264),
        Some("h264_vaapi")
    );
    assert_eq!(
        ffmpeg_codec_name(LinuxBackend::Nvenc, Codec::Hevc),
        Some("hevc_nvenc")
    );
    assert_eq!(
        ffmpeg_codec_name(LinuxBackend::Nvenc, Codec::H264),
        Some("h264_nvenc")
    );
    assert_eq!(ffmpeg_codec_name(LinuxBackend::OpenH264, Codec::H264), None);
    assert_eq!(
        ffmpeg_codec_name(LinuxBackend::Vaapi, Codec::Unspecified),
        None
    );
    assert_eq!(input_pix_fmt(LinuxBackend::Vaapi), Some("vaapi"));
    assert_eq!(input_pix_fmt(LinuxBackend::Nvenc), Some("bgr0"));
    assert_eq!(input_pix_fmt(LinuxBackend::OpenH264), None);
    assert_eq!(VAAPI_SW_FORMAT, "nv12");
    assert_eq!(SRC_PIX_FMT, "bgr0");
    assert_eq!(HWDEVICE_VAAPI, "vaapi");
}

#[test]
fn validate_and_vaapi_options() {
    let p = validate(Codec::Hevc, 1920, 1080, 60, 8000).unwrap();
    let options = ffmpeg_options(LinuxBackend::Vaapi, &p);
    let expected = [
        ("pixel_format", "vaapi", true),
        ("video_size", "1920x1080", true),
        ("time_base", "1/60", true),
        ("b", "8000000", true),
        ("maxrate", "8000000", true),
        ("bufsize", &vbv_bits(8_000_000, 60).to_string(), true),
        ("bf", "0", true),
        ("g", "2147483647", true),
        ("keyint_min", "2147483647", false),
        ("flags", "+cgop+low_delay", false),
        ("color_range", "tv", true),
        ("colorspace", "bt709", true),
        ("color_primaries", "bt709", true),
        ("color_trc", "bt709", true),
        ("rc_mode", "CBR", true),
        ("async_depth", "1", false),
        ("aud", "0", false),
        ("profile", "main", false),
    ];
    assert_eq!(options.len(), expected.len());
    for (actual, (name, value, required)) in options.iter().zip(expected) {
        assert_eq!(actual.name, name);
        assert_eq!(actual.value, value);
        assert_eq!(actual.required, required);
    }

    let h264 = EncParams {
        codec: Codec::H264,
        ..p
    };
    let h264_options = ffmpeg_options(LinuxBackend::Vaapi, &h264);
    assert_eq!(
        h264_options[h264_options.len() - 2..]
            .iter()
            .map(|option| (option.name, option.value.as_str()))
            .collect::<Vec<_>>(),
        [("profile", "high"), ("sei", "0")]
    );
}

#[test]
fn nvenc_options_match_windows() {
    let p = validate(Codec::Hevc, 1920, 1080, 60, 8000).unwrap();
    let linux = ffmpeg_options(LinuxBackend::Nvenc, &p);
    let windows = encode_model::ffmpeg_options(Backend::FfNvenc, &p);
    assert_eq!(linux.len(), windows.len());
    for (linux, windows) in linux.iter().zip(windows.iter()) {
        if linux.name == "pixel_format" {
            assert_eq!(linux.value, "bgr0");
            assert_eq!(windows.value, "d3d11");
        } else {
            assert_eq!(
                (linux.name, linux.value.as_str(), linux.required),
                (windows.name, windows.value.as_str(), windows.required)
            );
        }
    }
    assert_eq!(linux[0].value, "bgr0");
}

#[test]
fn retry_and_rate() {
    let p = validate(Codec::Hevc, 1920, 1080, 60, 8000).unwrap();
    let options = ffmpeg_options(LinuxBackend::Vaapi, &p);
    let retry = ffmpeg_options_retry(LinuxBackend::Vaapi, &p).unwrap();
    assert_eq!(&retry[..options.len()], options.as_slice());
    assert_eq!(
        retry
            .last()
            .map(|option| (option.name, option.value.as_str(), option.required)),
        Some(("low_power", "1", false))
    );
    assert_eq!(ffmpeg_options_retry(LinuxBackend::Nvenc, &p), None);
    assert_eq!(ffmpeg_options_retry(LinuxBackend::OpenH264, &p), None);
    assert_eq!(ffmpeg_options(LinuxBackend::OpenH264, &p), Vec::new());
    assert_eq!(
        ffmpeg_rate_options(LinuxBackend::Nvenc, 8_000_000, 60)
            .iter()
            .map(|option| (option.name, option.value.as_str(), option.required))
            .collect::<Vec<_>>(),
        [
            ("b", "8000000", true),
            ("maxrate", "8000000", true),
            ("bufsize", "133334", true),
        ]
    );
    assert!(ffmpeg_rate_options(LinuxBackend::Vaapi, 8_000_000, 60).is_empty());
    assert!(ffmpeg_rate_options(LinuxBackend::OpenH264, 8_000_000, 60).is_empty());
}

#[test]
fn error_classes() {
    for code in [
        AVERROR_ENOSYS_LINUX,
        AVERROR_EINVAL,
        AVERROR_ENOENT,
        AVERROR_EACCES,
        AVERROR_ENCODER_NOT_FOUND,
        AVERROR_EXTERNAL,
    ] {
        assert!(open_error_is_unsupported(code), "{code}");
    }
    for code in [AVERROR_ENOMEM, AVERROR_EAGAIN, 0] {
        assert!(!open_error_is_unsupported(code), "{code}");
    }
    assert_eq!(err_name(-38), "ENOSYS");
    assert_eq!(err_name(-40), "errno 40");
    assert_eq!(err_name(-2), "ENOENT");
}

#[test]
fn lib_dirs_and_pick() {
    let dirs = lib_dirs(Path::new("/opt/rh/usr/bin"), None);
    assert_eq!(
        dirs,
        [
            PathBuf::from("/opt/rh/usr/bin/../lib/ffmpeg"),
            PathBuf::from("/opt/rh/usr/bin/ffmpeg")
        ]
    );
    assert_eq!(
        lib_dirs(Path::new("/ignored"), Some(" /x ")),
        [PathBuf::from("/x")]
    );
    assert_eq!(
        lib_dirs(Path::new("/opt/rh/usr/bin"), Some("")),
        [
            PathBuf::from("/opt/rh/usr/bin/../lib/ffmpeg"),
            PathBuf::from("/opt/rh/usr/bin/ffmpeg")
        ]
    );
    assert_eq!(LIB_LOAD_ORDER, [AVUTIL_SO, SWSCALE_SO, AVCODEC_SO]);

    let dirs = vec![PathBuf::from("/first"), PathBuf::from("/second")];
    let existing: HashSet<PathBuf> = [
        PathBuf::from("/second").join(AVUTIL_SO),
        PathBuf::from("/second").join(SWSCALE_SO),
        PathBuf::from("/second").join(AVCODEC_SO),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        pick_lib_dir(&dirs, |path| existing.contains(path)),
        Some(PathBuf::from("/second"))
    );

    let missing = HashSet::from([PathBuf::from("/first").join(AVUTIL_SO)]);
    assert_eq!(pick_lib_dir(&dirs, |path| missing.contains(path)), None);
}
