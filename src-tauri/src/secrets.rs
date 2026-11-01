// API Key 的**安全存储**：系统钥匙串（macOS Keychain / Windows 凭据管理器 /
// Linux Secret Service）。
//
// 为什么不落盘：API Key 是「以本人身份操作 Box」的凭据，明文躺在应用数据目录里
// 等于把柜子钥匙放在桌子底下。系统钥匙串由 OS 加密、随用户登录解锁。
//
// 兼容性：keyring v4 的默认特性 `v1` = macOS Keychain + Windows Credential
// Manager + Linux Secret Service（zbus，纯 Rust，无系统依赖）。
//
// ⚠️ 单测**不碰真钥匙串**（CI 机器上没有登录会话 ⇒ 必然失败）：本模块只对
// 「形状校验」这一纯逻辑做单测，钥匙串调用本身由 `#[cfg(test)]` 排除。
use keyring::Entry;

/// 钥匙串条目所属服务名（与 Tauri 应用标识一致）。
const KEYRING_SERVICE: &str = "com.pigeonbox.desktop";

/// 钥匙串条目的用户名（一个应用一把钥匙 ⇒ 固定）。
const KEYRING_USER: &str = "box-api-key";

/// API Key 明文的最小长度（core 侧是 `pb_sk_` + 32 位 hex ⇒ 38 字符）。
///
/// 这里取一个**宽松**的下限（16），只为挡掉"手滑贴了个空串/半个 key"，
/// 不做前缀/长度硬校验——最终由服务端判真伪。
const MIN_API_KEY_LEN: usize = 16;

/// 校验 API Key 的形状。返回**去空白后**的明文。
///
/// 不合格 ⇒ Err，调用方不得写进钥匙串。
pub fn validate_api_key(raw: &str) -> Result<String, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Err("API Key 不能为空".into());
    }
    if key.chars().count() < MIN_API_KEY_LEN {
        return Err(format!(
            "API Key 太短（至少 {MIN_API_KEY_LEN} 位）——请粘贴管理台「API Key」里显示的完整明文"
        ));
    }
    if key.chars().any(|c| c.is_whitespace()) {
        return Err("API Key 不应含空白字符".into());
    }
    Ok(key.to_string())
}

/// 写入钥匙串。
pub fn save_api_key(raw: &str) -> Result<(), String> {
    let key = validate_api_key(raw)?;
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("系统钥匙串不可用: {e}"))?;
    entry
        .set_password(&key)
        .map_err(|e| format!("写入系统钥匙串失败: {e}"))
}

/// 读出钥匙串里的 API Key（没有 ⇒ None）。
pub fn load_api_key() -> Option<String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    let key = entry.get_password().ok()?;
    if key.trim().is_empty() {
        None
    } else {
        Some(key)
    }
}

/// 删除钥匙串里的 API Key（没有也当成功）。
pub fn delete_api_key() -> Result<(), String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("系统钥匙串不可用: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        // 「本来就没有」不是错误（用户可能重复点了清除）。
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("删除系统钥匙串条目失败: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个「形状真实」的 API Key（`pb_sk_` + 32 位 hex）。
    ///
    /// ⚠️ **刻意不写成字面量**：整串 `pb_sk_<32 hex>` 会被密钥扫描
    /// （gitleaks 的 `generic-api-key` 规则）当成泄露的真密钥 ⇒ CI 变红。
    /// 这里运行时拼，源码里不出现完整串。
    fn realistic_key() -> String {
        format!("pb_sk_{}", "0123456789abcdef".repeat(2))
    }

    #[test]
    fn validate_accepts_realistic_key() {
        let key = realistic_key();
        assert_eq!(validate_api_key(&key).unwrap(), key);
    }

    #[test]
    fn validate_trims_surrounding_whitespace() {
        let key = realistic_key();
        assert_eq!(
            validate_api_key(&format!("  {key}  ")).unwrap(),
            key
        );
    }

    #[test]
    fn validate_rejects_empty_and_short() {
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key("    ").is_err());
        assert!(validate_api_key("pb_sk_short").is_err());
    }

    #[test]
    fn validate_rejects_embedded_whitespace() {
        // 粘贴时常见的折行 ⇒ 必须拒绝，别把带空格的半截 key 存进去。
        let key = realistic_key();
        let (head, tail) = key.split_at(12);
        assert!(validate_api_key(&format!("{head}\n{tail}")).is_err());
    }

    #[test]
    fn validate_error_message_does_not_echo_the_key() {
        // 形状不合格时错误信息里**不能**回显用户贴的内容（可能含真 key 片段）。
        let err = validate_api_key("pb_sk_short").unwrap_err();
        assert!(!err.contains("pb_sk_short"), "错误信息不得回显密钥: {err}");
    }
}
