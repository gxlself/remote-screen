use std::path::Path;

use host_core::ffmpeg_abi as host;
use rs_desktop::{
    platform::VideoCodec,
    win_ffmpeg_abi::{self as abi, off},
};

#[test]
fn constants_match_host() {
    assert_eq!(abi::AVCODEC_MAJOR, host::AVCODEC_MAJOR);
    assert_eq!(abi::AVUTIL_MAJOR, host::AVUTIL_MAJOR);
    assert_eq!(abi::AVCODEC_DLL, host::AVCODEC_DLL);
    assert_eq!(abi::AVUTIL_DLL, host::AVUTIL_DLL);
    assert_eq!(abi::FFMPEG_DIR_ENV, host::FFMPEG_DIR_ENV);
    assert_eq!(abi::FFMPEG_SUBDIR, host::FFMPEG_SUBDIR);
    assert_eq!(abi::AV_PIX_FMT_D3D11, host::AV_PIX_FMT_D3D11);
    assert_eq!(
        abi::AV_HWDEVICE_TYPE_D3D11VA,
        host::AV_HWDEVICE_TYPE_D3D11VA
    );
    assert_eq!(abi::AVERROR_EAGAIN, host::AVERROR_EAGAIN);
    assert_eq!(abi::AVERROR_EOF, host::AVERROR_EOF);
}

#[test]
fn offsets_match_host() {
    assert_eq!(off::ctx::FLAGS, host::off::ctx::FLAGS);
    assert_eq!(off::ctx::WIDTH, host::off::ctx::WIDTH);
    assert_eq!(off::ctx::HEIGHT, host::off::ctx::HEIGHT);
    assert_eq!(off::ctx::PIX_FMT, host::off::ctx::PIX_FMT);
    assert_eq!(off::ctx::HW_DEVICE_CTX, host::off::ctx::HW_DEVICE_CTX);
    assert_eq!(off::ctx::EXTRA_HW_FRAMES, host::off::ctx::EXTRA_HW_FRAMES);
    assert_eq!(off::frame::DATA, host::off::frame::DATA);
    assert_eq!(off::frame::WIDTH, host::off::frame::WIDTH);
    assert_eq!(off::frame::HEIGHT, host::off::frame::HEIGHT);
    assert_eq!(off::frame::FORMAT, host::off::frame::FORMAT);
    assert_eq!(off::frame::PTS, host::off::frame::PTS);
    assert_eq!(off::packet::PTS, host::off::packet::PTS);
    assert_eq!(off::packet::DATA, host::off::packet::DATA);
    assert_eq!(off::packet::SIZE, host::off::packet::SIZE);
    assert_eq!(off::packet::FLAGS, host::off::packet::FLAGS);
    assert_eq!(off::buffer_ref::DATA, host::off::buffer_ref::DATA);
    assert_eq!(off::hwdev::TYPE, host::off::hwdev::TYPE);
    assert_eq!(off::hwdev::HWCTX, host::off::hwdev::HWCTX);
    assert_eq!(off::d3d11_dev::DEVICE, host::off::d3d11_dev::DEVICE);
    assert_eq!(off::option::OFFSET, host::off::option::OFFSET);
}

#[test]
fn dec_option_offsets_subset_of_host() {
    for &(name, value) in abi::DEC_OPTION_OFFSETS {
        assert_eq!(
            host::OPTION_OFFSETS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v),
            Some(value)
        );
    }
}

#[test]
fn ffmpeg_dir_same_as_host() {
    let exe = Path::new(r"C:\bin");
    for value in [Some(" C:\\ff "), Some(""), None] {
        assert_eq!(abi::ffmpeg_dir(exe, value), host::ffmpeg_dir(exe, value));
    }
}

#[test]
fn version_and_license_same_as_host() {
    for (avcodec, avutil) in [(0x3e0100, 0x3d0000), (0x3d0000, 0x3c0200)] {
        assert_eq!(
            abi::check_versions(avcodec, avutil).is_ok(),
            host::check_versions(avcodec, avutil).is_ok()
        );
    }
    for (avcodec, avutil, config) in [
        (
            "LGPL version 2.1 or later",
            "LGPL version 2.1 or later",
            "--disable-gpl",
        ),
        (
            "GPL version 2 or later",
            "LGPL version 2.1 or later",
            "--disable-gpl",
        ),
        (
            "LGPL version 2.1 or later",
            "LGPL version 2.1 or later",
            "--enable-gpl",
        ),
    ] {
        assert_eq!(
            abi::check_license(avcodec, avutil, config).is_ok(),
            host::check_license(avcodec, avutil, config).is_ok()
        );
    }
}

#[test]
fn hw_config_d3d11() {
    assert!(abi::hw_config_is_d3d11(171, 1, 7));
    assert!(abi::hw_config_is_d3d11(171, 3, 7));
    assert!(!abi::hw_config_is_d3d11(171, 2, 7));
    assert!(!abi::hw_config_is_d3d11(23, 1, 7));
    assert!(!abi::hw_config_is_d3d11(171, 1, 5));
}

#[test]
fn err_name_known() {
    assert_eq!(abi::err_name(abi::AVERROR_INVALIDDATA), "INVALIDDATA");
    assert_eq!(
        abi::err_name(abi::AVERROR_EAGAIN),
        host::err_name(host::AVERROR_EAGAIN)
    );
    assert_eq!(
        abi::err_name(abi::AVERROR_EOF),
        host::err_name(host::AVERROR_EOF)
    );
    assert_eq!(abi::err_name(-5), "errno 5");
}

#[test]
fn decoder_names_and_options() {
    assert_eq!(abi::decoder_name(VideoCodec::Hevc), c"hevc");
    assert_eq!(abi::decoder_name(VideoCodec::H264), c"h264");
    assert_eq!(
        abi::DECODER_OPTIONS,
        [
            ("threads", "1"),
            ("flags", "+low_delay"),
            ("extra_hw_frames", "2")
        ]
    );
}

#[test]
fn output_matcher_in_order() {
    let mut matcher = abi::OutputMatcher::new(8);
    assert!(matcher.push(1, "a").is_empty());
    assert!(matcher.push(2, "b").is_empty());
    assert!(matcher.push(3, "c").is_empty());
    assert_eq!(matcher.output(1), [("a", abi::Outcome::Shown)]);
    assert_eq!(matcher.output(2), [("b", abi::Outcome::Shown)]);
    assert_eq!(matcher.output(3), [("c", abi::Outcome::Shown)]);
}

#[test]
fn output_matcher_gap_fails_earlier() {
    let mut matcher = abi::OutputMatcher::new(8);
    matcher.push(1, "a");
    matcher.push(2, "b");
    matcher.push(3, "c");
    assert_eq!(
        matcher.output(3),
        [
            ("a", abi::Outcome::Failed),
            ("b", abi::Outcome::Failed),
            ("c", abi::Outcome::Shown),
        ]
    );
}

#[test]
fn output_matcher_cap_and_fail() {
    let mut matcher = abi::OutputMatcher::new(3);
    assert!(matcher.push(1, "a").is_empty());
    assert!(matcher.push(2, "b").is_empty());
    assert!(matcher.push(3, "c").is_empty());
    assert_eq!(matcher.push(4, "d"), [("a", abi::Outcome::Failed)]);
    assert_eq!(matcher.fail(3), [("c", abi::Outcome::Failed)]);
    assert!(matcher.output(9).is_empty());
    assert_eq!(
        matcher.fail_all(),
        [("b", abi::Outcome::Failed), ("d", abi::Outcome::Failed)]
    );
}

#[test]
fn output_matcher_wraps() {
    let mut matcher = abi::OutputMatcher::new(8);
    matcher.push(u32::MAX, "max");
    matcher.push(0, "zero");
    assert_eq!(
        matcher.output(0),
        [("max", abi::Outcome::Failed), ("zero", abi::Outcome::Shown)]
    );
}

#[test]
fn log_mapping_same_as_host() {
    for level in [0, 16, 24, 32, 48, 56, 64] {
        assert_eq!(
            abi::log_target(level).map(|target| format!("{target:?}")),
            host::log_target(level).map(|target| format!("{target:?}"))
        );
    }
    let mut ours = abi::LogLimiter::new(2);
    let mut theirs = host::LogLimiter::new(2);
    for now in [0, 1, 2, 1_000_001] {
        assert_eq!(
            format!("{:?}", ours.admit(now)),
            format!("{:?}", theirs.admit(now))
        );
    }
}
