use rsp_core::ids::{fp_b64url, fp_display, fp_display_b64url, fp_from_b64url, id_from_fp};

const CROCKFORD_BASE32: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[test]
fn fixed_fingerprint_vectors() {
    let fp = std::array::from_fn(|index| index as u8);
    assert_eq!(id_from_fp(&fp), "aaaqeayeaudaocaj");
    assert_eq!(fp_display(&fp), "000G 40R4 0M30 E209");
    assert_eq!(
        fp_b64url(&fp),
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
    );
    assert_eq!(id_from_fp(&[0xff; 32]), "7777777777777777");
    assert_eq!(fp_display(&[0xff; 32]), "ZZZZ ZZZZ ZZZZ ZZZZ");
}

#[test]
fn display_vectors() {
    let vectors = [
        (
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
            "aaaqeayeaudaocaj",
            "000G 40R4 0M30 E209",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "__________________________________________8",
            "7777777777777777",
            "ZZZZ ZZZZ ZZZZ ZZZZ",
        ),
        (
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU",
            "4oymiquy7qobjgx3",
            "WERC 8GMR ZGE1 96QV",
        ),
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "aaaaaaaaaaaaaaaa",
            "0000 0000 0000 0000",
        ),
    ];

    for (hex, encoded, id, display) in vectors {
        let fp: [u8; 32] = std::array::from_fn(|index| {
            u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap()
        });
        assert_eq!(fp_display(&fp), display);
        assert_eq!(id_from_fp(&fp), id);
        assert_eq!(fp_b64url(&fp), encoded);
    }
}

#[test]
fn id_depends_only_on_first_ten_bytes() {
    let mut fp = [0; 32];
    assert_eq!(id_from_fp(&fp), "aaaaaaaaaaaaaaaa");
    fp[10..].fill(0xff);
    assert_eq!(id_from_fp(&fp), "aaaaaaaaaaaaaaaa");
}

#[test]
fn display_only_depends_on_first_ten_bytes() {
    let mut fp = std::array::from_fn(|index| index as u8);
    let display = fp_display(&fp);
    fp[10..].fill(0xff);
    assert_eq!(fp_display(&fp), display);

    fp[9] ^= 1;
    let changed = fp_display(&fp);
    assert_eq!(&changed[..18], &display[..18]);
    assert_ne!(changed.chars().last(), display.chars().last());
}

#[test]
fn display_shape() {
    for byte in 0..=u8::MAX {
        let display = fp_display(&[byte; 32]);
        assert_eq!(display.len(), 19);
        let chars: Vec<char> = display.chars().collect();
        for index in [4, 9, 14] {
            assert_eq!(chars[index], ' ');
        }
        for (index, character) in chars.into_iter().enumerate() {
            if matches!(index, 4 | 9 | 14) {
                continue;
            }
            assert!(
                CROCKFORD_BASE32.contains(character),
                "{character:?} is not Crockford base32"
            );
            assert!(!matches!(character, 'I' | 'L' | 'O' | 'U'));
        }
    }
}

#[test]
fn display_from_base64url() {
    assert_eq!(
        fp_display_b64url("47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU").unwrap(),
        "WERC 8GMR ZGE1 96QV"
    );
    assert!(fp_display_b64url("not a fingerprint").is_err());
}

#[test]
fn base64url_round_trip_and_strict_validation() {
    for byte in 0..=u8::MAX {
        let fp = [byte; 32];
        let encoded = fp_b64url(&fp);
        assert_eq!(encoded.len(), 43);
        assert!(!encoded.contains(['=', '+', '/']));
        assert_eq!(fp_from_b64url(&encoded).unwrap(), fp);
    }
    for invalid in [
        "",
        "AA",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9", // Nonzero trailing bits.
        "//////////////////////////////////////////8",
        "++++++++++++++++++++++++++++++++++++++++++8",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\n",
    ] {
        assert!(fp_from_b64url(invalid).is_err(), "accepted {invalid:?}");
    }
}
