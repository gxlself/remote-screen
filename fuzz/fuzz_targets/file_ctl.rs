#![no_main]

use libfuzzer_sys::fuzz_target;
use prost::Message;
use rsp_core::file_ctl::{
    validate_cancel, validate_list, validate_query, validate_request, validate_status, CtlError,
    MAX_PATH_BYTES,
};
use rsp_core::pb;

const SHA256_BYTES: usize = 32;
const TRANSFER_ID_BYTES: usize = 32;
const CASE_COUNT: u8 = 5;

fn expected_path_result(path: &str, allow_empty: bool) -> Result<(), CtlError> {
    if path.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(CtlError::EmptyPath)
        };
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(CtlError::PathTooLong(path.len()));
    }
    if path.contains('\0') {
        return Err(CtlError::NulInPath);
    }
    Ok(())
}

fn valid_transfer_id(transfer_id: &str) -> bool {
    transfer_id.len() == TRANSFER_ID_BYTES
        && transfer_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn assert_accepted_path(path: &str) {
    assert!(
        path.len() <= MAX_PATH_BYTES,
        "an accepted path exceeds MAX_PATH_BYTES"
    );
    assert!(
        !path.contains('\0'),
        "an accepted path contains a NUL character"
    );
}

fn check_list(message: &pb::FileList, result: Result<(), CtlError>) {
    assert_eq!(result, expected_path_result(&message.path, true));
    if result.is_ok() {
        assert_accepted_path(&message.path);
    }
}

fn expected_request_result(message: &pb::FileRequest) -> Result<(), CtlError> {
    expected_path_result(&message.path, false)?;
    if !message.transfer_id.is_empty() && !valid_transfer_id(&message.transfer_id) {
        return Err(CtlError::BadTransferId);
    }
    if message.offset > 0 {
        if message.transfer_id.is_empty() {
            return Err(CtlError::OffsetWithoutId);
        }
        if message.offset > message.expect_size {
            return Err(CtlError::OffsetBeyondSize {
                offset: message.offset,
                size: message.expect_size,
            });
        }
    }
    if !message.expect_sha256.is_empty() && message.expect_sha256.len() != SHA256_BYTES {
        return Err(CtlError::BadSha256(message.expect_sha256.len()));
    }
    Ok(())
}

fn check_request(message: &pb::FileRequest, result: Result<(), CtlError>) {
    assert_eq!(result, expected_request_result(message));
    if result.is_ok() {
        assert_accepted_path(&message.path);
        assert!(
            message.transfer_id.is_empty() || valid_transfer_id(&message.transfer_id),
            "an accepted request has an invalid transfer ID"
        );
        assert!(
            message.offset == 0
                || (!message.transfer_id.is_empty() && message.offset <= message.expect_size),
            "an accepted request has an invalid resume offset"
        );
        assert!(
            message.expect_sha256.is_empty() || message.expect_sha256.len() == SHA256_BYTES,
            "an accepted request has an invalid SHA-256 length"
        );
    }
}

fn check_required_transfer_id(transfer_id: &str, result: Result<(), CtlError>) {
    if valid_transfer_id(transfer_id) {
        assert_eq!(result, Ok(()));
    } else {
        assert_eq!(result, Err(CtlError::BadTransferId));
    }
}

fuzz_target!(|input: &[u8]| {
    let Some((&selector, body)) = input.split_first() else {
        return;
    };

    match selector % CASE_COUNT {
        0 => {
            if let Ok(message) = pb::FileList::decode(body) {
                check_list(&message, validate_list(&message));
            }
        }
        1 => {
            if let Ok(message) = pb::FileRequest::decode(body) {
                check_request(&message, validate_request(&message));
            }
        }
        2 => {
            if let Ok(message) = pb::FileQuery::decode(body) {
                check_required_transfer_id(&message.transfer_id, validate_query(&message));
            }
        }
        3 => {
            if let Ok(message) = pb::FileStatus::decode(body) {
                check_required_transfer_id(&message.transfer_id, validate_status(&message));
            }
        }
        _ => {
            if let Ok(message) = pb::FileCancel::decode(body) {
                check_required_transfer_id(&message.transfer_id, validate_cancel(&message));
            }
        }
    }
});
