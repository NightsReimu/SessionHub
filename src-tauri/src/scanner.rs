use std::time::Instant;

use crate::adapters::{DetectCtx, HarnessAdapter};
use crate::db::Db;
use crate::models::{AdapterScanStat, ScanReport};

/// 扫描所有 adapter：enumerate → 增量跳过 → parse → upsert。
/// full=true 时强制重解析，并清理索引里已不存在的会话。
pub fn scan_all(
    db: &Db,
    adapters: &[Box<dyn HarnessAdapter>],
    ctx: &DetectCtx,
    full: bool,
) -> ScanReport {
    let start = Instant::now();
    let mut stats = Vec::new();

    for adapter in adapters {
        let detected = adapter.detect(ctx);
        let mut stat = AdapterScanStat {
            adapter_id: adapter.id().to_string(),
            detected,
            scanned: 0,
            parsed: 0,
            skipped: 0,
            errors: 0,
        };
        if !detected {
            stats.push(stat);
            continue;
        }
        let mut seen_ids: Vec<String> = Vec::new();
        for root in adapter.roots(ctx) {
            for raw in adapter.enumerate(&root, ctx) {
                stat.scanned += 1;
                // 增量：identity + (size, mtime) 未变则跳过昂贵 parse
                if !full {
                    if let Some(id) = raw.identity.as_deref() {
                        if db.stamp(adapter.id(), id) == Some((raw.size, raw.mtime_ms)) {
                            stat.skipped += 1;
                            continue;
                        }
                    }
                }
                let Ok(parsed_probe) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    adapter.parse(&raw)
                })) else {
                    stat.errors += 1;
                    continue;
                };
                let Some(session) = parsed_probe else {
                    stat.errors += 1;
                    continue;
                };
                seen_ids.push(session.session_id.clone());
                match db.upsert_session(&session) {
                    Ok(_) => stat.parsed += 1,
                    Err(e) => {
                        eprintln!("[scan] upsert failed {}: {e}", session.session_id);
                        stat.errors += 1;
                    }
                }
            }
        }
        if full {
            let _ = db.prune_not_in(adapter.id(), &seen_ids);
        }
        stats.push(stat);
    }

    let total = db.counts().map(|c| c.total).unwrap_or(0);
    ScanReport {
        adapters: stats,
        total_sessions: total,
        duration_ms: start.elapsed().as_millis(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::all_adapters;

    /// 对本机真实 harness 目录做只读冒烟扫描，验证各 adapter 解析与入库
    #[test]
    fn scan_real_machine_smoke() {
        let tmp = std::env::temp_dir().join(format!("sessionhub-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let db = Db::open(&tmp).unwrap();
        let adapters = all_adapters();
        let report = scan_all(&db, &adapters, &DetectCtx::new(), false);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        assert!(report.total_sessions > 0, "本机应能扫描到会话");
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(tmp.with_extension("db-wal"));
        let _ = std::fs::remove_file(tmp.with_extension("db-shm"));
    }
}
