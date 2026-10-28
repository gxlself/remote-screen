use std::collections::HashSet;
use std::io::{self, ErrorKind};

use prost::Message;
use rsp_core::file_ctl::{
    fail_to_pb, io_to_pb, page, pb_to_fail, stream_code, validate_cancel, validate_list,
    validate_query, validate_request, validate_status, CtlError, MAX_CONCURRENT_RECEIVES,
    MAX_CONCURRENT_SENDS, MAX_ENTRY_NAME_BYTES, MAX_LISTING_BYTES, MAX_LISTING_ENTRIES,
    MAX_PATH_BYTES, STREAM_BUSY, STREAM_CANCELLED, STREAM_DISABLED, STREAM_NOT_ALLOWED,
    STREAM_PAUSED, STREAM_REJECTED,
};
use rsp_core::file_progress::FailReason;
use rsp_core::pb::{self, FileEntry, FileListing};

const TRANSFER_ID: &str = "00112233445566778899aabbccddeeff";
const LONG_NAME_BYTES: usize = 1000;

fn list(path: &str) -> pb::FileList {
    pb::FileList {
        path: path.to_owned(),
        ..Default::default()
    }
}

fn request(path: &str) -> pb::FileRequest {
    pb::FileRequest {
        path: path.to_owned(),
        ..Default::default()
    }
}

fn entry(name: impl Into<String>) -> FileEntry {
    FileEntry {
        name: name.into(),
        ..Default::default()
    }
}

#[test]
fn constants_match_the_file_channel_contract() {
    assert_eq!(STREAM_REJECTED, 1);
    assert_eq!(STREAM_PAUSED, 2);
    assert_eq!(STREAM_CANCELLED, 3);
    assert_eq!(STREAM_BUSY, 4);
    assert_eq!(STREAM_DISABLED, 5);
    assert_eq!(STREAM_NOT_ALLOWED, 6);
    assert_eq!(MAX_LISTING_ENTRIES, 1000);
    assert_eq!(MAX_LISTING_BYTES, 256 * 1024);
    assert_eq!(MAX_PATH_BYTES, 4096);
    assert_eq!(MAX_ENTRY_NAME_BYTES, 1024);
    assert_eq!(MAX_CONCURRENT_SENDS, 2);
    assert_eq!(MAX_CONCURRENT_RECEIVES, 4);
}

#[test]
fn validate_list_accepts_empty_and_bounded_paths() {
    assert_eq!(validate_list(&list("")), Ok(()));
    assert_eq!(validate_list(&list(&"a".repeat(MAX_PATH_BYTES))), Ok(()));
    assert_eq!(
        validate_list(&list(&"a".repeat(MAX_PATH_BYTES + 1))),
        Err(CtlError::PathTooLong(MAX_PATH_BYTES + 1))
    );
    assert_eq!(validate_list(&list("a\0b")), Err(CtlError::NulInPath));
}

#[test]
fn validate_request_checks_path_id_hash_and_resume_fields() {
    let mut first = request("/home/user/file.txt");
    first.expect_sha256 = vec![0; 32];
    assert_eq!(validate_request(&first), Ok(()));

    let mut resume = request("/home/user/file.txt");
    resume.transfer_id = TRANSFER_ID.into();
    resume.offset = 10;
    resume.expect_size = 20;
    assert_eq!(validate_request(&resume), Ok(()));

    assert_eq!(validate_request(&request("")), Err(CtlError::EmptyPath));
    assert_eq!(
        validate_request(&request(&"a".repeat(MAX_PATH_BYTES + 1))),
        Err(CtlError::PathTooLong(MAX_PATH_BYTES + 1))
    );
    assert_eq!(validate_request(&request("a\0b")), Err(CtlError::NulInPath));

    let mut bad_id = request("/home/user/file.txt");
    bad_id.transfer_id = "abc".into();
    assert_eq!(validate_request(&bad_id), Err(CtlError::BadTransferId));

    let mut bad_upper_id = request("/home/user/file.txt");
    bad_upper_id.transfer_id = TRANSFER_ID.to_uppercase();
    assert_eq!(
        validate_request(&bad_upper_id),
        Err(CtlError::BadTransferId)
    );

    let mut bad_hash = request("/home/user/file.txt");
    bad_hash.expect_sha256 = vec![0; 31];
    assert_eq!(validate_request(&bad_hash), Err(CtlError::BadSha256(31)));

    let mut no_id = request("/home/user/file.txt");
    no_id.offset = 10;
    assert_eq!(validate_request(&no_id), Err(CtlError::OffsetWithoutId));

    let mut beyond_size = request("/home/user/file.txt");
    beyond_size.transfer_id = TRANSFER_ID.into();
    beyond_size.offset = 10;
    beyond_size.expect_size = 5;
    assert_eq!(
        validate_request(&beyond_size),
        Err(CtlError::OffsetBeyondSize {
            offset: 10,
            size: 5,
        })
    );
}

#[test]
fn validate_query_status_and_cancel_require_a_valid_transfer_id() {
    assert_eq!(
        validate_query(&pb::FileQuery::default()),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_query(&pb::FileQuery {
            transfer_id: "abc".into(),
            ..Default::default()
        }),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_query(&pb::FileQuery {
            transfer_id: TRANSFER_ID.into(),
            ..Default::default()
        }),
        Ok(())
    );

    assert_eq!(
        validate_status(&pb::FileStatus::default()),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_status(&pb::FileStatus {
            transfer_id: "abc".into(),
            ..Default::default()
        }),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_status(&pb::FileStatus {
            transfer_id: TRANSFER_ID.into(),
            ..Default::default()
        }),
        Ok(())
    );

    assert_eq!(
        validate_cancel(&pb::FileCancel::default()),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_cancel(&pb::FileCancel {
            transfer_id: "abc".into(),
        }),
        Err(CtlError::BadTransferId)
    );
    assert_eq!(
        validate_cancel(&pb::FileCancel {
            transfer_id: TRANSFER_ID.into(),
        }),
        Ok(())
    );
}

#[test]
fn control_errors_map_to_protocol_on_the_wire() {
    let errors = [
        CtlError::EmptyPath,
        CtlError::PathTooLong(MAX_PATH_BYTES + 1),
        CtlError::NulInPath,
        CtlError::BadTransferId,
        CtlError::BadSha256(31),
        CtlError::OffsetWithoutId,
        CtlError::OffsetBeyondSize {
            offset: 10,
            size: 5,
        },
    ];
    for error in errors {
        assert_eq!(error.to_pb(), pb::FileError::Protocol);
    }
}

#[test]
fn fail_reasons_round_trip_through_wire_errors() {
    let pairs = [
        (FailReason::Io, pb::FileError::Io),
        (FailReason::Sha256Mismatch, pb::FileError::Sha256Mismatch),
        (FailReason::ResumeMismatch, pb::FileError::ResumeMismatch),
        (FailReason::SourceChanged, pb::FileError::SourceChanged),
        (FailReason::Protocol, pb::FileError::Protocol),
        (FailReason::TooLarge, pb::FileError::TooLarge),
    ];
    for (reason, wire) in pairs {
        assert_eq!(fail_to_pb(reason.clone()), wire);
        assert_eq!(pb_to_fail(wire), Some(reason));
    }
    assert_eq!(pb_to_fail(pb::FileError::NotFound), None);
}

#[test]
fn io_error_kinds_map_to_wire_errors() {
    let cases = [
        (ErrorKind::NotFound, pb::FileError::NotFound),
        (ErrorKind::PermissionDenied, pb::FileError::PermissionDenied),
        (ErrorKind::StorageFull, pb::FileError::NoSpace),
        (ErrorKind::NotADirectory, pb::FileError::NotADir),
        (ErrorKind::IsADirectory, pb::FileError::NotAFile),
        (ErrorKind::FileTooLarge, pb::FileError::TooLarge),
        (ErrorKind::Other, pb::FileError::Io),
    ];
    for (kind, wire) in cases {
        assert_eq!(io_to_pb(&io::Error::from(kind)), wire);
    }
}

#[test]
fn stream_codes_follow_the_stop_reset_contract() {
    assert_eq!(stream_code(pb::FileError::Disabled), STREAM_DISABLED);
    assert_eq!(stream_code(pb::FileError::NotAllowed), STREAM_NOT_ALLOWED);
    assert_eq!(stream_code(pb::FileError::Busy), STREAM_BUSY);
    assert_eq!(stream_code(pb::FileError::NotFound), STREAM_REJECTED);
}

#[test]
fn short_entries_are_limited_by_count() {
    let entries: Vec<_> = (0..2500).map(|i| entry(format!("f{i}"))).collect();

    let (first, next) = page(&entries, 0);
    assert_eq!(first.len(), MAX_LISTING_ENTRIES);
    assert_eq!(next, MAX_LISTING_ENTRIES as u32);
    assert_eq!(first.first().map(|entry| entry.name.as_str()), Some("f0"));
    assert_eq!(first.last().map(|entry| entry.name.as_str()), Some("f999"));

    let (second, next) = page(&entries, 2000);
    assert_eq!(second.len(), 500);
    assert_eq!(next, 0);
    assert_eq!(
        second.first().map(|entry| entry.name.as_str()),
        Some("f2000")
    );

    assert!(page(&entries, 9999).0.is_empty());
    assert_eq!(page(&entries, 9999).1, 0);
}

#[test]
fn empty_input_yields_an_empty_page() {
    let entries = Vec::new();
    assert!(page(&entries, 0).0.is_empty());
    assert_eq!(page(&entries, 0).1, 0);
}

fn long_name(i: usize) -> String {
    let suffix = format!("-{i:04}");
    let mut name = "x".repeat(LONG_NAME_BYTES - suffix.len());
    name.push_str(&suffix);
    assert_eq!(name.len(), LONG_NAME_BYTES);
    name
}

fn listing_with_max_paths(entries: Vec<FileEntry>, next_offset: u32) -> FileListing {
    FileListing {
        request_id: u32::MAX,
        path: "p".repeat(MAX_PATH_BYTES),
        parent: "q".repeat(MAX_PATH_BYTES),
        entries,
        next_offset,
        error: pb::FileError::None as i32,
    }
}

#[test]
fn long_names_are_paged_without_overlap_and_fit_a_listing() {
    let entries: Vec<_> = (0..300).map(|i| entry(long_name(i))).collect();
    let mut seen = HashSet::new();
    let mut offset = 0u32;
    let mut pages = 0usize;

    let (first, first_next) = page(&entries, 0);
    assert!(!first.is_empty());
    assert_ne!(first_next, 0);

    loop {
        let (entries_page, next) = page(&entries, offset);
        assert!(!entries_page.is_empty());
        assert_eq!(
            next,
            if offset as usize + entries_page.len() < entries.len() {
                offset + entries_page.len() as u32
            } else {
                0
            }
        );

        let page_bytes: usize = entries_page
            .iter()
            .map(|entry| entry.encoded_len() + 3)
            .sum();
        assert!(page_bytes <= MAX_LISTING_BYTES);

        let listing = listing_with_max_paths(entries_page.clone(), next);
        assert!(listing.encoded_len() <= MAX_LISTING_BYTES);

        for entry in entries_page {
            assert!(seen.insert(entry.name), "entry appeared in two pages");
        }

        pages += 1;
        assert!(
            pages <= 3,
            "300 long names should fit in at most three pages"
        );
        if next == 0 {
            break;
        }
        assert!(next > offset, "next offset must advance");
        offset = next;
    }

    assert_eq!(seen.len(), entries.len());
}
