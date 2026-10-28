use std::fs;
use std::path::PathBuf;

use data_encoding::{BASE64URL_NOPAD, HEXLOWER_PERMISSIVE};
use ed25519_dalek::SigningKey;
use host_core::update::manifest::{
    check_applicable, signing_bytes, verify_manifest, verify_package_file, ManifestError,
    PackageError, PackageHasher, TrustedKeys, VerifiedManifest, MAX_SAFE_INTEGER, SIGNING_DOMAIN,
};
use host_core::update::types::{Arch, Channel, HostOs};
use host_core::update::version::{compare, parse};
use serde::Deserialize;
use std::cmp::Ordering;

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/update/release_vectors.json"
);

#[derive(Deserialize)]
struct VectorFile {
    keys: std::collections::BTreeMap<String, VectorKey>,
    cases: Vec<VectorCase>,
}

#[derive(Deserialize)]
struct VectorKey {
    seed_hex: String,
    public: String,
}

#[derive(Deserialize)]
struct VectorCase {
    name: String,
    manifest: String,
    trusted: Vec<String>,
    signing_bytes_hex: String,
    expect: String,
}

fn vectors() -> (VectorFile, Vec<SigningKey>) {
    let raw = fs::read(VECTORS).expect("read release vectors");
    let file: VectorFile = serde_json::from_slice(&raw).expect("parse release vectors");
    let mut keys = Vec::new();
    for key in file.keys.values() {
        let seed: [u8; 32] = HEXLOWER_PERMISSIVE
            .decode(key.seed_hex.as_bytes())
            .expect("decode seed")
            .try_into()
            .expect("32 byte seed");
        let signing = SigningKey::from_bytes(&seed);
        let public = BASE64URL_NOPAD.encode(signing.verifying_key().as_bytes());
        assert_eq!(public, key.public);
        keys.push(signing);
    }
    (file, keys)
}

fn public_key(file: &VectorFile, name: &str) -> String {
    file.keys
        .get(name)
        .unwrap_or_else(|| panic!("missing key {name}"))
        .public
        .clone()
}

fn trusted(file: &VectorFile, names: &[String]) -> TrustedKeys {
    let keys: Vec<String> = names.iter().map(|name| public_key(file, name)).collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    TrustedKeys::from_base64url(&refs).expect("valid trusted keys")
}

fn case<'a>(file: &'a VectorFile, name: &str) -> &'a VectorCase {
    file.cases
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("missing vector {name}"))
}

fn verified(file: &VectorFile, name: &str) -> VerifiedManifest {
    let vector = case(file, name);
    verify_manifest(vector.manifest.as_bytes(), &trusted(file, &vector.trusted))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn shared_vectors_match() {
    let (file, _) = vectors();
    assert!(!file.cases.is_empty());
    for vector in &file.cases {
        let keys = trusted(&file, &vector.trusted);
        let result = verify_manifest(vector.manifest.as_bytes(), &keys);
        match vector.expect.as_str() {
            "ok" => assert!(result.is_ok(), "{}: {result:?}", vector.name),
            "invalid" => assert!(
                matches!(result, Err(ManifestError::Invalid(_))),
                "{}: {result:?}",
                vector.name
            ),
            "signature" => assert_eq!(result, Err(ManifestError::Signature), "{}", vector.name),
            other => panic!("unknown expectation {other}"),
        }

        let bytes = signing_bytes(vector.manifest.as_bytes());
        if vector.signing_bytes_hex.is_empty() {
            assert!(
                bytes.is_err(),
                "{}: signing bytes unexpectedly succeeded",
                vector.name
            );
        } else {
            let signing = bytes.unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert_eq!(
                HEXLOWER_PERMISSIVE.encode(&signing).to_ascii_lowercase(),
                vector.signing_bytes_hex,
                "{}",
                vector.name
            );
            assert!(signing.starts_with(SIGNING_DOMAIN));
        }
    }
}

#[test]
fn verified_fields_match_example() {
    let (file, _) = vectors();
    let manifest = verified(&file, "ok_example");
    assert_eq!(manifest.channel, Channel::Stable);
    assert_eq!(manifest.os, HostOs::Macos);
    assert_eq!(manifest.arch, Arch::Universal);
    assert_eq!(manifest.version, "0.3.2");
    assert_eq!(
        manifest.url,
        "https://dl.example.com/remote-host/0.3.2/RemoteHost-0.3.2.dmg"
    );
    assert_eq!(manifest.sha256[0], 0x01);
    assert_eq!(manifest.size, 84_736_211);
    assert_eq!(manifest.min_from.as_deref(), Some("0.3.0"));
    assert_eq!(manifest.published_at, 1_792_465_134_000_000);
    assert_eq!(manifest.notes, "修复网络切换后的重连问题。");
    assert_eq!(verified(&file, "ok_min_from_null").min_from, None);
    assert_eq!(verified(&file, "ok_min_from_absent").min_from, None);
}

#[test]
fn versions_follow_go_ordering() {
    let ordered = [
        "0.3.9-alpha",
        "0.3.9-alpha.1",
        "0.3.9-alpha.beta",
        "0.3.9-beta",
        "0.3.9-beta.2",
        "0.3.9-beta.11",
        "0.3.9-rc.1",
        "0.3.9",
        "0.3.10",
        "1.0.0",
        "999999999999999999999.0.0",
    ];
    for (left_index, left) in ordered.iter().enumerate() {
        for (right_index, right) in ordered.iter().enumerate() {
            assert_eq!(
                compare(left, right).expect("valid version"),
                left_index.cmp(&right_index),
                "{left} vs {right}"
            );
        }
    }
    assert_eq!(compare("1.0.0+foo", "1.0.0+bar"), Ok(Ordering::Equal));
    for invalid in [
        "v1.0.0",
        "1.0",
        "01.0.0",
        "1.0.0-01",
        "1.0.0-",
        "1.0.0+",
        "1.0.0-alpha..1",
        "1.0.0+foo+bar",
        "1.0.0-alpha_1",
        "",
    ] {
        assert!(parse(invalid).is_err(), "accepted {invalid:?}");
    }
    assert_eq!(
        parse("1.0.0+foo").expect("valid"),
        parse("1.0.0+bar").expect("valid")
    );
}

#[test]
fn trusted_keys_are_strict() {
    let (file, _) = vectors();
    let k1 = public_key(&file, "k1");
    let k2 = public_key(&file, "k2");
    assert!(TrustedKeys::from_base64url(&[k1.as_str()]).is_ok());

    let mut padded = k1.clone();
    padded.push('=');
    assert!(matches!(
        TrustedKeys::from_base64url(&[padded.as_str()]),
        Err(ManifestError::Invalid("public_key"))
    ));
    let standard: String = k1
        .chars()
        .map(|character| match character {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    assert!(matches!(
        TrustedKeys::from_base64url(&[standard.as_str()]),
        Err(ManifestError::Invalid("public_key"))
    ));
    assert!(TrustedKeys::from_base64url(&[""]).is_err());
    assert!(TrustedKeys::from_base64url(&["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"]).is_err());
    assert!(
        TrustedKeys::from_base64url(&["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"]).is_err()
    );

    let trusted = TrustedKeys::from_csv(&format!("{k1}, ,{k2},bad"));
    assert_eq!(trusted.len(), 2);
    assert!(!trusted.is_empty());
    assert!(TrustedKeys::from_csv("").is_empty());
}

#[test]
fn empty_trust_store_is_reported_after_manifest_validation() {
    let (file, _) = vectors();
    let vector = case(&file, "ok_example");
    assert_eq!(
        verify_manifest(
            vector.manifest.as_bytes(),
            &TrustedKeys::from_base64url(&[]).expect("empty")
        ),
        Err(ManifestError::NoTrustedKeys)
    );
}

#[test]
fn applicability_prevents_downgrades() {
    let (file, _) = vectors();
    let manifest = verified(&file, "ok_example");
    let selector = (Channel::Stable, HostOs::Macos, Arch::Universal);
    assert_eq!(
        check_applicable(&manifest, "0.3.1", &selector.0, &selector.1, &selector.2),
        Ok(())
    );
    assert_eq!(
        check_applicable(&manifest, "0.3.2", &selector.0, &selector.1, &selector.2),
        Err(ManifestError::NotApplicable("not_newer"))
    );
    assert_eq!(
        check_applicable(&manifest, "0.3.3", &selector.0, &selector.1, &selector.2),
        Err(ManifestError::NotApplicable("not_newer"))
    );
    assert_eq!(
        check_applicable(&manifest, "0.2.9", &selector.0, &selector.1, &selector.2),
        Err(ManifestError::NotApplicable("min_from"))
    );
    assert_eq!(
        check_applicable(&manifest, "dev", &selector.0, &selector.1, &selector.2),
        Err(ManifestError::NotApplicable("current_version"))
    );
    assert_eq!(
        check_applicable(&manifest, "0.3.1", &Channel::Beta, &selector.1, &selector.2),
        Err(ManifestError::NotApplicable("selector"))
    );
}

#[test]
fn signing_bytes_rejects_non_manifest_json_values() {
    for raw in [
        br#"[]"#.as_slice(),
        br#""text""#.as_slice(),
        br#"{"a":true}"#.as_slice(),
        br#"{"a":[]}"#.as_slice(),
        br#"{"a":1.0}"#.as_slice(),
        br#"{"a":9007199254740992}"#.as_slice(),
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":1} trailing"#.as_slice(),
        b"{\"a\":\"\xff\"}".as_slice(),
        br#"{"a":"\ud800"}"#.as_slice(),
    ] {
        assert!(
            signing_bytes(raw).is_err(),
            "accepted {}",
            String::from_utf8_lossy(raw)
        );
    }
    assert_eq!(
        signing_bytes(br#"{"a":1,"a":2}"#),
        Err(ManifestError::Invalid("duplicate_key"))
    );
    assert_eq!(
        signing_bytes(br#"{"a":1} {"b":2}"#),
        Err(ManifestError::Invalid("json"))
    );
    assert!(signing_bytes(br#"{"min_from":null,"signature":"x"}"#).is_ok());
    assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);
}

#[test]
fn package_hash_and_verification() {
    let (file, _) = vectors();
    let mut manifest = verified(&file, "ok_example");
    let contents = b"release package contents";
    manifest.size = contents.len() as u64;
    manifest.sha256 = {
        let mut hasher = PackageHasher::new();
        hasher.update(contents);
        hasher.finish().0
    };

    let mut split = PackageHasher::new();
    split.update(&contents[..8]);
    split.update(&contents[8..16]);
    split.update(&contents[16..]);
    assert_eq!(split.finish(), (manifest.sha256, contents.len() as u64));

    let path = package_path("good");
    fs::write(&path, contents).expect("write package");
    assert!(verify_package_file(&path, &manifest).is_ok());

    let bigger = package_path("bigger");
    let mut data = contents.to_vec();
    data.push(0);
    fs::write(&bigger, data).expect("write package");
    assert!(matches!(
        verify_package_file(&bigger, &manifest),
        Err(PackageError::Size { expected, actual })
            if expected == contents.len() as u64 && actual == contents.len() as u64 + 1
    ));

    let changed = package_path("changed");
    let mut data = contents.to_vec();
    data[0] ^= 1;
    fs::write(&changed, data).expect("write package");
    assert!(matches!(
        verify_package_file(&changed, &manifest),
        Err(PackageError::Sha256)
    ));

    assert!(matches!(
        verify_package_file(&package_path("missing"), &manifest),
        Err(PackageError::Io(_))
    ));

    for path in [path, bigger, changed] {
        fs::remove_file(path).expect("remove package");
    }
}

fn package_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "host-core-update-{name}-{}.bin",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    path
}
