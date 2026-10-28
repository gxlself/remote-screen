//! Protobuf 生成类型。见 docs/dev/03-protocol.md §3、§6.3、§8；04-identity-security.md 信任声明。

include!(concat!(env!("OUT_DIR"), "/rsp.v1.rs"));

/// 手写 Debug：账户名和密码一律打成 `<redacted>`（build.rs 对它跳过了 prost 自动 Debug）。
impl std::fmt::Debug for PairPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("PairPassword");
        out.field("device_name", &self.device_name);
        match Platform::try_from(self.platform) {
            Ok(platform) => out.field("platform", &platform),
            Err(_) => out.field("platform", &self.platform),
        };
        out.field("account", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}
