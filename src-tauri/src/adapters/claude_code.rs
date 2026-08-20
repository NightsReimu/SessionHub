use std::path::{Path, PathBuf};

use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};

/// Claude Code: ~/.claude/projects/<编码项目>/<id>.jsonl (+ 可选 sessions-index.json)
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    /// 读取项目目录下的 sessions-index.json，返回 fullPath -> entry 的映射（容错）
    fn read_index(project_dir: &Path) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        let index_path = project_dir.join("sessions-index.json");
        let Ok(text) = std::fs::read_to_string(&index_path) else {
            return map;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return map;
        };
        if let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
            for e in entries {
                if let Some(fp) = json_str(e, "fullPath") {
                    map.insert(fp.to_string(), e.clone());
                }
            }
        }
        map
    }
}

impl HarnessAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        "claude-code"
    }
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn detect(&self, ctx: &DetectCtx) -> bool {
        ctx.join(".claude/projects").is_dir()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        vec![ctx.join(".claude/projects")]
    }

    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> Vec<RawRef> {
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(root) else { return out };
        for dir in dirs.flatten() {
            let project_dir = dir.path();
            if !project_dir.is_dir() {
                continue;
            }
            let index = Self::read_index(&project_dir);
            let Ok(files) = std::fs::read_dir(&project_dir) else { continue };
            for f in files.flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(mut raw) = file_raw_ref(&path) {
                    let key = path.to_string_lossy().into_owned();
                    raw.inline = index.get(&key).cloned();
                    out.push(raw);
                }
            }
        }
        out
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let mut session_id = file_stem(&raw.path);
        let mut cwd = String::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;
        let mut msg_count: u32 = 0;
        let mut first_user_text: Option<String> = None;
        let mut tokens_in: u64 = 0;
        let mut tokens_out: u64 = 0;

        let ok = for_each_jsonl_line(&raw.path, |v| {
            let ty = json_str(&v, "type").unwrap_or("");
            if let Some(sid) = json_str(&v, "sessionId") {
                if !sid.is_empty() {
                    session_id = sid.to_string();
                }
            }
            if cwd.is_empty() {
                if let Some(c) = json_str(&v, "cwd") {
                    cwd = c.to_string();
                }
            }
            if let Some(ts) = json_str(&v, "timestamp").and_then(parse_iso_ms) {
                if first_ts.is_none_or(|f| ts < f) {
                    first_ts = Some(ts);
                }
                if last_ts.is_none_or(|l| ts > l) {
                    last_ts = Some(ts);
                }
            }
            match ty {
                "user" => {
                    msg_count += 1;
                    if first_user_text.is_none() {
                        if let Some(t) = v
                            .get("message")
                            .and_then(|m| m.get("content"))
                            .and_then(extract_claude_text)
                        {
                            let t = t.trim().to_string();
                            // 跳过系统注入的命令回显等
                            if !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:") {
                                first_user_text = Some(t);
                            }
                        }
                    }
                }
                "assistant" => {
                    msg_count += 1;
                    if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                        tokens_in += json_u64(u, "input_tokens").unwrap_or(0)
                            + json_u64(u, "cache_creation_input_tokens").unwrap_or(0)
                            + json_u64(u, "cache_read_input_tokens").unwrap_or(0);
                        tokens_out += json_u64(u, "output_tokens").unwrap_or(0);
                    }
                }
                _ => {}
            }
        });
        if !ok {
            return None;
        }

        // sessions-index.json 里的 summary / firstPrompt 是更好的标题来源
        let idx = raw.inline.as_ref();
        let title = idx
            .and_then(|e| json_str(e, "summary"))
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                idx.and_then(|e| json_str(e, "firstPrompt"))
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| truncate(s, 80))
            })
            .or_else(|| first_user_text.as_deref().map(|t| truncate(t, 80)))
            .unwrap_or_default();

        let project_path = idx
            .and_then(|e| json_str(e, "projectPath"))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(cwd);

        let started = idx
            .and_then(|e| json_str(e, "created"))
            .and_then(parse_iso_ms)
            .or(first_ts);
        let ended = idx
            .and_then(|e| json_str(e, "modified"))
            .and_then(parse_iso_ms)
            .or(last_ts);
        let count = idx
            .and_then(|e| json_u64(e, "messageCount"))
            .map(|n| n as u32)
            .or(if msg_count > 0 { Some(msg_count) } else { None });

        Some(Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path,
            title,
            started_at: started,
            ended_at: ended,
            message_count: count,
            tokens_in: if tokens_in > 0 { Some(tokens_in) } else { None },
            tokens_out: if tokens_out > 0 { Some(tokens_out) } else { None },
            cost_usd: None,
            status: derive_status(raw.mtime_ms),
            raw_path: raw.path.to_string_lossy().into_owned(),
            source_format: "jsonl".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
        })
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: format!("claude --resume {}", shell_quote(&s.session_id)),
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

    fn read_messages(&self, s: &Session, limit: usize) -> Vec<MessagePreview> {
        let path = PathBuf::from(&s.raw_path);
        let mut msgs: Vec<MessagePreview> = Vec::new();
        for_each_jsonl_line(&path, |v| {
            let ty = json_str(&v, "type").unwrap_or("");
            if ty != "user" && ty != "assistant" {
                return;
            }
            let Some(text) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(extract_claude_text)
            else {
                return;
            };
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            msgs.push(MessagePreview {
                role: ty.to_string(),
                text: truncate(&text, 2000),
                timestamp: json_str(&v, "timestamp").and_then(parse_iso_ms),
            });
        });
        if msgs.len() > limit {
            msgs = msgs.split_off(msgs.len() - limit);
        }
        msgs
    }
}

pub fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn non_empty(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
