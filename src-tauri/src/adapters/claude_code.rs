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

    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        let mut out = Vec::new();
        let mut errors = 0usize;
        let dirs = match std::fs::read_dir(root) {
            Ok(d) => d,
            Err(_) => return (out, 1),
        };
        for dir in dirs {
            let dir = match dir {
                Ok(d) => d,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let project_dir = dir.path();
            if !project_dir.is_dir() {
                continue;
            }
            let index = Self::read_index(&project_dir);
            let files = match std::fs::read_dir(&project_dir) {
                Ok(f) => f,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            for f in files {
                let f = match f {
                    Ok(f) => f,
                    Err(_) => {
                        errors += 1;
                        continue;
                    }
                };
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                match file_raw_ref(&path) {
                    Some(mut raw) => {
                        let key = path.to_string_lossy().into_owned();
                        raw.inline = index.get(&key).cloned();
                        out.push(raw);
                    }
                    None => errors += 1,
                }
            }
        }
        (out, errors)
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let mut session_id = file_stem(&raw.path);
        let mut cwd = String::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;
        let mut msg_count: u32 = 0;
        let mut first_user_text: Option<String> = None;
        let mut model: Option<String> = None;
        let (mut tin, mut tout, mut tcw, mut tcr) = (0u64, 0u64, 0u64, 0u64);

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
                    if let Some(m) = v.get("message") {
                        if model.is_none() {
                            model = json_str(m, "model").map(|s| s.to_string());
                        }
                        if let Some(u) = m.get("usage") {
                            tin += json_u64(u, "input_tokens").unwrap_or(0);
                            tout += json_u64(u, "output_tokens").unwrap_or(0);
                            tcw += json_u64(u, "cache_creation_input_tokens").unwrap_or(0);
                            tcr += json_u64(u, "cache_read_input_tokens").unwrap_or(0);
                        }
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

        let tokens_in = tin + tcw + tcr;
        Some(Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path,
            title,
            started_at: started,
            ended_at: ended,
            message_count: count,
            tokens_in: if tokens_in > 0 { Some(tokens_in) } else { None },
            tokens_out: if tout > 0 { Some(tout) } else { None },
            cost_usd: claude_cost(model.as_deref(), tin, tout, tcw, tcr),
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

    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: "claude".to_string(),
            cwd: non_empty(&s.project_path),
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_resume: true,
            can_delete: true,
            can_backup: true,
            can_read_messages: true,
            can_launch: true,
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

/// 按模型价格表估算费用（美元/百万 token：输入, 输出, 缓存写, 缓存读）。
/// 与 ccusage 同一路径：token 数 × 公开刊例价。
fn claude_cost(model: Option<&str>, tin: u64, tout: u64, tcw: u64, tcr: u64) -> Option<f64> {
    if tin + tout + tcw + tcr == 0 {
        return None;
    }
    let m = model.unwrap_or("").to_lowercase();
    let (pi, po, pcw, pcr) = if m.contains("opus") {
        (15.0, 75.0, 18.75, 1.5)
    } else if m.contains("haiku") {
        (1.0, 5.0, 1.25, 0.1)
    } else {
        (3.0, 15.0, 3.75, 0.3) // sonnet 系默认
    };
    let usd = (tin as f64 * pi + tout as f64 * po + tcw as f64 * pcw + tcr as f64 * pcr) / 1e6;
    if usd > 0.0 { Some(usd) } else { None }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 流式解析：坏行跳过、title 取首条用户文本、token 累加、时间取首尾
    #[test]
    fn parses_streaming_jsonl() {
        let p = std::env::temp_dir().join(format!("sh-claude-{}.jsonl", std::process::id()));
        std::fs::write(
            &p,
            concat!(
                r#"{"type":"queue-operation","timestamp":"2026-01-01T00:00:00.000Z","sessionId":"sid-1"}"#,
                "\n",
                r#"{"type":"user","cwd":"/tmp/proj","timestamp":"2026-01-01T00:00:01.000Z","sessionId":"sid-1","message":{"role":"user","content":[{"type":"text","text":"hello world"}]}}"#,
                "\n",
                "这不是 JSON，必须被跳过\n",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02.000Z","sessionId":"sid-1","message":{"role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":90}}}"#,
                "\n",
            ),
        )
        .unwrap();
        let raw = file_raw_ref(&p).unwrap();
        let a = ClaudeCodeAdapter;
        let s = a.parse(&raw).unwrap();
        assert_eq!(s.session_id, "sid-1");
        assert_eq!(s.project_path, "/tmp/proj");
        assert_eq!(s.title, "hello world");
        assert_eq!(s.message_count, Some(2));
        assert_eq!(s.tokens_in, Some(100));
        assert_eq!(s.tokens_out, Some(5));
        // sonnet 刊例：10×$3/M + 5×$15/M + 90×$0.3/M = $0.000132
        let cost = s.cost_usd.expect("应按价格表估算费用");
        assert!((cost - 0.000132).abs() < 1e-9, "cost = {cost}");
        assert!(s.started_at.is_some());
        assert!(s.ended_at >= s.started_at);
        let msgs = a.read_messages(&s, 10);
        assert_eq!(msgs.len(), 2);
        let _ = std::fs::remove_file(&p);
    }
}
