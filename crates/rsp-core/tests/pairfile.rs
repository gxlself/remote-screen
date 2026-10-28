use std::{collections::BTreeMap, fs, path::PathBuf};

use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use rsp_core::{
    ids::{fp_b64url, id_from_fp},
    pairfile::{
        file_name, parse, spake_password, PairFile, PairFileError, PAIRFILE_LIFETIME_S,
        PAIRFILE_MAX_BYTES, PAIRFILE_PASSWORD_PREFIX,
    },
    DEFAULT_PORT,
};
use zeroize::Zeroizing;

fn file() -> PairFile {
    let fp = [0x33; 32];
    PairFile {
        v: 1,
        host_id: id_from_fp(&fp),
        fp,
        file_id: [0x11; 16],
        secret: Zeroizing::new([0x22; 32]),
        name: "小明的 Mac mini".into(),
        addrs: vec![
            "100.64.0.7:7843".parse().unwrap(),
            "192.168.31.20:7843".parse().unwrap(),
        ],
        expires_at_s: 1_792_465_134 + PAIRFILE_LIFETIME_S,
    }
}

fn assert_same(actual: &PairFile, expected: &PairFile) {
    assert_eq!(actual.v, expected.v);
    assert_eq!(actual.host_id, expected.host_id);
    assert_eq!(actual.fp, expected.fp);
    assert_eq!(actual.file_id, expected.file_id);
    assert_eq!(*actual.secret, *expected.secret);
    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.addrs, expected.addrs);
    assert_eq!(actual.expires_at_s, expected.expires_at_s);
}

fn replace(uri: &str, key: &str, value: &str) -> String {
    let query = uri.strip_prefix("rsp://pairfile?").unwrap();
    let params = query
        .split('&')
        .map(|param| {
            if param.split_once('=').unwrap().0 == key {
                format!("{key}={value}")
            } else {
                param.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("rsp://pairfile?{params}")
}

#[test]
fn round_trip_preserves_fields_key_order_and_surrounding_whitespace() {
    let file = file();
    let uri = file.to_uri();
    let keys: Vec<_> = uri
        .strip_prefix("rsp://pairfile?")
        .unwrap()
        .split('&')
        .map(|param| param.split_once('=').unwrap().0)
        .collect();
    assert_eq!(keys, ["v", "h", "fp", "f", "s", "n", "a", "e"]);
    assert!(!uri.contains('\n'));
    assert!(uri.contains("&n=%E5%B0%8F%E6%98%8E%E7%9A%84%20Mac%20mini&"));
    assert_same(&parse(&uri).unwrap(), &file);
    assert_same(&parse(&format!(" \r\n{uri}\r\n ")).unwrap(), &file);
    let padded = format!("{uri}{}", " ".repeat(PAIRFILE_MAX_BYTES - uri.len()));
    assert_same(&parse(&padded).unwrap(), &file);
    assert_eq!(parse(&(padded + " ")).unwrap_err(), PairFileError::TooLarge);
}

#[test]
fn rejects_each_invalid_field_with_the_exact_error() {
    let uri = file().to_uri();
    assert_eq!(
        parse(&" ".repeat(4097)).unwrap_err(),
        PairFileError::TooLarge
    );
    assert_eq!(
        parse(&uri.replacen("rsp://pairfile?", "rsp://pair?", 1)).unwrap_err(),
        PairFileError::Url
    );
    assert_eq!(parse(&format!("{uri}#x")).unwrap_err(), PairFileError::Url);
    assert_eq!(
        parse(&replace(&uri, "v", "2")).unwrap_err(),
        PairFileError::Version
    );
    assert_eq!(
        parse(&replace(&uri, "h", "aaaaaaaaaaaaaaaa")).unwrap_err(),
        PairFileError::HostId
    );
    assert_eq!(
        parse(&replace(&uri, "f", &BASE64URL_NOPAD.encode(&[0x11; 15]))).unwrap_err(),
        PairFileError::FileId
    );
    assert_eq!(
        parse(&replace(&uri, "f", &BASE64URL_NOPAD.encode(&[0x11; 17]))).unwrap_err(),
        PairFileError::FileId
    );
    assert_eq!(
        parse(&replace(&uri, "s", &BASE64URL_NOPAD.encode(&[0x22; 31]))).unwrap_err(),
        PairFileError::Secret
    );
    assert_eq!(
        parse(&replace(&uri, "n", &"龙".repeat(65))).unwrap_err(),
        PairFileError::Name
    );
    assert_eq!(
        parse(&replace(&uri, "a", "")).unwrap_err(),
        PairFileError::Addresses
    );
    assert_eq!(
        parse(&replace(&uri, "a", &["100.64.0.7:7843"; 9].join(","))).unwrap_err(),
        PairFileError::Addresses
    );
    assert_eq!(
        parse(&replace(&uri, "a", "hs.example.com:7843")).unwrap_err(),
        PairFileError::Address
    );
    assert_eq!(
        parse(&replace(&uri, "e", "abc")).unwrap_err(),
        PairFileError::Expiry
    );
    assert_eq!(
        parse(&replace(&uri, "e", "0")).unwrap_err(),
        PairFileError::Expiry
    );
    assert_eq!(
        parse(&format!("{uri}&future=one&future=two")).unwrap_err(),
        PairFileError::DuplicateParameter
    );
    let without_secret = uri
        .split('&')
        .filter(|param| !param.starts_with("s="))
        .collect::<Vec<_>>()
        .join("&");
    assert_eq!(
        parse(&without_secret).unwrap_err(),
        PairFileError::MissingParameter("s")
    );
    assert_eq!(
        parse(&replace(&uri, "n", "%GG")).unwrap_err(),
        PairFileError::Encoding
    );
}

#[test]
fn enforces_uri_base64_decimal_and_duplicate_key_rules() {
    let uri = file().to_uri();
    for suffix in ["n=again", "%6e=again", "future=1&%66uture=2"] {
        assert_eq!(
            parse(&format!("{uri}&{suffix}")).unwrap_err(),
            PairFileError::DuplicateParameter
        );
    }
    for suffix in ["future=%", "future=%0", "future=%FF", "future"] {
        assert_eq!(
            parse(&format!("{uri}&{suffix}")).unwrap_err(),
            PairFileError::Encoding
        );
    }
    for key in ["v", "h", "fp", "f", "s", "n", "a", "e"] {
        let query = uri.strip_prefix("rsp://pairfile?").unwrap();
        let query = query
            .split('&')
            .filter(|param| param.split_once('=').unwrap().0 != key)
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(
            parse(&format!("rsp://pairfile?{query}")).unwrap_err(),
            PairFileError::MissingParameter(key)
        );
    }
    for (key, error) in [
        ("fp", PairFileError::Fingerprint),
        ("f", PairFileError::FileId),
        ("s", PairFileError::Secret),
    ] {
        for value in ["", "!!", "AA", "AA=="] {
            assert_eq!(parse(&replace(&uri, key, value)).unwrap_err(), error);
        }
    }
    for value in ["", "+1", "-1", "1.5", "1e3", "18446744073709551616", "１２"] {
        assert_eq!(
            parse(&replace(&uri, "e", value)).unwrap_err(),
            PairFileError::Expiry
        );
    }
    let max_expiry = parse(&replace(&uri, "e", &u64::MAX.to_string())).unwrap();
    assert_eq!(max_expiry.expires_at_s, u64::MAX);
    assert!(!max_expiry.is_expired(u64::MAX));
}

#[test]
fn accepts_reordered_parameters_unknown_keys_and_qr_address_rules() {
    let uri = file().to_uri();
    let mut params: Vec<_> = uri
        .strip_prefix("rsp://pairfile?")
        .unwrap()
        .split('&')
        .collect();
    params.reverse();
    assert_same(
        &parse(&format!("rsp://pairfile?{}&future=ok", params.join("&"))).unwrap(),
        &file(),
    );
    let uri = replace(
        &uri,
        "a",
        "100.64.0.7,[fd7a:115c:a1e0::7],[fd7a:115c:a1e0::8]:7843",
    );
    let mut file = parse(&uri).unwrap();
    assert_eq!(
        file.addrs[0],
        format!("100.64.0.7:{DEFAULT_PORT}").parse().unwrap()
    );
    assert_eq!(
        file.addrs[1],
        format!("[fd7a:115c:a1e0::7]:{DEFAULT_PORT}")
            .parse()
            .unwrap()
    );
    assert_eq!(file.addrs[2], "[fd7a:115c:a1e0::8]:7843".parse().unwrap());
    file.name = "龙".repeat(64);
    file.addrs = vec![file.addrs[0]; 8];
    assert_same(&parse(&file.to_uri()).unwrap(), &file);
    file.name = "Mac + & #?=%,".into();
    assert_same(&parse(&file.to_uri()).unwrap(), &file);
    assert_eq!(
        parse(&replace(&uri, "n", "Mac+mini")).unwrap().name,
        "Mac+mini"
    );
    for value in ["100.64.0.7,", ",100.64.0.7"] {
        assert_eq!(
            parse(&replace(&uri, "a", value)).unwrap_err(),
            PairFileError::Addresses
        );
    }
    for value in [
        "100.64.0.999",
        "100.64.0.7:65536",
        "fd7a:115c:a1e0::7",
        "[invalid]",
    ] {
        assert_eq!(
            parse(&replace(&uri, "a", value)).unwrap_err(),
            PairFileError::Address
        );
    }
}

#[test]
fn expiry_has_an_exact_microsecond_boundary() {
    let file = file();
    assert!(!file.is_expired(file.expires_at_s * 1_000_000 - 1));
    assert!(file.is_expired(file.expires_at_s * 1_000_000));
    assert!(file.is_expired(file.expires_at_s * 1_000_000 + 1));
    assert_eq!(PAIRFILE_LIFETIME_S, 86_400);
}

#[test]
fn filenames_sanitize_truncate_unicode_and_use_utc_gregorian_dates() {
    assert_eq!(
        file_name("Studio: Mac/Pro", 1_792_465_134),
        "Studio- Mac-Pro-20261020-0258.rspair"
    );
    assert_eq!(file_name("Host", 0), "Host-19700101-0000.rspair");
    assert_eq!(file_name("Host", 951_782_400), "Host-20000229-0000.rspair");
    assert_eq!(
        file_name("Host", 4_107_542_400),
        "Host-21000301-0000.rspair"
    );
    assert_eq!(file_name("Host", 86_399), "Host-19700101-2359.rspair");
    assert_eq!(file_name("Host", 86_400), "Host-19700102-0000.rspair");
    assert_eq!(file_name("  ", 0), "Remote-Host-19700101-0000.rspair");
    assert_eq!(file_name("  Mac  ", 0), "Mac-19700101-0000.rspair");
    assert_eq!(
        file_name("/\\:*?\"<>|\n\t\u{7f}", 0),
        "-------------19700101-0000.rspair"
    );
    let name = file_name(&"龙🦀".repeat(80), 0);
    assert_eq!(name.chars().count(), 80);
    assert!(name.ends_with("-19700101-0000.rspair"));
    assert!(String::from_utf8(name.into_bytes()).is_ok());
    assert!(file_name(&"龙".repeat(100), u64::MAX).chars().count() <= 80);
}

#[test]
fn debug_redacts_secret_encodings_and_bytes() {
    let file = file();
    let debug = format!("{file:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(&BASE64URL_NOPAD.encode(file.secret.as_ref())));
    assert!(!debug.contains(&HEXLOWER.encode(file.secret.as_ref())));
    assert!(!debug.contains(&format!("{:?}", *file.secret)));
}

#[test]
fn fixed_pairfile_and_password_vector() {
    let file = file();
    let password = spake_password(&file.file_id, &file.secret);
    assert_eq!(&password[..16], PAIRFILE_PASSWORD_PREFIX);
    assert_eq!(&password[16..32], &file.file_id);
    assert_eq!(&password[32..], file.secret.as_ref());
    let actual: BTreeMap<String, String> = [
        ("file_id", HEXLOWER.encode(&file.file_id)),
        ("secret", HEXLOWER.encode(file.secret.as_ref())),
        ("fp", HEXLOWER.encode(&file.fp)),
        ("name", file.name.clone()),
        ("addrs", "100.64.0.7:7843,192.168.31.20:7843".into()),
        ("expires_at_s", file.expires_at_s.to_string()),
        ("uri", file.to_uri()),
        ("spake_password", HEXLOWER.encode(password.as_ref())),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/pairing/pairfile.txt");
    if std::env::var_os("RSP_REGEN_VECTORS").is_some_and(|value| value == "1") {
        let mut text = String::from(
            "# Fixed pairing file inputs, URI, and 64-byte SPAKE2 password. Test secrets only.\n",
        );
        for (key, value) in &actual {
            text.push_str(&format!("{key}={value}\n"));
        }
        fs::write(&path, text).expect("write pairing file vector");
    }
    let text = fs::read_to_string(path).expect("read pairing file vector");
    let expected: BTreeMap<String, String> = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.into(), value.into())
        })
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (key, value) in &actual {
        assert_eq!(
            Some(value),
            expected.get(key),
            "pairfile vector differs for {key}"
        );
    }
    assert_same(&parse(&expected["uri"]).unwrap(), &file);
    assert!(expected["uri"].contains(&fp_b64url(&file.fp)));
}
