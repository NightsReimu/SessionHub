use std::path::{Path, PathBuf};

use super::claude_code::{non_empty, shell_quote};
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};

/// Codex: ~/.codex/sessions/<年>/<月>/<日>/rollout-*.jsonl 递归 + ~/.codex/archived_sessions/
pub struct CodexAdapter;

fn extract_codex_text(content: &serde_json::Value) -> Option<String> {
    let arr = content.as_array()?;
    let mut out = String::new();
    for item in arr {
        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(t);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 在 token_count 之类的 payload 里递归找 token 数字（格式无文档，防御式）
fn find_token_numbers(v: &serde_json::Value, input: &mut Option<u64>, output: &mut Option<u64>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                match k.as_str() {
                    "input_tokens" | "uncachedInputTokens" => {
                        if let Some(n) = val.as_u64() {
                            *input = Some(n);
                        }
                    }
                    "output_tokens" | "outputTokens" => {
                        if let Some(n) = val.as_u64() {
                            *output = Some(n);
                        }
                    }
                    _ => find_token_numbers(val, input, output),
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for x in arr {
                find_token_numbers(x, input, output);
            }
        }
        _ => {}
    }
}

impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn name(&self) -> &'static str {
        "Codex"
    }
    fn detect(&self, ctx: &DetectCtx) -> bool {
        ctx.join(".codex").is_dir()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        vec![ctx.join(".codex/sessions"), ctx.join(".codex/archived_sessions")]
    }

    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        let (files, mut errors) = collect_files(root, &[".jsonl"]);
        let mut out = Vec::new();
        for p in files {
            let is_rollout = p
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("rollout-"))
                .unwrap_or(false);
            if !is_rollout {
                continue;
            }
            match file_raw_ref(&p) {
                Some(mut raw) => {
                    // rollout-<19字符时间戳>-<uuid> → 取 uuid 作为 identity
                    raw.identity = file_stem(&p)
                        .strip_prefix("rollout-")
                        .and_then(|s| s.get(20..))
                        .map(|s| s.to_string())
                        .or_else(|| Some(file_stem(&p)));
                    out.push(raw);
                }
                None => errors += 1,
            }
        }
        (out, errors)
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let mut session_id: Option<String> = None;
        let mut cwd = String::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;
        let mut msg_count: u32 = 0;
        let mut first_user_text: Option<String> = None;
        let mut tokens_in: Option<u64> = None;
        let mut tokens_out: Option<u64> = None;
        let archived = raw.path.to_string_lossy().contains("archived_sessions");

        let ok = for_each_jsonl_line(&raw.path, |v| {
            if let Some(ts) = json_str(&v, "timestamp").and_then(parse_iso_ms) {
                if first_ts.is_none_or(|f| ts < f) {
                    first_ts = Some(ts);
                }
                if last_ts.is_none_or(|l| ts > l) {
                    last_ts = Some(ts);
                }
            }
            let ty = json_str(&v, "type").unwrap_or("");
            let payload = v.get("payload").cloned().unwrap_or(serde_json::Value::Null);
            match ty {
                "session_meta" => {
                    if session_id.is_none() {
                        session_id = json_str(&payload, "session_id")
                            .or_else(|| json_str(&payload, "id"))
                            .map(|s| s.to_string());
                    }
                    if cwd.is_empty() {
                        if let Some(c) = json_str(&payload, "cwd") {
                            cwd = c.to_string();
                        }
                    }
                }
                "response_item" | "event_msg" => {
                    let pty = json_str(&payload, "type").unwrap_or("");
                    match pty {
                        "message" => {
                            msg_count += 1;
                            let role = json_str(&payload, "role").unwrap_or("");
                            if role == "user" && first_user_text.is_none() {
                                if let Some(t) = payload.get("content").and_then(extract_codex_text) {
                                    let t = t.trim().to_string();
                                    // 跳过环境上下文等注入内容
                                    if !t.is_empty() && !t.starts_with('<') {
                                        first_user_text = Some(t);
                                    }
                                }
                            }
                        }
                        "user_message" => {
                            msg_count += 1;
                            if first_user_text.is_none() {
                                if let Some(t) = json_str(&payload, "message") {
                                    let t = t.trim();
                                    if !t.is_empty() && !t.starts_with('<') {
                                        first_user_text = Some(t.to_string());
                                    }
                                }
                            }
                        }
                        "token_count" => {
                            find_token_numbers(&payload, &mut tokens_in, &mut tokens_out);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        });
        if !ok {
            return None;
        }

        // 兜底：从文件名 rollout-<ts>-<uuid>.jsonl 取 id
        let session_id = session_id.unwrap_or_else(|| {
            let stem = file_stem(&raw.path);
            stem.strip_prefix("rollout-")
                .and_then(|s| s.get(20..))
                .map(|s| s.to_string())
                .unwrap_or(stem)
        });

        Some(Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title: first_user_text.as_deref().map(|t| truncate(t, 80)).unwrap_or_default(),
            started_at: first_ts,
            ended_at: last_ts,
            message_count: if msg_count > 0 { Some(msg_count) } else { None },
            tokens_in,
            tokens_out,
            cost_usd: None,
            status: if archived { "archived".to_string() } else { derive_status(raw.mtime_ms) },
            raw_path: raw.path.to_string_lossy().into_owned(),
            source_format: "jsonl".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
        })
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: format!("codex resume {}", shell_quote(&s.session_id)),
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
            let payload = match v.get("payload") {
                Some(p) => p,
                None => return,
            };
            let pty = json_str(payload, "type").unwrap_or("");
            let (role, text) = match pty {
                "message" => {
                    let role = json_str(payload, "role").unwrap_or("").to_string();
                    let text = payload.get("content").and_then(extract_codex_text);
                    (role, text)
                }
                "user_message" => (
                    "user".to_string(),
                    json_str(payload, "message").map(|s| s.to_string()),
                ),
                "agent_message" => (
                    "assistant".to_string(),
                    json_str(payload, "message").map(|s| s.to_string()),
                ),
                _ => return,
            };
            let Some(text) = text else { return };
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            msgs.push(MessagePreview {
                role,
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
