mod actions;
mod adapters;
mod db;
mod models;
mod scanner;
mod watcher;

use std::sync::{Arc, Mutex};

use adapters::{all_adapters, DetectCtx, HarnessAdapter};
use db::Db;
use models::*;

pub struct AppState {
    db: Arc<Db>,
    adapters: Arc<Vec<Box<dyn HarnessAdapter>>>,
    watcher: Mutex<Option<watcher::WatcherHandle>>,
}

impl AppState {
    fn adapter(&self, id: &str) -> Option<&(dyn HarnessAdapter + 'static)> {
        self.adapters.iter().find(|a| a.id() == id).map(|a| a.as_ref())
    }
}

#[tauri::command]
fn list_adapters(state: tauri::State<AppState>) -> Vec<AdapterInfo> {
    let ctx = DetectCtx::new();
    state
        .adapters
        .iter()
        .map(|a| AdapterInfo {
            id: a.id().to_string(),
            name: a.name().to_string(),
            detected: a.detect(&ctx),
            roots: a
                .roots(&ctx)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            capabilities: a.capabilities(),
        })
        .collect()
}

#[tauri::command]
fn scan_sessions(state: tauri::State<AppState>, full: bool) -> ScanReport {
    scanner::scan_all(&state.db, &state.adapters, &DetectCtx::new(), full)
}

#[tauri::command]
fn list_sessions(
    state: tauri::State<AppState>,
    harness: Option<String>,
    favorites_only: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<SessionDto>, String> {
    state
        .db
        .list_sessions(
            harness.as_deref(),
            favorites_only,
            limit.unwrap_or(800),
            offset.unwrap_or(0),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn search_sessions(state: tauri::State<AppState>, query: String) -> Result<Vec<SessionDto>, String> {
    state.db.search(&query, 300).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_session_messages(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
    limit: Option<usize>,
) -> Result<Vec<MessagePreview>, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中，请先扫描".to_string())?;
    let adapter = state
        .adapter(&harness_id)
        .ok_or_else(|| format!("未知 harness：{harness_id}"))?;
    Ok(adapter.read_messages(&dto.session, limit.unwrap_or(300)))
}

#[tauri::command]
fn resume_session(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
) -> Result<String, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中，请先扫描".to_string())?;
    let adapter = state
        .adapter(&harness_id)
        .ok_or_else(|| format!("未知 harness：{harness_id}"))?;
    let spec = adapter
        .resume_spec(&dto.session)
        .ok_or_else(|| "该 harness 暂不支持续接".to_string())?;
    actions::launch_in_terminal(&spec.command, spec.cwd.as_deref())
}

#[tauri::command]
fn delete_session(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
) -> Result<String, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中".to_string())?;
    let adapter = state
        .adapter(&harness_id)
        .ok_or_else(|| format!("未知 harness：{harness_id}"))?;
    if !adapter.capabilities().can_delete {
        return Err("该 harness 使用共享数据库存储，不支持删除单个会话".to_string());
    }
    actions::trash_raw(&dto.session)?;
    let _ = state.db.delete_session_row(&harness_id, &session_id);
    Ok(dto.session.raw_path.clone())
}

#[tauri::command]
fn backup_session(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
) -> Result<String, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中".to_string())?;
    let dest = actions::backup_raw(&dto.session)?;
    Ok(dest.to_string_lossy().into_owned())
}

#[tauri::command]
fn export_session(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
    format: String,
) -> Result<String, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中".to_string())?;
    let messages = state
        .adapter(&harness_id)
        .map(|a| a.read_messages(&dto.session, 500))
        .unwrap_or_default();
    let dest = actions::export_session(&dto.session, &messages, &format)?;
    Ok(dest.to_string_lossy().into_owned())
}

#[tauri::command]
fn reveal_raw(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
) -> Result<String, String> {
    let dto = state
        .db
        .get_session(&harness_id, &session_id)
        .ok_or_else(|| "会话不在索引中".to_string())?;
    actions::reveal_raw(&dto.session)?;
    Ok(dto.session.raw_path.clone())
}

#[tauri::command]
fn set_session_meta(
    state: tauri::State<AppState>,
    harness_id: String,
    session_id: String,
    meta: SessionMeta,
) -> Result<(), String> {
    state
        .db
        .set_meta(&harness_id, &session_id, &meta)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_counts(state: tauri::State<AppState>) -> Result<Counts, String> {
    state.db.counts().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_hub_paths() -> HubPaths {
    let hub = actions::hub_dir();
    HubPaths {
        hub_dir: hub.to_string_lossy().into_owned(),
        backups_dir: hub.join("backups").to_string_lossy().into_owned(),
        exports_dir: hub.join("exports").to_string_lossy().into_owned(),
        db_path: hub.join("sessionhub.db").to_string_lossy().into_owned(),
    }
}

#[tauri::command]
fn watcher_start(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<bool, String> {
    let mut guard = state.watcher.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        return Ok(true);
    }
    let handle = watcher::start(app, state.db.clone(), state.adapters.clone())?;
    *guard = Some(handle);
    Ok(true)
}

#[tauri::command]
fn watcher_stop(state: tauri::State<AppState>) -> Result<bool, String> {
    let mut guard = state.watcher.lock().map_err(|e| e.to_string())?;
    *guard = None;
    Ok(false)
}

#[tauri::command]
fn watcher_status(state: tauri::State<AppState>) -> bool {
    state.watcher.lock().map(|g| g.is_some()).unwrap_or(false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = actions::ensure_hub_dirs();
    let db_path = actions::hub_dir().join("sessionhub.db");
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        panic!("无法打开 SessionHub 数据库 {}: {e}", db_path.display())
    });

    let state = AppState {
        db: Arc::new(db),
        adapters: Arc::new(all_adapters()),
        watcher: Mutex::new(None),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_adapters,
            scan_sessions,
            list_sessions,
            search_sessions,
            get_session_messages,
            resume_session,
            delete_session,
            backup_session,
            export_session,
            reveal_raw,
            set_session_meta,
            get_counts,
            get_hub_paths,
            watcher_start,
            watcher_stop,
            watcher_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SessionHub");
}
