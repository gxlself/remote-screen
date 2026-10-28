use host_win::audio_convert::{
    parse_wave_format, plan_open, qpc_to_unix_us, to_stereo_f32, FormatError, OpenPlan,
    SampleFormat, WaveFormat, SUBTYPE_IEEE_FLOAT, SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE,
    WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM,
};

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "{actual:?} != {expected:?}"
        );
    }
}

#[test]
fn parses_supported_and_rejects_bad_wave_formats() {
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 32, 8, None)
            .unwrap()
            .format,
        SampleFormat::F32
    );
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_PCM, 2, 44_100, 16, 4, None)
            .unwrap()
            .format,
        SampleFormat::I16
    );
    assert_eq!(
        parse_wave_format(
            WAVE_FORMAT_EXTENSIBLE,
            2,
            48_000,
            32,
            8,
            Some((24, SUBTYPE_PCM))
        )
        .unwrap()
        .format,
        SampleFormat::I24In32
    );
    let six_channel = parse_wave_format(
        WAVE_FORMAT_EXTENSIBLE,
        6,
        48_000,
        32,
        24,
        Some((32, SUBTYPE_IEEE_FLOAT)),
    )
    .unwrap();
    assert_eq!(six_channel.format, SampleFormat::F32);
    assert_eq!(six_channel.channels, 6);
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_PCM, 2, 48_000, 8, 2, None),
        Err(FormatError::Unsupported)
    );
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 32, 7, None),
        Err(FormatError::Bad)
    );
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_PCM, 0, 48_000, 16, 0, None),
        Err(FormatError::Bad)
    );
}

#[test]
fn parses_pcm_containers_and_extensible_subtypes() {
    for (bits, expected) in [
        (16, SampleFormat::I16),
        (24, SampleFormat::I24),
        (32, SampleFormat::I32),
    ] {
        for (tag, ext) in [
            (WAVE_FORMAT_PCM, None),
            (WAVE_FORMAT_EXTENSIBLE, Some((bits, SUBTYPE_PCM))),
        ] {
            let parsed = parse_wave_format(tag, 1, 48_000, bits, bits / 8, ext).unwrap();
            assert_eq!(parsed.format, expected);
            assert_eq!(parsed.sample_rate, 48_000);
            assert_eq!(parsed.channels, 1);
            assert_eq!(parsed.block_align, bits / 8);
        }
    }
    for (tag, bits, align, ext) in [
        (WAVE_FORMAT_EXTENSIBLE, 32, 8, None),
        (WAVE_FORMAT_EXTENSIBLE, 32, 8, Some((32, 0))),
        (
            WAVE_FORMAT_EXTENSIBLE,
            64,
            16,
            Some((64, SUBTYPE_IEEE_FLOAT)),
        ),
        (WAVE_FORMAT_IEEE_FLOAT, 64, 16, None),
        (0x1234, 32, 8, None),
    ] {
        assert_eq!(
            parse_wave_format(tag, 2, 48_000, bits, align, ext),
            Err(FormatError::Unsupported)
        );
    }
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_PCM, 1, 0, 16, 2, None),
        Err(FormatError::Bad)
    );
    assert_eq!(
        parse_wave_format(WAVE_FORMAT_PCM, u16::MAX, 48_000, 32, 0, None),
        Err(FormatError::Bad)
    );
}

#[test]
fn converts_i16_stereo_and_mono() {
    let stereo = WaveFormat {
        sample_rate: 48_000,
        channels: 2,
        block_align: 4,
        format: SampleFormat::I16,
    };
    let mut out = Vec::new();
    to_stereo_f32(&[0x00, 0x40, 0x00, 0xc0], &stereo, 1, &mut out).unwrap();
    assert_samples(&out, &[0.5, -0.5]);

    let mono = WaveFormat {
        channels: 1,
        block_align: 2,
        ..stereo
    };
    out.clear();
    to_stereo_f32(&[0xff, 0x7f, 0x00, 0x80], &mono, 2, &mut out).unwrap();
    assert_samples(&out, &[32767.0 / 32768.0, 32767.0 / 32768.0, -1.0, -1.0]);
}

#[test]
fn converts_i24_and_32_bit_samples() {
    let i24 = WaveFormat {
        sample_rate: 48_000,
        channels: 2,
        block_align: 6,
        format: SampleFormat::I24,
    };
    let mut out = Vec::new();
    to_stereo_f32(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xc0], &i24, 1, &mut out).unwrap();
    assert_samples(&out, &[0.5, -0.5]);

    for format in [SampleFormat::I24In32, SampleFormat::I32] {
        let fmt = WaveFormat {
            sample_rate: 48_000,
            channels: 1,
            block_align: 4,
            format,
        };
        out.clear();
        to_stereo_f32(&0x4000_0000_i32.to_le_bytes(), &fmt, 1, &mut out).unwrap();
        assert_samples(&out, &[0.5, 0.5]);
    }
}

#[test]
fn converts_signed_integer_boundaries() {
    for (format, bytes) in [
        (SampleFormat::I24, vec![0xff, 0xff, 0x7f, 0, 0, 0x80]),
        (
            SampleFormat::I24In32,
            vec![0, 0xff, 0xff, 0x7f, 0, 0, 0, 0x80],
        ),
        (
            SampleFormat::I32,
            vec![0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0x80],
        ),
    ] {
        let fmt = WaveFormat {
            sample_rate: 48_000,
            channels: 2,
            block_align: bytes.len() as u16,
            format,
        };
        let mut out = Vec::new();
        to_stereo_f32(&bytes, &fmt, 1, &mut out).unwrap();
        assert!((out[0] - 1.0).abs() < 1.0e-6);
        assert_eq!(out[1], -1.0);
    }
}

#[test]
fn converts_float_front_channels_and_sanitizes_non_finite_values() {
    let fmt = WaveFormat {
        sample_rate: 48_000,
        channels: 6,
        block_align: 24,
        format: SampleFormat::F32,
    };
    let values = [0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6];
    let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut out = Vec::new();
    to_stereo_f32(&data, &fmt, 1, &mut out).unwrap();
    assert_samples(&out, &[0.1, 0.2]);

    let stereo = WaveFormat {
        channels: 2,
        block_align: 8,
        ..fmt
    };
    let data: Vec<_> = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    out.clear();
    to_stereo_f32(&data, &stereo, 2, &mut out).unwrap();
    assert_eq!(out, [0.0, 0.0, 0.0, 1.5]);
}

#[test]
fn conversion_checks_length_and_appends() {
    let fmt = WaveFormat {
        sample_rate: 48_000,
        channels: 2,
        block_align: 4,
        format: SampleFormat::I16,
    };
    let mut out = vec![9.0];
    assert_eq!(
        to_stereo_f32(&[0, 0, 0], &fmt, 1, &mut out),
        Err(FormatError::Bad)
    );
    assert_eq!(out, vec![9.0]);
    to_stereo_f32(&[0, 0, 0, 0], &fmt, 1, &mut out).unwrap();
    assert_eq!(out, vec![9.0, 0.0, 0.0]);
}

#[test]
fn conversion_advances_by_all_channels_and_ignores_trailing_bytes() {
    let fmt = WaveFormat {
        sample_rate: 48_000,
        channels: 3,
        block_align: 6,
        format: SampleFormat::I16,
    };
    let mut out = vec![9.0];
    let data: Vec<_> = [16_384_i16, -16_384, 123, 0, i16::MIN, 456, 789]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    to_stereo_f32(&data, &fmt, 2, &mut out).unwrap();
    assert_eq!(out, [9.0, 0.5, -0.5, 0.0, -1.0]);
    to_stereo_f32(&[], &fmt, 0, &mut out).unwrap();
    assert_eq!(out, [9.0, 0.5, -0.5, 0.0, -1.0]);
}

#[test]
fn conversion_rejects_malformed_public_formats_without_appending() {
    let valid = WaveFormat {
        sample_rate: 48_000,
        channels: 2,
        block_align: 8,
        format: SampleFormat::F32,
    };
    for fmt in [
        WaveFormat {
            channels: 0,
            ..valid
        },
        WaveFormat {
            block_align: 0,
            ..valid
        },
        WaveFormat {
            block_align: 7,
            ..valid
        },
        WaveFormat {
            sample_rate: 0,
            ..valid
        },
    ] {
        let mut out = vec![9.0];
        assert_eq!(
            to_stereo_f32(&[0; 8], &fmt, 1, &mut out),
            Err(FormatError::Bad)
        );
        assert_eq!(out, [9.0]);
    }
}

#[test]
fn converts_qpc_and_plans_open() {
    assert_eq!(qpc_to_unix_us(1_000_000, 1_010_000, 5_000_000), 4_999_000);
    assert_eq!(qpc_to_unix_us(2, 1, 5_000_000), 5_000_000);
    assert_eq!(qpc_to_unix_us(1, u64::MAX, 5), 0);
    assert_eq!(qpc_to_unix_us(100, 109, 5), 5);
    assert_eq!(qpc_to_unix_us(100, 110, 5), 4);
    assert_eq!(qpc_to_unix_us(100, 100, u64::MAX), u64::MAX);

    let mix = WaveFormat {
        sample_rate: 48_000,
        channels: 2,
        block_align: 8,
        format: SampleFormat::F32,
    };
    assert_eq!(plan_open(48_000, true, None), OpenPlan::AutoConvert);
    assert_eq!(plan_open(48_000, false, Some(mix)), OpenPlan::UseMix(mix));
    assert_eq!(
        plan_open(
            48_000,
            false,
            Some(WaveFormat {
                sample_rate: 44_100,
                ..mix
            })
        ),
        OpenPlan::Unsupported
    );
    assert_eq!(plan_open(48_000, false, None), OpenPlan::Unsupported);
    for invalid in [
        WaveFormat { channels: 0, ..mix },
        WaveFormat {
            block_align: 7,
            ..mix
        },
        WaveFormat {
            sample_rate: 0,
            ..mix
        },
    ] {
        assert_eq!(
            plan_open(48_000, false, Some(invalid)),
            OpenPlan::Unsupported
        );
    }
}
