#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::file_name::{candidate, is_safe_component, sanitize, MAX_CANDIDATES, MAX_NAME_BYTES};

fn candidate_number(input: &[u8]) -> u32 {
    let mut bytes = [0u8; 4];
    let filled = input.len().min(bytes.len());
    bytes[..filled].copy_from_slice(&input[..filled]);
    u32::from_be_bytes(bytes)
}

fuzz_target!(|input: &[u8]| {
    let name = String::from_utf8_lossy(input);
    let number = candidate_number(input) % MAX_CANDIDATES;

    let sanitized = sanitize(&name);
    assert!(!sanitized.is_empty(), "sanitized name must not be empty");
    assert!(
        sanitized.len() <= MAX_NAME_BYTES,
        "sanitized name exceeds the byte limit"
    );
    assert!(
        is_safe_component(&sanitized),
        "sanitized name is not a safe component"
    );
    assert!(
        !sanitized.contains(['/', '\\', '\0']),
        "sanitized name contains a path or NUL character"
    );
    assert!(
        !sanitized.starts_with('.'),
        "sanitized name must not be hidden"
    );
    assert_eq!(
        sanitize(&sanitized),
        sanitized,
        "sanitize must be idempotent"
    );

    let candidate = candidate(&sanitized, number);
    assert!(
        is_safe_component(&candidate),
        "collision candidate is not a safe component"
    );
    assert!(
        candidate.len() <= MAX_NAME_BYTES,
        "collision candidate exceeds the byte limit"
    );
});
