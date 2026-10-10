use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::claude_code::non_empty;
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session, UsageEvent};
use crate::pricing::Tokens;

/// DeepSeek Harness: 直接解析 ~/.dsh/storages/session_projcache.json
/// （它已经是“项目 → session”索引），原始会话目录在 ~/.dsh/sessions/<编码项目>/<session-id>/。
pub struct DshAdapter;

impl DshAdapter {
    /// 建立 session 目录名 -> 完整路径 的映射（项目目录名是转义编码，不可靠，直接用 projcache 的 cwd）
    fn session_dirs(ctx: &DetectCtx) -> HashMap<String, PathBuf> {
        let mut map = HashMap::new();
        let root = ctx.join(".dsh/sessions");
        let Ok(projects) = std::fs::read_dir(&root) else {
            return map;
        };
        for p in projects.flatten() {
            let pdir = p.path();
            if !pdir.is_dir() {
                continue;
            }
            if let Ok(children) = std::fs::read_dir(&pdir) {
                for c in children.flatten() {
                    let cpath = c.path();
                    if cpath.is_dir() {
                        if let Some(name) = cpath.file_name() {
                            map.insert(name.to_string_lossy().into_owned(), cpath);
                        }
                    }
                }
            }
        }
        map
    }
}

impl HarnessAdapter for DshAdapter {
    fn id(&self) -> &'static str {
        "dsh"
    }
    fn name(&self) -> &'static str {
        "DeepSeek Harness"
    }
    fn detect(&self, ctx: &DetectCtx) -> bool {
        ctx.join(".dsh/storages/session_projcache.json").is_file()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        // projcache 是唯一的枚举入口；不存在时 detect 为 false，
        // 扫描器直接跳过本 adapter，不会触碰索引
        [ctx.join(".dsh/storages/session_projcache.json")]
            .into_iter()
            .filter(|p| p.is_file())
            .collect()
    }

    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        let Ok(text) = std::fs::read_to_string(root) else {
            return (Vec::new(), 1);
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return (Vec::new(), 1);
        };
        let Some(sessions) = v
            .get("tables")
            .and_then(|t| t.get("sessions"))
            .and_then(|s| s.as_object())
        else {
            return (Vec::new(), 1);
        };
        let dirs = Self::session_dirs(ctx);
        let (size, mtime) = std::fs::metadata(root)
            .ok()
            .map(|md| {
                let m = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                (md.len(), m)
            })
            .unwrap_or((0, 0));

        let raws = sessions
            .iter()
            .map(|(key, value)| {
                // 目录名可能是 "session-<uuid>" 也可能是裸 "<uuid>"
                let raw_dir = dirs
                    .get(key)
                    .or_else(|| key.strip_prefix("session-").and_then(|k| dirs.get(k)))
                    .map(|p| p.to_string_lossy().into_owned());
                RawRef {
                    path: root.to_path_buf(),
                    size,
                    mtime_ms: mtime,
                    identity: Some(key.clone()),
                    inline: Some(serde_json::json!({
                        "key": key,
                        "value": value,
                        "raw_dir": raw_dir,
                    })),
                    usage: Vec::new(),
                }
            })
            .collect();
        (raws, 0)
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let inline = raw.inline.as_ref()?;
        let key = json_str(inline, "key")?.to_string();
        let v = inline.get("value")?;
        let identity = v
            .get("identity")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let rows = v.get("rows").cloned().unwrap_or(serde_json::Value::Null);

        let created = json_i64(&identity, "createdAt");
        let cwd = json_str(&identity, "cwd").unwrap_or("").to_string();

        let row_val = |name: &str| -> Option<&serde_json::Value> {
            rows.get(name).and_then(|r| r.get("val"))
        };

        let title = row_val("title")
            .and_then(|t| t.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let turns = row_val("sessionStats").and_then(|s| json_u64(s, "turns"));

        // 汇总值兜底：日志不可读时，把 projcache 的累计用量作为一条明细
        let totals = row_val("tokenUsage")
            .and_then(|t| t.get("totals"))
            .map(|t| Tokens {
                input: json_u64(t, "uncachedInputTokens").unwrap_or(0),
                output: json_u64(t, "outputTokens").unwrap_or(0),
                cache_read: json_u64(t, "cacheReadTokens").unwrap_or(0),
                cache_write: json_u64(t, "cacheWriteTokens").unwrap_or(0),
                cache_write_1h: 0,
            });

        let updated = row_val("sessionListMetadata")
            .and_then(|m| json_i64(m, "lastPromptAt"))
            .or(created);

        let raw_path = json_str(inline, "raw_dir")
            .map(|s| s.to_string())
            .unwrap_or_else(|| raw.path.to_string_lossy().into_owned());

        // 逐步明细：每个 step 的 usage chunk 带时间和当时的模型（会话里可切换模型）
        let mut usage = json_str(inline, "raw_dir")
            .and_then(|d| step_usage(&PathBuf::from(d).join("session.jsonl.zstd")))
            .unwrap_or_default();
        if usage.is_empty() {
            if let Some(t) =
                totals.filter(|t| t.input + t.output + t.cache_read + t.cache_write > 0)
            {
                usage.push(priced_event(
                    updated.unwrap_or(raw.mtime_ms),
                    "deepseek",
                    t,
                    1.0,
                ));
            }
        }

        let mut session = Session {
            session_id: key,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title,
            started_at: created,
            ended_at: updated,
            message_count: turns.map(|t| t as u32),
            status: derive_status(updated.unwrap_or(raw.mtime_ms)),
            raw_path,
            source_format: "dsh-projcache".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
            usage,
            ..Default::default()
        };
        session.apply_usage_totals();
        Some(session)
    }

    fn resume_spec(&self, _s: &Session) -> Option<ResumeSpec> {
        // dsh CLI 没有按会话恢复的参数（dsh --help 实测），不声明续接能力
        None
    }

    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec::new(
            vec!["dsh".to_string()],
            non_empty(&s.project_path),
        ))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_resume: false,
            can_delete: true,
            can_backup: true,
            can_read_messages: true,
            can_launch: true,
        }
    }

    /// 会话目录缺失时 raw_path 会回退到全局 session_projcache.json，
    /// 此时删除/备份/定位都会误伤整个 DSH 索引，一律禁止
    fn can_use_raw_path(&self, s: &Session) -> bool {
        !s.raw_path.ends_with("session_projcache.json")
    }

    fn read_messages(&self, s: &Session, limit: usize) -> Vec<MessagePreview> {
        self.read_limited(s, limit, 2000).unwrap_or_default()
    }

    fn read_messages_full(&self, s: &Session) -> Option<Vec<MessagePreview>> {
        self.read_limited(s, usize::MAX, usize::MAX)
    }
}

/// session.jsonl.zstd 的用量缓存：projcache 一变，所有 DSH 会话都会被重新 parse，
/// 但各会话日志大多没变——按 (size, mtime) 复用上次的解析结果，避免反复解压
type StepCache = HashMap<PathBuf, ((u64, i64), Vec<UsageEvent>)>;
static STEP_CACHE: std::sync::OnceLock<std::sync::Mutex<StepCache>> = std::sync::OnceLock::new();

fn file_stamp(p: &Path) -> Option<(u64, i64)> {
    let md = std::fs::metadata(p).ok()?;
    let m = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((md.len(), m))
}

/// 从 DSH 会话日志读出每个 step 的用量：`request/header` 给出当前模型，
/// `assistant/chunk` 里 `chunk.type == "usage"` 是该 step 的计数
/// （inputTokens 不含缓存命中，与 projcache 的 uncachedInputTokens 口径一致，已实测核对）
fn step_usage(zstd_path: &Path) -> Option<Vec<UsageEvent>> {
    let stamp = file_stamp(zstd_path)?;
    let cache = STEP_CACHE.get_or_init(Default::default);
    if let Some((st, ev)) = cache.lock().ok()?.get(zstd_path) {
        if *st == stamp {
            return Some(ev.clone());
        }
    }
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(zstd_path).ok()?;
    let decoder = zstd::stream::read::Decoder::new(file).ok()?;
    let mut reader = BufReader::with_capacity(1 << 20, decoder);
    let mut buf = Vec::new();
    let mut model = String::new();
    let mut events = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        let is_header = line.contains("\"request/header\"");
        if !is_header && !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let data = v.get("data").unwrap_or(&serde_json::Value::Null);
        if is_header {
            if let Some(m) = data
                .pointer("/header/config/model")
                .and_then(|m| m.as_str())
            {
                model = m.to_string();
            }
            continue;
        }
        let Some(chunk) = data.get("chunk") else {
            continue;
        };
        if json_str(chunk, "type") != Some("usage") {
            continue;
        }
        let Some(u) = chunk.get("usage") else {
            continue;
        };
        let t = Tokens {
            input: json_u64(u, "inputTokens").unwrap_or(0),
            output: json_u64(u, "outputTokens").unwrap_or(0),
            cache_read: json_u64(u, "cacheReadTokens").unwrap_or(0),
            cache_write: json_u64(u, "cacheWriteTokens").unwrap_or(0),
            cache_write_1h: 0,
        };
        if t.input + t.output + t.cache_read + t.cache_write == 0 {
            continue;
        }
        let ts = json_i64(&v, "time").unwrap_or(0);
        events.push(priced_event(ts, &model, t, 1.0));
    }
    if let Ok(mut c) = cache.lock() {
        c.insert(zstd_path.to_path_buf(), (stamp, events.clone()));
    }
    Some(events)
}

impl DshAdapter {
    fn read_limited(
        &self,
        s: &Session,
        limit: usize,
        max_len: usize,
    ) -> Option<Vec<MessagePreview>> {
        use std::collections::VecDeque;
        use std::io::{BufRead, BufReader};

        let zstd_path = PathBuf::from(&s.raw_path).join("session.jsonl.zstd");
        let Ok(file) = std::fs::File::open(&zstd_path) else {
            return None;
        };
        let Ok(decoder) = zstd::stream::read::Decoder::new(file) else {
            return None;
        };
        // 有界时用环形缓冲：内存只与 limit 相关；无界（迁移）时全量收集
        let bounded = limit != usize::MAX;
        let mut ring: VecDeque<MessagePreview> = VecDeque::new();
        let reader = BufReader::with_capacity(1 << 20, decoder);
        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                continue;
            };
            let ty = json_str(&v, "type").unwrap_or("");
            if ty != "user/message" && ty != "assistant/message" {
                continue;
            }
            let Some(data) = v.get("data") else { continue };
            let Some(text_val) = data.get("content").and_then(extract_claude_text) else {
                continue;
            };
            let text_val = text_val.trim().to_string();
            if text_val.is_empty() {
                continue;
            }
            let role = if ty == "user/message" {
                "user"
            } else {
                "assistant"
            };
            ring.push_back(MessagePreview {
                role: role.to_string(),
                text: truncate(&text_val, max_len),
                timestamp: json_i64(&v, "time"),
            });
            if bounded && ring.len() > limit.max(1) {
                ring.pop_front();
            }
        }
        Some(ring.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Session;

    fn stub(raw_path: &str) -> Session {
        Session {
            session_id: "session-x".to_string(),
            harness_id: "dsh".to_string(),
            project_path: "/tmp/p".to_string(),
            title: String::new(),
            started_at: None,
            ended_at: None,
            message_count: None,
            tokens_in: None,
            tokens_out: None,
            cost_usd: None,
            status: "idle".to_string(),
            raw_path: raw_path.to_string(),
            source_format: "dsh-projcache".to_string(),
            file_size: 0,
            file_mtime: 0,
            ..Default::default()
        }
    }

    /// raw_path 回退到全局索引文件时，删除/备份/定位全部禁止
    #[test]
    fn raw_path_guard_blocks_projcache_fallback() {
        let a = DshAdapter;
        let fallback = stub("/Users/x/.dsh/storages/session_projcache.json");
        assert!(!a.can_use_raw_path(&fallback));
        assert!(!a.can_delete_session(&fallback));

        let normal = stub("/Users/x/.dsh/sessions/--proj--/session-x");
        assert!(a.can_use_raw_path(&normal));
        assert!(a.can_delete_session(&normal));
    }
}
