use std::time::Instant;

use crate::adapters::{DetectCtx, HarnessAdapter};
use crate::db::Db;
use crate::models::{AdapterScanStat, ScanProgress, ScanReport};

/// 解析器版本：解析逻辑产出字段发生变化（如新增费用估算）时 +1。
/// 按 harness 分别记录迁移状态——全局单一版本会在某 harness 恰好未安装时
/// 误标完成，导致其旧会话永远不补算新字段
const PARSE_VERSION: &str = "4";

/// 扫描所有 adapter：enumerate → 增量跳过 → parse → upsert。
/// full=true 时强制重解析，并清理索引里已不存在的会话。
/// progress 为可选的进度回调（每 adapter 先枚举得到总数，之后每 25 条上报一次）。
pub fn scan_all(
    db: &Db,
    adapters: &[Box<dyn HarnessAdapter>],
    ctx: &DetectCtx,
    full: bool,
    progress: Option<&dyn Fn(ScanProgress)>,
) -> ScanReport {
    let start = Instant::now();
    let mut stats = Vec::new();
    let adapter_count = adapters.iter().filter(|a| a.detect(ctx)).count();
    let mut adapter_index = 0usize;

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
        // 按 harness 的迁移版本：未检测到的 harness 不会被误标，
        // 之后恢复时仍会强制全量补算
        let version_key = format!("parse_version:{}", adapter.id());
        let adapter_outdated = db.get_setting(&version_key).as_deref() != Some(PARSE_VERSION);
        let full_for_adapter = full || adapter_outdated;
        let this_index = adapter_index;
        adapter_index += 1;
        let roots = adapter.roots(ctx);
        if roots.is_empty() {
            // detected 但当前没有任何可扫描根：无法区分“全被删了”和“暂时不可用”，
            // 按错误处理，全量扫描绝不在这种状态 prune
            stat.errors += 1;
            stats.push(stat);
            continue;
        }
        if let Some(p) = progress {
            p(ScanProgress {
                adapter_id: adapter.id().to_string(),
                adapter_index: this_index,
                adapter_count,
                done: 0,
                total: 0,
                parsed: 0,
                skipped: 0,
                errors: 0,
            });
        }
        // 先枚举所有根，拿到总数后才能显示 x/y 进度
        let mut all_raws = Vec::new();
        for root in roots {
            let (raws, enum_errors) = adapter.enumerate(&root, ctx);
            stat.errors += enum_errors;
            all_raws.extend(raws);
        }
        let total = all_raws.len();
        let mut seen_ids: Vec<String> = Vec::new();
        for (i, raw) in all_raws.into_iter().enumerate() {
            stat.scanned += 1;
            // 处理逻辑放进闭包，保证跳过/失败/成功所有分支都汇聚到
            // 后面的统一进度上报点，而不是 continue 绕过回调
            (|| {
                // 增量：identity + (size, mtime) 未变则跳过昂贵 parse
                if !full_for_adapter {
                    if let Some(id) = raw.identity.as_deref() {
                        if db.stamp(adapter.id(), id) == Some((raw.size, raw.mtime_ms)) {
                            stat.skipped += 1;
                            return;
                        }
                    }
                }
                let Ok(parsed_probe) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adapter.parse(&raw)))
                else {
                    stat.errors += 1;
                    return;
                };
                let Some(session) = parsed_probe else {
                    stat.errors += 1;
                    return;
                };
                seen_ids.push(session.session_id.clone());
                match db.upsert_session(&session) {
                    Ok(_) => stat.parsed += 1,
                    Err(e) => {
                        eprintln!("[scan] upsert failed {}: {e}", session.session_id);
                        stat.errors += 1;
                    }
                }
            })();
            // 统一的进度上报点：所有处理分支共用
            if let Some(p) = progress {
                if (i + 1) % 25 == 0 || i + 1 == total {
                    p(ScanProgress {
                        adapter_id: adapter.id().to_string(),
                        adapter_index: this_index,
                        adapter_count,
                        done: i + 1,
                        total,
                        parsed: stat.parsed,
                        skipped: stat.skipped,
                        errors: stat.errors,
                    });
                }
            }
        }
        // prune 安全规则（全量扫描）：
        // - 有任何错误（目录遍历失败/解析失败/索引损坏/必需根目录消失）→ 不动索引
        // - 零错误但一条都没扫到 → 根目录可读但确实是空的，正常清理
        stat.errors += adapter.required_roots_missing(ctx);
        if full_for_adapter && stat.errors == 0 {
            let _ = db.prune_not_in(adapter.id(), &seen_ids);
        }
        // 只有零错误完成才标记该 harness 已迁移
        if stat.errors == 0 {
            let _ = db.set_setting(&version_key, PARSE_VERSION);
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
    use crate::models::{Capabilities, RawRef, ResumeSpec, Session};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    fn temp_db(tag: &str) -> (Db, PathBuf) {
        let p =
            std::env::temp_dir().join(format!("sessionhub-ut-{}-{}.db", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        (Db::open(&p).unwrap(), p)
    }

    fn cleanup_db(p: &PathBuf) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(p.with_extension("db-wal"));
        let _ = std::fs::remove_file(p.with_extension("db-shm"));
    }

    #[derive(Default)]
    struct MockShared {
        raws: StdMutex<Vec<RawRef>>,
        errors: StdMutex<usize>,
        fail_parse: StdMutex<Vec<String>>,
        required_missing: StdMutex<usize>,
        roots_empty: StdMutex<bool>,
    }

    struct MockAdapter {
        shared: Arc<MockShared>,
    }

    fn mock_set(shared: &MockShared, ids: &[&str], errors: usize, fail: &[&str]) {
        *shared.raws.lock().unwrap() = ids
            .iter()
            .map(|id| RawRef {
                path: PathBuf::from(format!("/mock/{id}.jsonl")),
                size: 1,
                mtime_ms: 1,
                inline: None,
                identity: Some(id.to_string()),
            })
            .collect();
        *shared.errors.lock().unwrap() = errors;
        *shared.fail_parse.lock().unwrap() = fail.iter().map(|s| s.to_string()).collect();
    }

    impl HarnessAdapter for MockAdapter {
        fn id(&self) -> &'static str {
            "mock"
        }
        fn name(&self) -> &'static str {
            "Mock"
        }
        fn detect(&self, _ctx: &DetectCtx) -> bool {
            true
        }
        fn roots(&self, _ctx: &DetectCtx) -> Vec<PathBuf> {
            if *self.shared.roots_empty.lock().unwrap() {
                Vec::new()
            } else {
                vec![PathBuf::from("/mock")]
            }
        }
        fn required_roots_missing(&self, _ctx: &DetectCtx) -> usize {
            *self.shared.required_missing.lock().unwrap()
        }
        fn enumerate(&self, _root: &std::path::Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
            (
                self.shared.raws.lock().unwrap().clone(),
                *self.shared.errors.lock().unwrap(),
            )
        }
        fn parse(&self, raw: &RawRef) -> Option<Session> {
            let id = raw.identity.clone()?;
            if self.shared.fail_parse.lock().unwrap().contains(&id) {
                return None;
            }
            Some(Session {
                session_id: id.clone(),
                harness_id: "mock".to_string(),
                project_path: "/mock".to_string(),
                title: id,
                started_at: Some(1),
                ended_at: Some(1),
                message_count: None,
                tokens_in: None,
                tokens_out: None,
                cost_usd: None,
                status: "idle".to_string(),
                raw_path: raw.path.to_string_lossy().into_owned(),
                source_format: "mock".to_string(),
                file_size: raw.size,
                file_mtime: raw.mtime_ms,
            })
        }
        fn resume_spec(&self, _s: &Session) -> Option<ResumeSpec> {
            None
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
    }

    /// prune 安全矩阵：真删→清理；目录不可读/解析失败→保留；目录可读但空→清理
    #[test]
    fn full_scan_prune_matrix() {
        let (db, path) = temp_db("prune");
        let shared = Arc::new(MockShared::default());
        let adapters: Vec<Box<dyn HarnessAdapter>> = vec![Box::new(MockAdapter {
            shared: shared.clone(),
        })];
        let ctx = DetectCtx::new();

        // 初始：a、b 入库
        mock_set(&shared, &["a", "b"], 0, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 2);

        // b 的源文件真被删了 → prune 掉 b
        mock_set(&shared, &["a"], 0, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 1);
        assert!(db.get_session("mock", "b").is_none());

        // 目录遍历失败 → 即使扫到 a，也绝不动索引
        mock_set(&shared, &["a"], 1, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 1);

        // b 回来了但解析失败（文件写入中/损坏）→ 旧索引必须保留
        mock_set(&shared, &["a", "b"], 0, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 2);
        mock_set(&shared, &["a", "b"], 0, &["b"]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 2, "解析失败时不得 prune");
        assert!(db.get_session("mock", "b").is_some());

        // 目录可读但确实空了 → 正常清理
        mock_set(&shared, &[], 0, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 0, "源文件全删后应清理索引");

        cleanup_db(&path);
    }

    /// 必需根目录消失 / 无根可扫：即使其它根可读且零枚举错误，也不得 prune
    #[test]
    fn full_scan_prune_blocked_by_missing_required_root() {
        let (db, path) = temp_db("reqroot");
        let shared = Arc::new(MockShared::default());
        let adapters: Vec<Box<dyn HarnessAdapter>> = vec![Box::new(MockAdapter {
            shared: shared.clone(),
        })];
        let ctx = DetectCtx::new();

        mock_set(&shared, &["a", "b"], 0, &[]);
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 2);

        // 主根消失（enumerable 只剩 b）：不得 prune a
        mock_set(&shared, &["b"], 0, &[]);
        *shared.required_missing.lock().unwrap() = 1;
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 2, "必需根缺失时不得 prune");
        assert!(db.get_session("mock", "a").is_some());

        // 主根恢复 → prune 恢复工作
        *shared.required_missing.lock().unwrap() = 0;
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 1);

        // detected 但没有任何可扫描根 → 同样按错误处理
        *shared.roots_empty.lock().unwrap() = true;
        scan_all(&db, &adapters, &ctx, true, None);
        assert_eq!(db.counts().unwrap().total, 1, "无根可扫时不得 prune");

        cleanup_db(&path);
    }

    /// 对本机真实 harness 目录做只读冒烟扫描，验证各 adapter 解析与入库。
    /// 依赖开发者机器上实际装有 harness 会话，默认跳过：
    /// `cargo test scan_real_machine_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "依赖本机真实 harness 数据，CI/干净机器上无意义，手动运行"]
    fn scan_real_machine_smoke() {
        let tmp = std::env::temp_dir().join(format!("sessionhub-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let db = Db::open(&tmp).unwrap();
        let adapters = all_adapters();
        let report = scan_all(&db, &adapters, &DetectCtx::new(), false, None);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        if report.adapters.iter().any(|a| a.detected) {
            assert!(
                report.total_sessions > 0,
                "检测到 harness 但一个会话都没解析出来"
            );
        }
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(tmp.with_extension("db-wal"));
        let _ = std::fs::remove_file(tmp.with_extension("db-shm"));
    }
}
