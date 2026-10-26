// PigeonBox 桌面客户端（Tauri 2）。
//
// 设计（v1 远程模式）：
//   - 主窗加载内置设置页，用户填服务器地址后经 `connect` 命令导航到该地址，
//     此后主窗即服务器 Web UI（托盘「显示主窗」随时唤回设置页）。
//   - 地址持久化在应用数据目录（config.json），下次启动自动重连。
//   - 信任模型：远程页面运行在主 webview 中但只暴露 connect/get_saved/save_url
//     等无危险命令（withGlobalTauri 注入的 API 面即以上）；关闭窗口=退出。
//
// 设备直传（2026-10-22 起）：直传已拆为独立应用 PigeonBeam（信鸽直传），
// 桌面端不再随包 p2pc sidecar、也不再编排设备直传。**设置页的直传面板与
// 对应命令一并退场**：面板只留一句「去 PigeonBeam 做直传」的指引，
// 死控件（注册中心/传输模式/中继/发送/接收/配对）与其占位命令全部删除
// ——留着只会让人以为还能用（点下去必失败）。
// 连带：tauri-plugin-dialog 原本只服务「选择文件发送」，现已无调用方；
// tauri-plugin-shell 随 sidecar 退场一并无调用方 —— 两个依赖与 dialog 的
// capability 权限一并摘除（Cargo.lock 随之更新，见同提交）。
use std::path::{Path, PathBuf};

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, Url,
};

/// 设置页资源（内嵌前端入口，相对于本地 app 源）。
const SETTINGS_RESOURCE: &str = "index.html";

/// 配置文件名（应用数据目录内）。
const CONFIG_FILE: &str = "config.json";

/// 本地 app 源：启动时捕获的主窗 URL（tauri://localhost/index.html 等）。
///
/// 主窗 `connect` 导航到服务器后，文档源变成**远端源**；此时任何**相对**
/// URL（如 `index.html`）都会解析到远端服务器自己 ⇒ 托盘「服务器设置」
/// 打开的是远端 UI，用户回不到本地设置页、换不了服务器
/// （2026-10-26 审计 P1）。故启动时把本地 app 源抓下来，之后一律按它导航。
#[derive(Clone, Debug)]
struct AppBase(Url);

/// 设置页目标 URL：由本地 app 源派生的**绝对** URL。
///
/// 绝不能是相对路径（见 `AppBase` 注释）。scheme/host 恒为本地 app 源，
/// 与主窗当前文档源（可能是远端服务器）无关。
fn settings_target_url(app_base: &Url) -> String {
    let mut u = app_base.clone();
    u.set_path(SETTINGS_RESOURCE);
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

/// 本地 app 源（未捕获时返回 None）。
fn app_base(app: &tauri::AppHandle) -> Option<Url> {
    app.try_state::<AppBase>().map(|s| s.0.clone())
}

/// 回设置页（托盘「服务器设置」用）：导航到**本地 app 源**的绝对 URL。
#[tauri::command]
fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let base = app_base(&app).ok_or("本地 app 源未知，无法回到设置页")?;
    let target = settings_target_url(&base);
    let url: Url = target
        .parse()
        .map_err(|e| format!("设置页 URL 非法: {e}"))?;
    win.navigate(url).map_err(|e| format!("导航失败: {e}"))
}

/// URL 归一：无 scheme 补 `http://`（内网 NAS 场景的既有便利，明文确认由
/// 前端负责），有 scheme 则大小写不敏感识别并解析校验。
///
/// 2026-10-26 审计 P1：此前用 `starts_with("http://")` 判 scheme（**大小写
/// 敏感**），输入 `HTTPS://box.local` 不被识别 ⇒ 被补成
/// `http://HTTPS://box.local` ⇒ URL 解析失败，且提示语与真实原因无关。
/// 口径与前端 `ui/app.js` 的 `/^[a-z][a-z0-9+.-]*:\/\//i` 对齐。
fn normalize_url(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("地址不能为空".into());
    }
    let candidate = if has_scheme(t) {
        t.to_string()
    } else {
        format!("http://{}", t.trim_end_matches('/'))
    };
    // 解析校验：失败时给真实原因（url crate 会把 scheme 归一为小写）
    let mut parsed: Url = candidate
        .parse()
        .map_err(|e| format!("URL 解析失败: {e}"))?;
    // 路径尾斜杠归一（既有存盘形态：地址不带尾斜杠）
    let trimmed_path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(&trimmed_path);
    let mut out = parsed.to_string();
    if parsed.path() == "/" && out.ends_with('/') {
        out.pop();
    }
    Ok(out)
}

/// 是否带 scheme（大小写不敏感，规则同 RFC 3986：首字符为字母，后续为
/// 字母/数字/`+`/`-`/`.`）。
fn has_scheme(s: &str) -> bool {
    match s.find("://") {
        Some(i) if i > 0 => {
            let mut chars = s[..i].chars();
            match chars.next() {
                Some(first) => {
                    first.is_ascii_alphabetic()
                        && chars
                            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
                }
                None => false,
            }
        }
        _ => false,
    }
}

/// 配置文件路径（应用数据目录内）。
fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// 读已存地址（不存在/读失败=空串）。
fn read_config_from(dir: &Path) -> String {
    std::fs::read_to_string(config_path(dir))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 写已存地址。
fn write_config_to(dir: &Path, url: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
    std::fs::write(config_path(dir), url).map_err(|e| format!("write config: {e}"))
}

#[tauri::command]
fn get_saved(app: tauri::AppHandle) -> String {
    app.path()
        .app_data_dir()
        .map(|dir| read_config_from(&dir))
        .unwrap_or_default()
}

#[tauri::command]
fn save_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    write_config_to(&dir, &url)
}

/// 导航主窗到服务器地址；成功后持久化。
#[tauri::command]
fn connect(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let normalized = normalize_url(&url)?;
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let parsed: Url = normalized
        .parse()
        .map_err(|e| format!("URL 解析失败: {e}"))?;
    win.navigate(parsed).map_err(|e| format!("导航失败: {e}"))?;
    save_url(app, normalized)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            connect,
            get_saved,
            save_url,
            open_settings
        ])
        .setup(|app| {
            // 捕获本地 app 源（设置页导航锚点）：此刻主窗文档源还是本地 app，
            // 之后 connect 导航到远端，相对 URL 就再也回不来了。
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(u) = win.url() {
                    app.manage(AppBase(u));
                }
            }
            // 托盘：左键/菜单「显示主窗」，菜单含「服务器设置」「退出」。
            let show = MenuItem::with_id(app, "show", "显示主窗", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "服务器设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("PigeonBox")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "settings" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                            // 与 open_settings 命令同一 helper：导航到本地 app 源
                            // 的绝对 URL（相对路径会落到远端服务器自己身上）。
                            if let Some(base) = app_base(app) {
                                if let Ok(url) = settings_target_url(&base).parse::<Url>() {
                                    let _ = win.navigate(url);
                                }
                            }
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键点击托盘 = 唤回主窗
                    if let tauri::tray::TrayIconEvent::Click { .. } = event {
                        if let Some(win) = tray.app_handle().get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- URL 归一（2026-10-26 审计 P1）----

    #[test]
    fn normalize_url_adds_http_scheme_when_missing() {
        assert_eq!(normalize_url("box.local").unwrap(), "http://box.local");
        assert_eq!(
            normalize_url("box.local:8080").unwrap(),
            "http://box.local:8080"
        );
        assert_eq!(normalize_url("  box.local  ").unwrap(), "http://box.local");
    }

    #[test]
    fn normalize_url_scheme_is_case_insensitive() {
        // 大写 scheme 是合法写法：不得被补成 "http://HTTPS://…"。
        assert_eq!(
            normalize_url("HTTPS://box.local").unwrap(),
            "https://box.local"
        );
        assert_eq!(
            normalize_url("HtTp://box.local").unwrap(),
            "http://box.local"
        );
        assert_eq!(
            normalize_url("HTTPS://box.local/").unwrap(),
            "https://box.local"
        );
    }

    #[test]
    fn normalize_url_rejects_empty_and_broken() {
        assert!(normalize_url("").is_err());
        assert!(normalize_url("   ").is_err());
        assert!(normalize_url("http://").is_err());
    }

    #[test]
    fn has_scheme_matches_frontend_rule() {
        // 前端 ui/app.js 的 /^[a-z][a-z0-9+.-]*:\/\//i 口径
        assert!(has_scheme("http://x"));
        assert!(has_scheme("HTTPS://x"));
        assert!(has_scheme("foo+bar://x"));
        assert!(!has_scheme("box.local"));
        assert!(!has_scheme("1http://x"));
        assert!(!has_scheme("://x"));
    }

    // ---- 设置页导航目标（2026-10-26 审计 P1）----

    /// 同源判定（scheme+host+port）。
    ///
    /// 不用 `Url::origin()`：非特殊 scheme（如 `tauri:`）的 origin 是
    /// **不透明**的，每次 parse 都产出新实例 ⇒ 跨 URL 比较恒不相等。
    fn same_origin(a: &Url, b: &Url) -> bool {
        a.scheme() == b.scheme()
            && a.host_str() == b.host_str()
            && a.port_or_known_default() == b.port_or_known_default()
    }

    #[test]
    fn settings_target_is_absolute_local_app_url() {
        let base = Url::parse("tauri://localhost/index.html").unwrap();
        let target = settings_target_url(&base);
        assert!(
            !target.starts_with(SETTINGS_RESOURCE),
            "设置页目标不得是相对路径: {target}"
        );
        let parsed = Url::parse(&target).expect("设置页目标必须是绝对 URL");
        assert!(same_origin(&parsed, &base));
        assert_eq!(parsed.path(), "/index.html");
    }

    #[test]
    fn settings_target_stays_on_local_app_origin_after_connect() {
        let base = Url::parse("tauri://localhost/index.html").unwrap();
        // 连上服务器后主窗文档源 = 远端
        let remote = Url::parse("http://box.local/").unwrap();
        let target = Url::parse(&settings_target_url(&base)).unwrap();
        // 目标恒在本地 app 源上，与当前文档源无关
        assert!(same_origin(&target, &base));
        assert!(!same_origin(&target, &remote));
        assert_ne!(
            target.as_str(),
            remote.join(SETTINGS_RESOURCE).unwrap().as_str(),
            "相对路径会落到远端服务器自己身上"
        );
    }

    #[test]
    fn settings_navigation_uses_absolute_helper_not_relative_eval() {
        // 源码级守卫：相对 location.replace 是本次修复的缺陷形态，不得复现。
        // 探针串运行时拼（否则守卫会匹配到本文件里的探针字面量本身）。
        let needle = format!("location.replace('{}')", SETTINGS_RESOURCE);
        let src = include_str!("lib.rs");
        assert!(
            !src.contains(&needle),
            "不得用相对 location.replace 回设置页（会打开远端服务器自己的 index.html）"
        );
        assert!(
            src.contains("settings_target_url"),
            "设置页导航必须走 settings_target_url helper"
        );
    }

    // ---- config.json 持久化 ----

    #[test]
    fn config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pb-desktop-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_config_from(&dir), "", "未写过的配置应为空串");
        write_config_to(&dir, "http://box.local").unwrap();
        assert_eq!(read_config_from(&dir), "http://box.local");
        // 读回内容去空白
        write_config_to(&dir, "  http://box.local  ").unwrap();
        assert_eq!(read_config_from(&dir), "http://box.local");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_path_is_config_json_in_dir() {
        assert_eq!(
            config_path(Path::new("/tmp/x")),
            PathBuf::from("/tmp/x").join("config.json")
        );
    }
}
