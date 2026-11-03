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
async fn source_profile(s: State<'_, Shared>, source_id: String) -> Res<Vec<ProfileRow>> {
    with(&s, |a| a.source_profile(&source_id))
}

#[tauri::command]
async fn source_page(s: State<'_, Shared>, source_id: String, offset: u64, limit: u64) -> Res<Page> {
    with(&s, |a| a.source_page(&source_id, offset, limit))
}

#[tauri::command]
async fn add_source(s: State<'_, Shared>, csv: String) -> Res<AnalysisView> {
    with(&s, |a| a.add_source(std::path::Path::new(&csv)))
}

#[tauri::command]
async fn source_sheets(s: State<'_, Shared>, id: String) -> Res<Vec<String>> {
    with(&s, |a| a.source_sheets(&id))
}

#[tauri::command]
async fn set_source_read(s: State<'_, Shared>, id: String, table: Option<String>, query: Option<String>) -> Res<AnalysisView> {
    with(&s, |a| a.set_source_read(&id, table, query))
}

#[tauri::command]
async fn undo(s: State<'_, Shared>) -> Res<AnalysisView> {
    with(&s, |a| a.undo())
}

#[tauri::command]
async fn redo(s: State<'_, Shared>) -> Res<AnalysisView> {
    with(&s, |a| a.redo())
}

#[tauri::command]
async fn history(s: State<'_, Shared>, limit: usize) -> Res<Vec<vda_app::VersionView>> {
    with(&s, |a| a.history(limit))
}

#[tauri::command]
async fn restore_version(s: State<'_, Shared>, version: u64) -> Res<AnalysisView> {
    with(&s, |a| a.restore_version(version))
}

#[tauri::command]
async fn comments(s: State<'_, Shared>) -> Res<Vec<vda_app::CommentView>> {
    with(&s, |a| a.comments())
}

#[tauri::command]
async fn add_comment(s: State<'_, Shared>, target: String, text: String, reply_to: Option<String>) -> Res<Vec<vda_app::CommentView>> {
    with(&s, |a| a.add_comment(&target, &text, reply_to))
}

#[tauri::command]
async fn resolve_comment(s: State<'_, Shared>, id: String, resolved: bool) -> Res<Vec<vda_app::CommentView>> {
    with(&s, |a| a.resolve_comment(&id, resolved))
}

#[tauri::command]
async fn delete_comment(s: State<'_, Shared>, id: String) -> Res<Vec<vda_app::CommentView>> {
    with(&s, |a| a.delete_comment(&id))
}

#[tauri::command]
async fn python_environment(s: State<'_, Shared>) -> Res<vda_app::PythonView> {
    with(&s, |a| a.python_environment())
}

#[tauri::command]
async fn set_python(s: State<'_, Shared>, path: PathBuf) -> Res<vda_app::PythonView> {
    with(&s, |a| a.set_python(&path))
}

#[tauri::command]
async fn schedule(s: State<'_, Shared>, every: String, hour: u32, minute: u32, exports: Vec<(String, PathBuf)>) -> Res<vda_app::refresh::ScheduleView> {
    with(&s, |a| a.schedule(&every, hour, minute, &exports))
}

#[tauri::command]
async fn update_source(s: State<'_, Shared>, id: String, new_id: Option<String>, sheet: Option<String>) -> Res<AnalysisView> {
    with(&s, |a| a.update_source(&id, new_id, sheet))
}

#[tauri::command]
async fn problems(s: State<'_, Shared>) -> Res<vda_app::Problems> {
    with(&s, |a| a.problems())
}

#[tauri::command]
async fn check_draft(s: State<'_, Shared>, id: Option<String>, inputs: Vec<String>, op: Value) -> Res<Vec<vda_app::Problem>> {
    with(&s, |a| a.check_draft(id.as_deref(), &inputs, &op))
}

#[tauri::command]
async fn dismiss(s: State<'_, Shared>, step: Option<String>, code: String, dismissed: bool) -> Res<vda_app::Problems> {
    with(&s, |a| a.dismiss(step, &code, dismissed))
}

#[tauri::command]
async fn restore_dismissed(s: State<'_, Shared>) -> Res<vda_app::Problems> {
    with(&s, |a| a.restore_dismissed())
}

#[tauri::command]
async fn run_log(s: State<'_, Shared>, limit: usize) -> Res<Vec<vda_app::RunView>> {
    with(&s, |a| a.run_log(limit))
}

#[tauri::command]
async fn export(s: State<'_, Shared>, id: String, path: String) -> Res<u64> {
    with(&s, |a| a.export(&id, std::path::Path::new(&path)))
}

#[tauri::command]
async fn remove_source(s: State<'_, Shared>, id: String) -> Res<AnalysisView> {
    with(&s, |a| a.remove_source(&id))
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
async fn add_step(s: State<'_, Shared>, inputs: Vec<String>, label: Option<String>, op: Value) -> Res<AnalysisView> {
    with(&s, |a| a.add_step(&inputs, label, &op))
}

#[tauri::command]
async fn update_step(
    s: State<'_, Shared>,
    id: String,
    label: Option<String>,
    op: Value,
    inputs: Option<Vec<String>>,
) -> Res<AnalysisView> {
    with(&s, |a| a.update_step(&id, label, &op, inputs))
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
async fn set_parameters(s: State<'_, Shared>, params: Value) -> Res<AnalysisView> {
    with(&s, |a| a.set_parameters(&params))
}

#[tauri::command]
async fn rename_parameter(s: State<'_, Shared>, old: String, new: String) -> Res<(AnalysisView, Vec<String>)> {
    with(&s, |a| a.rename_parameter(&old, &new))
}

#[tauri::command]
async fn column_details(s: State<'_, Shared>, id: String) -> Res<Value> {
    with(&s, |a| a.column_details(&id))
}

#[tauri::command]
async fn export_notebook(s: State<'_, Shared>, path: PathBuf) -> Res<()> {
    with(&s, |a| a.export_notebook(&path))
}

#[tauri::command]
async fn save_report(s: State<'_, Shared>, path: PathBuf, html: String) -> Res<()> {
    with(&s, |a| a.save_report(&path, &html))
}

#[tauri::command]
async fn check_report(s: State<'_, Shared>, step_id: String) -> Res<Option<Value>> {
    with(&s, |a| a.check_report(&step_id))
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

/// With `VDA_SAVE_DIR` set, saving a notebook writes into that folder under
/// its suggested name instead of asking; for scripted use and the end-to-end
/// tests, where file dialogs cannot be answered.
#[tauri::command]
fn preset_save_path(name: String) -> Option<PathBuf> {
    let dir = std::env::var_os("VDA_SAVE_DIR").filter(|d| !d.is_empty())?;
    // Only the file name: a suggested name cannot point outside the folder.
    let file = std::path::Path::new(&name).file_name()?;
    Some(PathBuf::from(dir).join(file))
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
            add_source,
            remove_source,
            export,
            problems,
            check_draft,
            dismiss,
            restore_dismissed,
            run_log,
            source_sheets,
            update_source,
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
            set_parameters,
            rename_parameter,
            check_report,
            column_details,
            export_notebook,
            save_report,
            preset_save_path,
            set_source_read,
            undo,
            redo,
            history,
            restore_version,
            comments,
            add_comment,
            resolve_comment,
            delete_comment,
            python_environment,
            set_python,
            schedule,
            run,
            cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running the VDA desktop app");
}
