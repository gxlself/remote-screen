//! 系统账户密码校验。见 04「其他配对方式」、06「平台接口」。实现方不得记录或保存密码。

/// 账户信息，只用于配对策略判断，不落盘。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemAccount {
    pub short_name: String,
    pub full_name: String,
    pub is_admin: bool,
    pub is_console_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SysAuthError {
    #[error("wrong password")]
    WrongPassword,
    #[error("system authentication unavailable: {0}")]
    Unavailable(String),
}

/// 两个方法都可能阻塞（OpenDirectory / LogonUserW），调用方放进 spawn_blocking。
pub trait SystemAuthenticator: Send + Sync {
    /// 按短名或全名查账户，不验证密码。找不到 → Ok(None)。
    fn lookup(&self, account: &str) -> Result<Option<SystemAccount>, SysAuthError>;
    /// 校验 `short_name` 的密码（UTF-8 字节）。错 → Err(WrongPassword)。
    fn verify(&self, short_name: &str, password: &[u8]) -> Result<(), SysAuthError>;
}
