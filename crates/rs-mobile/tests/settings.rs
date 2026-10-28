use rs_mobile::rscore::{
    dto_core::Quality,
    settings::{parse_cidr, NetworkEntry, Settings, SettingsError, SettingsState},
};
use serde_json::{json, Value};

#[test]
fn defaults_match_app_settings() {
    let settings = Settings::default();
    assert_eq!(settings.quality_default, Quality::Auto);
    assert_eq!(settings.tailnet_mode, "system");
    assert!(settings.prewarm_enabled);
    assert!(settings.networks.is_empty());
    assert_eq!(*SettingsState::default().0.lock().unwrap(), settings);
}

#[test]
fn tailnet_control_url_normalizes_and_rejects_invalid_urls() {
    use rs_mobile::rscore::settings::normalize_control_url;

    assert_eq!(
        normalize_control_url(" https://hs.example.com/ "),
        Some("https://hs.example.com".into())
    );
    assert_eq!(
        normalize_control_url("https://hs.example.com/base/"),
        Some("https://hs.example.com/base".into())
    );
    assert_eq!(normalize_control_url("hs.example.com"), None);
    assert_eq!(normalize_control_url("ftp://x"), None);
    assert_eq!(normalize_control_url("https://hs.example.com?a=1"), None);
}

#[test]
fn tailnet_settings_accept_clearing_and_app_active_boolean() {
    let mut settings = Settings::default();
    assert_eq!(
        settings.set("tailnet.controlUrl", json!("https://hs.example.com/")),
        Ok(true)
    );
    assert_eq!(
        settings.tailnet_control_url.as_deref(),
        Some("https://hs.example.com")
    );
    assert_eq!(settings.set("tailnet.controlUrl", Value::Null), Ok(true));
    assert_eq!(settings.tailnet_control_url, None);
    assert_eq!(
        settings.set("tailnet.controlUrl", json!("https://hs.example.com")),
        Ok(true)
    );
    assert_eq!(settings.set("tailnet.controlUrl", json!("")), Ok(true));
    assert_eq!(settings.tailnet_control_url, None);
    assert_eq!(settings.set("device.appActive", json!(false)), Ok(true));
    assert_eq!(settings.set("device.appActive", json!(false)), Ok(false));
    assert!(!settings.app_active);
    assert_eq!(
        settings.set("device.appActive", json!("yes")),
        Err(SettingsError("bad value for device.appActive".into()))
    );
    assert!(!settings.app_active);
    assert_eq!(settings.set("device.appActive", json!(true)), Ok(true));
    assert!(settings.app_active);
}

#[test]
fn tailnet_settings_default_to_unset_control_url_and_active_app() {
    let settings = Settings::default();
    assert_eq!(settings.tailnet_control_url, None);
    assert!(settings.app_active);
}

#[test]
fn tailnet_control_url_validates_length_host_and_url_components() {
    use rs_mobile::rscore::settings::normalize_control_url;

    let longest = format!("https://hs.example.com/{}", "a".repeat(489));
    assert_eq!(longest.len(), 512);
    assert_eq!(normalize_control_url(&longest), Some(longest.clone()));
    assert_eq!(normalize_control_url(&format!("{longest}a")), None);
    assert_eq!(
        normalize_control_url("http://hs.example.com/base/"),
        Some("http://hs.example.com/base".into())
    );
    for invalid in [
        "",
        "   ",
        "https://",
        "https://hs.example.com#fragment",
        "https://user@hs.example.com",
        "https://user:password@hs.example.com",
    ] {
        assert_eq!(normalize_control_url(invalid), None, "{invalid}");
    }

    let mut settings = Settings::default();
    settings
        .set("tailnet.controlUrl", json!("https://hs.example.com"))
        .unwrap();
    for invalid in [json!(false), json!(42), json!("hs.example.com")] {
        let before = settings.clone();
        assert_eq!(
            settings.set("tailnet.controlUrl", invalid),
            Err(SettingsError("bad value for tailnet.controlUrl".into()))
        );
        assert_eq!(settings, before);
    }
    assert_eq!(settings.set("tailnet.controlUrl", json!("  ")), Ok(true));
    assert_eq!(settings.tailnet_control_url, None);
}

#[test]
fn known_settings_validate_and_report_changes() {
    let mut settings = Settings::default();
    assert_eq!(settings.set("quality.default", json!("clear")), Ok(true));
    assert_eq!(settings.set("quality.default", json!("clear")), Ok(false));
    assert_eq!(
        settings.set("quality.default", json!("ultra")),
        Err(SettingsError("bad value for quality.default".into()))
    );
    assert_eq!(settings.quality_default, Quality::Clear);
    for (value, quality) in [
        ("smooth", Quality::Smooth),
        ("eco", Quality::Eco),
        ("auto", Quality::Auto),
    ] {
        assert_eq!(settings.set("quality.default", json!(value)), Ok(true));
        assert_eq!(settings.quality_default, quality);
    }
    for mode in ["embedded", "auto", "system"] {
        assert_eq!(settings.set("tailnet.mode", json!(mode)), Ok(true));
        assert_eq!(settings.set("tailnet.mode", json!(mode)), Ok(false));
        assert_eq!(settings.tailnet_mode, mode);
    }
    assert_eq!(settings.set("prewarm.enabled", json!(false)), Ok(true));
    assert_eq!(settings.set("prewarm.enabled", json!(false)), Ok(false));
    assert!(!settings.prewarm_enabled);
}

#[test]
fn invalid_values_leave_every_setting_unchanged() {
    let mut settings = Settings::default();
    for (key, invalid) in [
        ("quality.default", json!(true)),
        ("quality.default", Value::Null),
        ("tailnet.mode", json!("manual")),
        ("tailnet.mode", json!(["system"])),
        ("prewarm.enabled", json!("yes")),
        ("prewarm.enabled", Value::Null),
        ("networks", json!({})),
        ("networks", json!([{"id": "n1"}])),
        (
            "networks",
            json!([
                {"id": "n1", "name": "公司", "cidrs": [], "vpnPools": []},
                {"id": "n2", "name": "公司", "cidrs": [42], "vpnPools": []}
            ]),
        ),
    ] {
        let before = settings.clone();
        assert_eq!(
            settings.set(key, invalid),
            Err(SettingsError(format!("bad value for {key}")))
        );
        assert_eq!(settings, before);
    }
}

#[test]
fn networks_parse_valid_ranges_and_null_clears() {
    let mut settings = Settings::default();
    let networks = json!([
        {"id":"n1", "name":"公司", "cidrs":["10.30.0.0/16", "bad"],
         "vpnPools":["10.99.0.0/16"]},
        {"id":"n2", "name":"家里", "cidrs":["192.168.31.0/24"],
         "vpnPools":["x", "fd7a:115c:a1e0::/48"]}
    ]);
    assert_eq!(settings.set("networks", networks.clone()), Ok(true));
    assert_eq!(settings.set("networks", networks.clone()), Ok(false));
    assert_eq!(
        settings.intranet_cidrs(),
        [
            parse_cidr("10.30.0.0/16").unwrap(),
            parse_cidr("192.168.31.0/24").unwrap()
        ]
    );
    assert_eq!(
        settings.vpn_pools(),
        [
            parse_cidr("10.99.0.0/16").unwrap(),
            parse_cidr("fd7a:115c:a1e0::/48").unwrap()
        ]
    );
    assert_eq!(serde_json::to_value(&settings.networks).unwrap(), networks);
    let before = settings.clone();
    assert!(settings.set("networks", json!([null])).is_err());
    assert_eq!(settings, before);
    assert_eq!(settings.set("networks", Value::Null), Ok(true));
    assert!(settings.networks.is_empty());
    assert!(settings.intranet_cidrs().is_empty());
    assert!(settings.vpn_pools().is_empty());
    assert_eq!(settings.set("networks", Value::Null), Ok(false));
}

#[test]
fn single_network_matches_u1e_camel_case_schema() {
    let entry = json!({"id":"n1", "name":"公司", "cidrs":["10.30.0.0/16", "bad"],
                       "vpnPools":["10.99.0.0/16"]});
    let network: NetworkEntry = serde_json::from_value(entry.clone()).unwrap();
    assert_eq!(network.vpn_pools, ["10.99.0.0/16"]);
    assert_eq!(serde_json::to_value(network).unwrap(), entry);
    let mut settings = Settings::default();
    settings.set("networks", json!([entry])).unwrap();
    assert_eq!(
        settings.intranet_cidrs(),
        [parse_cidr("10.30.0.0/16").unwrap()]
    );
    assert_eq!(settings.vpn_pools(), [parse_cidr("10.99.0.0/16").unwrap()]);
}

#[test]
fn unknown_keys_are_ignored_without_side_effects() {
    let mut settings = Settings::default();
    let before = settings.clone();
    assert_eq!(settings.set("session.hud", json!(true)), Ok(false));
    // Older apps still push crash.upload; it is ignored.
    assert_eq!(settings.set("crash.upload", json!(true)), Ok(false));
    assert_eq!(
        settings.set("future.setting", json!({"anything": null})),
        Ok(false)
    );
    assert_eq!(settings, before);
}

#[test]
fn cidr_parser_handles_both_families_and_host_addresses() {
    for (value, addr, prefix) in [
        ("192.168.31.0/24", "192.168.31.0", 24),
        ("fd7a:115c:a1e0::/48", "fd7a:115c:a1e0::", 48),
        ("10.30.1.2", "10.30.1.2", 32),
        ("fd7a:115c:a1e0::1", "fd7a:115c:a1e0::1", 128),
        ("10.30.1.2/0", "10.30.1.2", 0),
        ("fd7a:115c:a1e0::1/128", "fd7a:115c:a1e0::1", 128),
    ] {
        let net = parse_cidr(value).unwrap();
        assert_eq!(net.addr().to_string(), addr);
        assert_eq!(net.prefix_len(), prefix);
    }
    for invalid in [
        "10.30.0.0/33",
        "fd7a:115c:a1e0::/129",
        "x",
        "",
        "10.30.0.0/",
        "10.30.0.0/-1",
        "10.30.0.0/16/24",
    ] {
        assert_eq!(parse_cidr(invalid), None, "{invalid}");
    }
}
