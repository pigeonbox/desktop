// PigeonBox 桌面客户端（Tauri 2）。
//
// 设计（v1 远程模式）：
//   - 主窗加载内置设置页，用户填服务器地址后经 `connect` 命令导航到该地址，
//     此后主窗即服务器 Web UI（托盘「显示主窗」随时唤回设置页）。
//   - 地址持久化在应用数据目录（config.json），下次启动自动重连。
//   - 信任模型：远程页面运行在主 webview 中，而 withGlobalTauri 注入的自定义命令
//     **不受** Tauri capability 约束 ⇒ 除白名单外**全部**命令过 `ensure_local_origin`
//     闸：只允许**本地设置页**调用，主窗导航到远端服务器后一律拒绝。覆盖
//     save_api_key / clear_api_key / set_pickup_scope / copy_pickup_code /
//     check_pickup_now / get_pickup_status（读侧，#144）/ save_url / connect /
//     get_saved（地址读写三件套，#143）；白名单仅 open_settings——只把窗口导航回
//     本地设置页，远端页调用反而把用户带回设置页，无害（#144 正文结论）。
//     机械守卫：tests::all_commands_pass_local_origin_gate_except_whitelist。
//     （#143 正文「10 个命令只有 3 个漏闸」的对账不准：实际漏 5 个——含
//     get_pickup_status 与无害的 open_settings，见 #144。）
//
// 一等取件端（2026-11-01 起）：托盘常驻当「快递柜」——
//   - 后台轮询 Box 的「我的分享」+「我的通知」两个面，算出**待取件**；
//   - 有新件 ⇒ 托盘菜单出现「一键取件」，并弹系统通知「你的柜子有件待取」；
//   - 一键取件 ⇒ **剪贴板里只有 6 位取件码**（不放链接、不放口令）。
//
// 设备直传（2026-10-22 起）：直传已拆为独立应用 PigeonBeam（信鸽直传），
// 桌面端不再随包 p2pc sidecar、也不再编排设备直传。**设置页的直传面板与
// 对应命令一并退场**：面板只留一句「去 PigeonBeam 做直传」的指引，
// 死控件（注册中心/传输模式/中继/发送/接收/配对）与其占位命令全部删除
// ——留着只会让人以为还能用（点下去必失败）。
// 连带：tauri-plugin-dialog 原本只服务「选择文件发送」，现已无调用方；
// tauri-plugin-shell 随 sidecar 退场一并无调用方 —— 两个依赖与 dialog 的
// capability 权限一并摘除（Cargo.lock 随之更新，见同提交）。
mod boxapi;
mod pickup;
mod secrets;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Manager, Url,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::NotificationExt;

use boxapi::BoxClient;
use pickup::{delivery_pickup_codes, select_pending, PendingItem, Scope};

/// 设置页资源（内嵌前端入口，相对于本地 app 源）。
const SETTINGS_RESOURCE: &str = "index.html";

/// 配置文件名（应用数据目录内）。
const CONFIG_FILE: &str = "config.json";

/// 取件配置文件名（应用数据目录内）：口径 + 已通知过的取件码台账。
const PICKUP_CONFIG_FILE: &str = "pickup.json";

/// 轮询间隔（秒）。
const POLL_INTERVAL_SECS: u64 = 60;

/// 已通知台账的容量上限（超出丢最旧的 ⇒ 文件不会无限长）。
const NOTIFIED_LEDGER_MAX: usize = 500;

/// 托盘「待取清单」子菜单最多列几件（再多就不像菜单了）。
const TRAY_LIST_MAX: usize = 20;

/// 本地 app 源：启动时捕获的主窗 URL（tauri://localhost/index.html 等）。
///
/// 主窗 `connect` 导航到服务器后，文档源变成**远端源**；此时任何**相对**
/// URL（如 `index.html`）都会解析到远端服务器自己 ⇒ 托盘「服务器设置」
/// 打开的是远端 UI，用户回不到本地设置页、换不了服务器
/// （2026-10-26 审计 P1）。故启动时把本地 app 源抓下来，之后一律按它导航。
#[derive(Clone, Debug)]
struct AppBase(Url);

/// 取件态：托盘菜单、设置页与轮询器共用的唯一真相源。
#[derive(Default)]
struct PickupState {
    /// 当前待取件（已按口径筛过）。
    pending: Vec<PendingItem>,
    /// 已知「投递到件」的取件码（由到件通知反查；失败时沿用上一轮的值）。
    delivered: BTreeSet<String>,
    /// 柜子范围口径。
    scope: Scope,
    /// 当前生效的服务器地址（None = 还没连过）。
    server: Option<String>,
    /// 是否已配置 API Key。
    configured: bool,
    /// 上次检查时刻（epoch 秒）。
    last_checked: Option<u64>,
    /// 上次检查的错误（None = 正常）。
    last_error: Option<String>,
    /// 最近一次复制到剪贴板的取件码（设置页回显；**只可能是 6 位码**）。
    last_copied: Option<String>,
}

/// 取件配置（落盘）：口径 + 已通知台账。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PickupConfig {
    /// 口径（"delivered" / "all"）。
    #[serde(default)]
    scope: Option<String>,
    /// 已通知过的取件码（去重台账）。
    #[serde(default)]
    notified: Vec<String>,
}

/// 设置页拿到的取件状态（**不含**任何明文密钥）。
#[derive(Debug, Clone, Serialize)]
struct PickupStatus {
    /// 是否已配置 API Key。
    configured: bool,
    /// 当前服务器地址。
    server: Option<String>,
    /// 柜子范围口径。
    scope: String,
    /// 待取件数量。
    pending_count: usize,
    /// 待取件清单。
    pending: Vec<PendingItem>,
    /// 上次检查时刻（epoch 秒）。
    last_checked: Option<u64>,
    /// 上次检查的错误。
    last_error: Option<String>,
    /// 最近一次复制到剪贴板的取件码。
    last_copied: Option<String>,
}

/// 设置页目标 URL：由本地 app 源派生的**绝对** URL。
///
/// 绝不能是相对路径（见 `AppBase` 注释）。scheme/host 恒为本地 app 源，
/// 与主窗当前文档源（可能是远端服务器）无关。
///
/// `manual = true` ⇒ 附带 `manual=1`：**用户是主动来设置页的，别把我弹回
/// 远端服务器**。少了它：设置页在 DOMContentLoaded 里会自动重连 ⇒ 页面立刻
/// 导航到远端服务器 ⇒ 设置页（含取件待办面板）**永远站不住** —— 用户既改不了
/// 服务器，也配不了 API Key（托盘「服务器设置」菜单项形同虚设）。
fn settings_target_url(app_base: &Url, manual: bool) -> String {
    let mut u = app_base.clone();
    u.set_path(SETTINGS_RESOURCE);
    u.set_query(if manual { Some("manual=1") } else { None });
    u.set_fragment(None);
    u.to_string()
}

/// 本地 app 源（未捕获时返回 None）。
fn app_base(app: &AppHandle) -> Option<Url> {
    app.try_state::<AppBase>().map(|s| s.0.clone())
}

/// 同源判定（scheme + host + port）。
///
/// 不用 `Url::origin()`：非特殊 scheme（如 `tauri:`）的 origin 是
/// **不透明**的，每次 parse 都产出新实例 ⇒ 跨 URL 比较恒不相等。
fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// 确保调用来自**本地设置页**，而不是主窗导航后的远端服务器页面。
///
/// ⚠️ 自定义命令**不受** Tauri capability 约束（capability 只管插件命令与
/// core 命令）⇒ 涉及 API Key / 剪贴板的命令必须自己把关：主窗 `connect`
/// 之后文档源就是远端服务器（可能是不受信任的站点），它不该能读写本机的
/// 钥匙串或剪贴板。
fn ensure_local_origin(app: &AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("主窗不存在")?;
    let current = win.url().map_err(|e| format!("读取当前页面地址失败: {e}"))?;
    let base = app_base(app).ok_or("本地 app 源未知，无法确认调用来源")?;
    if same_origin(&current, &base) {
        Ok(())
    } else {
        Err("该命令只允许在本地设置页调用".into())
    }
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

/// 取件配置文件路径（应用数据目录内）。
fn pickup_config_path(dir: &Path) -> PathBuf {
    dir.join(PICKUP_CONFIG_FILE)
}

/// 读取取件配置（不存在/读失败=默认值）。
fn read_pickup_config(dir: &Path) -> PickupConfig {
    std::fs::read_to_string(pickup_config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 写取件配置：**临时文件 + rename**（原子替换，#146）。
///
/// 直接整文件 write 会先截断再写：并发读方可能读到半截/空 JSON ⇒
/// read_pickup_config 的 unwrap_or_default 把台账清空 ⇒ 下一轮全量重通知。
fn write_pickup_config(dir: &Path, cfg: &PickupConfig) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
    let body = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化取件配置失败: {e}"))?;
    let tmp = dir.join(format!("{PICKUP_CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, body).map_err(|e| format!("写取件配置临时文件失败: {e}"))?;
    std::fs::rename(&tmp, pickup_config_path(dir)).map_err(|e| format!("写取件配置失败: {e}"))
}

/// 一次轮询的台账步：读台账 → 算新到件 → 追加/裁剪 → 写回 → 返回新到件码。
///
/// 台账里没有的取件码 ⇒ 新到件（重启/下一轮不会重复通知同一件）；
/// 写回即便无变化也执行（首次运行写出台账，避免下次重启重复通知）。
///
/// ⚠️ 调用方必须已持 POLL_LOCK（#146：整段读-改-写无互斥 ⇒ 并发轮询会
/// 丢台账、重复通知）；测试入口 poll_ledger_step 自己取同一把锁。
fn ledger_step_locked(dir: &Path, pending: &[String]) -> Vec<String> {
    let mut cfg = read_pickup_config(dir);
    let notified: BTreeSet<String> = cfg.notified.iter().cloned().collect();
    let fresh: Vec<String> = pending
        .iter()
        .filter(|c| !notified.contains(*c))
        .cloned()
        .collect();
    if !fresh.is_empty() {
        for code in &fresh {
            if !cfg.notified.contains(code) {
                cfg.notified.push(code.clone());
            }
        }
        if cfg.notified.len() > NOTIFIED_LEDGER_MAX {
            let drop = cfg.notified.len() - NOTIFIED_LEDGER_MAX;
            cfg.notified.drain(0..drop);
        }
    }
    let _ = write_pickup_config(dir, &cfg);
    fresh
}

/// 台账步的测试入口：与 poll_once 共用同一把 POLL_LOCK（#146）。
#[cfg(test)]
fn poll_ledger_step(dir: &Path, pending: &[String]) -> Vec<String> {
    let _g = lock_poll();
    ledger_step_locked(dir, pending)
}

/// 当前 epoch 秒（给「上次检查」用；只做相对时间，不需要时区）。
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 轮询互斥：poll_once 整体 + set_pickup_scope 的台账读改写共用这一把锁。
///
/// 三条独立路径（check_pickup_now 立即检查 / save_api_key 触发 spawn_poll /
/// spawn_poller 60s 循环）会并发跑 poll_once，整段读-改-写 pickup.json 此前
/// 无任何互斥 ⇒ 重复通知、后写覆盖先写丢台账、文件撕裂成半截 JSON（#146）。
///
/// 锁序恒为 POLL_LOCK → 状态锁（with_state），任何路径不得反序取锁。
static POLL_LOCK: Mutex<()> = Mutex::new(());

/// 取轮询锁（中毒时取回内部值，口径同 lock_state）。
fn lock_poll() -> std::sync::MutexGuard<'static, ()> {
    POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取状态锁（中毒时取回内部值而不是 panic —— 状态坏了不该让整个应用崩）。
fn lock_state(state: &Mutex<PickupState>) -> std::sync::MutexGuard<'_, PickupState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// 在状态锁内跑一段逻辑。
///
/// ⚠️ 回调里**不要**再去拿别的锁或重建托盘菜单（`Mutex` 不可重入）。
fn with_state<R>(app: &AppHandle, f: impl FnOnce(&mut PickupState) -> R) -> R {
    let state = app.state::<Mutex<PickupState>>();
    let mut guard = lock_state(&state);
    f(&mut guard)
}

/// 回设置页（托盘「服务器设置」用）：导航到**本地 app 源**的绝对 URL。
///
/// 带 `manual=1` ⇒ 前端不自动重连（否则设置页站不住，见
/// `settings_target_url` 的注释）。
#[tauri::command]
fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let base = app_base(&app).ok_or("本地 app 源未知，无法回到设置页")?;
    let target = settings_target_url(&base, true);
    let url: Url = target
        .parse()
        .map_err(|e| format!("设置页 URL 非法: {e}"))?;
    win.navigate(url).map_err(|e| format!("导航失败: {e}"))
}

/// 读回存盘地址（设置页回显用）。
#[tauri::command]
fn get_saved(app: tauri::AppHandle) -> Result<String, String> {
    ensure_local_origin(&app)?;
    Ok(app
        .path()
        .app_data_dir()
        .map(|dir| read_config_from(&dir))
        .unwrap_or_default())
}

/// 持久化地址（内部 helper，**不过闸**）——调用方必须已过
/// ensure_local_origin（save_url 入口过闸；connect 在导航**之前**过闸，
/// 导航后文档源已是远端，不能再依赖入口处的闸复检）。
fn persist_url(app: &AppHandle, url: &str) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    write_config_to(&dir, url)
}

/// 保存服务器地址（**只允许本地设置页**：否则远端页可静默改写 API Key
/// 的投递目的地并持久化，#143）。
#[tauri::command]
fn save_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    ensure_local_origin(&app)?;
    persist_url(&app, &url)
}

/// 导航主窗到服务器地址；成功后持久化。
///
/// 过闸在导航**之前**（此刻还在本地设置页；导航后再过闸就过不去了），
/// 持久化与 save_url 同源（#143：远端页不得经 connect 持久化任意地址）。
#[tauri::command]
fn connect(app: tauri::AppHandle, url: String) -> Result<(), String> {
    ensure_local_origin(&app)?;
    let normalized = normalize_url(&url)?;
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let parsed: Url = normalized
        .parse()
        .map_err(|e| format!("URL 解析失败: {e}"))?;
    win.navigate(parsed).map_err(|e| format!("导航失败: {e}"))?;
    persist_url(&app, &normalized)
}

// ==================== 一等取件端：命令 ====================

/// 保存 API Key 到系统钥匙串（**不明文落盘**）。
#[tauri::command]
fn save_api_key(app: AppHandle, key: String) -> Result<(), String> {
    ensure_local_origin(&app)?;
    secrets::save_api_key(&key)?;
    // 立刻用新钥匙查一轮 ⇒ 用户当场看到「配上没配上」。
    spawn_poll(&app);
    Ok(())
}

/// 清除系统钥匙串里的 API Key。
#[tauri::command]
fn clear_api_key(app: AppHandle) -> Result<(), String> {
    ensure_local_origin(&app)?;
    secrets::delete_api_key()?;
    with_state(&app, |s| {
        s.configured = false;
        s.pending.clear();
        s.last_error = None;
    });
    rebuild_tray_menu(&app);
    Ok(())
}

/// 设置柜子范围口径（"delivered" / "all"）。
#[tauri::command]
fn set_pickup_scope(app: AppHandle, scope: String) -> Result<(), String> {
    ensure_local_origin(&app)?;
    // 与 poll_once 同一把锁：否则并发轮询的整文件写回会把这里的 scope 改动
    // 覆盖掉（#146：丢 scope）。
    let _g = lock_poll();
    let parsed = Scope::parse(&scope).ok_or("口径只能是 delivered 或 all")?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    let mut cfg = read_pickup_config(&dir);
    cfg.scope = Some(parsed.as_str().to_string());
    write_pickup_config(&dir, &cfg)?;
    with_state(&app, |s| s.scope = parsed);
    spawn_poll(&app);
    Ok(())
}

/// 取件状态（设置页用；**不含**任何明文密钥）。
///
/// 读侧同样过闸（#144）：pending 里是每个待取件的 6 位取件码（= 取件凭据），
/// 远端页不得读走。
#[tauri::command]
fn get_pickup_status(app: AppHandle) -> Result<PickupStatus, String> {
    ensure_local_origin(&app)?;
    Ok(build_status(&app))
}

/// 立即查一轮并回状态。
#[tauri::command]
async fn check_pickup_now(app: AppHandle) -> Result<PickupStatus, String> {
    ensure_local_origin(&app)?;
    let worker = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        poll_once(&worker);
    })
    .await
    .map_err(|e| format!("检查任务失败: {e}"))?;
    Ok(build_status(&app))
}

/// 一键取件：把某件的**取件码**写进剪贴板。
///
/// 三道闸：
///   1. 只允许本地设置页调用（远端服务器页面无权触发）；
///   2. 取件码必须**当前就在待取清单里**（不接受前端传来的任意字符串）；
///   3. `pickup::clipboard_payload` 再验一次 6 位字母数字 —— 不合法就拒写。
///
/// ⇒ 剪贴板里只可能出现「某个待取件的 6 位取件码」，绝不会是链接或口令。
#[tauri::command]
fn copy_pickup_code(app: AppHandle, code: String) -> Result<(), String> {
    ensure_local_origin(&app)?;
    let known = with_state(&app, |s| {
        s.pending
            .iter()
            .find(|item| item.pickup_code == code)
            .map(|item| item.encrypted)
    });
    let Some(encrypted) = known else {
        return Err("该取件码不在当前待取清单里".into());
    };
    copy_code_to_clipboard(&app, &code, encrypted)
}

/// 写剪贴板：只写 6 位取件码。
///
/// `encrypted` 只影响提示文案：密件复制到的只是两因子之一
/// ⇒ toast 必须指路浏览器端（`pickup::copy_toast_body`）。
fn copy_code_to_clipboard(app: &AppHandle, code: &str, encrypted: bool) -> Result<(), String> {
    let payload = pickup::clipboard_payload(code)?;
    app.clipboard()
        .write_text(payload)
        .map_err(|e| format!("写入剪贴板失败: {e}"))?;
    with_state(app, |s| s.last_copied = Some(code.to_string()));
    notify_toast(app, "取件码已复制", pickup::copy_toast_body(encrypted));
    Ok(())
}

/// 组装给设置页的状态快照。
fn build_status(app: &AppHandle) -> PickupStatus {
    with_state(app, |s| PickupStatus {
        configured: s.configured,
        server: s.server.clone(),
        scope: s.scope.as_str().to_string(),
        pending_count: s.pending.len(),
        pending: s.pending.clone(),
        last_checked: s.last_checked,
        last_error: s.last_error.clone(),
        last_copied: s.last_copied.clone(),
    })
}

// ==================== 一等取件端：轮询与托盘 ====================

/// 弹一条系统通知（失败不致命 —— 托盘菜单仍然显示待取清单）。
fn notify_toast(app: &AppHandle, title: &str, body: &str) {
    let handle = app.clone();
    let title = title.to_string();
    let body = body.to_string();
    let _ = app.run_on_main_thread(move || {
        let _ = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    });
}

/// 重建托盘菜单（待取清单会变 ⇒ 菜单跟着变）。
///
/// 菜单的**创建与安装都在主线程**做（AppKit 的 NSMenu 是主线程对象）⇒
/// 这里只做状态快照，然后派发到主线程。
fn rebuild_tray_menu(app: &AppHandle) {
    let (pending, configured) = with_state(app, |s| (s.pending.clone(), s.configured));
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = build_tray_menu(&handle, &pending, configured);
    });
}

/// 在**主线程**上构建并安装托盘菜单。
fn build_tray_menu(
    app: &AppHandle,
    pending: &[PendingItem],
    configured: bool,
) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return Ok(());
    };
    let count = pending.len();

    // 一键取件：复制**最新**一件的取件码。
    let copy = MenuItem::with_id(
        app,
        "pickup-copy",
        if count > 0 {
            format!("一键取件（复制取件码，共 {count} 件）")
        } else {
            "一键取件（暂无待取件）".to_string()
        },
        count > 0,
        None::<&str>,
    )?;

    // 待取清单（子菜单）：每件一行，点了就把那一件的码复制走。
    // 没有待取件时**不建**空子菜单（muda 对空子菜单不友好）。
    let sub = if count > 0 {
        let sub_items: Vec<MenuItem<tauri::Wry>> = pending
            .iter()
            .enumerate()
            .take(TRAY_LIST_MAX)
            .map(|(idx, item)| {
                MenuItem::with_id(
                    app,
                    format!("pickup-item:{idx}"),
                    item.tray_label(),
                    true,
                    None::<&str>,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let sub_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = sub_items
            .iter()
            .map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>)
            .collect();
        Some(Submenu::with_items(
            app,
            format!("待取清单（{count}）"),
            true,
            &sub_refs,
        )?)
    } else {
        None
    };

    let show = MenuItem::with_id(app, "show", "显示主窗", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "服务器设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![&copy];
    if let Some(sub) = sub.as_ref() {
        items.push(sub);
    }
    items.push(&show);
    items.push(&settings);
    items.push(&quit);

    let menu = Menu::with_items(app, &items)?;
    tray.set_menu(Some(menu))?;

    // 托盘标题（macOS 菜单栏文字；Windows 不支持 ⇒ 无副作用）。
    let tooltip = if count > 0 {
        format!("PigeonBox · 有件待取（{count}）")
    } else if !configured {
        "PigeonBox · 未配置 API Key".to_string()
    } else {
        "PigeonBox".to_string()
    };
    let _ = tray.set_tooltip(Some(tooltip.clone()));
    let _ = tray.set_title(if count > 0 {
        Some(format!("待取{count}"))
    } else {
        None
    });
    Ok(())
}

/// 处理托盘菜单事件。
fn on_tray_menu(app: &AppHandle, id: &str) {
    match id {
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
                // 的绝对 URL（相对路径会落到远端服务器自己身上）；
                // manual=1 ⇒ 前端不自动重连，设置页站得住。
                if let Some(base) = app_base(app) {
                    if let Ok(url) = settings_target_url(&base, true).parse::<Url>() {
                        let _ = win.navigate(url);
                    }
                }
            }
        }
        "pickup-copy" => {
            // 复制最新一件的取件码。
            let picked = with_state(app, |s| {
                s.pending
                    .first()
                    .map(|item| (item.pickup_code.clone(), item.encrypted))
            });
            if let Some((code, encrypted)) = picked {
                if let Err(e) = copy_code_to_clipboard(app, &code, encrypted) {
                    notify_toast(app, "取件码复制失败", &e);
                }
            }
        }
        id if id.starts_with("pickup-item:") => {
            let idx: usize = id
                .trim_start_matches("pickup-item:")
                .parse()
                .unwrap_or(usize::MAX);
            let picked = with_state(app, |s| {
                s.pending
                    .get(idx)
                    .map(|item| (item.pickup_code.clone(), item.encrypted))
            });
            if let Some((code, encrypted)) = picked {
                if let Err(e) = copy_code_to_clipboard(app, &code, encrypted) {
                    notify_toast(app, "取件码复制失败", &e);
                }
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// 起一个后台线程查一轮。
fn spawn_poll(app: &AppHandle) {
    let handle = app.clone();
    let _ = std::thread::Builder::new()
        .name("pb-pickup-poll".into())
        .spawn(move || poll_once(&handle));
}

/// 查一轮：拉「我的分享」+「我的通知」⇒ 算待取 ⇒ 更新状态/托盘/通知。
///
/// 整体持 POLL_LOCK（#146）：三条路径并发时台账读改写必须串行，
/// 否则重复通知/丢台账/半截 JSON。
fn poll_once(app: &AppHandle) {
    let _g = lock_poll();
    let Some(dir) = app.path().app_data_dir().ok() else {
        return;
    };
    let server = read_config_from(&dir);
    let server = if server.trim().is_empty() {
        None
    } else {
        Some(server)
    };
    let api_key = secrets::load_api_key();

    // 配置态先落状态（设置页立刻能看到「配没配」）。
    with_state(app, |s| {
        s.server = server.clone();
        s.configured = api_key.is_some();
    });

    let (Some(server), Some(api_key)) = (server, api_key) else {
        // 没连服务器 / 没配钥匙 ⇒ 没有「柜子」可看，清空待取。
        with_state(app, |s| {
            s.pending.clear();
            s.last_error = None;
        });
        rebuild_tray_menu(app);
        return;
    };

    let client = match BoxClient::new(&server, &api_key) {
        Ok(c) => c,
        Err(e) => {
            with_state(app, |s| {
                s.last_error = Some(e);
                s.last_checked = Some(now_epoch_secs());
            });
            rebuild_tray_menu(app);
            return;
        }
    };

    let shares = match client.list_shares() {
        Ok(s) => s,
        Err(e) => {
            with_state(app, |s| {
                s.last_error = Some(e);
                s.last_checked = Some(now_epoch_secs());
            });
            rebuild_tray_menu(app);
            return;
        }
    };

    // 通知面失败**不致命**：沿用上一轮的「投递到件」集合（投递只会增加，不会减少）。
    let notices = client.list_notices().ok();
    let delivered = match notices {
        Some(ref list) => {
            let codes = delivery_pickup_codes(list);
            with_state(app, |s| s.delivered = codes.clone());
            codes
        }
        None => with_state(app, |s| s.delivered.clone()),
    };

    let scope = with_state(app, |s| s.scope);
    let pending = select_pending(&shares, &delivered, scope);

    // 新件判定 + 台账登记（互斥与原子写见 ledger_step_locked / #146）。
    let pending_codes: Vec<String> = pending.iter().map(|i| i.pickup_code.clone()).collect();
    let fresh = ledger_step_locked(&dir, &pending_codes);

    with_state(app, |s| {
        s.pending = pending.clone();
        s.last_checked = Some(now_epoch_secs());
        s.last_error = None;
    });

    if !fresh.is_empty() {
        let body = if fresh.len() == 1 {
            "你的柜子有件待取".to_string()
        } else {
            format!("你的柜子有 {} 件待取", fresh.len())
        };
        notify_toast(app, "PigeonBox", &body);
    }

    rebuild_tray_menu(app);
}

/// 起轮询线程：每 `POLL_INTERVAL_SECS` 秒查一轮。
fn spawn_poller(app: &AppHandle) {
    let handle = app.clone();
    let _ = std::thread::Builder::new()
        .name("pb-pickup-loop".into())
        .spawn(move || loop {
            poll_once(&handle);
            std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            connect,
            get_saved,
            save_url,
            open_settings,
            save_api_key,
            clear_api_key,
            set_pickup_scope,
            get_pickup_status,
            check_pickup_now,
            copy_pickup_code
        ])
        .setup(|app| {
            // 捕获本地 app 源（设置页导航锚点）：此刻主窗文档源还是本地 app，
            // 之后 connect 导航到远端，相对 URL 就再也回不来了。
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(u) = win.url() {
                    app.manage(AppBase(u));
                }
            }
            // 取件态：口径从落盘配置读，其余运行时填。
            let dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cfg = read_pickup_config(&dir);
            let scope = cfg
                .scope
                .as_deref()
                .and_then(Scope::parse)
                .unwrap_or_default();
            app.manage(Mutex::new(PickupState {
                scope,
                server: None,
                configured: false,
                ..Default::default()
            }));

            // 托盘：左键/菜单「显示主窗」，菜单含「一键取件」「服务器设置」「退出」。
            let show = MenuItem::with_id(app, "show", "显示主窗", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "服务器设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let copy = MenuItem::with_id(
                app,
                "pickup-copy",
                "一键取件（暂无待取件）",
                false,
                None::<&str>,
            )?;
            let menu = Menu::with_items(app, &[&copy, &show, &settings, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("PigeonBox")
                .menu(&menu)
                .on_menu_event(|app, event| on_tray_menu(app, event.id.as_ref()))
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
            // 轮询：托盘常驻 ⇒ 后台一直盯着柜子。
            spawn_poller(app.handle());
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

    #[test]
    fn settings_target_is_absolute_local_app_url() {
        let base = Url::parse("tauri://localhost/index.html").unwrap();
        let target = settings_target_url(&base, false);
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
        let target = Url::parse(&settings_target_url(&base, false)).unwrap();
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

    #[test]
    fn manual_settings_url_marks_manual_and_stays_local() {
        // 托盘进设置页 ⇒ 带 manual=1（前端据此不自动重连 ⇒ 设置页站得住）。
        let base = Url::parse("tauri://localhost/index.html").unwrap();
        let target = settings_target_url(&base, true);
        let parsed = Url::parse(&target).expect("必须是绝对 URL");
        assert_eq!(parsed.path(), "/index.html");
        assert_eq!(parsed.query(), Some("manual=1"));
        assert!(same_origin(&parsed, &base));
    }

    #[test]
    fn manual_settings_url_keeps_prefix_path() {
        // base 带前缀路径时（Windows 的 http://tauri.localhost/ 形态）不能丢前缀。
        let base = Url::parse("http://tauri.localhost/index.html").unwrap();
        let parsed = Url::parse(&settings_target_url(&base, true)).unwrap();
        assert_eq!(parsed.path(), "/index.html");
        assert_eq!(parsed.query(), Some("manual=1"));
    }

    #[test]
    fn plain_settings_url_has_no_query() {
        // 非手动路径（启动自动重连用）不带 manual ⇒ 前端照旧自动重连。
        let base = Url::parse("tauri://localhost/index.html").unwrap();
        let parsed = Url::parse(&settings_target_url(&base, false)).unwrap();
        assert_eq!(parsed.query(), None);
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

    // ---- 取件配置（口径 + 已通知台账）----

    #[test]
    fn pickup_config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pb-desktop-pickup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = PickupConfig {
            scope: Some("all".into()),
            notified: vec!["A1B2C3".into()],
        };
        write_pickup_config(&dir, &cfg).unwrap();
        let got = read_pickup_config(&dir);
        assert_eq!(got.scope.as_deref(), Some("all"));
        assert_eq!(got.notified, vec!["A1B2C3".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pickup_config_missing_file_is_default() {
        let dir = std::env::temp_dir().join(format!("pb-desktop-pickup-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let got = read_pickup_config(&dir);
        assert!(got.scope.is_none());
        assert!(got.notified.is_empty());
    }

    // ---- 本地 origin 闸（远端页面不得碰钥匙串/剪贴板）----

    #[test]
    fn local_origin_gate_rejects_remote_page() {
        let local = Url::parse("tauri://localhost/index.html").unwrap();
        let remote = Url::parse("http://evil.example.com/").unwrap();
        assert!(same_origin(&local, &local));
        assert!(!same_origin(&local, &remote));
        // 同 host 不同端口 ⇒ 不同源
        let other_port = Url::parse("http://box.local:9999/").unwrap();
        let same_host = Url::parse("http://box.local/").unwrap();
        assert!(!same_origin(&other_port, &same_host));
    }

    #[test]
    fn local_origin_gate_survives_opaque_scheme() {
        // tauri: 这类非特殊 scheme 的 origin 不透明 ⇒ 必须比 scheme+host+port。
        let a = Url::parse("tauri://localhost/index.html").unwrap();
        let b = Url::parse("tauri://localhost/other.html").unwrap();
        assert!(same_origin(&a, &b));
    }

    // ---- 命令闸的机械守卫（#143/#144：防下次再漏闸）----

    #[test]
    fn all_commands_pass_local_origin_gate_except_whitelist() {
        let src = include_str!("lib.rs");
        // 标识符运行时拼（否则守卫会匹配到本文件里的字面量本身，同
        // settings_navigation_uses_absolute_helper_not_relative_eval 的做法）。
        let marker = format!("#[tauri::{}", "command]");
        // 白名单：open_settings —— 只把窗口导航回本地设置页，远端页调用它
        // 反而把用户带回设置页，无害（#144 正文结论）。
        const WHITELIST: &[&str] = &["open_settings"];
        let mut gated: Vec<String> = Vec::new();
        for chunk in src.split(&marker).skip(1) {
            let rest = chunk.trim_start();
            let after_fn = rest.split("fn ").nth(1).unwrap_or("");
            let name: String = after_fn
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            assert!(!name.is_empty(), "找不到命令函数名: {:.80}", rest);
            if WHITELIST.contains(&name.as_str()) {
                continue;
            }
            assert!(
                chunk.contains("ensure_local_origin"),
                "命令 {name} 未过 ensure_local_origin 闸（#143/#144）"
            );
            gated.push(name);
        }
        // 关键命令逐一点名：解析静默漏掉任何一个都要红。
        for n in [
            "save_url",
            "connect",
            "get_saved",
            "get_pickup_status",
            "save_api_key",
            "clear_api_key",
            "set_pickup_scope",
            "check_pickup_now",
            "copy_pickup_code",
        ] {
            assert!(gated.iter().any(|g| g == n), "命令 {n} 没被对账到");
        }
        assert_eq!(gated.len(), 9, "过闸命令对账数不对: {gated:?}");
    }

    // ---- 轮询并发互斥与原子写（#146）----

    #[test]
    fn poll_once_and_scope_serialized_and_config_written_atomically() {
        let src = include_str!("lib.rs");
        // poll_once 整体互斥（三条路径并发跑同一读改写）。
        let start = src.find("fn poll_once(").expect("poll_once 存在");
        let body = &src[start..];
        let end = body.find("\n}").expect("poll_once 函数结束");
        assert!(
            body[..end].contains("lock_poll()"),
            "poll_once 必须整体持 POLL_LOCK（#146）"
        );
        // set_pickup_scope 的台账读改写同样互斥（否则丢 scope）。
        let start = src.find("fn set_pickup_scope(").expect("set_pickup_scope 存在");
        let body = &src[start..];
        let end = body.find("\n}").expect("set_pickup_scope 函数结束");
        assert!(
            body[..end].contains("lock_poll()"),
            "set_pickup_scope 必须持 POLL_LOCK（#146：丢 scope）"
        );
        // 写 pickup.json 必须「临时文件 + rename」（防半截 JSON ⇒ 台账清空）。
        let start = src.find("fn write_pickup_config(").expect("write_pickup_config 存在");
        let body = &src[start..];
        let end = body.find("\n}").expect("write_pickup_config 函数结束");
        let f = &body[..end];
        assert!(
            f.contains(".tmp") && f.contains("rename"),
            "写 pickup.json 必须临时文件 + rename（#146）"
        );
    }

    #[test]
    fn concurrent_ledger_steps_claim_each_code_exactly_once() {
        // 同一批到件被多条并发轮询路径看到 ⇒ 每个码只该被认领一次（不重复通知）。
        let dir = std::env::temp_dir().join(format!("pb-ledger-once-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let codes: Vec<String> = (0..10).map(|i| format!("C{i:05}")).collect();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let d = dir.clone();
            let c = codes.clone();
            handles.push(std::thread::spawn(move || {
                let mut claimed = 0usize;
                for _ in 0..10 {
                    claimed += poll_ledger_step(&d, &c).len();
                }
                claimed
            }));
        }
        let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(
            total,
            codes.len(),
            "同一批到件被并发重复认领 ⇒ 会重复通知（#146），实收 {total}"
        );

        let cfg = read_pickup_config(&dir);
        assert_eq!(cfg.notified.len(), codes.len(), "台账丢码（#146）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_ledger_steps_do_not_lose_distinct_codes() {
        // 4 路并发各登记 50 个不同码 ⇒ 后写不得覆盖先写，台账应有 200 个。
        let dir = std::env::temp_dir().join(format!("pb-ledger-dist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut handles = Vec::new();
        for t in 0..4u32 {
            let d = dir.clone();
            handles.push(std::thread::spawn(move || {
                let mine: Vec<String> = (0..50).map(|i| format!("{t}S{i:04}")).collect();
                for _ in 0..10 {
                    let _ = poll_ledger_step(&d, &mine);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let cfg = read_pickup_config(&dir);
        assert_eq!(
            cfg.notified.len(),
            200,
            "并发登记互相覆盖 ⇒ 台账丢码（#146），实收 {}",
            cfg.notified.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_reads_never_observe_torn_pickup_json() {
        // 写方反复整文件覆盖，读方任何一次读到半截/空 JSON 都算红
        // （读失败 = unwrap_or_default ⇒ 台账清空全量重通知，#146）。
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("pb-ledger-torn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let big = |n: usize| PickupConfig {
            scope: Some(if n % 2 == 0 { "all" } else { "delivered" }.into()),
            notified: (0..400).map(|i| format!("N{i:03}{}", "x".repeat(200))).collect(),
        };
        write_pickup_config(&dir, &big(0)).unwrap();

        let done = Arc::new(AtomicBool::new(false));
        let writer = {
            let d = dir.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                for n in 1..300 {
                    let _ = write_pickup_config(&d, &big(n));
                }
                done.store(true, Ordering::SeqCst);
            })
        };

        let mut reads = 0usize;
        while !done.load(Ordering::SeqCst) {
            let raw =
                std::fs::read_to_string(pickup_config_path(&dir)).expect("台账文件应一直存在");
            serde_json::from_str::<PickupConfig>(&raw).unwrap_or_else(|e| {
                panic!("读到半截 JSON ⇒ 台账清空全量重通知（#146），第 {reads} 次: {e}")
            });
            reads += 1;
        }
        writer.join().unwrap();
        assert!(reads > 0, "读方至少要读到一次");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
