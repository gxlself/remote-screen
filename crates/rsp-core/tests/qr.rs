use rsp_core::{
    ids::id_from_fp,
    qr::{parse, PairQr, QrError},
};

fn qr() -> PairQr {
    let fp = std::array::from_fn(|index| index as u8);
    PairQr {
        v: 1,
        host_id: id_from_fp(&fp),
        fp,
        code: "012345".to_owned(),
        name: "小明的电脑 & Mac + #?=%,".to_owned(),
        addrs: vec![
            "100.64.0.1:7400".parse().unwrap(),
            "[fd7a:115c:a1e0::1]:7401".parse().unwrap(),
        ],
    }
}

#[test]
fn round_trip_chinese_name_ipv6_and_reserved_characters() {
    let qr = qr();
    let url = qr.to_url();
    assert!(url.starts_with("rsp://pair?v=1&h=aaaqeayeaudaocaj&fp="));
    assert!(url.contains("&c=012345&n=%E5%B0%8F"));
    assert!(url.contains("%20%26%20Mac%20%2B%20%23%3F%3D%25%2C"));
    assert!(url.ends_with("&a=100.64.0.1:7400,[fd7a:115c:a1e0::1]:7401"));
    assert_eq!(parse(&url).unwrap(), qr);
    assert_eq!(PairQr::parse(&url).unwrap(), qr);
}

#[test]
fn defaults_ports_and_ignores_unknown_parameters() {
    let url = qr().to_url().replace(
        "100.64.0.1:7400,[fd7a:115c:a1e0::1]:7401",
        "100.64.0.1,[fd7a:115c:a1e0::1]",
    );
    let parsed = parse(&(url + "&future=extension")).unwrap();
    assert!(parsed.addrs.iter().all(|addr| addr.port() == 7400));
}

#[test]
fn rejects_spliced_fingerprint_and_invalid_codes() {
    let url = qr().to_url();
    assert_eq!(
        parse(&url.replace("h=aaaqeayeaudaocaj", "h=aaaaaaaaaaaaaaaa")),
        Err(QrError::HostId)
    );
    for code in [
        "12345",
        "1234567",
        "12345x",
        "１２３４５６",
        "123 56",
        "-12345",
    ] {
        assert_eq!(
            parse(&url.replace("c=012345", &format!("c={code}"))),
            Err(QrError::Code)
        );
    }
}

#[test]
fn rejects_wrong_url_and_unsupported_version() {
    let url = qr().to_url();
    for prefix in [
        "http://pair?",
        "rsp://other?",
        "rsp://pair/path?",
        "rsp://pair/?",
        "rsp://pair.example.com?",
        "rsp://user@pair?",
        "rsp://pair:7400?",
    ] {
        assert_eq!(
            parse(&url.replace("rsp://pair?", prefix)),
            Err(QrError::Url)
        );
    }
    assert_eq!(parse(&(url.clone() + "#fragment")), Err(QrError::Url));
    assert_eq!(parse(&url.replace("v=1", "v=2")), Err(QrError::Version));
}

#[test]
fn rejects_duplicate_parameters_including_encoded_and_unknown_keys() {
    let url = qr().to_url();
    for extra in [
        "v=1",
        "%76=1",
        "h=aaaqeayeaudaocaj",
        "fp=ignored",
        "c=012345",
        "n=other",
        "a=100.64.0.2",
        "extra=one&extra=two",
    ] {
        assert_eq!(
            parse(&format!("{url}&{extra}")),
            Err(QrError::DuplicateParameter)
        );
    }
}

#[test]
fn requires_all_fields_and_valid_percent_encoded_utf8() {
    let url = qr().to_url();
    for key in ["v", "h", "fp", "c", "n", "a"] {
        let query = url.strip_prefix("rsp://pair?").unwrap();
        let query = query
            .split('&')
            .filter(|param| !param.starts_with(&format!("{key}=")))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(
            parse(&format!("rsp://pair?{query}")),
            Err(QrError::MissingParameter(key))
        );
    }
    for suffix in ["bad=%", "bad=%0", "bad=%GG", "bad=%FF", "missing_equals"] {
        assert_eq!(parse(&format!("{url}&{suffix}")), Err(QrError::Encoding));
    }
    assert!(matches!(
        parse(&url.replace("fp=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8", "fp=AA")),
        Err(QrError::Fingerprint(_))
    ));
}

#[test]
fn limits_names_by_characters_and_address_counts() {
    let mut qr = qr();
    qr.name = "龙".repeat(64);
    assert_eq!(parse(&qr.to_url()).unwrap(), qr);
    qr.name.push('龙');
    assert_eq!(parse(&qr.to_url()), Err(QrError::Name));
    qr.name.clear();
    qr.addrs = vec!["100.64.0.1:7400".parse().unwrap(); 8];
    assert_eq!(parse(&qr.to_url()).unwrap(), qr);
    qr.addrs.push("100.64.0.2:7400".parse().unwrap());
    assert_eq!(parse(&qr.to_url()), Err(QrError::Addresses));
    qr.addrs.clear();
    assert_eq!(parse(&qr.to_url()), Err(QrError::Addresses));
}

#[test]
fn rejects_invalid_addresses_and_ambiguous_ipv6() {
    let url = qr().to_url();
    let prefix = url.split_once("&a=").unwrap().0;
    for address in [
        "100.64.0.999",
        "100.64.0.1:65536",
        "100.64.0.1:",
        "[fd7a:115c:a1e0::1]:",
        "fd7a:115c:a1e0::1",
        "[invalid]",
        "hs.example.com:7400",
        "100.64.0.1/path",
    ] {
        assert_eq!(
            parse(&format!("{prefix}&a={address}")),
            Err(QrError::Address)
        );
    }
    assert_eq!(
        parse(&format!("{prefix}&a=100.64.0.1,")),
        Err(QrError::Addresses)
    );
}
