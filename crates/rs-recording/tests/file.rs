use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rs_recording::crypto::testing::TestWrapper;
use rs_recording::crypto::{
    chunk_nonce, open_chunk, seal_chunk, AuthFailed, FileKey, SecretWrapper, WrapError, KEY_LEN,
    TAG_LEN,
};
use rs_recording::file::{
    finalize_path, read_header, recover_part, ChunkInfo, FileError, RecordingReader,
    RecordingWriter, RecoverKind, CHUNK_HEADER_LEN, FLAG_FINAL, FLAG_KEYFRAME_START,
    HEADER_FIXED_LEN, MAGIC, MAX_CHUNK_CT, PART_SUFFIX, TRAILER_LEN, TRAILER_MAGIC, VERSION,
};
use zeroize::Zeroizing;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const PAYLOADS: [&[u8]; 3] = [
    b"first opaque chunk",
    b"second opaque chunk",
    b"third opaque chunk",
];
const TIMES: [u64; 3] = [10, 20, 30];
const END: &[u8] = b"opaque final record";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-recording-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn part(&self) -> PathBuf {
        self.0.join(format!("recording.rsrec{PART_SUFFIX}"))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_three(part: &Path, wrapper: &TestWrapper) -> (RecordingWriter, [u64; 3]) {
    let mut writer = RecordingWriter::create(part, wrapper, [1; 16], 7).unwrap();
    let mut offsets = [0u64; 3];
    for i in 0..3 {
        offsets[i] = writer.append(TIMES[i], i == 0, PAYLOADS[i]).unwrap();
    }
    (writer, offsets)
}

fn finished(part: &Path, wrapper: &TestWrapper) -> [u64; 3] {
    let (writer, offsets) = write_three(part, wrapper);
    let total = writer.finish(40, END).unwrap();
    assert_eq!(fs::metadata(part).unwrap().len(), total);
    offsets
}

fn edit(path: &Path, offset: u64, bytes: &[u8]) {
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn flip(path: &Path, offset: u64) {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0u8];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0x80;
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
}

fn truncate(path: &Path, len: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(len)
        .unwrap();
}

#[test]
fn round_trip_three_chunks_and_final() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let (writer, offsets) = write_three(&part, &wrapper);
    assert_eq!(writer.path(), part);
    assert_eq!(writer.chunks_written(), 3);
    assert_eq!(writer.bytes_written(), fs::metadata(&part).unwrap().len());
    let total = writer.finish(40, END).unwrap();
    let path = finalize_path(&part).unwrap();
    assert_eq!(path, dir.0.join("recording.rsrec"));
    assert!(!part.exists());
    assert_eq!(fs::metadata(&path).unwrap().len(), total);
    let mut reader = RecordingReader::open(&path, &wrapper).unwrap();
    assert_eq!(reader.chunks().len(), 3);
    assert_eq!(reader.header().file_id, [1; 16]);
    assert_eq!(reader.header().created_at_us, 7);
    for i in 0..3 {
        let info = reader.chunks()[i];
        assert_eq!(info.index, i as u64);
        assert_eq!(info.offset, offsets[i]);
        assert_eq!(info.t_start_us, TIMES[i]);
        assert_eq!(info.ct_len as usize, PAYLOADS[i].len() + TAG_LEN);
        assert_eq!(info.flags, if i == 0 { FLAG_KEYFRAME_START } else { 0 });
        assert_eq!(info.keyframe_start(), i == 0);
        assert!(!info.is_final());
        assert_eq!(reader.read_chunk(i).unwrap(), PAYLOADS[i]);
    }
    assert_eq!(reader.final_chunk().index, 3);
    assert_eq!(reader.final_chunk().flags, FLAG_FINAL);
    assert_eq!(reader.final_chunk().t_start_us, 40);
    assert!(reader.final_chunk().is_final());
    assert!(!reader.final_chunk().keyframe_start());
    assert_eq!(reader.read_final().unwrap(), END);
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[bytes.len() - 8..], TRAILER_MAGIC);
    assert_eq!(
        u64::from_be_bytes(
            bytes[bytes.len() - TRAILER_LEN..bytes.len() - 8]
                .try_into()
                .unwrap()
        ),
        reader.final_chunk().offset
    );
}

#[test]
fn header_has_contract_layout_and_read_header_round_trips() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let writer = RecordingWriter::create(&part, &wrapper, [1; 16], 7).unwrap();
    assert_eq!(writer.bytes_written(), (HEADER_FIXED_LEN + 61) as u64);
    assert_eq!(writer.chunks_written(), 0);
    drop(writer);
    let bytes = fs::read(&part).unwrap();
    assert_eq!(&bytes[..8], MAGIC);
    assert_eq!(bytes[8], VERSION);
    assert_eq!(bytes[9], 0);
    assert_eq!(u16::from_be_bytes(bytes[10..12].try_into().unwrap()), 101);
    assert_eq!(&bytes[12..28], [1; 16]);
    assert_eq!(bytes[28], 0xF0);
    assert_eq!(bytes[29], 0);
    assert_eq!(u16::from_be_bytes(bytes[30..32].try_into().unwrap()), 61);
    assert_eq!(u64::from_be_bytes(bytes[93..101].try_into().unwrap()), 7);
    let header = read_header(&part).unwrap();
    assert_eq!(header.file_id, [1; 16]);
    assert_eq!(header.created_at_us, 7);
    assert_eq!(header.wrap_kind, 0xF0);
    assert_eq!(header.wrapped_key, bytes[32..93]);
}

#[test]
fn ciphertext_tampering_only_invalidates_that_chunk() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let offsets = finished(&part, &wrapper);
    flip(&part, offsets[1] + CHUNK_HEADER_LEN as u64);
    let mut reader = RecordingReader::open(&part, &wrapper).unwrap();
    assert_eq!(reader.read_chunk(0).unwrap(), PAYLOADS[0]);
    assert!(matches!(
        reader.read_chunk(1),
        Err(FileError::Auth { chunk: 1 })
    ));
    assert_eq!(reader.read_chunk(2).unwrap(), PAYLOADS[2]);
    assert_eq!(reader.read_final().unwrap(), END);
}

#[test]
fn chunk_timestamp_is_authenticated() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let offsets = finished(&part, &wrapper);
    flip(&part, offsets[1] + 13);
    let mut reader = RecordingReader::open(&part, &wrapper).unwrap();
    assert!(matches!(
        reader.read_chunk(1),
        Err(FileError::Auth { chunk: 1 })
    ));
}

#[test]
fn whole_header_is_authenticated_by_every_chunk() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    finished(&part, &wrapper);
    flip(&part, 93);
    let mut reader = RecordingReader::open(&part, &wrapper).unwrap();
    for i in 0..3 {
        assert!(
            matches!(reader.read_chunk(i), Err(FileError::Auth { chunk }) if chunk == i as u64)
        );
    }
    assert!(matches!(
        reader.read_final(),
        Err(FileError::Auth { chunk: 3 })
    ));
}

#[test]
fn swapping_chunks_is_rejected_before_decryption() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let offsets = finished(&part, &wrapper);
    let reader = RecordingReader::open(&part, &wrapper).unwrap();
    let end = reader.final_chunk().offset as usize;
    drop(reader);
    let bytes = fs::read(&part).unwrap();
    let mut swapped = Vec::new();
    swapped.extend_from_slice(&bytes[offsets[2] as usize..end]);
    swapped.extend_from_slice(&bytes[offsets[1] as usize..offsets[2] as usize]);
    edit(&part, offsets[1], &swapped);
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::Corrupt(_))
    ));
}

struct OtherKind(TestWrapper);

impl SecretWrapper for OtherKind {
    fn kind(&self) -> u8 {
        0xF1
    }

    fn wrap(&self, key: &[u8; 32]) -> Result<Vec<u8>, WrapError> {
        self.0.wrap(key)
    }

    fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, WrapError> {
        self.0.unwrap(wrapped)
    }
}

#[test]
fn wrapper_errors_and_kind_mismatch_are_preserved() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    finished(&part, &wrapper);
    assert!(matches!(
        RecordingReader::open(&part, &TestWrapper::new(2)),
        Err(FileError::Wrap(WrapError::Rejected))
    ));
    wrapper.set_available(false);
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::Wrap(WrapError::Unavailable))
    ));
    wrapper.set_available(true);
    assert!(matches!(
        RecordingReader::open(&part, &OtherKind(TestWrapper::new(1))),
        Err(FileError::Wrap(WrapError::Rejected))
    ));
}

#[test]
fn enormous_ciphertext_length_is_rejected_before_allocation() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let offsets = finished(&part, &wrapper);
    edit(&part, offsets[1], &u32::MAX.to_be_bytes());
    assert!(fs::metadata(&part).unwrap().len() < 1000);
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::TooLarge)
    ));
}

#[test]
fn recover_half_written_last_chunk_keeps_verified_prefix() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let (mut writer, _) = write_three(&part, &wrapper);
    writer.sync().unwrap();
    drop(writer);
    truncate(&part, fs::metadata(&part).unwrap().len() - 10);
    let mut seen = Vec::new();
    let mut reasons = Vec::new();
    let outcome = recover_part(
        &part,
        &wrapper,
        &mut |info, bytes| seen.push((info.index, bytes.to_vec())),
        &mut |kind| {
            reasons.push(kind);
            (45, b"recovered end".to_vec())
        },
    )
    .unwrap();
    assert_eq!(outcome.kind, RecoverKind::Recovered);
    assert_eq!(outcome.valid_chunks, 2);
    assert_eq!(reasons, [RecoverKind::Recovered]);
    assert_eq!(seen, [(0, PAYLOADS[0].to_vec()), (1, PAYLOADS[1].to_vec())]);
    assert!(!part.exists());
    let mut reader = RecordingReader::open(&outcome.path, &wrapper).unwrap();
    assert_eq!(reader.chunks().len(), 2);
    assert_eq!(reader.final_chunk().index, 2);
    assert_eq!(reader.final_chunk().t_start_us, 45);
    assert_eq!(reader.read_final().unwrap(), b"recovered end");
}

#[test]
fn recover_middle_damage_only_keeps_chunks_before_damage() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let (mut writer, offsets) = write_three(&part, &wrapper);
    writer.sync().unwrap();
    drop(writer);
    flip(&part, offsets[1] + CHUNK_HEADER_LEN as u64);
    let mut seen = Vec::new();
    let outcome = recover_part(
        &part,
        &wrapper,
        &mut |info, plaintext| seen.push((info.index, plaintext.to_vec())),
        &mut |kind| {
            assert_eq!(kind, RecoverKind::Damaged);
            (50, b"damaged end".to_vec())
        },
    )
    .unwrap();
    assert_eq!(outcome.kind, RecoverKind::Damaged);
    assert_eq!(outcome.valid_chunks, 1);
    assert_eq!(seen, [(0, PAYLOADS[0].to_vec())]);
    let mut reader = RecordingReader::open(&outcome.path, &wrapper).unwrap();
    assert_eq!(reader.chunks().len(), 1);
    assert_eq!(reader.read_chunk(0).unwrap(), PAYLOADS[0]);
    assert_eq!(reader.final_chunk().index, 1);
    assert_eq!(reader.read_final().unwrap(), b"damaged end");
}

#[test]
fn recover_finished_part_is_complete_without_building_end() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    finished(&part, &wrapper);
    let before = fs::read(&part).unwrap();
    let mut seen = Vec::new();
    let outcome = recover_part(
        &part,
        &wrapper,
        &mut |info, plaintext| seen.push((info.index, plaintext.to_vec())),
        &mut |_| panic!("Complete recovery must retain the original final block"),
    )
    .unwrap();
    assert_eq!(outcome.kind, RecoverKind::Complete);
    assert_eq!(outcome.valid_chunks, 3);
    assert_eq!(
        seen,
        [
            (0, PAYLOADS[0].to_vec()),
            (1, PAYLOADS[1].to_vec()),
            (2, PAYLOADS[2].to_vec())
        ]
    );
    assert_eq!(fs::read(&outcome.path).unwrap(), before);
    let mut reader = RecordingReader::open(&outcome.path, &wrapper).unwrap();
    assert_eq!(reader.read_final().unwrap(), END);
}

#[test]
fn recover_header_only_creates_empty_recording() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    drop(RecordingWriter::create(&part, &wrapper, [1; 16], 7).unwrap());
    let outcome = recover_part(
        &part,
        &wrapper,
        &mut |_, _| panic!("there are no ordinary chunks"),
        &mut |kind| {
            assert_eq!(kind, RecoverKind::Recovered);
            (0, b"empty recovered end".to_vec())
        },
    )
    .unwrap();
    assert_eq!(outcome.kind, RecoverKind::Recovered);
    assert_eq!(outcome.valid_chunks, 0);
    let mut reader = RecordingReader::open(&outcome.path, &wrapper).unwrap();
    assert!(reader.chunks().is_empty());
    assert_eq!(reader.final_chunk().index, 0);
    assert_eq!(reader.read_final().unwrap(), b"empty recovered end");
}

#[test]
fn create_existing_path_preserves_original_contents() {
    let dir = TestDir::new();
    let part = dir.part();
    fs::write(&part, b"must not truncate").unwrap();
    assert!(matches!(
        RecordingWriter::create(&part, &TestWrapper::new(1), [1; 16], 7),
        Err(FileError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(fs::read(part).unwrap(), b"must not truncate");
}

#[test]
fn nonce_layout_and_chunk_authentication_round_trip() {
    assert_eq!(chunk_nonce(0x0102), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2]);
    assert_eq!(
        chunk_nonce(u64::MAX),
        [0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255]
    );
    let key = FileKey::from_bytes(Zeroizing::new([0x3A; KEY_LEN]));
    for plaintext in [b"opaque bytes".as_slice(), b"".as_slice()] {
        let sealed = seal_chunk(&key, 7, b"aad", plaintext);
        assert_eq!(sealed.len(), plaintext.len() + TAG_LEN);
        assert_eq!(open_chunk(&key, 7, b"aad", &sealed).unwrap(), plaintext);
        assert_eq!(open_chunk(&key, 8, b"aad", &sealed), Err(AuthFailed));
        assert_eq!(
            open_chunk(&key, 7, b"changed aad", &sealed),
            Err(AuthFailed)
        );
    }
    assert_eq!(open_chunk(&key, 0, b"", &[0; TAG_LEN - 1]), Err(AuthFailed));
    assert_ne!(FileKey::random().unwrap().as_bytes(), &[0; KEY_LEN]);
}

#[test]
fn key_debug_is_redacted() {
    let key = FileKey::from_bytes(Zeroizing::new([0xAB; KEY_LEN]));
    let debug = format!("{key:?}");
    assert_eq!(debug, "FileKey(..)");
    assert!(!debug.contains(&"ab".repeat(KEY_LEN)));
    assert!(!debug.contains(&"AB".repeat(KEY_LEN)));
}

#[test]
fn test_wrapper_round_trip_rejects_tampering_and_bad_envelopes() {
    let wrapper = TestWrapper::new(1);
    let key = [0xAB; KEY_LEN];
    let wrapped = wrapper.wrap(&key).unwrap();
    assert_eq!(wrapper.kind(), 0xF0);
    assert_eq!(wrapped.len(), 61);
    assert_eq!(wrapped[0], 1);
    assert_eq!(wrapper.unwrap(&wrapped).unwrap().as_ref(), &key);
    for i in 0..wrapped.len() {
        let mut damaged = wrapped.clone();
        damaged[i] ^= 0x80;
        assert_eq!(wrapper.unwrap(&damaged), Err(WrapError::Rejected));
    }
    for len in [0, 1, 12, 60, 62, 255] {
        assert_eq!(wrapper.unwrap(&vec![0; len]), Err(WrapError::Rejected));
    }
    assert_eq!(
        TestWrapper::new(2).unwrap(&wrapped),
        Err(WrapError::Rejected)
    );
    wrapper.set_available(false);
    assert_eq!(wrapper.wrap(&key), Err(WrapError::Unavailable));
    assert_eq!(wrapper.unwrap(&wrapped), Err(WrapError::Unavailable));
    wrapper.set_available(true);
    assert_eq!(wrapper.unwrap(&wrapped).unwrap().as_ref(), &key);
}

#[test]
fn recover_complete_final_repairs_truncated_or_corrupt_trailers() {
    for missing in 1..=TRAILER_LEN {
        let dir = TestDir::new();
        let part = dir.part();
        let wrapper = TestWrapper::new(1);
        finished(&part, &wrapper);
        let before = fs::read(&part).unwrap();
        truncate(&part, (before.len() - missing) as u64);
        let outcome = recover_part(&part, &wrapper, &mut |_, _| {}, &mut |_| {
            panic!("FINAL is still complete")
        })
        .unwrap();
        assert_eq!(outcome.kind, RecoverKind::Complete);
        assert_eq!(outcome.valid_chunks, 3);
        assert_eq!(fs::read(&outcome.path).unwrap(), before);
        assert_eq!(
            RecordingReader::open(&outcome.path, &wrapper)
                .unwrap()
                .chunks()
                .len(),
            3
        );
    }
    for trailer_byte in [0, 8, 15] {
        let dir = TestDir::new();
        let part = dir.part();
        let wrapper = TestWrapper::new(1);
        finished(&part, &wrapper);
        let before = fs::read(&part).unwrap();
        flip(&part, (before.len() - TRAILER_LEN + trailer_byte) as u64);
        let outcome = recover_part(&part, &wrapper, &mut |_, _| {}, &mut |_| {
            panic!("FINAL is still complete")
        })
        .unwrap();
        assert_eq!(outcome.kind, RecoverKind::Complete);
        assert_eq!(fs::read(outcome.path).unwrap(), before);
    }
}

#[test]
fn recovery_errors_leave_part_and_callbacks_untouched() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let (writer, _) = write_three(&part, &wrapper);
    drop(writer);
    let before = fs::read(&part).unwrap();
    let mut no_chunk = |_: &ChunkInfo, _: &[u8]| panic!("must validate the key first");
    let mut no_end = |_| panic!("must validate the key first");
    for bad_wrapper in [
        &TestWrapper::new(2) as &dyn SecretWrapper,
        &OtherKind(TestWrapper::new(1)),
    ] {
        assert!(matches!(
            recover_part(&part, bad_wrapper, &mut no_chunk, &mut no_end),
            Err(FileError::Wrap(WrapError::Rejected))
        ));
        assert_eq!(fs::read(&part).unwrap(), before);
    }
    wrapper.set_available(false);
    assert!(matches!(
        recover_part(&part, &wrapper, &mut no_chunk, &mut no_end),
        Err(FileError::Wrap(WrapError::Unavailable))
    ));
    assert_eq!(fs::read(&part).unwrap(), before);
    wrapper.set_available(true);
    flip(&part, 0);
    let corrupted = fs::read(&part).unwrap();
    assert!(matches!(
        recover_part(&part, &wrapper, &mut no_chunk, &mut no_end),
        Err(FileError::BadMagic)
    ));
    assert_eq!(fs::read(part).unwrap(), corrupted);
}

struct InvalidLengthWrapper {
    inner: TestWrapper,
    len: usize,
}

impl SecretWrapper for InvalidLengthWrapper {
    fn kind(&self) -> u8 {
        self.inner.kind()
    }

    fn wrap(&self, key: &[u8; 32]) -> Result<Vec<u8>, WrapError> {
        let mut wrapped = self.inner.wrap(key)?;
        wrapped.resize(self.len, 0);
        Ok(wrapped)
    }

    fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, WrapError> {
        self.inner.unwrap(wrapped)
    }
}

#[test]
fn create_cleans_up_when_wrapping_fails_or_has_invalid_length() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    wrapper.set_available(false);
    assert!(matches!(
        RecordingWriter::create(&part, &wrapper, [1; 16], 7),
        Err(FileError::Wrap(WrapError::Unavailable))
    ));
    assert!(!part.exists());
    for len in [0, 256] {
        let wrapper = InvalidLengthWrapper {
            inner: TestWrapper::new(1),
            len,
        };
        assert!(matches!(
            RecordingWriter::create(&part, &wrapper, [1; 16], 7),
            Err(FileError::Wrap(WrapError::Rejected))
        ));
        assert!(!part.exists());
    }
}

#[test]
fn oversized_plaintext_does_not_write_or_consume_an_index() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let mut writer = RecordingWriter::create(&part, &wrapper, [1; 16], 7).unwrap();
    let before = fs::read(&part).unwrap();
    let too_large = vec![0u8; MAX_CHUNK_CT as usize - TAG_LEN + 1];
    assert!(matches!(
        writer.append(0, true, &too_large),
        Err(FileError::TooLarge)
    ));
    assert_eq!(writer.chunks_written(), 0);
    assert_eq!(writer.bytes_written(), before.len() as u64);
    assert_eq!(fs::read(&part).unwrap(), before);
    assert_eq!(
        writer.append(1, true, b"valid").unwrap(),
        before.len() as u64
    );
    let after_append = fs::read(&part).unwrap();
    assert!(matches!(
        writer.finish(2, &too_large),
        Err(FileError::TooLarge)
    ));
    assert_eq!(fs::read(&part).unwrap(), after_append);
}

#[test]
fn reader_rejects_malformed_headers_trailers_and_chunk_boundaries() {
    let dir = TestDir::new();
    let part = dir.part();
    let wrapper = TestWrapper::new(1);
    let offsets = finished(&part, &wrapper);
    let original = fs::read(&part).unwrap();
    for (offset, value) in [(0usize, 0u8), (9, 1), (10, 0xFF), (29, 1), (30, 1), (31, 0)] {
        let mut bytes = original.clone();
        bytes[offset] = value;
        fs::write(&part, &bytes).unwrap();
        assert!(
            RecordingReader::open(&part, &wrapper).is_err(),
            "header byte {offset}"
        );
    }
    let mut bytes = original.clone();
    bytes[8] = 2;
    fs::write(&part, &bytes).unwrap();
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::UnsupportedVersion(2))
    ));
    for len in [
        0,
        7,
        31,
        40,
        HEADER_FIXED_LEN + 60,
        original.len() - TRAILER_LEN,
    ] {
        fs::write(&part, &original[..len]).unwrap();
        assert!(
            RecordingReader::open(&part, &wrapper).is_err(),
            "truncated to {len}"
        );
    }
    for ct_len in [0, 15, 1000] {
        fs::write(&part, &original).unwrap();
        edit(&part, offsets[1], &u32::to_be_bytes(ct_len));
        assert!(matches!(
            RecordingReader::open(&part, &wrapper),
            Err(FileError::Corrupt(_))
        ));
    }
    fs::write(&part, &original).unwrap();
    edit(
        &part,
        (original.len() - TRAILER_LEN) as u64,
        &offsets[1].to_be_bytes(),
    );
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::Corrupt(_))
    ));
    fs::write(&part, &original).unwrap();
    flip(&part, (original.len() - 1) as u64);
    assert!(matches!(
        RecordingReader::open(&part, &wrapper),
        Err(FileError::Corrupt(_))
    ));
}

#[test]
fn finalize_rejects_paths_without_part_suffix() {
    let dir = TestDir::new();
    let path = dir.0.join("recording.rsrec");
    fs::write(&path, b"untouched").unwrap();
    assert!(matches!(finalize_path(&path), Err(FileError::Corrupt(_))));
    assert_eq!(fs::read(path).unwrap(), b"untouched");
}
