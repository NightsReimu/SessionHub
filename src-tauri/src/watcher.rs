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
/// scan_lock 与手动扫描互斥，防止并发扫描互相覆盖索引。
pub fn start(
    app: AppHandle,
    db: Arc<Db>,
    adapters: Arc<Vec<Box<dyn HarnessAdapter>>>,
    scan_lock: Arc<parking_lot::Mutex<()>>,
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
                    root.parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or(root.clone()),
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

    // 两次扫描之间至少间隔这么久：活跃会话持续追加写入时，
    // 不让监听线程把几百 MB 的 rollout 一遍接一遍地重解析
    const COOLDOWN: Duration = Duration::from_secs(5);
    std::thread::spawn(move || {
        let mut last_scan_end: Option<std::time::Instant> = None;
        loop {
            match rx.recv_timeout(Duration::from_millis(800)) {
                Ok(_) => {
                    // 去抖：排空积压事件，但设总时长上限。会话活跃写入时事件间隔
                    // 可能持续小于 200ms，无上限会让内层循环永不退出、扫描永不触发
                    // （界面恰在最该刷新时停止更新）。
                    let drain_deadline = std::time::Instant::now() + Duration::from_secs(2);
                    while rx.recv_timeout(Duration::from_millis(200)).is_ok() {
                        if std::time::Instant::now() >= drain_deadline {
                            break;
                        }
                    }
                    // 冷却期内到达的事件一并吸收，冷却结束后统一扫一次
                    if let Some(t) = last_scan_end {
                        let since = t.elapsed();
                        if since < COOLDOWN {
                            let wait_until = std::time::Instant::now() + (COOLDOWN - since);
                            while let Some(left) =
                                wait_until.checked_duration_since(std::time::Instant::now())
                            {
                                match rx.recv_timeout(left) {
                                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                                    Err(RecvTimeoutError::Disconnected) => return,
                                }
                            }
                        }
                    }
                    let _scan_guard = scan_lock.lock();
                    let report = scan_all(
                        &db,
                        &adapters,
                        &DetectCtx::new(),
                        false,
                        Some(&|p| {
                            let _ = app.emit("scan-progress", p);
                        }),
                    );
                    drop(_scan_guard);
                    last_scan_end = Some(std::time::Instant::now());
                    let _ = app.emit("scan-update", report);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    Ok(WatcherHandle { _watcher: watcher })
}
