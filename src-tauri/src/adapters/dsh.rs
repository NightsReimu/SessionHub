use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::claude_code::non_empty;
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};

/// DeepSeek Harness: 直接解析 ~/.dsh/storages/session_projcache.json
/// （它已经是“项目 → session”索引），原始会话目录在 ~/.dsh/sessions/<编码项目>/<session-id>/。
pub struct DshAdapter;

impl DshAdapter {
    /// 建立 session 目录名 -> 完整路径 的映射（项目目录名是转义编码，不可靠，直接用 projcache 的 cwd）
    fn session_dirs(ctx: &DetectCtx) -> HashMap<String, PathBuf> {
        let mut map = HashMap::new();
        let root = ctx.join(".dsh/sessions");
        let Ok(projects) = std::fs::read_dir(&root) else { return map };
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
        ctx.join(".dsh").is_dir()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        vec![ctx.join(".dsh/storages/session_projcache.json")]
    }

    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> Vec<RawRef> {
        let Ok(text) = std::fs::read_to_string(root) else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        let Some(sessions) = v
            .get("tables")
            .and_then(|t| t.get("sessions"))
            .and_then(|s| s.as_object())
        else {
            return Vec::new();
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

        sessions
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
                }
            })
            .collect()
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let inline = raw.inline.as_ref()?;
        let key = json_str(inline, "key")?.to_string();
        let v = inline.get("value")?;
        let identity = v.get("identity").cloned().unwrap_or(serde_json::Value::Null);
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

        let (mut tin, mut tout) = (None, None);
        if let Some(totals) = row_val("tokenUsage").and_then(|t| t.get("totals")) {
            let i = json_u64(totals, "uncachedInputTokens").unwrap_or(0)
                + json_u64(totals, "cacheReadTokens").unwrap_or(0)
                + json_u64(totals, "cacheWriteTokens").unwrap_or(0);
            let o = json_u64(totals, "outputTokens").unwrap_or(0);
            if i > 0 {
                tin = Some(i);
            }
            if o > 0 {
                tout = Some(o);
            }
        }

        let updated = row_val("sessionListMetadata")
            .and_then(|m| json_i64(m, "lastPromptAt"))
            .or(created);

        let raw_path = json_str(inline, "raw_dir")
            .map(|s| s.to_string())
            .unwrap_or_else(|| raw.path.to_string_lossy().into_owned());

        Some(Session {
            session_id: key,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title,
            started_at: created,
            ended_at: updated,
            message_count: turns.map(|t| t as u32),
            tokens_in: tin,
            tokens_out: tout,
            cost_usd: None,
            status: derive_status(updated.unwrap_or(raw.mtime_ms)),
            raw_path,
            source_format: "dsh-projcache".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
        })
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: "dsh".to_string(),
            cwd: non_empty(&s.project_path),
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_resume: true,
            can_delete: true,
            can_backup: true,
            can_read_messages: true,
        }
    }

    /// raw_path 回退到全局 session_projcache.json 时禁止删除，
    /// 否则会把整个 DSH 会话索引移入回收站
    fn can_delete_session(&self, s: &Session) -> bool {
        !s.raw_path.ends_with("session_projcache.json")
    }

    fn read_messages(&self, s: &Session, limit: usize) -> Vec<MessagePreview> {
        use std::collections::VecDeque;
        use std::io::{BufRead, BufReader};

        let zstd_path = PathBuf::from(&s.raw_path).join("session.jsonl.zstd");
        let Ok(file) = std::fs::File::open(&zstd_path) else {
            return Vec::new();
        };
        let Ok(decoder) = zstd::stream::read::Decoder::new(file) else {
            return Vec::new();
        };
        // 流式解压 + 环形缓冲：内存占用只与 limit 相关，与文件大小无关
        let limit = limit.max(1);
        let mut ring: VecDeque<MessagePreview> = VecDeque::with_capacity(limit + 1);
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
            let role = if ty == "user/message" { "user" } else { "assistant" };
            ring.push_back(MessagePreview {
                role: role.to_string(),
                text: truncate(&text_val, 2000),
                timestamp: json_i64(&v, "time"),
            });
            if ring.len() > limit {
                ring.pop_front();
            }
        }
        ring.into_iter().collect()
    }
}
