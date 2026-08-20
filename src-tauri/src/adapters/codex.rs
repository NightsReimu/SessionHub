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
    if out.is_empty() {
        None
    } else {
        Some(out)
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
        ctx.join(".codex/sessions").is_dir() || ctx.join(".codex/archived_sessions").is_dir()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        // 只返回实际存在的根：archived_sessions 是可选目录，尚未创建时
        // 不能让它变成“遍历错误”而永久阻断全量扫描的 prune
        [
            ctx.join(".codex/sessions"),
            ctx.join(".codex/archived_sessions"),
        ]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
    }

    /// sessions 是主存储根：它在 detect 之后若消失（被改名/移动），
    /// 必须让全量扫描视为错误，否则 archived 可读就会误 prune 活跃会话
    fn required_roots_missing(&self, ctx: &DetectCtx) -> usize {
        if ctx.join(".codex/sessions").is_dir() {
            0
        } else {
            1
        }
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
        // total_token_usage 是累计快照（取最后一个非零值，忽略全零记录）；
        // last_token_usage 是单轮增量（全文件求和作为兜底）
        let mut cumulative: Option<(u64, u64, u64)> = None;
        let mut turn_sum = (0u64, 0u64, 0u64);
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
                                if let Some(t) = payload.get("content").and_then(extract_codex_text)
                                {
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
                            // 实测结构：payload.info.{total_token_usage, last_token_usage}
                            let info = payload.get("info").unwrap_or(&payload);
                            if let Some(t) = info.get("total_token_usage") {
                                let v = (
                                    json_u64(t, "input_tokens").unwrap_or(0),
                                    json_u64(t, "cached_input_tokens").unwrap_or(0),
                                    json_u64(t, "output_tokens").unwrap_or(0),
                                );
                                if v.0 + v.2 > 0 {
                                    cumulative = Some(v);
                                }
                            }
                            if let Some(t) = info.get("last_token_usage") {
                                turn_sum.0 += json_u64(t, "input_tokens").unwrap_or(0);
                                turn_sum.1 += json_u64(t, "cached_input_tokens").unwrap_or(0);
                                turn_sum.2 += json_u64(t, "output_tokens").unwrap_or(0);
                            }
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

        // 优先累计快照；快照全零时退回单轮求和
        let (tin, tcached, tout) = match cumulative {
            Some(c) if c.0 + c.2 > 0 => c,
            _ => turn_sum,
        };
        let tokens_in = if tin > 0 { Some(tin) } else { None };
        let tokens_out = if tout > 0 { Some(tout) } else { None };

        // GPT-5 系刊例价：普通输入 $1.25/M、缓存输入 $0.125/M、输出 $10/M
        let cost = {
            let usd = (tin.saturating_sub(tcached) as f64 * 1.25
                + tcached as f64 * 0.125
                + tout as f64 * 10.0)
                / 1e6;
            if usd > 0.0 {
                Some(usd)
            } else {
                None
            }
        };

        Some(Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title: first_user_text
                .as_deref()
                .map(|t| truncate(t, 80))
                .unwrap_or_default(),
            started_at: first_ts,
            ended_at: last_ts,
            message_count: if msg_count > 0 { Some(msg_count) } else { None },
            tokens_in,
            tokens_out,
            cost_usd: cost,
            status: if archived {
                "archived".to_string()
            } else {
                derive_status(raw.mtime_ms)
            },
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

    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: "codex".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// archived_sessions 是可选目录：不存在时既不算检测失败，也不能成为
    /// “遍历错误”阻断 prune；两个根都不存在时才算未安装
    #[test]
    fn optional_roots_filtered_and_detect_requires_any() {
        let home = std::env::temp_dir().join(format!("sh-codex-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let ctx = DetectCtx { home: home.clone() };
        let a = CodexAdapter;

        assert!(!a.detect(&ctx));
        assert!(a.roots(&ctx).is_empty());

        std::fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        assert!(a.detect(&ctx));
        assert_eq!(a.roots(&ctx).len(), 1, "archived 不存在时不得成为根");

        std::fs::create_dir_all(home.join(".codex/archived_sessions")).unwrap();
        assert_eq!(a.roots(&ctx).len(), 2);

        let _ = std::fs::remove_dir_all(&home);
    }

    /// token 解析：忽略全零 total 快照，用最后一个非零累计值；
    /// 缓存输入按折扣价计费；累计全零时退回 last 求和
    #[test]
    fn token_count_prefers_last_nonzero_cumulative() {
        let p = std::env::temp_dir().join(format!("sh-codex-tok-{}.jsonl", std::process::id()));
        let line = |total: (u64, u64, u64), last: (u64, u64, u64)| {
            format!(
                r#"{{"timestamp":"2026-01-01T00:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                total.0, total.1, total.2, last.0, last.1, last.2
            )
        };
        let mut body = String::from(
            r#"{"timestamp":"2026-01-01T00:00:00.000Z","type":"session_meta","payload":{"id":"sid-9","cwd":"/tmp/p"}}"#,
        );
        body.push('\n');
        body.push_str(&line((0, 0, 0), (9804, 9000, 100)));
        body.push('\n');
        body.push_str(&line((50000, 40000, 1000), (0, 0, 0)));
        body.push('\n');
        std::fs::write(&p, body).unwrap();

        let a = CodexAdapter;
        let s = a.parse(&file_raw_ref(&p).unwrap()).unwrap();
        assert_eq!(s.tokens_in, Some(50000));
        assert_eq!(s.tokens_out, Some(1000));
        // 未缓存 10000×$1.25/M + 缓存 40000×$0.125/M + 输出 1000×$10/M
        // = 0.0125 + 0.005 + 0.01 = 0.0275
        let cost = s.cost_usd.expect("应估算费用");
        assert!((cost - 0.0275).abs() < 1e-9, "cost = {cost}");

        let _ = std::fs::remove_file(&p);
    }

    /// 累计快照全零 → 退回 last_token_usage 求和
    #[test]
    fn token_count_falls_back_to_turn_sum() {
        let p = std::env::temp_dir().join(format!("sh-codex-tok2-{}.jsonl", std::process::id()));
        let line = |total: (u64, u64, u64), last: (u64, u64, u64)| {
            format!(
                r#"{{"timestamp":"2026-01-01T00:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                total.0, total.1, total.2, last.0, last.1, last.2
            )
        };
        let mut body = String::from(
            r#"{"timestamp":"2026-01-01T00:00:00.000Z","type":"session_meta","payload":{"id":"sid-8","cwd":"/tmp/p"}}"#,
        );
        body.push('\n');
        body.push_str(&line((0, 0, 0), (5000, 1000, 200)));
        body.push('\n');
        body.push_str(&line((0, 0, 0), (4000, 0, 300)));
        body.push('\n');
        std::fs::write(&p, body).unwrap();

        let a = CodexAdapter;
        let s = a.parse(&file_raw_ref(&p).unwrap()).unwrap();
        assert_eq!(s.tokens_in, Some(9000));
        assert_eq!(s.tokens_out, Some(500));

        let _ = std::fs::remove_file(&p);
    }
}
