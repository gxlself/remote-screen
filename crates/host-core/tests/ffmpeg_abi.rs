use host_core::ffmpeg_abi::*;
use std::path::{Path, PathBuf};

#[test]
fn versions_accept_only_ffmpeg_8_majors_in_order() {
    assert_eq!(version_parts(4070501), (62, 28, 101));
    assert_eq!(version_parts(3938917), (60, 26, 101));
    assert_eq!(check_versions(4070501, 3938917), Ok(()));
    assert_eq!(check_versions(62 << 16, 60 << 16), Ok(()));
    for found in [(61 << 16) | 0x0a64, 63 << 16] {
        assert_eq!(
            check_versions(found, 60 << 16),
            Err(AbiError::Version {
                lib: "avcodec",
                found
            })
        );
    }
    assert_eq!(
        check_versions(62 << 16, 59 << 16),
        Err(AbiError::Version {
            lib: "avutil",
            found: 59 << 16
        })
    );
    assert_eq!(
        check_versions(0, 0),
        Err(AbiError::Version {
            lib: "avcodec",
            found: 0
        })
    );
}

#[test]
fn licenses_reject_gpl_nonfree_and_forbidden_build_flags() {
    let lgpl = "LGPL version 2.1 or later";
    assert_eq!(
        check_license(
            lgpl,
            lgpl,
            "--enable-shared --disable-static --enable-libvpl --enable-amf --enable-ffnvcodec"
        ),
        Ok(())
    );
    assert_eq!(
        check_license(
            "LGPL version 3 or later",
            "LGPL version 3 or later",
            "--enable-version3 --enable-shared"
        ),
        Ok(())
    );
    for text in [
        "GPL version 2 or later",
        "nonfree and unredistributable",
        "",
    ] {
        assert_eq!(
            check_license(text, lgpl, ""),
            Err(AbiError::License {
                lib: "avcodec",
                text: text.into()
            })
        );
    }
    assert_eq!(
        check_license(lgpl, "GPL version 3 or later", ""),
        Err(AbiError::License {
            lib: "avutil",
            text: "GPL version 3 or later".into()
        })
    );
    for (configuration, flag) in [
        ("--enable-shared --enable-gpl", "--enable-gpl"),
        ("--enable-nonfree", "--enable-nonfree"),
        ("--enable-nonfree --enable-gpl", "--enable-gpl"),
    ] {
        assert_eq!(
            check_license(lgpl, lgpl, configuration),
            Err(AbiError::Config(flag))
        );
    }
}

#[test]
fn directory_uses_trimmed_override_or_install_subdir() {
    for value in [None, Some(""), Some("  ")] {
        assert_eq!(
            ffmpeg_dir(Path::new("C:/rs/bin"), value),
            PathBuf::from("C:/rs/bin").join("ffmpeg")
        );
    }
    for value in ["D:/ff", "  D:/ff \n"] {
        assert_eq!(
            ffmpeg_dir(Path::new("C:/rs/bin"), Some(value)),
            PathBuf::from("D:/ff")
        );
    }
}

#[test]
fn offsets_are_complete_and_checked_in_contract_order() {
    assert_eq!(OPTION_OFFSETS.len(), 10);
    let found: Vec<_> = OPTION_OFFSETS
        .iter()
        .map(|&(key, offset)| (key, Some(offset)))
        .collect();
    assert_eq!(check_offsets(&found), Ok(()));
    for (field, value, want) in [
        ("g", Some(336), 332),
        ("bf", None, 200),
        ("maxrate", None, 464),
    ] {
        let mut altered = found.clone();
        if field == "maxrate" {
            altered.retain(|&(name, _)| name != field);
        } else {
            altered
                .iter_mut()
                .find(|(name, _)| *name == field)
                .unwrap()
                .1 = value;
        }
        assert_eq!(
            check_offsets(&altered),
            Err(AbiError::Offset {
                field,
                found: value.unwrap_or(-1),
                want
            })
        );
    }
    let mut reversed = found.clone();
    reversed.reverse();
    assert_eq!(check_offsets(&reversed), Ok(()));
    assert_eq!(
        check_offsets(&[]),
        Err(AbiError::Offset {
            field: "b",
            found: -1,
            want: 56
        })
    );
}

#[test]
fn abi_constants_snapshot() {
    assert_eq!(off::ctx::HW_FRAMES_CTX, 552);
    assert_eq!(off::frame::PTS, 136);
    assert_eq!(off::frame::PICT_TYPE, 120);
    assert_eq!(off::frame::FORMAT, 116);
    assert_eq!(off::packet::FLAGS, 40);
    assert_eq!(off::hwframes::INITIAL_POOL_SIZE, 56);
    assert_eq!(off::d3d11_frames::BIND_FLAGS, 8);
    assert_eq!(off::side_data::DATA, 8);
    assert_eq!(AV_PIX_FMT_D3D11, 171);
    assert_eq!(AV_PIX_FMT_QSV, 114);
    assert_eq!(AV_HWDEVICE_TYPE_QSV, 5);
    assert_eq!(AV_FRAME_DATA_REGIONS_OF_INTEREST, 18);
    assert_eq!(AVERROR_EAGAIN, -11);
    assert_eq!(
        (AVUTIL_DLL, AVCODEC_DLL),
        ("avutil-60.dll", "avcodec-62.dll")
    );
    assert_eq!((RECEIVE_WAIT_US, FLUSH_WAIT_US), (20_000, 100_000));
}

#[test]
fn native_qsv_and_mapped_d3d11_validate_different_data_slots() {
    assert_eq!(hw_frame_data_offset(AV_PIX_FMT_QSV), 24);
    assert_eq!(hw_frame_data_offset(AV_PIX_FMT_D3D11), 0);
}

#[test]
fn error_names_cover_errno_tags_unknown_and_integer_minimum() {
    for (code, name) in [
        (-11, "EAGAIN"),
        (-22, "EINVAL"),
        (-12, "ENOMEM"),
        (-40, "ENOSYS"),
        (-541478725, "EOF"),
        (-1414549496, "OPTION_NOT_FOUND"),
        (-1129203192, "ENCODER_NOT_FOUND"),
        (-542398533, "EXTERNAL"),
        (-1313558101, "UNKNOWN"),
        (-558323010, "BUG!"),
        (-5, "errno 5"),
        (7, "7"),
        (-1179861752, "error -1179861752"),
        (i32::MIN, "error -2147483648"),
        (0, "0"),
    ] {
        assert_eq!(err_name(code), name);
    }
    for code in [
        AVERROR_ENCODER_NOT_FOUND,
        AVERROR_OPTION_NOT_FOUND,
        -40,
        -22,
    ] {
        assert!(open_error_is_unsupported(code));
    }
    for code in [
        AVERROR_EXTERNAL,
        -12,
        AVERROR_UNKNOWN,
        AVERROR_EOF,
        AVERROR_EAGAIN,
        0,
    ] {
        assert!(!open_error_is_unsupported(code));
    }
}

#[test]
fn roi_has_exact_little_endian_c_layout() {
    assert_eq!(
        roi_bytes(1920, 1080, (-8, 51)),
        [
            28, 0, 0, 0, 0, 0, 0, 0, 0x38, 4, 0, 0, 0, 0, 0, 0, 0x80, 7, 0, 0, 0xf8, 0xff, 0xff,
            0xff, 51, 0, 0, 0
        ]
    );
}

#[test]
fn log_levels_downgrade_info_and_ignore_trace() {
    for level in [0, 8, 16] {
        assert_eq!(log_target(level), Some(LogTarget::Error));
    }
    assert_eq!(log_target(-8), None);
    assert_eq!(log_target(24), Some(LogTarget::Warn));
    assert_eq!(log_target(32), Some(LogTarget::Debug));
    for level in [40, 48] {
        assert_eq!(log_target(level), Some(LogTarget::Trace));
    }
    assert_eq!(log_target(56), None);
}

#[test]
fn log_lines_are_bounded_by_characters_and_sanitized() {
    assert_eq!(
        clean_log_line(b"[hevc_nvenc @ 000001] hello\n"),
        "[hevc_nvenc @ 000001] hello"
    );
    assert_eq!(clean_log_line(b"a\r\n"), "a");
    assert_eq!(clean_log_line(b"x\x01y\tz"), "x y\tz");
    assert_eq!(clean_log_line(&[0xff]), "\u{fffd}");
    assert_eq!(clean_log_line("a".repeat(600).as_bytes()), "a".repeat(512));
    assert_eq!(clean_log_line("é".repeat(300).as_bytes()), "é".repeat(300));
    assert_eq!(clean_log_line("é".repeat(600).as_bytes()), "é".repeat(512));
}

#[test]
fn limiter_reports_previous_window_and_counts_the_summary_line() {
    let mut limiter = LogLimiter::new(3);
    for now in [0, 1, 2] {
        assert_eq!(limiter.admit(now), LogAdmit::Pass);
    }
    for now in [3, 500_000] {
        assert_eq!(limiter.admit(now), LogAdmit::Drop);
    }
    assert_eq!(limiter.admit(1_000_000), LogAdmit::PassWithSuppressed(2));
    for now in [1_000_001, 1_000_002] {
        assert_eq!(limiter.admit(now), LogAdmit::Pass);
    }
    assert_eq!(limiter.admit(1_000_003), LogAdmit::Drop);
    assert_eq!(limiter.admit(5_000_000), LogAdmit::PassWithSuppressed(1));
    assert_eq!(limiter.admit(5_000_001), LogAdmit::Pass);
}

#[test]
fn limiter_window_starts_on_first_call_and_does_not_overflow() {
    let mut limiter = LogLimiter::new(1);
    assert_eq!(limiter.admit(500_000), LogAdmit::Pass);
    assert_eq!(limiter.admit(1_000_000), LogAdmit::Drop);
    assert_eq!(limiter.admit(1_500_000), LogAdmit::PassWithSuppressed(1));
    let mut end = LogLimiter::new(1);
    assert_eq!(end.admit(u64::MAX - 1), LogAdmit::Pass);
    assert_eq!(end.admit(u64::MAX), LogAdmit::Drop);
}
