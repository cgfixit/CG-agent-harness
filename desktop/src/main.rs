mod backend;
mod instance;

use backend::Backend;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

#[derive(Default)]
struct Owner {
    backend: Option<Backend>,
    message: String,
    details: String,
}

#[derive(Default)]
struct Desktop {
    instance_owned: bool,
    owner: Mutex<Owner>,
    quitting: AtomicBool,
    quit_dialog: AtomicBool,
    preparing: AtomicBool,
    cancel_preparation: AtomicBool,
    external_dialog: AtomicBool,
}

#[derive(Serialize)]
struct Status {
    message: String,
    details: String,
}

fn show(app: &tauri::AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn external_link(app: &tauri::AppHandle, url: tauri::Url) {
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return;
    }
    if app.state::<Desktop>().external_dialog.swap(true, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    app.dialog()
        .message(format!(
            "Open this URL in your external browser? Review its full address before continuing.\n\n{}",
            url.as_str()
        ))
        .title("Leave CG Agent Harness?")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Open browser".into(),
            "Stay here".into(),
        ))
        .show(move |approved| {
            handle.state::<Desktop>().external_dialog.store(false, Ordering::SeqCst);
            if approved {
                // Fixed OS opener and scheme-checked data argv. The webview has
                // no shell command; only this native confirmation authorizes it.
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("--")
                    .arg(url.as_str())
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
}

#[tauri::command]
fn desktop_status(state: tauri::State<'_, Desktop>) -> Status {
    match state.owner.try_lock() {
        Ok(owner) => Status {
            message: owner.message.clone(),
            details: owner.details.clone(),
        },
        Err(_) => Status {
            message: "Waiting for the owned backend…".into(),
            details: "If this persists, check for a macOS file-access prompt. Move the app to Applications and retry; do not change global security settings.".into(),
        },
    }
}

fn launch(app: &tauri::AppHandle, initialize_key: Option<String>) -> Result<(), String> {
    let state = app.state::<Desktop>();
    if !state.instance_owned {
        return Err("Desktop ownership unavailable; relaunch after resolving the conflict.".into());
    }
    let mut owner = state.owner.lock().map_err(|_| "Desktop owner unavailable.")?;
    if let Some(backend) = owner.backend.as_mut() {
        if initialize_key.is_none() {
            drop(owner);
            show(app, "harness");
            return Ok(());
        }
        let active = backend.status()?;
        if active["active"].as_u64().unwrap_or(0) > 0 || active["chat_active"] == true || active["run_active"] == true {
            return Err("Wait for active work before changing startup credentials.".into());
        }
        owner.backend.take();
        if let Some(window) = app.get_webview_window("harness") {
            let _ = window.destroy();
        }
    }
    owner.message = "Starting owned backend…".into();
    let home = backend::home()?;
    match Backend::start(&home, initialize_key) {
        Err(message) => {
            eprintln!("desktop startup: {message}");
            owner.message = message.clone();
            owner.details = format!("Application home: {}", home.display());
            Err(message)
        }
        Ok(backend) => {
            let origin = backend.origin.clone();
            let allowed = origin.clone();
            let navigation_app = app.clone();
            let new_window_app = app.clone();
            let window = WebviewWindowBuilder::new(
                app,
                "harness",
                WebviewUrl::External(origin.parse().map_err(|_| "Invalid origin.")?),
            )
            .title("CG Agent Harness")
            .incognito(true)
            .inner_size(1150.0, 820.0)
            .min_inner_size(700.0, 480.0)
            .on_navigation(move |url| {
                if backend::allowed_navigation(url, &allowed) {
                    true
                } else {
                    external_link(&navigation_app, url.clone());
                    false
                }
            })
            .on_new_window(move |url, _| {
                external_link(&new_window_app, url);
                tauri::webview::NewWindowResponse::Deny
            })
            .on_download(|_, _| false)
            .devtools(false)
            .zoom_hotkeys_enabled(true)
            .build()
            .map_err(|_| "Cannot create native harness webview.")?;
            let key_state = if backend.hello["api_key_optional"] == true {
                "Ready to use. API key entry and account login are optional."
            } else if backend.hello["key_configured"] == true {
                "Enter your existing API key in the console."
            } else {
                "No API key configured. Use Setup to save a new key, then enter it in the console."
            };
            owner.message = format!("Backend ready. {key_state}");
            let tools: Vec<_> = ["git", "gh", "cargo", "rustc", "python3", "xcrun"]
                .into_iter()
                .map(|name| {
                    format!(
                        "{name}: {}",
                        backend::executable(name)
                            .map(|p| p.display().to_string())
                            .unwrap_or("missing".into())
                    )
                })
                .collect();
            owner.details = format!("Home: {}\nOwned endpoint: {}\nChat model: {}\nPlanner model: {}\n{}\nTool paths show discovery, not authenticated or prepared readiness.", home.display(), origin,
                backend.hello["chat_model"].as_str().unwrap_or("unknown"), backend.hello["planner_model"].as_str().unwrap_or("unknown"), tools.join("\n"));
            owner.backend = Some(backend);
            let _ = window.set_focus();
            if let Some(setup) = app.get_webview_window("setup") {
                let _ = setup.hide();
            }
            Ok(())
        }
    }
}

#[tauri::command]
async fn retry_backend(app: tauri::AppHandle, initialize_key: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || launch(&app, initialize_key))
        .await
        .map_err(|_| "Startup task failed.".to_string())?
}

#[tauri::command]
async fn check_models(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Desktop>();
        let mut owner = state.owner.lock().map_err(|_| "Desktop owner unavailable.")?;
        let result = owner
            .backend
            .as_mut()
            .ok_or("Start the backend before checking models.")?
            .models()?;
        owner.message = "Local model inventory checked. This does not run inference or download models.".into();
        owner.details = serde_json::to_string_pretty(&result).map_err(|_| "Cannot format readiness result.")?;
        Ok(())
    })
    .await
    .map_err(|_| "Readiness task failed.".to_string())?
}

#[tauri::command]
async fn prepare_cargo(app: tauri::AppHandle) -> Result<(), String> {
    if app.state::<Desktop>().preparing.swap(true, Ordering::SeqCst) {
        return Err("Preparation is already running.".into());
    }
    let worker_app = app.clone();
    app.state::<Desktop>().cancel_preparation.store(false, Ordering::SeqCst);
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let folder = worker_app.dialog().file().set_title("Choose repository for offline Cargo preparation").blocking_pick_folder();
        let Some(folder) = folder else { return Ok(()); };
        let repo = folder.into_path().map_err(|_| "Select a local repository folder.")?;
        if !repo.join("Cargo.lock").is_file() { return Err("Selected repository has no Cargo.lock.".to_string()); }
        let python = backend::executable("python3").ok_or("Python 3 is required for Cargo preparation.")?;
        let helper = std::env::current_exe().map_err(|_| "Cannot locate bundle.")?.parent().ok_or("Invalid bundle.")?.join("../Resources/prepare-cargo.py");
        let home = backend::home()?;
        use std::os::unix::process::CommandExt;
        let tool_path = backend::configured_path(&home)?;
        let mut child = std::process::Command::new(python).arg(helper).arg(repo).arg(home).arg("--desktop-parent")
            .env("PATH", tool_path).env("RUSTUP_AUTO_INSTALL", "0")
            .current_dir("/").stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .process_group(0).spawn().map_err(|_| "Cannot launch the bundled preparation helper.")?;
        // Output is discarded rather than retaining untrusted paths/credential
        // diagnostics. The helper itself never runs build scripts or tests.
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        loop {
            match child.try_wait() {
                Ok(Some(exit)) => return if exit.success() { Ok(()) } else { Err("Offline preparation failed. Check installed Rust/Cargo/SDK, cached dependencies, or an existing snapshot. No dependencies were downloaded.".into()) },
                Ok(None) if std::time::Instant::now() < deadline && !worker_app.state::<Desktop>().cancel_preparation.load(Ordering::SeqCst) => std::thread::sleep(Duration::from_millis(100)),
                _ => {
                    // SAFETY: unreaped child owns this process group. Never signal
                    // a matching process name or a PID read from a file.
                    unsafe { libc::killpg(child.id() as i32, libc::SIGKILL); }
                    let _ = child.wait(); return Err("Preparation stopped; inspect incomplete setup before retrying.".into());
                }
            }
        }
    }).await.map_err(|_| "Preparation task failed.".to_string()).and_then(|v| v);
    let state = app.state::<Desktop>();
    state.preparing.store(false, Ordering::SeqCst);
    if let Ok(mut owner) = state.owner.lock() {
        owner.message = outcome
            .as_ref()
            .map(|_| "Offline preparation finished or folder selection was cancelled.".to_string())
            .unwrap_or_else(|e| e.clone());
    }
    outcome
}

fn request_quit(app: &tauri::AppHandle) {
    let state = app.state::<Desktop>();
    if state.quit_dialog.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Desktop>();
        let mut owner = state.owner.lock().unwrap_or_else(|p| p.into_inner());
        let active = owner.backend.as_mut().map(|b| b.status());
        let busy = state.preparing.load(Ordering::SeqCst)
            || match active {
                Some(Ok(v)) => {
                    v["active"].as_u64().unwrap_or(0) > 0 || v["chat_active"] == true || v["run_active"] == true
                }
                Some(Err(_)) => true,
                None => false,
            };
        if busy && !app.dialog().message("Work is active or backend status is unavailable. Cancel owned work and quit? Review interrupted runs after restart. Descendants that escape native observation may require inspection.")
            .title("Quit CG Agent Harness?")
            .buttons(MessageDialogButtons::OkCancelCustom("Cancel work and quit".into(), "Keep running".into())).blocking_show() {
            state.quit_dialog.store(false, Ordering::SeqCst); return;
        }
        owner.message = "Cancelling owned work and quitting…".into();
        state.cancel_preparation.store(true, Ordering::SeqCst);
        owner.backend.take();
        state.quitting.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

fn main() {
    let instance = match backend::home().and_then(|home| instance::Instance::acquire(&home)) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => return,
        Err(message) => {
            // The normal setup window still starts to show actionable failure.
            eprintln!("{message}");
            None
        }
    };
    let instance_ok = instance.is_some();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Desktop { instance_owned: instance_ok, ..Desktop::default() })
        .invoke_handler(tauri::generate_handler![desktop_status, retry_backend, prepare_cargo, check_models])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); let _ = window.hide(); }
        })
        .setup(move |app| {
            let setup = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
                .title("CG Agent Harness — Setup and recovery").inner_size(870.0, 820.0)
                .on_navigation(|url| url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny).devtools(false).build()?;
            let menu = tauri::menu::Menu::default(app.handle())?;
            let recovery = tauri::menu::Submenu::with_items(app, "Harness", true, &[
                &tauri::menu::MenuItem::with_id(app, "setup", "Setup and recovery", true, Some("CmdOrCtrl+,"))?
            ])?;
            menu.append(&recovery)?; app.set_menu(menu)?;
            app.on_menu_event(|app, event| { if event.id().as_ref() == "setup" { show(app, "setup"); } });
            let _ = setup.set_focus();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if instance_ok {
                    if let Err(message) = launch(&handle, None) {
                        eprintln!("desktop startup: {message}");
                    }
                }
                else if let Ok(mut owner) = handle.state::<Desktop>().owner.lock() {
                    owner.message = "Desktop ownership could not be established. Quit the other instance or inspect home permissions, then relaunch.".into();
                }
            });
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                let state = handle.state::<Desktop>();
                if state.quitting.load(Ordering::SeqCst) { break; }
                if instance.as_ref().is_some_and(|i| i.requested_focus()) {
                    show(&handle, if handle.get_webview_window("harness").is_some() { "harness" } else { "setup" });
                }
                if let Ok(mut owner) = state.owner.try_lock() {
                    if let Some(backend) = owner.backend.as_mut() {
                        if let Err(message) = backend.status() {
                            owner.message = message; owner.backend.take();
                            if let Some(window) = handle.get_webview_window("harness") { let _ = window.destroy(); }
                            show(&handle, "setup");
                        }
                    }
                };
            });
            Ok(())
        });
    match builder.build(tauri::generate_context!()) {
        Ok(app) => app.run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if !app.state::<Desktop>().quitting.load(Ordering::SeqCst) {
                    api.prevent_exit();
                    request_quit(app);
                }
            }
            tauri::RunEvent::Reopen { .. } => show(
                app,
                if app.get_webview_window("harness").is_some() {
                    "harness"
                } else {
                    "setup"
                },
            ),
            _ => (),
        }),
        Err(_) => eprintln!("CG Agent Harness could not initialize its native window."),
    }
}
