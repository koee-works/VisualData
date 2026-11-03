//! Tauri 2 shell (ADR-0003). Every command forwards to `vda_app::App`; this
//! file only adds threading, events and dialogs.
//!
//! Commands are `async` so they run off the main thread: a long analysis run
//! holds the app lock on a worker thread without freezing the window.
//! Progress is emitted as `run://step` events. Cancel uses a flag taken
//! before the run starts, so it never waits for the busy lock.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use vda_app::{AnalysisView, App, AppError, Page, ProfileRow, RunSummary};

struct Shared {
    app: Arc<Mutex<App>>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    startup: Option<String>,
}

type Res<T> = Result<T, AppError>;

fn with<T>(s: &State<'_, Shared>, f: impl FnOnce(&mut App) -> Res<T>) -> Res<T> {
    let mut app = s.app.lock().map_err(|_| AppError::from("the app state is unavailable after an earlier crash"))?;
    f(&mut app)
}

#[tauri::command]
async fn startup_project(s: State<'_, Shared>) -> Res<Option<String>> {
    Ok(s.startup.clone())
}

#[tauri::command]
async fn create_project(s: State<'_, Shared>, dir: String, csv: String, name: String) -> Res<AnalysisView> {
    with(&s, |a| a.create_project(&PathBuf::from(dir), &PathBuf::from(csv), &name))
}

#[tauri::command]
async fn open_project(s: State<'_, Shared>, dir: String) -> Res<AnalysisView> {
    with(&s, |a| a.open_project(&PathBuf::from(dir)))
}

#[tauri::command]
async fn view(s: State<'_, Shared>) -> Res<AnalysisView> {
    with(&s, |a| a.view())
}

#[tauri::command]
async fn source_profile(s: State<'_, Shared>) -> Res<Vec<ProfileRow>> {
    with(&s, |a| a.source_profile())
}

#[tauri::command]
async fn source_page(s: State<'_, Shared>, offset: u64, limit: u64) -> Res<Page> {
    with(&s, |a| a.source_page(offset, limit))
}

#[tauri::command]
async fn page(s: State<'_, Shared>, step_id: String, offset: u64, limit: u64) -> Res<Page> {
    with(&s, |a| a.page(&step_id, offset, limit))
}

#[tauri::command]
async fn chart(s: State<'_, Shared>, step_id: String) -> Res<Value> {
    with(&s, |a| a.chart(&step_id))
}

#[tauri::command]
async fn add_step(s: State<'_, Shared>, input: String, label: Option<String>, op: Value) -> Res<AnalysisView> {
    with(&s, |a| a.add_step(&input, label, &op))
}

#[tauri::command]
async fn update_step(s: State<'_, Shared>, id: String, label: Option<String>, op: Value) -> Res<AnalysisView> {
    with(&s, |a| a.update_step(&id, label, &op))
}

#[tauri::command]
async fn remove_step(s: State<'_, Shared>, id: String) -> Res<AnalysisView> {
    with(&s, |a| a.remove_step(&id))
}

#[tauri::command]
async fn set_cells(s: State<'_, Shared>, cells: Value) -> Res<AnalysisView> {
    with(&s, |a| a.set_cells(&cells))
}

/// Approve the shown code of these steps (ADR-0004).
#[tauri::command]
async fn approve(s: State<'_, Shared>, step_ids: Vec<String>) -> Res<AnalysisView> {
    with(&s, |a| a.approve(&step_ids))
}

/// "Trust all in this session": Python runs without review until the app
/// quits. Kept in memory only.
#[tauri::command]
async fn set_trust_all(s: State<'_, Shared>, on: bool) -> Res<Option<AnalysisView>> {
    with(&s, |a| a.set_trust_all(on))
}

#[tauri::command]
async fn rename(s: State<'_, Shared>, name: String) -> Res<AnalysisView> {
    with(&s, |a| a.rename(&name))
}

#[tauri::command]
async fn run(window: tauri::Window, s: State<'_, Shared>) -> Res<RunSummary> {
    let flag = with(&s, |a| a.cancel_handle())?;
    *s.cancel.lock().unwrap() = Some(flag);
    let app = s.app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut a = app.lock().map_err(|_| AppError::from("the app state is unavailable"))?;
        a.run(&mut |event| {
            let _ = window.emit("run://step", &event);
        })
    })
    .await
    .map_err(|e| AppError::from(format!("the run stopped unexpectedly: {e}")))?;
    *s.cancel.lock().unwrap() = None;
    result
}

#[tauri::command]
async fn cancel(s: State<'_, Shared>) -> Res<()> {
    if let Some(flag) = s.cancel.lock().unwrap().as_ref() {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// `--open <dir>` (or `VDA_OPEN`) opens a project at startup; used by the
/// end-to-end tests and handy from a terminal.
fn startup_project_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--open" {
            return args.next();
        }
    }
    std::env::var("VDA_OPEN").ok().filter(|s| !s.is_empty())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Shared {
            app: Arc::new(Mutex::new(App::default())),
            cancel: Mutex::new(None),
            startup: startup_project_arg(),
        })
        .invoke_handler(tauri::generate_handler![
            startup_project,
            approve,
            set_trust_all,
            create_project,
            open_project,
            view,
            source_profile,
            source_page,
            page,
            chart,
            add_step,
            update_step,
            remove_step,
            set_cells,
            rename,
            run,
            cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running the VDA desktop app");
}
