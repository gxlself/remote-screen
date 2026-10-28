#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::pairfile::{parse, PairFileError, PAIRFILE_MAX_BYTES};

fuzz_target!(|input: &[u8]| {
    let text = String::from_utf8_lossy(input);

    if text.len() > PAIRFILE_MAX_BYTES {
        assert_eq!(
            parse(&text).unwrap_err(),
            PairFileError::TooLarge,
            "an oversized pairfile must fail with TooLarge"
        );
        return;
    }

    let Ok(pairfile) = parse(&text) else {
        return;
    };

    assert_eq!(pairfile.v, 1, "an accepted pairfile must use version 1");
    assert_ne!(
        pairfile.expires_at_s, 0,
        "an accepted pairfile must have a nonzero expiry"
    );
    assert!(
        (1..=8).contains(&pairfile.addrs.len()),
        "an accepted pairfile must contain one to eight addresses"
    );
    assert!(
        pairfile.name.chars().count() <= 64,
        "an accepted pairfile name exceeds 64 characters"
    );

    let round_trip = parse(&pairfile.to_uri()).expect("an accepted pairfile URI must re-parse");
    assert_eq!(round_trip.v, pairfile.v, "version changed on round trip");
    assert_eq!(
        round_trip.host_id, pairfile.host_id,
        "host ID changed on round trip"
    );
    assert_eq!(
        round_trip.fp, pairfile.fp,
        "fingerprint changed on round trip"
    );
    assert_eq!(
        round_trip.file_id, pairfile.file_id,
        "file ID changed on round trip"
    );
    assert_eq!(
        round_trip.secret.as_ref(),
        pairfile.secret.as_ref(),
        "secret changed on round trip"
    );
    assert_eq!(round_trip.name, pairfile.name, "name changed on round trip");
    assert_eq!(
        round_trip.addrs, pairfile.addrs,
        "addresses changed on round trip"
    );
    assert_eq!(
        round_trip.expires_at_s, pairfile.expires_at_s,
        "expiry changed on round trip"
    );
});
