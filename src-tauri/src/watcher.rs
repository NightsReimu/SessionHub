use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

use crate::adapters::{DetectCtx, HarnessAdapter};
use crate::db::Db;
use crate::scanner::scan_all;

pub struct WatcherHandle {
    _watcher: RecommendedWatcher,
}

/// 监听所有已检测 adapter 的根目录；文件变化去抖 800ms 后做增量扫描并通知前端。
pub fn start(
    app: AppHandle,
    db: Arc<Db>,
    adapters: Arc<Vec<Box<dyn HarnessAdapter>>>,
) -> Result<WatcherHandle, String> {
    let ctx = DetectCtx::new();
    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .map_err(|e| format!("创建文件监听器失败：{e}"))?;

    let mut watched = 0usize;
    for adapter in adapters.iter() {
        if !adapter.detect(&ctx) {
            continue;
        }
        for root in adapter.roots(&ctx) {
            // 文件型根（如 opencode.db / projcache.json）监听其父目录
            let (dir, mode) = if root.is_file() {
                (
                    root.parent().map(|p| p.to_path_buf()).unwrap_or(root.clone()),
                    RecursiveMode::NonRecursive,
                )
            } else {
                (root.clone(), RecursiveMode::Recursive)
            };
            if watcher.watch(&dir, mode).is_ok() {
                watched += 1;
            }
        }
    }
    if watched == 0 {
        return Err("没有可监听的目录".to_string());
    }

    std::thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(800)) {
            Ok(_) => {
                // 去抖：排空积压事件
                while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
                let report = scan_all(&db, &adapters, &DetectCtx::new(), false);
                let _ = app.emit("scan-update", report);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    });

    Ok(WatcherHandle { _watcher: watcher })
}
