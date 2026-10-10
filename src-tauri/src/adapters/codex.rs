use std::path::{Path, PathBuf};

use super::claude_code::non_empty;
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session, UsageEvent};
use crate::pricing::Tokens;

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
                    raw.identity = Some(file_id(&p));
                    out.push(raw);
                }
                None => errors += 1,
            }
        }
        split_resumed_parts(&mut out);
        (out, errors)
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        let mut cwd = String::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts_raw = String::new();
        let mut msg_count: u32 = 0;
        let mut first_user_text: Option<String> = None;
        let mut model = String::new();
        let mut first_model: Option<String> = None;
        let mut usage = CodexUsage::default();
        let archived = raw.path.to_string_lossy().contains("archived_sessions");

        let ok = for_each_line(&raw.path, |line| {
            // 时间戳是每行的第一个字段：只记原文，最后再解析一次
            if let Some(ts) = leading_timestamp(line) {
                if first_ts.is_none() {
                    first_ts = parse_iso_ms(ts);
                }
                last_ts_raw.clear();
                last_ts_raw.push_str(ts);
            }
            // 预筛：函数调用 / 工具输出 / 推理块占了 rollout 的绝大部分体积，
            // 它们与统计无关，连 JSON 都不必解析
            if !is_relevant_line(line) {
                return;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return;
            };
            let ty = json_str(&v, "type").unwrap_or("");
            let payload = v.get("payload").unwrap_or(&serde_json::Value::Null);
            match ty {
                "session_meta" => {
                    if cwd.is_empty() {
                        if let Some(c) = json_str(payload, "cwd") {
                            cwd = c.to_string();
                        }
                    }
                    if model.is_empty() {
                        if let Some(m) = json_str(payload, "model") {
                            model = m.to_string();
                        }
                    }
                }
                "turn_context" => {
                    // 每轮的实际模型（会话中途可切换）
                    if let Some(m) = json_str(payload, "model") {
                        model = m.to_string();
                        if first_model.is_none() {
                            first_model = Some(model.clone());
                        }
                    }
                }
                "response_item" | "event_msg" => {
                    let pty = json_str(payload, "type").unwrap_or("");
                    match pty {
                        "message" => {
                            msg_count += 1;
                            let role = json_str(payload, "role").unwrap_or("");
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
                                if let Some(t) = json_str(payload, "message") {
                                    let t = t.trim();
                                    if !t.is_empty() && !t.starts_with('<') {
                                        first_user_text = Some(t.to_string());
                                    }
                                }
                            }
                        }
                        "token_count" => {
                            let ts = json_str(&v, "timestamp").and_then(parse_iso_ms);
                            // 实测结构：payload.info.{total_token_usage, last_token_usage}
                            let info = payload.get("info").unwrap_or(payload);
                            // turn_context 出现前模型未知，结束时回填（见 CodexUsage::finish）
                            let m = (!model.is_empty()).then_some(model.as_str());
                            usage.observe(info, ts.or(first_ts).unwrap_or(0), m);
                            if first_model.is_none() && !model.is_empty() {
                                first_model = Some(model.clone());
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

        // 索引键一律取文件名里的 id（与 enumerate 的 identity 一致）：Codex 的子线程文件
        // `rollout-<ts>-<uuid>_<线程>.jsonl` 的 session_meta 里是父会话的 id，
        // 用它当键会和父会话互相覆盖（用量丢失、扫描戳永远对不上而反复重解析）。
        // 续接产生的同名旧分段由 enumerate 改成 `<uuid>@<时间>`。
        let session_id = raw
            .identity
            .clone()
            .filter(|id| id.contains('@'))
            .unwrap_or_else(|| file_id(&raw.path));

        let mut session = Session {
            session_id,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title: first_user_text
                .as_deref()
                .map(|t| truncate(t, 80))
                .unwrap_or_default(),
            started_at: first_ts,
            ended_at: parse_iso_ms(&last_ts_raw).or(first_ts),
            message_count: if msg_count > 0 { Some(msg_count) } else { None },
            status: if archived {
                "archived".to_string()
            } else {
                derive_status(raw.mtime_ms)
            },
            raw_path: raw.path.to_string_lossy().into_owned(),
            source_format: "jsonl".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
            usage: usage.finish(first_model.as_deref()),
            ..Default::default()
        };
        session.apply_usage_totals();
        Some(session)
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        // 子线程文件 / 旧分段都续接到所属的 Codex 会话 uuid
        let id = s
            .session_id
            .split(['@', '_'])
            .next()
            .unwrap_or(&s.session_id);
        Some(ResumeSpec::new(
            vec!["codex".to_string(), "resume".to_string(), id.to_string()],
            non_empty(&s.project_path),
        ))
    }

    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec::new(
            vec!["codex".to_string()],
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

/// rollout-<19字符时间戳>-<id>.jsonl → <id>（子线程文件为 `<uuid>_<线程 id>`）
fn file_id(path: &Path) -> String {
    let stem = file_stem(path);
    stem.strip_prefix("rollout-")
        .and_then(|s| s.get(20..))
        .map(|s| s.to_string())
        .unwrap_or(stem)
}

/// Codex 续接（resume）会话时会新开一个 rollout 文件，沿用同一个会话 uuid，
/// 累计计数从上一个文件的终值继续。同 uuid 的多个文件如果共用一个索引键，
/// 只有最后写入的那个能留下（用量丢失），其余文件的扫描戳永远对不上、每次都被重解析。
/// 这里让最新的分段保留原 uuid（`codex resume <uuid>` 的目标），
/// 更早的分段改用 `<uuid>@<文件时间戳>`，每个文件各自入库、各自计量。
fn split_resumed_parts(raws: &mut [RawRef]) {
    let mut groups: std::collections::HashMap<String, Vec<usize>> = Default::default();
    for (i, r) in raws.iter().enumerate() {
        if let Some(id) = r.identity.as_deref() {
            groups.entry(id.to_string()).or_default().push(i);
        }
    }
    for (id, mut idx) in groups {
        if idx.len() < 2 {
            continue;
        }
        // 文件名以时间戳开头，字典序即时间序
        idx.sort_by_key(|&i| file_stem(&raws[i].path));
        idx.pop(); // 最新分段保留原 id
        for i in idx {
            let stamp = file_stem(&raws[i].path)
                .strip_prefix("rollout-")
                .and_then(|s| s.get(..19))
                .map(|s| s.to_string())
                .unwrap_or_default();
            raws[i].identity = Some(format!("{id}@{stamp}"));
        }
    }
}

/// `{"timestamp":"2026-…","type":…}`：Codex 每行的时间戳都在最前面，按前缀切出来即可
fn leading_timestamp(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("{\"timestamp\":\"")?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// 统计需要的行：会话元信息、每轮模型、消息、token 计数
fn is_relevant_line(line: &str) -> bool {
    line.contains("token_count")
        || line.contains("session_meta")
        || line.contains("turn_context")
        || line.contains("user_message")
        || line.contains("\"type\":\"message\"")
        || line.contains("\"type\": \"message\"")
}

/// Codex token 计数（四元组：输入（含缓存）、缓存读、缓存写、输出（含推理））
type Quad = (u64, u64, u64, u64);

fn read_quad(v: &serde_json::Value) -> Quad {
    (
        json_u64(v, "input_tokens").unwrap_or(0),
        json_u64(v, "cached_input_tokens").unwrap_or(0),
        json_u64(v, "cache_write_input_tokens").unwrap_or(0),
        json_u64(v, "output_tokens").unwrap_or(0),
    )
}

/// Codex 用量归集。
///
/// `total_token_usage` 是会话累计计数器；同一轮的 token_count 事件会重复写出，
/// 一轮里多次模型调用时 `last_token_usage` 又只反映最后一次——两种直接求和都不准
/// （本机实测：last 求和比累计值多 30%）。这里只计累计值越过历史最高点的增量，
/// 增量归到该事件的时间与当轮模型上。
///
/// 三个坑（均为本机实测）：
/// - 并发写入者交错写出略有差异的快照（1.1B 上下差几万）：必须忽略，
///   否则每次回退都会把整段累计值重算一遍（一个文件因此虚增 25 倍）；
/// - resume / fork 出来的新 rollout，第一个计数就带着父会话的累计值
///   （44 个文件、共 2.79B token 会被重复计入）：新计数段的第一个事件只计它自己这一轮的
///   `last_token_usage`，继承来的部分已经算在父会话里；
/// - 计数跌到峰值一半以下才视为真正的新计数段。
///
/// 只有整个文件都没有非零累计值时，才退回对 last 求和。
#[derive(Default)]
struct CodexUsage {
    /// 当前计数段的最高点
    high: Option<Quad>,
    /// (时间, 当时的模型——None 表示还没出现 turn_context, token)
    from_totals: Vec<(i64, Option<String>, Quad)>,
    from_last: Vec<(i64, Option<String>, Quad)>,
}

impl CodexUsage {
    fn event(ts: i64, model: &str, q: Quad) -> UsageEvent {
        let (input, cached, cache_write, output) = q;
        priced_event(
            ts,
            model,
            Tokens {
                input: input.saturating_sub(cached).saturating_sub(cache_write),
                output,
                cache_read: cached,
                cache_write,
                cache_write_1h: 0,
            },
            1.0,
        )
    }

    fn observe(&mut self, info: &serde_json::Value, ts: i64, model: Option<&str>) {
        let model = model.map(|m| m.to_string());
        let last = info
            .get("last_token_usage")
            .map(read_quad)
            .filter(|q| q.0 + q.3 > 0);
        if let Some(q) = last {
            self.from_last.push((ts, model.clone(), q));
        }
        let Some(total) = info.get("total_token_usage") else {
            return;
        };
        let t = read_quad(total);
        if t.0 + t.3 == 0 {
            return; // 全零快照：忽略
        }
        let delta = match self.high {
            // 只计越过最高点的部分；交错写入的旧快照增量为 0
            Some(h) if t.0 + t.3 >= (h.0 + h.3) / 2 => {
                let d = (
                    t.0.saturating_sub(h.0),
                    t.1.saturating_sub(h.1),
                    t.2.saturating_sub(h.2),
                    t.3.saturating_sub(h.3),
                );
                self.high = Some((h.0.max(t.0), h.1.max(t.1), h.2.max(t.2), h.3.max(t.3)));
                d
            }
            // 新计数段（文件开头 / 真正的重新累计）：只计本轮自己的用量，
            // 计数里继承自父会话的部分不重复计入
            _ => {
                self.high = Some(t);
                last.unwrap_or(t)
            }
        };
        if delta.0 + delta.3 > 0 {
            self.from_totals.push((ts, model, delta));
        }
    }

    /// 没出现 turn_context 之前的用量归到该会话第一次出现的模型；
    /// 整个文件都没有模型信息（早期 rollout）时按当时的默认模型 GPT-5 计价
    fn finish(self, session_model: Option<&str>) -> Vec<UsageEvent> {
        let raw = if self.from_totals.is_empty() {
            self.from_last
        } else {
            self.from_totals
        };
        let fallback = session_model.unwrap_or("gpt-5");
        raw.into_iter()
            .map(|(ts, m, q)| Self::event(ts, m.as_deref().unwrap_or(fallback), q))
            .collect()
    }
}

impl CodexAdapter {
    /// None = 源文件打不开（导出/迁移据此明确失败，而不是产出空内容）
    fn read_limited(
        &self,
        s: &Session,
        limit: usize,
        max_len: usize,
    ) -> Option<Vec<MessagePreview>> {
        let path = PathBuf::from(&s.raw_path);
        let mut msgs: Vec<MessagePreview> = Vec::new();
        if !for_each_line(&path, |line| {
            if !(line.contains("\"message\"")
                || line.contains("user_message")
                || line.contains("agent_message"))
            {
                return;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return;
            };
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

    /// 回归：重复的 token_count 事件不得重复计数；增量按当轮模型分别计价
    #[test]
    fn cumulative_deltas_attribute_model_per_turn() {
        let p = std::env::temp_dir().join(format!("sh-codex-delta-{}.jsonl", std::process::id()));
        let tc = |ts: &str, total: (u64, u64, u64)| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
                total.0, total.1, total.2, total.0, total.1, total.2
            )
        };
        let body = [
            r#"{"timestamp":"2026-05-01T08:00:00.000Z","type":"session_meta","payload":{"id":"sid-d","cwd":"/tmp/p"}}"#.to_string(),
            r#"{"timestamp":"2026-05-01T08:00:01.000Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_string(),
            tc("2026-05-01T08:00:02.000Z", (10_000, 8_000, 100)),
            // 同一累计值重复出现：不得再计一次
            tc("2026-05-01T08:00:03.000Z", (10_000, 8_000, 100)),
            // 并发写入者交错写出略小的旧快照：不得当成重新累计
            tc("2026-05-01T08:00:04.000Z", (9_990, 7_990, 99)),
            tc("2026-05-01T08:00:05.000Z", (10_000, 8_000, 100)),
            r#"{"timestamp":"2026-05-02T09:00:00.000Z","type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#.to_string(),
            tc("2026-05-02T09:00:01.000Z", (30_000, 26_000, 300)),
            r#"{"timestamp":"2026-05-02T09:00:02.000Z","type":"response_item","payload":{"type":"function_call_output","output":"token_count ignored"}}"#.to_string(),
        ]
        .join("\n");
        std::fs::write(&p, body).unwrap();

        let s = CodexAdapter.parse(&file_raw_ref(&p).unwrap()).unwrap();
        assert_eq!(s.usage.len(), 2);
        assert_eq!(s.usage[0].model, "gpt-5.4");
        assert_eq!((s.usage[0].input, s.usage[0].cache_read), (2_000, 8_000));
        assert_eq!(s.usage[1].model, "gpt-6.1-sol");
        assert_eq!(
            (s.usage[1].input, s.usage[1].cache_read, s.usage[1].output),
            (2_000, 18_000, 200)
        );
        assert_eq!(s.tokens_in, Some(30_000));
        assert_eq!(s.tokens_out, Some(300));
        assert_eq!(s.ended_at, parse_iso_ms("2026-05-02T09:00:02.000Z"));
        let _ = std::fs::remove_file(&p);
    }

    /// 真正的重新累计（fork / 恢复后计数器从小值开始）要整段计入
    #[test]
    fn genuine_counter_restart_counts_new_segment() {
        let mut u = CodexUsage::default();
        let info = |i: u64, o: u64| serde_json::json!({"total_token_usage": {"input_tokens": i, "cached_input_tokens": 0, "output_tokens": o}});
        u.observe(&info(100_000, 1_000), 1, Some("gpt-5"));
        u.observe(&info(120_000, 1_200), 2, Some("gpt-5"));
        u.observe(&info(5_000, 50), 3, Some("gpt-5"));
        u.observe(&info(7_000, 70), 4, Some("gpt-5"));
        let ev = u.finish(None);
        let input: u64 = ev.iter().map(|e| e.input).sum();
        let output: u64 = ev.iter().map(|e| e.output).sum();
        assert_eq!((input, output), (127_000, 1_270));
    }

    /// resume / fork 出来的 rollout：第一个计数带着父会话的累计值，只能计本轮自己的用量；
    /// turn_context 出现前的用量归到会话第一次出现的模型
    #[test]
    fn resumed_rollout_does_not_recount_inherited_total() {
        let mut u = CodexUsage::default();
        let info = |total: u64, last: u64| {
            serde_json::json!({
                "total_token_usage": {"input_tokens": total, "cached_input_tokens": 0, "output_tokens": 0},
                "last_token_usage": {"input_tokens": last, "cached_input_tokens": 0, "output_tokens": 0}
            })
        };
        u.observe(&info(5_000_000, 20_000), 1, None);
        u.observe(&info(5_030_000, 30_000), 2, Some("gpt-5.4"));
        let ev = u.finish(Some("gpt-5.4"));
        assert_eq!(ev.iter().map(|e| e.input).sum::<u64>(), 50_000);
        assert!(ev.iter().all(|e| e.model == "gpt-5.4"));
    }

    /// 回归：同一 uuid 的多个 rollout（续接产生）各自入库，最新分段保留原 id；
    /// 旧分段的续接命令仍指向原 uuid
    #[test]
    fn resumed_rollout_files_get_distinct_ids() {
        let dir = std::env::temp_dir().join(format!("sh-codex-parts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let uuid = "01a09638-db92-7dc1-a2a0-1580b3d91243";
        let meta = format!(
            r#"{{"timestamp":"2026-09-12T15:25:03.000Z","type":"session_meta","payload":{{"id":"{uuid}","cwd":"/tmp/p"}}}}"#
        );
        for ts in ["2026-09-12T23-25-03", "2026-10-03T23-17-45"] {
            std::fs::write(dir.join(format!("rollout-{ts}-{uuid}.jsonl")), &meta).unwrap();
        }
        let a = CodexAdapter;
        let (raws, errs) = a.enumerate(&dir, &crate::adapters::DetectCtx::new());
        assert_eq!(errs, 0);
        let mut ids: Vec<String> = raws
            .iter()
            .map(|r| a.parse(r).unwrap().session_id)
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![uuid.to_string(), format!("{uuid}@2026-09-12T23-25-03")]
        );
        let old = raws
            .iter()
            .map(|r| a.parse(r).unwrap())
            .find(|s| s.session_id.contains('@'))
            .unwrap();
        assert_eq!(a.resume_spec(&old).unwrap().argv[2], uuid);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：子线程 rollout（`<uuid>_<线程>`）的 session_meta 写的是父会话 id，
    /// 仍必须作为独立条目入库，续接命令指向父会话
    #[test]
    fn thread_rollouts_do_not_collide_with_parent() {
        let dir = std::env::temp_dir().join(format!("sh-codex-thread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parent = "01a09638-db92-7dc1-a2a0-1580b3d91243";
        let meta = format!(
            r#"{{"timestamp":"2026-09-12T15:25:03.000Z","type":"session_meta","payload":{{"id":"{parent}","cwd":"/tmp/p"}}}}"#
        );
        std::fs::write(
            dir.join(format!("rollout-2026-09-12T23-25-03-{parent}.jsonl")),
            &meta,
        )
        .unwrap();
        std::fs::write(
            dir.join(format!(
                "rollout-2026-10-04T09-45-27-{parent}_01a10496-66be-7fc1-9296-584751e771d1.jsonl"
            )),
            &meta,
        )
        .unwrap();
        let a = CodexAdapter;
        let (raws, _) = a.enumerate(&dir, &crate::adapters::DetectCtx::new());
        let sessions: Vec<Session> = raws.iter().map(|r| a.parse(r).unwrap()).collect();
        let mut ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 2, "父会话与子线程必须是两个索引键：{ids:?}");
        for s in &sessions {
            assert_eq!(a.resume_spec(s).unwrap().argv[2], parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
