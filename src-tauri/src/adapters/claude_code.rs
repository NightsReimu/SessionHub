use std::path::{Path, PathBuf};

use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};
use crate::pricing::Tokens;

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
        let mut usage = UsageCollector::default();

        let ok = for_each_line(&raw.path, |line| {
            // 轻量结构化解析：message.content（工具输出的大头）被跳过不建树
            let Ok(l) = serde_json::from_str::<CcLine>(line) else {
                return;
            };
            if let Some(sid) = l.session_id.as_deref().filter(|s| !s.is_empty()) {
                if sid != session_id {
                    session_id = sid.to_string();
                }
            }
            if cwd.is_empty() {
                if let Some(c) = l.cwd.as_deref() {
                    cwd = c.to_string();
                }
            }
            let ts = l.timestamp.as_deref().and_then(parse_iso_ms);
            if let Some(ts) = ts {
                if first_ts.is_none_or(|f| ts < f) {
                    first_ts = Some(ts);
                }
                if last_ts.is_none_or(|x| ts > x) {
                    last_ts = Some(ts);
                }
            }
            match l.ty.as_deref() {
                Some("user") => {
                    msg_count += 1;
                    if first_user_text.is_none() {
                        // 只有找标题时才把整行解析成 Value
                        if let Some(t) = serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .and_then(|v| {
                                v.get("message")
                                    .and_then(|m| m.get("content"))
                                    .and_then(extract_claude_text)
                            })
                        {
                            let t = t.trim().to_string();
                            // 跳过系统注入的命令回显等
                            if !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:") {
                                first_user_text = Some(t);
                            }
                        }
                    }
                }
                Some("assistant") => {
                    msg_count += 1;
                    usage.push(&l, ts.or(last_ts));
                }
                _ => {}
            }
        });
        if !ok {
            return None;
        }

        // 子代理（Task 工具）的对话单独落在 <会话 id>/subagents/*.jsonl，
        // 它们的 token 同样计费，归到父会话名下
        if let Some(parent) = raw.path.parent() {
            let sub_dir = parent.join(file_stem(&raw.path)).join("subagents");
            if let Ok(entries) = std::fs::read_dir(&sub_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                        continue;
                    }
                    for_each_line(&p, |line| {
                        if !line.contains("\"assistant\"") {
                            return;
                        }
                        if let Ok(l) = serde_json::from_str::<CcLine>(line) {
                            if l.ty.as_deref() == Some("assistant") {
                                let ts = l.timestamp.as_deref().and_then(parse_iso_ms);
                                usage.push(&l, ts.or(last_ts));
                            }
                        }
                    });
                }
            }
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

        let mut session = Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path,
            title,
            started_at: started,
            ended_at: ended,
            message_count: count,
            status: derive_status(raw.mtime_ms),
            raw_path: raw.path.to_string_lossy().into_owned(),
            source_format: "jsonl".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
            usage: usage.events,
            ..Default::default()
        };
        session.apply_usage_totals();
        Some(session)
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec::new(
            vec![
                "claude".to_string(),
                "--resume".to_string(),
                s.session_id.clone(),
            ],
            non_empty(&s.project_path),
        ))
    }

    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec::new(
            vec!["claude".to_string()],
            non_empty(&s.project_path),
        ))
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
        self.read_limited(s, limit, 2000).unwrap_or_default()
    }

    fn read_messages_full(&self, s: &Session) -> Option<Vec<MessagePreview>> {
        self.read_limited(s, usize::MAX, usize::MAX)
    }
}

impl ClaudeCodeAdapter {
    /// None = 源文件打不开（导出/迁移据此明确失败，而不是产出空内容）
    fn read_limited(
        &self,
        s: &Session,
        limit: usize,
        max_len: usize,
    ) -> Option<Vec<MessagePreview>> {
        let path = PathBuf::from(&s.raw_path);
        let mut msgs: Vec<MessagePreview> = Vec::new();
        if !for_each_jsonl_line(&path, |v| {
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
                text: truncate(&text, max_len),
                timestamp: json_str(&v, "timestamp").and_then(parse_iso_ms),
            });
        }) {
            return None;
        }
        if msgs.len() > limit {
            msgs = msgs.split_off(msgs.len() - limit);
        }
        Some(msgs)
    }
}

/// Claude Code JSONL 的一行（只取统计需要的字段，其余由 serde 直接跳过）
#[derive(serde::Deserialize)]
struct CcLine {
    #[serde(rename = "type")]
    ty: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    message: Option<CcMessage>,
}

#[derive(serde::Deserialize)]
struct CcMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<CcUsage>,
}

#[derive(serde::Deserialize, Default)]
struct CcUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CcCacheCreation>,
    speed: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct CcCacheCreation {
    ephemeral_1h_input_tokens: Option<u64>,
}

/// message 字段形态不符（如字符串）时当作缺失，而不是让整行解析失败
fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// 收集 assistant 用量并去重。
///
/// Claude Code 把一条回复按内容块（thinking / text / tool_use）拆成多行写入，
/// 每行都带着同一份 usage；直接累加会把用量放大 2~3 倍。
/// 与 ccusage 相同，按 `message.id + requestId` 去重，只计第一次出现。
#[derive(Default)]
struct UsageCollector {
    seen: std::collections::HashSet<String>,
    events: Vec<crate::models::UsageEvent>,
}

impl UsageCollector {
    fn push(&mut self, l: &CcLine, ts: Option<i64>) {
        let Some(m) = l.message.as_ref() else { return };
        let Some(u) = m.usage.as_ref() else { return };
        if let (Some(id), Some(req)) = (m.id.as_deref(), l.request_id.as_deref()) {
            if !self.seen.insert(format!("{id}:{req}")) {
                return;
            }
        } else if let Some(id) = m.id.as_deref() {
            if !self.seen.insert(id.to_string()) {
                return;
            }
        }
        let model = m.model.as_deref().unwrap_or("");
        let cache_write = u.cache_creation_input_tokens.unwrap_or(0);
        let t = Tokens {
            input: u.input_tokens.unwrap_or(0),
            output: u.output_tokens.unwrap_or(0),
            cache_read: u.cache_read_input_tokens.unwrap_or(0),
            cache_write,
            // Claude Code 默认用 1 小时缓存（2× 输入价），按明细分开计费
            cache_write_1h: u
                .cache_creation
                .as_ref()
                .and_then(|c| c.ephemeral_1h_input_tokens)
                .unwrap_or(0)
                .min(cache_write),
        };
        if t.input + t.output + t.cache_read + t.cache_write == 0 {
            return;
        }
        let mult = if u.speed.as_deref() == Some("fast") {
            crate::pricing::fast_multiplier(model)
        } else {
            1.0
        };
        self.events
            .push(priced_event(ts.unwrap_or(0), model, t, mult));
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
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03.000Z","sessionId":"sid-1","message":{"role":"assistant","model":"claude-opus-4-1","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1000,"output_tokens":100}}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:04.000Z","sessionId":"sid-1","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":7,"output_tokens":3}}}"#,
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
        assert_eq!(s.tokens_in, Some(10 + 90 + 1000 + 7));
        assert_eq!(s.tokens_out, Some(5 + 100 + 3));
        // sonnet：10×$3/M + 5×$15/M + 90×$0.3/M = $0.000132
        // opus：1000×$15/M + 100×$75/M = $0.0225
        // 无 model 行的 10 个 token 不计费
        let cost = s.cost_usd.expect("应按模型分别估算费用");
        assert!((cost - (0.000132 + 0.0225)).abs() < 1e-9, "cost = {cost}");
        assert_eq!(s.message_count, Some(4));
        assert!(s.started_at.is_some());
        assert!(s.ended_at >= s.started_at);
        let msgs = a.read_messages(&s, 10);
        assert_eq!(msgs.len(), 4);
        let _ = std::fs::remove_file(&p);
    }

    /// 回归：同一条回复按内容块拆成多行，usage 只能计一次；
    /// 1 小时缓存写入按 2× 输入价；子代理文件的用量归入父会话
    #[test]
    fn dedups_content_block_lines_and_counts_subagents() {
        let dir = std::env::temp_dir().join(format!("sh-claude-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sid-2/subagents")).unwrap();
        let line = |ty: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-03-01T10:00:00.000Z","sessionId":"sid-2","requestId":"req_1","message":{{"id":"msg_1","model":"claude-opus-5-5","content":[{{"type":"{ty}","text":"x"}}],"usage":{{"input_tokens":2,"output_tokens":100,"cache_creation_input_tokens":1000,"cache_read_input_tokens":50000,"cache_creation":{{"ephemeral_1h_input_tokens":1000,"ephemeral_5m_input_tokens":0}},"speed":"standard"}}}}}}"#
            )
        };
        let main = dir.join("sid-2.jsonl");
        std::fs::write(
            &main,
            [line("thinking"), line("text"), line("tool_use")].join("\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("sid-2/subagents/agent-a.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-03-01T10:05:00.000Z","isSidechain":true,"requestId":"req_9","message":{"id":"msg_9","model":"claude-haiku-4-5","content":[],"usage":{"input_tokens":1000,"output_tokens":10}}}"#,
        )
        .unwrap();

        let s = ClaudeCodeAdapter
            .parse(&file_raw_ref(&main).unwrap())
            .unwrap();
        assert_eq!(s.usage.len(), 2, "三行同一回复只计一次 + 一条子代理");
        let main_ev = &s.usage[0];
        assert_eq!(main_ev.output, 100);
        assert_eq!(main_ev.cache_read, 50000);
        // opus-5-5：2×$4 + 100×$20 + 1000×$8(1h) + 50000×$0.2 = $0.020008
        assert!(
            (main_ev.cost_usd - 0.020008).abs() < 1e-9,
            "{}",
            main_ev.cost_usd
        );
        assert_eq!(s.tokens_out, Some(110));
        assert_eq!(s.tokens_in, Some(2 + 1000 + 50000 + 1000));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
