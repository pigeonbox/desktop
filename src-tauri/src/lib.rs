// PigeonBox 桌面客户端（Tauri 2）。
//
// 设计（v1 远程模式）：
//   - 主窗加载内置设置页，用户填服务器地址后经 `connect` 命令导航到该地址，
//     此后主窗即服务器 Web UI（托盘「显示主窗」随时唤回设置页）。
//   - 地址持久化在应用数据目录（config.json），下次启动自动重连。
//   - 信任模型：远程页面运行在主 webview 中但只暴露 connect/get_saved/save_url
//     等无危险命令（withGlobalTauri 注入的 API 面即以上）；关闭窗口=退出。
//
// 设备直传（v1.3+）：内嵌 p2pc sidecar（tauri externalBin，构建期按 target
// triple 注入 binaries/p2pc-<triple>），Rust 侧 spawn 并把 stdout/stderr 以
// `p2p-log` 事件流式回传设置页；同一时刻至多一个传输进程（p2p_cancel 终止）。
// sidecar 从 Rust 调用，不经 JS capability 权限面；远端页面拿不到这些命令
// （导航走 http(s) origin，__TAURI__ 注入仅存在于内置设置页）。
use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, Url,
};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

#[tauri::command]
fn get_saved(app: tauri::AppHandle) -> String {
    read_saved_url(&app)
}

#[tauri::command]
fn save_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    std::fs::write(dir.join("config.json"), url)
        .map_err(|e| format!("write config: {e}"))?;
    Ok(())
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

/// 回设置页（托盘「服务器设置」用）。
#[tauri::command]
fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    win.eval("location.replace('index.html')").map_err(|e| e.to_string())
}

fn normalize_url(raw: &str) -> Result<String, String> {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err("地址不能为空".into());
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        Ok(t.to_string())
    } else {
        Ok(format!("http://{t}"))
    }
}

fn read_saved_url(app: &tauri::AppHandle) -> String {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join("config.json")).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

// ── p2pc 设备直传（sidecar） ─────────────────────────────────────────

/// 单会话状态：同时至多一个 p2pc 进程（send/recv 共用）。
struct P2pSession(Mutex<Option<CommandChild>>);

#[tauri::command]
fn get_p2p_registry(app: tauri::AppHandle) -> String {
    read_app_data_file(&app, "p2p_registry.txt")
}

#[tauri::command]
fn save_p2p_registry(app: tauri::AppHandle, registry: String) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    std::fs::write(dir.join("p2p_registry.txt"), registry.trim())
        .map_err(|e| format!("write registry: {e}"))?;
    Ok(())
}

/// 探测 sidecar 可用性（设置页据此显隐直传面板）。
#[tauri::command]
async fn p2p_version(app: tauri::AppHandle) -> Result<String, String> {
    let out = app
        .shell()
        .sidecar("p2pc")
        .map_err(|e| format!("sidecar 解析失败: {e}"))?
        .args(["-v"])
        .output()
        .await
        .map_err(|e| format!("sidecar 执行失败: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(text.trim().to_string())
}

#[tauri::command]
async fn p2p_send(
    app: tauri::AppHandle,
    state: tauri::State<'_, P2pSession>,
    file_path: String,
    registry: String,
    code: String,
    mode: String,
    relay: String,
    pair: bool,
) -> Result<(), String> {
    let path = file_path.trim().to_string();
    if path.is_empty() {
        return Err("请选择要发送的文件".into());
    }
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    let registry = normalize_registry(&registry)?;
    save_p2p_registry(app.clone(), registry.clone())?;
    let mut args = vec!["send".into(), path, "--registry".into(), registry];
    // 配对码模式:4 位码由子进程铸出经 --json 事件回传,裁决走 stdin 单行协议
    // (--pair-stdin);口令模式:UI 生成口令并已向用户展示,须经 --code 原样下发
    // (CLI 自动口令只在传完才打印)
    if pair {
        args.push("--pair".into());
        args.push("--json".into());
        args.push("--pair-stdin".into());
    } else if !code.trim().is_empty() {
        args.push("--code".into());
        args.push(code.trim().to_uppercase());
    }
    args.extend(mode_args(&mode, &relay));
    spawn_p2p(&app, state.inner(), args)
}

/// 传输模式映射:relay=仅中继(--no-punch)/direct=仅直连(--no-relay)/auto=先通后优;
/// relay 非空时显式指定中继地址(覆盖自动发现)。
fn mode_args(mode: &str, relay: &str) -> Vec<String> {
    let mut v = Vec::new();
    match mode {
        "relay" => v.push("--no-punch".to_string()),
        "direct" => v.push("--no-relay".to_string()),
        _ => {}
    }
    if !relay.trim().is_empty() {
        v.push("--relay".to_string());
        v.push(relay.trim().to_string());
    }
    v
}

#[tauri::command]
async fn p2p_recv(
    app: tauri::AppHandle,
    state: tauri::State<'_, P2pSession>,
    code: String,
    registry: String,
    mode: String,
    relay: String,
    reverse: bool,
) -> Result<(), String> {
    let code = code.trim().to_string();
    // reverse=反向亮码收件(2026-10-19):本端铸 4 位码等待对方敲码推送,
    // 接受门经 --pair-stdin 由网页/桌面裁决。
    if !reverse && code.is_empty() {
        return Err("请输入口令".into());
    }
    let registry = normalize_registry(&registry)?;
    save_p2p_registry(app.clone(), registry.clone())?;
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| format!("下载目录解析失败: {e}"))?;
    let _ = std::fs::create_dir_all(&dir);
    let mut args = if reverse {
        vec![
            "recv".into(),
            "--pair".into(),
            "--json".into(),
            "--pair-stdin".into(),
            "--registry".into(),
            registry,
            "--out".into(),
            dir.to_string_lossy().to_string(),
        ]
    } else {
        vec![
            "recv".into(),
            code.clone(),
            "--registry".into(),
            registry,
            "--out".into(),
            dir.to_string_lossy().to_string(),
        ]
    };
    // 4-8 位纯数字=配对码:--json 让 pending 态(pair_state)以结构化事件回传;
    // 形状识别与 claim/口令回退在 p2pc 侧,这里只透传。
    if !reverse && is_pairing_code(&code) {
        args.push("--json".into());
    }
    args.extend(mode_args(&mode, &relay));
    spawn_p2p(&app, state.inner(), args)
}

/// 与 p2pc IsPairingCode 同判据:4-8 位纯数字。
fn is_pairing_code(s: &str) -> bool {
    let n = s.chars().count();
    (4..=8).contains(&n) && s.chars().all(|c| c.is_ascii_digit())
}

/// 配对接受门裁决转发:向配对子进程 stdin 写单行 y/n(与 --pair-stdin 协议同)。
#[tauri::command]
fn p2p_pair_approve(
    state: tauri::State<'_, P2pSession>,
    accept: bool,
) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "会话锁异常".to_string())?;
    let child = guard
        .as_mut()
        .ok_or("当前无进行中的直传会话")?;
    let ans = if accept { "y\n" } else { "n\n" };
    child.write(ans.as_bytes()).map_err(|e| format!("裁决写入失败: {e}"))
}

/// 终止当前直传进程。返回是否确有进程被终止。
#[tauri::command]
fn p2p_cancel(state: tauri::State<'_, P2pSession>) -> Result<bool, String> {
    let mut guard = state.0.lock().map_err(|_| "会话锁异常".to_string())?;
    Ok(guard.take().map(|child| child.kill().is_ok()).unwrap_or(false))
}

/// spawn p2pc 并把输出以 `p2p-log` 事件流式回传（kind: stdout|stderr|error|exit）。
/// 进程终止后自动清空会话槽，允许下一次传输。
fn spawn_p2p(
    app: &tauri::AppHandle,
    session: &P2pSession,
    args: Vec<String>,
) -> Result<(), String> {
    let mut guard = session.0.lock().map_err(|_| "会话锁异常".to_string())?;
    if guard.is_some() {
        return Err("已有直传任务进行中，请等待完成或先取消".into());
    }
    let (mut rx, child) = app
        .shell()
        .sidecar("p2pc")
        .map_err(|e| format!("sidecar 解析失败: {e}"))?
        .args(args)
        .spawn()
        .map_err(|e| format!("sidecar 启动失败: {e}"))?;
    *guard = Some(child);
    drop(guard);

    let emitter = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let payload = match event {
                CommandEvent::Stdout(line) => {
                    serde_json::json!({"kind": "stdout", "line": String::from_utf8_lossy(&line)})
                }
                CommandEvent::Stderr(line) => {
                    serde_json::json!({"kind": "stderr", "line": String::from_utf8_lossy(&line)})
                }
                CommandEvent::Error(err) => serde_json::json!({"kind": "error", "line": err}),
                CommandEvent::Terminated(status) => {
                    if let Some(s) = emitter.try_state::<P2pSession>() {
                        if let Ok(mut guard) = s.0.lock() {
                            *guard = None;
                        }
                    }
                    serde_json::json!({"kind": "exit", "code": status.code})
                }
                _ => continue,
            };
            let _ = emitter.emit("p2p-log", payload);
        }
    });
    Ok(())
}

fn normalize_registry(raw: &str) -> Result<String, String> {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err("注册中心地址不能为空（即 p2pd 服务地址）".into());
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        Ok(t.to_string())
    } else {
        Ok(format!("http://{t}"))
    }
}

fn read_app_data_file(app: &tauri::AppHandle, name: &str) -> String {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(name)).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(P2pSession(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            connect,
            get_saved,
            save_url,
            open_settings,
            get_p2p_registry,
            save_p2p_registry,
            p2p_version,
            p2p_send,
            p2p_recv,
            p2p_pair_approve,
            p2p_cancel
        ])
        .setup(|app| {
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
                            let _ = win.eval("location.replace('index.html')");
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
