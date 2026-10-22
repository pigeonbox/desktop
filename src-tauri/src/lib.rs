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
// 桌面端不再随包 p2pc sidecar、也不再编排设备直传——直传相关命令保留同名
// 占位，未安装 PigeonBeam 时给明确提示（设置页据此显隐直传面板），
// 不留悬空引用。
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, Url,
};

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

// ── 设备直传：已独立为 PigeonBeam（信鸽直传）─────────────────────────
//
// 2026-10-22（Wave B'）：设备直传从 PigeonBox 拆为独立应用 PigeonBeam。
// 桌面端不再随包 p2pc sidecar，也不再 spawn/编排直传子进程——下列命令保留
// 同名占位，未安装 PigeonBeam 时给明确提示（设置页据此显隐直传面板），
// 不留悬空引用；注册中心地址仍可读写（交给 PigeonBeam 客户端时复用同一份）。

const BEAM_HINT: &str = "设备直传已独立为 PigeonBeam（信鸽直传）应用，请安装 PigeonBeam 后使用";

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

/// 探测直传组件可用性：桌面端已无内置 p2pc，恒失败并指向 PigeonBeam。
#[tauri::command]
fn p2p_version() -> Result<String, String> {
    Err(BEAM_HINT.into())
}

/// 直传发送：已随直传面迁往 PigeonBeam，此处恒失败并给出指引。
#[tauri::command]
fn p2p_send() -> Result<(), String> {
    Err(BEAM_HINT.into())
}

/// 直传接收：同上，恒失败并给出指引。
#[tauri::command]
fn p2p_recv() -> Result<(), String> {
    Err(BEAM_HINT.into())
}

/// 配对裁决：配对面已随直传面迁往 PigeonBeam。
#[tauri::command]
fn p2p_pair_approve() -> Result<(), String> {
    Err(BEAM_HINT.into())
}

/// 终止直传：桌面端不再持有直传进程，无可终止。
#[tauri::command]
fn p2p_cancel() -> Result<bool, String> {
    Err(BEAM_HINT.into())
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
        .plugin(tauri_plugin_dialog::init())
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
