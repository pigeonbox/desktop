// 一等取件端：**待取判定**与**剪贴板载荷**的纯逻辑。
//
// 本模块刻意不碰 I/O（不读文件、不发请求、不写剪贴板）⇒ 全部可单测。
//
// 隐私承诺「剪贴板只放 6 位取件码（不放链接、不放密码）」由 `clipboard_payload`
// 一处把关：写入前必须过 `is_pickup_code`，不合法就**拒写**而不是"尽力而为"。
// 这条承诺由同文件的单测钉死（见 tests 模块）。
//
// 「待取」的判定口径（Box 侧字段口径见 core 的 share.thrift / app/share）：
//   - 有 6 位取件码（E2E 密文分享与历史数据没有取件码 ⇒ 不算可取件）；
//   - viewer_count == 0（还没被任何人取走）；
//   - 未过期（is_expired == false）；
//   - 不在回收站（deleted_at 为空）；
//   - 未被管控（status == "normal"，blocked/pending_review 拒绝取件）。
//
// ⚠️ Box 侧目前**没有**「我的待收件列表」端点，也没有任何字段能区分
// 「投递到我这儿的件」与「我自己寄出去的件」（core 的 file_codes 无 channel 列，
// upload_type 两者都是 "authenticated"）⇒ 桌面端用「到件通知」反查投递来源：
// core/app/request/service.go 在访客投递成功时给链接主人发一条 type =
// "request_received" 的通知，正文里带取件码。桌面端据此判定「投递到件」。
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 取件码长度——**唯一真相源**。
///
/// 与 Box 侧 `core/repo/db/model/filecode.go` 的 `PickupCodeLength = 6` 对齐。
pub const PICKUP_CODE_LEN: usize = 6;

/// 到件通知的类型标记（Box 侧 core/app/request/service.go 写入）。
pub const DELIVERY_NOTICE_TYPE: &str = "request_received";

/// 取件码字符合法集：ASCII 字母 + 数字。
///
/// 刻意只收 ASCII：Box 侧取件码是 6 位字母数字，大小写敏感。
fn is_code_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// 是否 6 位取件码。
pub fn is_pickup_code(s: &str) -> bool {
    s.chars().count() == PICKUP_CODE_LEN && s.chars().all(is_code_char)
}

/// **剪贴板载荷**：只放取件码本身。
///
/// 这是隐私承诺的唯一出口：任何写剪贴板的动作都必须先过这里。
/// - 不合法（长度不对 / 含非字母数字）⇒ 返回 Err，**调用方不得写入**；
/// - 合法 ⇒ 原样返回 6 位码，**不拼链接、不拼口令、不加任何前后缀**。
pub fn clipboard_payload(code: &str) -> Result<String, String> {
    if !is_pickup_code(code) {
        return Err(format!("不是合法取件码，拒绝写入剪贴板: {code:?}"));
    }
    Ok(code.to_string())
}

/// Box 侧「我的分享」列表里的一条（只取本客户端用得到的字段）。
///
/// 未列出的字段（分享码等）由 serde 直接忽略 ⇒ 结构体保持最小、字段不腐化。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareItem {
    /// 6 位取件码（未铸造时为空/缺省）。
    #[serde(default)]
    pub pickup_code: Option<String>,
    /// 展示名（文本分享为空）。
    #[serde(default)]
    pub file_name: String,
    /// 字节数（文本分享为 0）。
    #[serde(default)]
    pub size: i64,
    /// 累计取件次数（0 = 还没被取走）。
    #[serde(default)]
    pub viewer_count: i64,
    /// 是否已过期。
    #[serde(default)]
    pub is_expired: bool,
    /// 管控状态：normal / blocked / pending_review。
    #[serde(default)]
    pub status: String,
    /// 回收站时间（软删除时间；非空 = 已删）。
    #[serde(default)]
    pub deleted_at: Option<String>,
    /// 创建时间（RFC3339Nano）。
    #[serde(default)]
    pub created_at: String,
}

/// Box 侧「我的通知」列表里的一条。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Notice {
    /// 通知正文（到件通知里带取件码）。
    #[serde(default)]
    pub content: String,
    /// 通知类型（`type` 是 Rust 关键字 ⇒ 改名 + serde 重命名）。
    #[serde(rename = "type", default)]
    pub notice_type: String,
}

/// 「柜子」的范围口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// 只算**投递到件**（反向收件）：别人投到我柜子里的、还没取走的件。
    /// 这是「你的柜子有件待取」最贴近原意的口径。
    #[default]
    Delivered,
    /// 算我账号下**所有**还没被取走的有效分享（不区分是我寄出的还是投给我的）。
    All,
}

impl Scope {
    /// 解析配置里的口径字符串。
    pub fn parse(raw: &str) -> Option<Scope> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "delivered" => Some(Scope::Delivered),
            "all" => Some(Scope::All),
            _ => None,
        }
    }

    /// 口径的稳定字符串（存盘 / 传参用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Delivered => "delivered",
            Scope::All => "all",
        }
    }
}

/// 一件待取的东西。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingItem {
    /// 6 位取件码（**唯一会被复制到剪贴板的东西**）。
    pub pickup_code: String,
    /// 展示名（文本分享为空 ⇒ 展示成「文本分享」）。
    pub file_name: String,
    /// 字节数。
    pub size: i64,
    /// 创建时间。
    pub created_at: String,
    /// 是否投递到件（由到件通知反查得出）。
    pub delivered: bool,
}

impl PendingItem {
    /// 托盘菜单项的展示文本。
    ///
    /// 形如 `123456 · 合同.pdf · 2.0 KB`。**不含链接、不含口令**。
    pub fn tray_label(&self) -> String {
        let name = if self.file_name.trim().is_empty() {
            "文本分享".to_string()
        } else {
            self.file_name.clone()
        };
        if self.size > 0 {
            format!("{} · {} · {}", self.pickup_code, name, format_size(self.size))
        } else {
            format!("{} · {}", self.pickup_code, name)
        }
    }
}

/// 这条分享是否「待取」。
pub fn is_pending(share: &ShareItem) -> bool {
    let Some(code) = share.pickup_code.as_deref() else {
        return false;
    };
    if !is_pickup_code(code) {
        return false;
    }
    if share.viewer_count > 0 {
        return false; // 已被取走
    }
    if share.is_expired {
        return false;
    }
    if share.deleted_at.as_deref().is_some_and(|d| !d.trim().is_empty()) {
        return false; // 回收站
    }
    if !share.status.trim().is_empty() && share.status.trim() != "normal" {
        return false; // 被管控（blocked / pending_review）
    }
    true
}

/// 从到件通知正文里抽出取件码。
///
/// Box 侧正文格式（core/app/request/service.go）：
/// ```text
/// 投递链接: 1a2b3c4d…
/// 文件数: 1
/// 总大小: 1234 字节
/// 投递人 IP: 10.0.0.1
/// 分享码: abcd1234
/// 取件码: A1B2C3
/// ```
///
/// 解析规则：「取件码」标签 + 可选 ASCII/全角冒号 + 可选空白 + 6 位字母数字。
/// 抽不到就返回 None——**不猜、不退化到别的字段**（宁可漏报也不误报）。
pub fn parse_delivery_code(content: &str) -> Option<String> {
    const LABEL: &str = "取件码";
    let idx = content.find(LABEL)?;
    let rest = &content[idx + LABEL.len()..];
    let rest = rest.trim_start();
    // 可选冒号（ASCII / 全角）
    let rest = rest
        .strip_prefix(':')
        .or_else(|| rest.strip_prefix('：'))
        .unwrap_or(rest)
        .trim_start();
    let code: String = rest.chars().take(PICKUP_CODE_LEN).collect();
    if is_pickup_code(&code) {
        Some(code)
    } else {
        None
    }
}

/// 从到件通知里抽出全部取件码（只认 `request_received` 类型的通知）。
pub fn delivery_pickup_codes(notices: &[Notice]) -> BTreeSet<String> {
    notices
        .iter()
        .filter(|n| n.notice_type.trim() == DELIVERY_NOTICE_TYPE)
        .filter_map(|n| parse_delivery_code(&n.content))
        .collect()
}

/// 从分享列表里挑出待取项。
///
/// - `delivered`：到件通知反查出来的取件码集合（投递来源）；
/// - `scope`：柜子范围口径。
///
/// 排序：投递到件优先，其次创建时间倒序（新的在前）。
pub fn select_pending(
    shares: &[ShareItem],
    delivered: &BTreeSet<String>,
    scope: Scope,
) -> Vec<PendingItem> {
    let mut out: Vec<PendingItem> = shares
        .iter()
        .filter(|s| is_pending(s))
        .filter_map(|s| {
            let code = s.pickup_code.clone().unwrap_or_default();
            let is_delivered = delivered.contains(&code);
            if scope == Scope::Delivered && !is_delivered {
                return None;
            }
            Some(PendingItem {
                pickup_code: code,
                file_name: s.file_name.clone(),
                size: s.size,
                created_at: s.created_at.clone(),
                delivered: is_delivered,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.delivered
            .cmp(&a.delivered)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    out
}

/// 人类可读的字节数（托盘/通知里用；纯展示，不进剪贴板）。
pub fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(pickup_code: Option<&str>) -> ShareItem {
        ShareItem {
            pickup_code: pickup_code.map(|s| s.to_string()),
            file_name: "合同.pdf".into(),
            size: 2048,
            viewer_count: 0,
            is_expired: false,
            status: "normal".into(),
            deleted_at: None,
            created_at: "2026-11-01T10:00:00Z".into(),
        }
    }

    fn notice(content: &str) -> Notice {
        Notice {
            content: content.into(),
            notice_type: DELIVERY_NOTICE_TYPE.into(),
        }
    }

    // ── 隐私承诺：剪贴板只放 6 位取件码 ──

    #[test]
    fn is_pickup_code_accepts_six_alnum() {
        assert!(is_pickup_code("A1B2C3"));
        assert!(is_pickup_code("abc123"));
        assert!(is_pickup_code("000000"));
        assert!(is_pickup_code("aB3dEf"));
    }

    #[test]
    fn is_pickup_code_rejects_wrong_shape() {
        assert!(!is_pickup_code(""));
        assert!(!is_pickup_code("A1B2C")); // 5 位
        assert!(!is_pickup_code("A1B2C3D")); // 7 位
        assert!(!is_pickup_code("A1B2C!")); // 非字母数字
        assert!(!is_pickup_code("A1B2 C")); // 含空格
        assert!(!is_pickup_code("A1B2C3\n")); // 含换行
        assert!(!is_pickup_code("取件码12")); // 含非 ASCII
    }

    #[test]
    fn clipboard_payload_is_exactly_the_code() {
        assert_eq!(clipboard_payload("A1B2C3").unwrap(), "A1B2C3");
        // 载荷长度必须恒等于取件码长度：多一个字符都不行。
        let payload = clipboard_payload("abc123").unwrap();
        assert_eq!(payload.len(), PICKUP_CODE_LEN);
        assert_eq!(payload.chars().count(), PICKUP_CODE_LEN);
    }

    #[test]
    fn clipboard_payload_refuses_non_code() {
        // 链接、带口令的串、纯文本 ⇒ 一律拒写（这是隐私承诺的硬边界）。
        for bad in [
            "",
            "A1B2C",
            "A1B2C3D",
            "https://box.local/s/abc",
            "A1B2C3 密码",
            "password",
            "A1B2C3;rm -rf /",
        ] {
            assert!(
                clipboard_payload(bad).is_err(),
                "不得写入剪贴板: {bad:?}"
            );
        }
    }

    #[test]
    fn clipboard_payload_never_contains_link_or_password() {
        // 正向用例：载荷里除取件码本身外没有任何内容。
        let payload = clipboard_payload("Z9Y8X7").unwrap();
        assert!(!payload.contains("://"));
        assert!(!payload.contains(" "));
        assert!(!payload.contains('/'));
        assert_eq!(payload, "Z9Y8X7");
    }

    // ── 待取判定 ──

    #[test]
    fn is_pending_requires_code_and_freshness() {
        assert!(is_pending(&share(Some("A1B2C3"))));

        let mut s = share(Some("A1B2C3"));
        s.pickup_code = None;
        assert!(!is_pending(&s), "没有取件码的分享不可取件");

        let mut s = share(Some("A1B2C3"));
        s.pickup_code = Some("SHORT".into());
        assert!(!is_pending(&s), "非法取件码不算待取");

        let mut s = share(Some("A1B2C3"));
        s.viewer_count = 1;
        assert!(!is_pending(&s), "已被取走的不算待取");

        let mut s = share(Some("A1B2C3"));
        s.is_expired = true;
        assert!(!is_pending(&s));

        let mut s = share(Some("A1B2C3"));
        s.deleted_at = Some("2026-11-01T11:00:00Z".into());
        assert!(!is_pending(&s), "回收站里的不算待取");

        let mut s = share(Some("A1B2C3"));
        s.status = "blocked".into();
        assert!(!is_pending(&s), "被管控的不算待取");

        let mut s = share(Some("A1B2C3"));
        s.status = "pending_review".into();
        assert!(!is_pending(&s));
    }

    #[test]
    fn is_pending_allows_empty_status() {
        // 老数据可能没有 status 字段 ⇒ 空值按 normal 处理，不误杀。
        let mut s = share(Some("A1B2C3"));
        s.status = String::new();
        assert!(is_pending(&s));
    }

    // ── 到件通知解析 ──

    #[test]
    fn parse_delivery_code_reads_box_format() {
        let content = "投递链接: 1a2b3c4d…\n文件数: 1\n总大小: 2048 字节\n投递人 IP: 10.0.0.1\n分享码: sharecode\n取件码: A1B2C3";
        assert_eq!(parse_delivery_code(content).as_deref(), Some("A1B2C3"));
    }

    #[test]
    fn parse_delivery_code_accepts_fullwidth_colon_and_spaces() {
        assert_eq!(
            parse_delivery_code("取件码：A1B2C3").as_deref(),
            Some("A1B2C3")
        );
        assert_eq!(
            parse_delivery_code("取件码 : A1B2C3").as_deref(),
            Some("A1B2C3")
        );
        assert_eq!(
            parse_delivery_code("取件码\nA1B2C3").as_deref(),
            Some("A1B2C3")
        );
    }

    #[test]
    fn parse_delivery_code_refuses_garbage() {
        assert_eq!(parse_delivery_code(""), None);
        assert_eq!(parse_delivery_code("投递链接: abc"), None);
        assert_eq!(parse_delivery_code("取件码: "), None);
        assert_eq!(parse_delivery_code("取件码: SHORT"), None);
        assert_eq!(parse_delivery_code("取件码: A1B2C!"), None);
    }

    #[test]
    fn delivery_pickup_codes_only_reads_delivery_notices() {
        let notices = vec![
            notice("取件码: A1B2C3"),
            Notice {
                notice_type: "system".into(),
                content: "取件码: D4E5F6".into(),
                ..notice("")
            },
        ];
        let codes = delivery_pickup_codes(&notices);
        assert!(codes.contains("A1B2C3"));
        assert!(!codes.contains("D4E5F6"), "非到件通知不得被当作投递来源");
    }    // ── 选件 ──

    #[test]
    fn select_pending_delivered_scope_filters_out_my_own_shares() {
        let shares = vec![share(Some("A1B2C3")), share(Some("D4E5F6"))];
        let mut delivered = BTreeSet::new();
        delivered.insert("A1B2C3".to_string());

        let picked = select_pending(&shares, &delivered, Scope::Delivered);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].pickup_code, "A1B2C3");
        assert!(picked[0].delivered);
    }

    #[test]
    fn select_pending_all_scope_keeps_everything_pending() {
        let shares = vec![share(Some("A1B2C3")), share(Some("D4E5F6"))];
        let mut delivered = BTreeSet::new();
        delivered.insert("A1B2C3".to_string());

        let picked = select_pending(&shares, &delivered, Scope::All);
        assert_eq!(picked.len(), 2);
        // 投递到件排前面
        assert_eq!(picked[0].pickup_code, "A1B2C3");
        assert!(picked[0].delivered);
        assert_eq!(picked[1].pickup_code, "D4E5F6");
        assert!(!picked[1].delivered);
    }

    #[test]
    fn select_pending_never_yields_a_non_code() {
        // 即便上游给了脏数据，进待取列表的取件码必须仍是合法 6 位码
        // （剪贴板写入口只在合法时才放行 ⇒ 这里提前拦一道）。
        let mut bad = share(Some("NOT-CODE"));
        bad.pickup_code = Some("NOT-CODE".into());
        let shares = vec![bad];
        let picked = select_pending(&shares, &BTreeSet::new(), Scope::All);
        assert!(picked.is_empty());
    }

    #[test]
    fn scope_parse_roundtrip() {
        assert_eq!(Scope::parse("delivered"), Some(Scope::Delivered));
        assert_eq!(Scope::parse("ALL"), Some(Scope::All));
        assert_eq!(Scope::parse(" all "), Some(Scope::All));
        assert_eq!(Scope::parse("nonsense"), None);
        assert_eq!(Scope::default().as_str(), "delivered");
        assert_eq!(Scope::All.as_str(), "all");
    }

    #[test]
    fn tray_label_has_no_link_or_password() {
        let item = PendingItem {
            pickup_code: "A1B2C3".into(),
            file_name: "合同.pdf".into(),
            size: 10,
            created_at: String::new(),
            delivered: true,
        };
        let label = item.tray_label();
        assert!(label.contains("A1B2C3"));
        assert!(label.contains("合同.pdf"));
        assert!(!label.contains("://"));

        // 文本分享（无文件名）也有可读标签
        let text_item = PendingItem {
            file_name: String::new(),
            ..item
        };
        assert!(text_item.tray_label().contains("文本分享"));
    }

    #[test]
    fn format_size_is_human_readable() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }
}
