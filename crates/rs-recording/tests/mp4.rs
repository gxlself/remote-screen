use std::fs::{self, OpenOptions};
use std::io::{Cursor, Seek, SeekFrom};
use std::path::PathBuf;

use rs_recording::mp4::{Mp4Codec, Mp4Error, Mp4Writer};

#[derive(Debug, Clone, Copy)]
struct BoxRef {
    kind: [u8; 4],
    start: usize,
    end: usize,
    payload: usize,
}

fn boxes(buf: &[u8], range: std::ops::Range<usize>) -> Vec<BoxRef> {
    let mut result = Vec::new();
    let mut cursor = range.start;
    while cursor + 8 <= range.end {
        let size32 = u32::from_be_bytes(buf[cursor..cursor + 4].try_into().unwrap());
        let size = if size32 == 1 {
            u64::from_be_bytes(buf[cursor + 8..cursor + 16].try_into().unwrap()) as usize
        } else {
            size32 as usize
        };
        let header = if size32 == 1 { 16 } else { 8 };
        assert!(size >= header && cursor + size <= range.end);
        let end = cursor + size;
        result.push(BoxRef {
            kind: buf[cursor + 4..cursor + 8].try_into().unwrap(),
            start: cursor,
            end,
            payload: cursor + header,
        });
        cursor = end;
    }
    assert_eq!(cursor, range.end);
    result
}

fn find_box(buf: &[u8], range: std::ops::Range<usize>, kind: &[u8; 4]) -> BoxRef {
    for cursor in range.start..range.end.saturating_sub(8) {
        if &buf[cursor + 4..cursor + 8] != kind {
            continue;
        }
        let size32 = u32::from_be_bytes(buf[cursor..cursor + 4].try_into().unwrap());
        let (size, header) = if size32 == 1 {
            if cursor + 16 > range.end {
                continue;
            }
            (
                u64::from_be_bytes(buf[cursor + 8..cursor + 16].try_into().unwrap()) as usize,
                16,
            )
        } else {
            (size32 as usize, 8)
        };
        if size >= header && cursor + size <= range.end {
            return BoxRef {
                kind: *kind,
                start: cursor,
                end: cursor + size,
                payload: cursor + header,
            };
        }
    }
    panic!("missing box {:?}", kind);
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap())
}

fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len() + 4);
    let mut zeroes = 0;
    for &byte in bytes {
        if zeroes >= 2 && byte <= 3 {
            output.push(3);
            zeroes = 0;
        }
        output.push(byte);
        if byte == 0 {
            zeroes += 1;
        } else {
            zeroes = 0;
        }
    }
    output
}

fn annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::new();
    for (index, nal) in nals.iter().enumerate() {
        output.extend_from_slice(if index % 2 == 0 {
            &[0, 0, 0, 1]
        } else {
            &[0, 0, 1]
        });
        output.extend_from_slice(nal);
    }
    output
}

fn hevc_nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut nal = vec![kind << 1, 1];
    nal.extend_from_slice(body);
    nal
}

fn hevc_sets(tag: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let vps = hevc_nal(32, &[tag, 0xaa]);
    let mut rbsp = vec![0x42, 0x01, 0x01];
    rbsp.extend_from_slice(&[
        0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5d,
    ]);
    rbsp.extend_from_slice(&[tag, 0, 0, 1, 2]);
    let sps = escape(&rbsp);
    let pps = hevc_nal(34, &[tag, 0xcc]);
    (vps, sps, pps)
}

fn hevc_key(tag: u8, idr_body: &[u8]) -> Vec<u8> {
    let (vps, sps, pps) = hevc_sets(tag);
    let aud = hevc_nal(35, &[0x50]);
    let idr = hevc_nal(19, idr_body);
    annexb(&[&aud, &vps, &sps, &pps, &idr])
}

fn hevc_p(body: &[u8]) -> Vec<u8> {
    let slice = hevc_nal(1, body);
    annexb(&[&slice])
}

fn h264_nal(header: u8, body: &[u8]) -> Vec<u8> {
    let mut nal = vec![header];
    nal.extend_from_slice(body);
    nal
}

fn h264_key(body: &[u8]) -> Vec<u8> {
    let sps = h264_nal(0x67, &[0x64, 0, 0x1f, 1]);
    let pps = h264_nal(0x68, &[2, 3]);
    let idr = h264_nal(0x65, body);
    annexb(&[&sps, &pps, &idr])
}

fn h264_p(body: &[u8]) -> Vec<u8> {
    let p = h264_nal(0x41, body);
    annexb(&[&p])
}

#[test]
fn hevc_layout_and_parameter_sets() {
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::Hevc, 1920, 1080).unwrap();
    for index in 0..90_u64 {
        let data = if index % 30 == 0 {
            hevc_key(0, &[index as u8, 0x77])
        } else {
            hevc_p(&[index as u8, 0x66])
        };
        writer
            .push_sample(index * 33_333, index % 30 == 0, &data)
            .unwrap();
    }
    assert_eq!(writer.sample_count(), 90);
    let output = writer.finish().unwrap().into_inner();
    let top = boxes(&output, 0..output.len());
    assert_eq!(
        top.iter().map(|item| item.kind).collect::<Vec<_>>(),
        [*b"ftyp", *b"mdat", *b"moov"]
    );

    let mdat = top[1];
    assert_eq!(read_u32(&output, mdat.start), 1);
    assert_eq!(
        read_u64(&output, mdat.start + 8) as usize,
        mdat.end - mdat.start
    );
    let stsz = find_box(&output, top[2].payload..top[2].end, b"stsz");
    assert_eq!(read_u32(&output, stsz.payload + 4), 0);
    assert_eq!(read_u32(&output, stsz.payload + 8), 90);
    let hdlr = find_box(&output, top[2].payload..top[2].end, b"hdlr");
    assert_eq!(&output[hdlr.payload + 8..hdlr.payload + 12], b"vide");
    let stss = find_box(&output, top[2].payload..top[2].end, b"stss");
    assert_eq!(read_u32(&output, stss.payload + 4), 3);
    assert_eq!(
        (0..3)
            .map(|i| read_u32(&output, stss.payload + 8 + i * 4))
            .collect::<Vec<_>>(),
        [1, 31, 61]
    );
    let stts = find_box(&output, top[2].payload..top[2].end, b"stts");
    assert_eq!(read_u32(&output, stts.payload + 4), 1);
    assert_eq!(read_u32(&output, stts.payload + 8), 90);

    let stsz_first = read_u32(&output, stsz.payload + 12) as usize;
    let co64 = find_box(&output, top[2].payload..top[2].end, b"co64");
    let first_offset = read_u64(&output, co64.payload + 8) as usize;
    assert_eq!(read_u32(&output, first_offset) as usize, stsz_first - 4);
    let sample = &output[first_offset..first_offset + stsz_first];
    assert!(!sample.windows(2).any(|window| window == [0x40, 1]));
    assert!(!sample.windows(2).any(|window| window == [0x46, 1]));

    let hvcc = find_box(&output, top[2].payload..top[2].end, b"hvcC");
    let hvcc_data = &output[hvcc.payload..hvcc.end];
    assert_eq!(hvcc_data[0], 1);
    let (vps, sps, pps) = hevc_sets(0);
    let expected_ptl = Vec::from([1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5d]);
    assert_eq!(&hvcc_data[1..13], expected_ptl.as_slice());
    assert_eq!(hvcc_data[21] & 0x18, 8);
    assert_eq!(hvcc_data[21] & 3, 3);
    assert_eq!(hvcc_data[22], 3);
    let mut cursor = 23;
    for (kind, nal) in [(32, vps), (33, sps), (34, pps)] {
        assert_eq!(hvcc_data[cursor] & 0x3f, kind);
        let count = u16::from_be_bytes(hvcc_data[cursor + 1..cursor + 3].try_into().unwrap());
        assert_eq!(count, 1);
        let length =
            u16::from_be_bytes(hvcc_data[cursor + 3..cursor + 5].try_into().unwrap()) as usize;
        assert_eq!(&hvcc_data[cursor + 5..cursor + 5 + length], nal.as_slice());
        cursor += 5 + length;
    }
    assert_eq!(cursor, hvcc_data.len());
}

#[test]
fn h264_uses_avc1_and_avcc() {
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::H264, 640, 480).unwrap();
    writer.push_sample(0, true, &h264_key(&[1, 2, 3])).unwrap();
    writer.push_sample(33_333, false, &h264_p(&[4, 5])).unwrap();
    let output = writer.finish().unwrap().into_inner();
    assert!(output[16..].windows(4).any(|window| window == b"avc1"));
    assert!(output[16..].windows(4).any(|window| window == b"avcC"));
    assert!(output[8..].windows(4).any(|window| window == b"avc1"));
    let top = boxes(&output, 0..output.len());
    let avcc = find_box(&output, top[2].payload..top[2].end, b"avcC");
    assert_eq!(
        &output[avcc.payload + 1..avcc.payload + 4],
        &[0x64, 0, 0x1f]
    );
    let ftyp = top[0];
    assert!(output[ftyp.payload + 8..ftyp.end]
        .windows(4)
        .any(|window| window == b"avc1"));
}

#[test]
fn errors_and_parameter_set_reuse() {
    assert_eq!(Mp4Codec::from_proto(1), Some(Mp4Codec::Hevc));
    assert_eq!(Mp4Codec::from_proto(2), Some(Mp4Codec::H264));
    assert_eq!(Mp4Codec::from_proto(0), None);
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::Hevc, 1, 1).unwrap();
    assert!(matches!(
        writer.push_sample(0, false, &hevc_p(&[1])),
        Err(Mp4Error::NeedKeyframe)
    ));
    assert!(matches!(
        writer.push_sample(0, true, &hevc_p(&[1])),
        Err(Mp4Error::NeedParameterSets)
    ));
    writer.push_sample(0, true, &hevc_key(0, &[1])).unwrap();
    writer.push_sample(1, true, &hevc_p(&[2])).unwrap();
    assert_eq!(writer.sample_count(), 2);

    let empty = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::H264, 1, 1).unwrap();
    assert!(matches!(empty.finish(), Err(Mp4Error::NoSamples)));

    let mut one = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::H264, 1, 1).unwrap();
    one.push_sample(999, true, &h264_key(&[1])).unwrap();
    let one = one.finish().unwrap().into_inner();
    let top = boxes(&one, 0..one.len());
    let stts = find_box(&one, top[2].payload..top[2].end, b"stts");
    assert_eq!(read_u32(&one, stts.payload + 12), 3000);

    let vps = hevc_nal(32, &[1]);
    let short_sps = hevc_nal(33, &[2, 3, 4, 5, 6, 7, 8, 9]);
    let pps = hevc_nal(34, &[3]);
    let idr = hevc_nal(19, &[4]);
    let short = annexb(&[&vps, &short_sps, &pps, &idr]);
    let mut invalid = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::Hevc, 1, 1).unwrap();
    assert!(matches!(
        invalid.push_sample(0, true, &short),
        Err(Mp4Error::BadParameterSet("sps"))
    ));
}

#[test]
fn changed_parameter_sets_open_a_new_description() {
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::Hevc, 10, 20).unwrap();
    writer.push_sample(0, true, &hevc_key(0, &[1])).unwrap();
    writer.push_sample(33_333, false, &hevc_p(&[2])).unwrap();
    writer.set_dimensions(30, 40);
    writer
        .push_sample(66_666, true, &hevc_key(1, &[3]))
        .unwrap();
    let output = writer.finish().unwrap().into_inner();
    let top = boxes(&output, 0..output.len());
    let stsd = find_box(&output, top[2].payload..top[2].end, b"stsd");
    assert_eq!(read_u32(&output, stsd.payload + 4), 2);
    let stsc = find_box(&output, top[2].payload..top[2].end, b"stsc");
    assert_eq!(read_u32(&output, stsc.payload + 4), 2);
    assert_eq!(
        &output[stsc.payload + 8..stsc.payload + 20],
        &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]
    );
    assert_eq!(
        &output[stsc.payload + 20..stsc.payload + 32],
        &[0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2]
    );
}

#[test]
fn non_monotonic_timestamps_are_clamped() {
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::H264, 1, 1).unwrap();
    writer.push_sample(100_000, true, &h264_key(&[1])).unwrap();
    writer.push_sample(133_333, false, &h264_p(&[2])).unwrap();
    writer.push_sample(100_001, false, &h264_p(&[3])).unwrap();
    let output = writer.finish().unwrap().into_inner();
    let top = boxes(&output, 0..output.len());
    let stts = find_box(&output, top[2].payload..top[2].end, b"stts");
    assert_eq!(read_u32(&output, stts.payload + 4), 2);
    assert_eq!(read_u32(&output, stts.payload + 16), 2);
}

#[test]
fn cursor_and_file_outputs_match() {
    let data = h264_key(&[1, 2, 3]);
    let mut cursor_writer = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Codec::H264, 2, 3).unwrap();
    cursor_writer.push_sample(0, true, &data).unwrap();
    let cursor_bytes = cursor_writer.finish().unwrap().into_inner();

    let mut path = PathBuf::from(std::env::var_os("TMPDIR").unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .into_os_string()
    }));
    fs::create_dir_all(&path).unwrap();
    path.push(format!("rs-recording-mp4-{}.tmp", std::process::id()));
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut file_writer = Mp4Writer::new(file, Mp4Codec::H264, 2, 3).unwrap();
    file_writer.push_sample(0, true, &data).unwrap();
    let mut file = file_writer.finish().unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let file_bytes = fs::read(&path).unwrap();
    drop(file);
    let _ = fs::remove_file(path);
    assert_eq!(cursor_bytes, file_bytes);
}
