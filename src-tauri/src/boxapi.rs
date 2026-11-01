// 与 Box（core 服务端）之间的**最小** HTTP 客户端：只取两个面。
//
//   GET {base}/api/v1/user/shares?status=active&page=1&page_size=50
//   GET {base}/api/v1/notifies/mine?page=1&page_size=50
//
// 两个端点都挂在 core 的 `UserOrAPIKey` 中间件上（gen/router/share/middleware.go
// 的 `_apiMw` 与 gen/router/notify/middleware.go 的 `_apiMw`）⇒ **API Key 可用**
// （JWT 也行，但桌面端常驻托盘、没有会话 ⇒ 用 API Key）。
//
// ⚠️ 别踩的坑（真机抓过）：`GET /user/api-keys` 是**会话面**，用 API Key 调会 401；
// `GET /api/v1/user/requests`（寄件码链接列表）同样只认 JWT（request 域的
// `_userMw` 挂的是 `UserAuth`）⇒ 桌面端**不能**用它。故投递来源改由
// 「我的通知」反查（见 pickup.rs 的说明）。
use crate::pickup::{Notice, ShareItem};
use serde::Deserialize;
use url::Url;

/// 取件列表单页条数（core 的 UserSharePolicy / NotifyPolicy 上限都是 100 ⇒ 取 50）。
const PAGE_SIZE: u32 = 50;

/// API Key 明文前缀（core/app/user/service.go 的 `apiKeyPrefix`）。
///
/// 只用于决定 Authorization 头的**形式**（Bearer vs ApiKey），不做任何校验——
/// 校验归服务端。
const API_KEY_PREFIX: &str = "pb_sk_";

/// 构造 Authorization 头的值。
///
/// core 的 `extractAPIKey` 认三种形式：
///   1. `Authorization: Bearer pb_sk_xxx`（首选；Bearer 只在 `pb_sk_` 前缀时才算 Key）
///   2. `Authorization: ApiKey xxx`（不限前缀）
///   3. `X-API-Key: xxx`
///
/// ⇒ `pb_sk_` 开头走 Bearer（服务端首选形式），其余走 ApiKey（不限前缀，最稳）。
/// **绝不放 query**（core 明确拒收 query 传参，防访问日志/Referer 泄漏）。
pub fn auth_header_value(api_key: &str) -> String {
    if api_key.starts_with(API_KEY_PREFIX) {
        format!("Bearer {api_key}")
    } else {
        format!("ApiKey {api_key}")
    }
}

/// Box 响应信封（core 统一 `code`/`message`/`data`）。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    /// 缺省 = 无 data（`Option` 天然可缺省 ⇒ 不能加 `default`，
    /// 那会要求 `T: Default`）。
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct SharesData {
    #[serde(default)]
    items: Vec<ShareItem>,
}

#[derive(Debug, Default, Deserialize)]
struct NoticesData {
    #[serde(default)]
    items: Vec<Notice>,
}

/// Box 客户端。
pub struct BoxClient {
    base: Url,
    api_key: String,
    http: reqwest::blocking::Client,
}

impl BoxClient {
    /// 构造客户端。`base` 是服务器根地址（不带尾斜杠），`api_key` 是明文 API Key。
    pub fn new(base: &str, api_key: &str) -> Result<Self, String> {
        let trimmed = base.trim();
        if trimmed.is_empty() {
            return Err("服务器地址为空".into());
        }
        if api_key.trim().is_empty() {
            return Err("API Key 为空".into());
        }
        let base: Url = trimmed
            .parse()
            .map_err(|e| format!("服务器地址非法: {e}"))?;
        if base.scheme() != "http" && base.scheme() != "https" {
            return Err(format!("服务器地址须是 http/https：{trimmed}"));
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(concat!("PigeonBoxDesktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))?;
        Ok(BoxClient {
            base,
            api_key: api_key.trim().to_string(),
            http,
        })
    }

    /// 拼端点 URL（在 base 之后追加 path，保留 base 可能带的前缀路径）。
    fn endpoint(&self, path: &str) -> Result<Url, String> {
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                "服务器地址不能是带查询串或片段的 URL".to_string()
            })?;
            segments.pop_if_empty();
            for seg in path.split('/').filter(|s| !s.is_empty()) {
                segments.push(seg);
            }
        }
        Ok(url)
    }

    /// 发起一次 GET 并解信封。
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: Url,
    ) -> Result<Envelope<T>, String> {
        let resp = self
            .http
            .get(url)
            .header("Authorization", auth_header_value(&self.api_key))
            .send()
            .map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| format!("读取响应失败: {e}"))?;
        if !status.is_success() {
            return Err(format!("服务端返回 {status}"));
        }
        let env: Envelope<T> =
            serde_json::from_str(&body).map_err(|e| format!("响应不是合法 JSON: {e}"))?;
        if env.code != 0 {
            let msg = if env.message.trim().is_empty() {
                "未知错误".to_string()
            } else {
                env.message.clone()
            };
            return Err(format!("服务端返回错误({})：{msg}", env.code));
        }
        Ok(env)
    }

    /// 我账号下**未过期**的分享列表（core 的 `status=active` 口径：
    /// 未过期且还有剩余取件次数；软删除的不在其中）。
    pub fn list_shares(&self) -> Result<Vec<ShareItem>, String> {
        let mut url = self.endpoint("/api/v1/user/shares")?;
        url.query_pairs_mut()
            .append_pair("status", "active")
            .append_pair("page", "1")
            .append_pair("page_size", &PAGE_SIZE.to_string());
        let env = self.get_json::<SharesData>(url)?;
        Ok(env.data.map(|d| d.items).unwrap_or_default())
    }

    /// 我的通知列表（最新在前）。用于反查「投递到件」的取件码。
    pub fn list_notices(&self) -> Result<Vec<Notice>, String> {
        let mut url = self.endpoint("/api/v1/notifies/mine")?;
        url.query_pairs_mut()
            .append_pair("page", "1")
            .append_pair("page_size", &PAGE_SIZE.to_string());
        let env = self.get_json::<NoticesData>(url)?;
        Ok(env.data.map(|d| d.items).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_uses_bearer_for_official_prefix() {
        assert_eq!(
            auth_header_value("pb_sk_abc123"),
            "Bearer pb_sk_abc123"
        );
    }

    #[test]
    fn auth_header_falls_back_to_apikey_scheme() {
        // 非 pb_sk_ 前缀：Bearer 会被 core 当成 JWT 拒掉 ⇒ 必须走 ApiKey。
        assert_eq!(auth_header_value("whatever"), "ApiKey whatever");
    }

    #[test]
    fn client_rejects_bad_base_url() {
        assert!(BoxClient::new("", "pb_sk_x").is_err());
        assert!(BoxClient::new("   ", "pb_sk_x").is_err());
        assert!(BoxClient::new("ftp://box.local", "pb_sk_x").is_err());
        assert!(BoxClient::new("not a url", "pb_sk_x").is_err());
    }

    #[test]
    fn client_requires_api_key() {
        assert!(BoxClient::new("http://box.local", "").is_err());
        assert!(BoxClient::new("http://box.local", "   ").is_err());
    }

    #[test]
    fn client_accepts_normal_base_url() {
        assert!(BoxClient::new("http://10.0.0.3:56789", "pb_sk_x").is_ok());
        assert!(BoxClient::new("https://box.example.com", "pb_sk_x").is_ok());
        // 带尾斜杠也接受
        assert!(BoxClient::new("https://box.example.com/", "pb_sk_x").is_ok());
    }

    #[test]
    fn endpoint_joins_paths_without_doubling_slash() {
        let c = BoxClient::new("http://box.local", "pb_sk_x").unwrap();
        assert_eq!(
            c.endpoint("/api/v1/user/shares").unwrap().as_str(),
            "http://box.local/api/v1/user/shares"
        );
        let c = BoxClient::new("http://box.local/", "pb_sk_x").unwrap();
        assert_eq!(
            c.endpoint("/api/v1/notifies/mine").unwrap().as_str(),
            "http://box.local/api/v1/notifies/mine"
        );
    }

    #[test]
    fn endpoint_keeps_base_path_prefix() {
        // 服务器挂在子路径（如 https://host/pigeonbox）时不能把前缀丢了。
        let c = BoxClient::new("https://host/pigeonbox", "pb_sk_x").unwrap();
        assert_eq!(
            c.endpoint("/api/v1/user/shares").unwrap().as_str(),
            "https://host/pigeonbox/api/v1/user/shares"
        );
    }

    #[test]
    fn envelope_rejects_nonzero_code() {
        let env: Envelope<SharesData> =
            serde_json::from_str(r#"{"code":401,"message":"Invalid API Key"}"#).unwrap();
        assert_eq!(env.code, 401);
        assert_eq!(env.message, "Invalid API Key");
        assert!(env.data.is_none());
    }

    #[test]
    fn envelope_parses_items() {
        let env: Envelope<SharesData> = serde_json::from_str(
            r#"{"code":0,"message":"ok","data":{"items":[{"code":"abc","pickup_code":"A1B2C3","file_name":"x.pdf","size":10,"viewer_count":0,"is_expired":false,"status":"normal","created_at":"2026-11-01T10:00:00Z"}],"total":1}}"#,
        )
        .unwrap();
        let items = env.data.unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pickup_code.as_deref(), Some("A1B2C3"));
        assert_eq!(items[0].file_name, "x.pdf");
    }

    // ── 真 HTTP 往返（打本地 mock Box）──

    /// 起一个一次性 HTTP 服务：按路径关键字返回预置响应，并记录收到的
    /// 请求行与请求头。
    fn spawn_mock(
        routes: Vec<(&'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
        let addr = listener.local_addr().expect("读本地地址");
        let handle = std::thread::spawn(move || {
            let mut seen: Vec<String> = Vec::new();
            for _ in 0..routes.len() {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                reader.read_line(&mut request_line).expect("读请求行");
                let mut headers: Vec<String> = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("读请求头");
                    if line.trim().is_empty() {
                        break;
                    }
                    headers.push(line.trim_end().to_string());
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("");
                let body = routes
                    .iter()
                    .find(|(key, _)| path.contains(key))
                    .map(|(_, body)| *body)
                    .unwrap_or(r#"{"code":404,"message":"no route"}"#);
                seen.push(format!("{path}|{}", headers.join(";")));
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).expect("写响应");
                stream.flush().expect("flush");
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn list_shares_hits_right_path_with_bearer_and_parses_items() {
        let (base, server) = spawn_mock(vec![(
            "/api/v1/user/shares",
            r#"{"code":0,"message":"ok","data":{"items":[{"pickup_code":"A1B2C3","file_name":"x.pdf","size":10,"viewer_count":0,"is_expired":false,"status":"normal","created_at":"2026-11-01T10:00:00Z"}],"total":1}}"#,
        )]);
        let client = BoxClient::new(&base, "pb_sk_testkey").expect("建客户端");
        let items = client.list_shares().expect("拉分享列表");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pickup_code.as_deref(), Some("A1B2C3"));

        let seen = server.join().expect("mock 线程结束");
        assert_eq!(seen.len(), 1);
        let line = &seen[0];
        assert!(
            line.starts_with("/api/v1/user/shares?"),
            "请求路径不对: {line}"
        );
        assert!(line.contains("status=active"), "必须只取未过期的: {line}");
        assert!(line.contains("page=1"), "必须翻第一页: {line}");
        assert!(
            line.to_ascii_lowercase().contains("authorization: bearer pb_sk_testkey"),
            "必须用 Bearer 带 API Key: {line}"
        );
        // ⚠️ 绝不把凭据放 query（core 明确拒收，防访问日志/Referer 泄漏）。
        // 只查**路径部分**（`|` 之后是请求头，里头本来就有 Authorization）。
        let path_part = line.split('|').next().unwrap_or("");
        assert!(
            !path_part.contains("pb_sk_"),
            "凭据不得进 query: {path_part}"
        );
    }

    #[test]
    fn list_notices_hits_right_path_and_parses_notices() {
        let (base, server) = spawn_mock(vec![(
            "/api/v1/notifies/mine",
            r#"{"code":0,"message":"ok","data":{"items":[{"type":"request_received","content":"取件码: A1B2C3"}],"total":1}}"#,
        )]);
        let client = BoxClient::new(&base, "pb_sk_testkey").expect("建客户端");
        let notices = client.list_notices().expect("拉通知列表");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].notice_type, "request_received");
        assert_eq!(
            crate::pickup::delivery_pickup_codes(&notices)
                .iter()
                .next()
                .map(|s| s.as_str()),
            Some("A1B2C3")
        );

        let seen = server.join().expect("mock 线程结束");
        assert!(
            seen[0].starts_with("/api/v1/notifies/mine?"),
            "请求路径不对: {}",
            seen[0]
        );
        assert!(seen[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer pb_sk_testkey"));
    }

    #[test]
    fn api_error_envelope_is_reported() {
        let (base, _server) = spawn_mock(vec![(
            "/api/v1/user/shares",
            r#"{"code":401,"message":"Invalid API Key"}"#,
        )]);
        let client = BoxClient::new(&base, "pb_sk_bad").expect("建客户端");
        let err = client.list_shares().expect_err("应报错");
        assert!(err.contains("401"), "错误信息应带服务端错误码: {err}");
        assert!(err.contains("Invalid API Key"), "错误信息应带服务端消息: {err}");
    }

    /// 端到端（打 mock Box）：分享列表 + 到件通知 ⇒ 待取清单 ⇒ 剪贴板载荷。
    ///
    /// 这条用例把隐私承诺串起来验：真 HTTP 取回 ⇒ 判定待取 ⇒ 剪贴板载荷
    /// **只有**那个 6 位码（链接/口令/文件名一律不进）。
    #[test]
    fn end_to_end_pipeline_yields_code_only_clipboard_payload() {
        use crate::pickup::{clipboard_payload, delivery_pickup_codes, select_pending, Scope};

        let shares_body = r#"{"code":0,"message":"ok","data":{"items":[
            {"pickup_code":"A1B2C3","file_name":"到件.pdf","size":100,"viewer_count":0,"is_expired":false,"status":"normal","created_at":"2026-11-01T12:00:00Z"},
            {"pickup_code":"D4E5F6","file_name":"我寄出的.pdf","size":200,"viewer_count":0,"is_expired":false,"status":"normal","created_at":"2026-11-01T11:00:00Z"},
            {"pickup_code":"G7H8I9","file_name":"已取走.pdf","size":300,"viewer_count":1,"is_expired":false,"status":"normal","created_at":"2026-11-01T10:00:00Z"}
        ],"total":3}}"#;
        let notices_body = r#"{"code":0,"message":"ok","data":{"items":[
            {"type":"request_received","content":"投递链接: 1a2b3c4d…\n文件数: 1\n总大小: 100 字节\n投递人 IP: 10.0.0.1\n分享码: sc1\n取件码: A1B2C3"}
        ],"total":1}}"#;

        let (base, server) = spawn_mock(vec![
            ("/api/v1/user/shares", shares_body),
            ("/api/v1/notifies/mine", notices_body),
        ]);
        let client = BoxClient::new(&base, "pb_sk_testkey").expect("建客户端");
        let shares = client.list_shares().expect("拉分享");
        let notices = client.list_notices().expect("拉通知");
        server.join().expect("mock 线程结束");

        let delivered = delivery_pickup_codes(&notices);
        let pending = select_pending(&shares, &delivered, Scope::Delivered);

        // 只有「投递到件 + 未被取走 + 未过期」的那一件进待取清单。
        assert_eq!(pending.len(), 1, "待取清单应只有投递到件的那件");
        assert_eq!(pending[0].pickup_code, "A1B2C3");
        assert!(pending[0].delivered);

        // 剪贴板载荷：**只有** 6 位码。
        let payload = clipboard_payload(&pending[0].pickup_code).expect("写剪贴板");
        assert_eq!(payload, "A1B2C3");
        assert_eq!(payload.len(), 6);
        assert!(!payload.contains("://"));
        assert!(!payload.contains("到件"));
        assert!(!payload.contains("pdf"));
    }
}
