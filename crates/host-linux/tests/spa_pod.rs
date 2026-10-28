use host_linux::spa_pod::*;

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect()
}

#[test]
fn meta_param_fixed_golden() {
    assert_eq!(
        words(&meta_param(meta_type::HEADER, MetaSize::Fixed(32))),
        vec![56, 15, 0x40005, 6, 1, 0, 4, 3, 1, 0, 2, 0, 4, 4, 32, 0,]
    );
}

#[test]
fn meta_param_range_golden() {
    assert_eq!(
        words(&meta_param(
            meta_type::VIDEO_DAMAGE,
            MetaSize::Range {
                default: 256,
                min: 16,
                max: 256,
            },
        )),
        vec![80, 15, 0x40005, 6, 1, 0, 4, 3, 3, 0, 2, 0, 28, 19, 1, 0, 4, 4, 256, 16, 256, 0,]
    );
}

#[test]
fn video_buffers_golden() {
    assert_eq!(
        words(&video_buffers(6)),
        vec![48, 15, 0x40004, 5, 6, 0, 20, 19, 4, 0, 4, 4, 6, 0]
    );
}

#[test]
fn audio_enum_format_golden() {
    assert_eq!(
        words(&audio_enum_format(audio_format::F32_LE, 48000, 2)),
        vec![
            160, 15, 0x40003, 3, 1, 0, 4, 3, 1, 0, 2, 0, 4, 3, 1, 0, 0x10001, 0, 4, 3, 0x11b, 0,
            0x10003, 0, 4, 4, 48000, 0, 0x10004, 0, 4, 4, 2, 0, 0x10005, 0, 16, 13, 4, 3, 3, 4,
        ]
    );
}

#[test]
fn video_enum_format_shape() {
    let pod = video_enum_format(
        &[
            video_format::BGRX,
            video_format::BGRA,
            video_format::RGBX,
            video_format::RGBA,
        ],
        (1920, 1080),
        (8192, 8192),
        60,
    );
    let object = parse_object(&pod).expect("valid video enum format");
    assert_eq!(object.object_type, object_type::FORMAT);
    assert_eq!(object.id, param::ENUM_FORMAT);
    assert_eq!(
        object.get(format_key::VIDEO_FORMAT),
        Some(Value::Choice {
            kind: choice::ENUM,
            values: vec![
                Value::Id(video_format::BGRX),
                Value::Id(video_format::BGRX),
                Value::Id(video_format::BGRA),
                Value::Id(video_format::RGBX),
                Value::Id(video_format::RGBA),
            ],
        })
    );
    assert_eq!(
        object.get(format_key::VIDEO_SIZE),
        Some(Value::Choice {
            kind: choice::RANGE,
            values: vec![
                Value::Rect {
                    width: 1920,
                    height: 1080,
                },
                Value::Rect {
                    width: 1,
                    height: 1,
                },
                Value::Rect {
                    width: 8192,
                    height: 8192,
                },
            ],
        })
    );
    assert_eq!(
        object.get(format_key::VIDEO_FRAMERATE),
        Some(Value::Choice {
            kind: choice::RANGE,
            values: vec![
                Value::Frac { num: 60, denom: 1 },
                Value::Frac { num: 0, denom: 1 },
                Value::Frac { num: 360, denom: 1 },
            ],
        })
    );
    assert_eq!(pod.len() % 8, 0);
}

fn fixed_video_props() -> Vec<(u32, Value)> {
    vec![
        (format_key::MEDIA_TYPE, Value::Id(media::TYPE_VIDEO)),
        (format_key::MEDIA_SUBTYPE, Value::Id(media::SUBTYPE_RAW)),
        (format_key::VIDEO_FORMAT, Value::Id(video_format::BGRX)),
        (
            format_key::VIDEO_SIZE,
            Value::Rect {
                width: 1280,
                height: 720,
            },
        ),
        (
            format_key::VIDEO_FRAMERATE,
            Value::Frac { num: 0, denom: 1 },
        ),
    ]
}

#[test]
fn parse_fixated_video_format() {
    let pod = object(object_type::FORMAT, param::FORMAT, &fixed_video_props());
    assert_eq!(
        parse_video_format(&pod).expect("fixed format"),
        VideoFormat {
            format: video_format::BGRX,
            width: 1280,
            height: 720,
            framerate: (0, 1),
            modifier: None,
        }
    );

    let mut choice_props = fixed_video_props();
    choice_props[2].1 = Value::Choice {
        kind: choice::NONE,
        values: vec![Value::Id(video_format::BGRA)],
    };
    let with_choice = object(object_type::FORMAT, param::FORMAT, &choice_props);
    assert_eq!(
        parse_video_format(&with_choice).unwrap().format,
        video_format::BGRA
    );

    choice_props.push((format_key::VIDEO_MODIFIER, Value::Long(0)));
    let with_modifier = object(object_type::FORMAT, param::FORMAT, &choice_props);
    assert_eq!(
        parse_video_format(&with_modifier).unwrap().modifier,
        Some(0)
    );
}

#[test]
fn parse_video_errors() {
    let mut props = fixed_video_props();
    props.retain(|(key, _)| *key != format_key::VIDEO_SIZE);
    let missing_size = object(object_type::FORMAT, param::FORMAT, &props);
    assert_eq!(
        parse_video_format(&missing_size),
        Err(PodError::Missing(format_key::VIDEO_SIZE))
    );

    let mut props = fixed_video_props();
    props[0].1 = Value::Id(media::TYPE_AUDIO);
    let wrong_media = object(object_type::FORMAT, param::FORMAT, &props);
    assert_eq!(
        parse_video_format(&wrong_media),
        Err(PodError::Type {
            found: media::TYPE_AUDIO,
            want: media::TYPE_VIDEO,
        })
    );

    let valid = object(object_type::FORMAT, param::FORMAT, &fixed_video_props());
    for cut in [1usize, 7, 9] {
        assert_eq!(
            parse_video_format(&valid[..valid.len() - cut]),
            Err(PodError::Truncated)
        );
    }

    let mut bad_size = valid.clone();
    // Locate the VIDEO_SIZE property's POD and make its payload impossibly large.
    let mut offset = 16usize;
    loop {
        let key = u32::from_le_bytes(bad_size[offset..offset + 4].try_into().unwrap());
        let value_start = offset + 8;
        if key == format_key::VIDEO_SIZE {
            bad_size[value_start..value_start + 4].copy_from_slice(&0xffffu32.to_le_bytes());
            break;
        }
        let size =
            u32::from_le_bytes(bad_size[value_start..value_start + 4].try_into().unwrap()) as usize;
        offset = value_start + 8 + ((size + 7) & !7);
    }
    assert_eq!(parse_video_format(&bad_size), Err(PodError::Truncated));
}

#[test]
fn parse_audio_roundtrip() {
    assert_eq!(
        parse_audio_format(&audio_enum_format(audio_format::F32_LE, 48000, 2)).unwrap(),
        AudioFormat {
            format: audio_format::F32_LE,
            rate: 48000,
            channels: 2,
        }
    );
}

#[test]
fn header_meta() {
    let mut bytes = vec![0; META_HEADER_SIZE];
    bytes[0..4].copy_from_slice(&header_flag::CORRUPTED.to_le_bytes());
    bytes[8..16].copy_from_slice(&123i64.to_le_bytes());
    bytes[24..32].copy_from_slice(&7u64.to_le_bytes());
    assert_eq!(
        parse_meta_header(&bytes),
        Some(MetaHeader {
            flags: header_flag::CORRUPTED,
            offset: 0,
            pts: 123,
            dts_offset: 0,
            seq: 7,
        })
    );
    assert_eq!(parse_meta_header(&bytes[..31]), None);
}

#[test]
fn damage_meta() {
    let mut bytes = Vec::new();
    for (x, y, width, height) in [(1i32, 2i32, 3u32, 4u32), (5, 6, 0, 8), (9, 10, 11, 12)] {
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    assert_eq!(
        parse_damage(&bytes),
        vec![Region {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        }]
    );
    assert_eq!(parse_damage(&bytes[..33]).len(), 1);
    let mut two = Vec::new();
    for (x, y, width, height) in [(1i32, 2i32, 3u32, 4u32), (5, 6, 7, 8)] {
        two.extend_from_slice(&x.to_le_bytes());
        two.extend_from_slice(&y.to_le_bytes());
        two.extend_from_slice(&width.to_le_bytes());
        two.extend_from_slice(&height.to_le_bytes());
    }
    two.push(0);
    assert_eq!(parse_damage(&two[..33]).len(), 2);
}

type BitmapInput<'a> = (u32, u32, u32, i32, u32, &'a [u8]);

fn cursor_bytes(id: u32, bitmap_offset: u32, bitmap: Option<BitmapInput<'_>>) -> Vec<u8> {
    let mut bytes = vec![0; 28];
    bytes[0..4].copy_from_slice(&id.to_le_bytes());
    bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&100i32.to_le_bytes());
    bytes[12..16].copy_from_slice(&50i32.to_le_bytes());
    bytes[16..20].copy_from_slice(&3i32.to_le_bytes());
    bytes[20..24].copy_from_slice(&4i32.to_le_bytes());
    bytes[24..28].copy_from_slice(&bitmap_offset.to_le_bytes());
    if let Some((format, width, height, stride, offset, pixels)) = bitmap {
        bytes.extend_from_slice(&format.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.resize(bytes.len().max(bitmap_offset as usize + offset as usize), 0);
        bytes.extend_from_slice(pixels);
    }
    bytes
}

#[test]
fn cursor_meta_with_bitmap() {
    let pixels: Vec<u8> = (0..16).collect();
    let bytes = cursor_bytes(5, 28, Some((video_format::BGRA, 2, 2, 8, 20, &pixels)));
    assert_eq!(
        parse_cursor(&bytes),
        Some(CursorMeta {
            id: 5,
            flags: 7,
            x: 100,
            y: 50,
            hot_x: 3,
            hot_y: 4,
            bitmap: Some(CursorBitmap {
                format: video_format::BGRA,
                width: 2,
                height: 2,
                stride: 8,
                pixels,
            }),
        })
    );
}

#[test]
fn cursor_meta_edge_cases() {
    assert_eq!(parse_cursor(&0u32.to_le_bytes()), None);
    let no_bitmap = cursor_bytes(5, 0, None);
    assert_eq!(parse_cursor(&no_bitmap).unwrap().bitmap, None);

    let too_short = cursor_bytes(5, 28, Some((video_format::BGRA, 64, 64, 256, 20, &[])));
    assert_eq!(parse_cursor(&too_short).unwrap().bitmap, None);
    let negative_stride = cursor_bytes(5, 28, Some((video_format::BGRA, 2, 2, -1, 20, &[])));
    assert_eq!(parse_cursor(&negative_stride).unwrap().bitmap, None);
}

#[test]
fn cursor_meta_size_const() {
    assert_eq!(cursor_meta_size(64, 64), 48 + 64 * 64 * 4);
}

#[test]
fn fuzz_like_no_panic() {
    let examples = vec![
        audio_enum_format(audio_format::F32_LE, 48000, 2),
        video_enum_format(
            &[
                video_format::BGRX,
                video_format::BGRA,
                video_format::RGBX,
                video_format::RGBA,
            ],
            (1920, 1080),
            (8192, 8192),
            60,
        ),
        cursor_bytes(
            5,
            28,
            Some((
                video_format::BGRA,
                2,
                2,
                8,
                20,
                &(0..16).collect::<Vec<_>>(),
            )),
        ),
    ];
    for example in examples {
        for index in 0..example.len() {
            let mut mutated = example.clone();
            mutated[index] = 0xff;
            let _ = parse_object(&mutated);
            let _ = parse_video_format(&mutated);
            let _ = parse_audio_format(&mutated);
            let _ = parse_cursor(&mutated);
        }
        for length in 0..=example.len() {
            let truncated = &example[..length];
            let _ = parse_object(truncated);
            let _ = parse_video_format(truncated);
            let _ = parse_audio_format(truncated);
            let _ = parse_cursor(truncated);
        }
    }
}
