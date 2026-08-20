use std::path::{Path, PathBuf};

use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, RawRef, ResumeSpec, Session};

/// 未确认格式的 harness 占位 adapter：探测安装目录，命中即报告“已检测”，
/// 枚举/解析交给 GenericAdapter 的启发式逻辑兜底。
pub struct PlaceholderAdapter {
    pub id: &'static str,
    pub display: &'static str,
    /// 相对 home 的候选目录（macOS / Windows 各列若干）
    pub candidates: &'static [&'static str],
}

impl PlaceholderAdapter {
    pub fn claude_desktop() -> Self {
        Self {
            id: "claude-desktop",
            display: "Claude Desktop",
            candidates: &[
                "Library/Application Support/Claude",
                "AppData/Roaming/Claude",
            ],
        }
    }
    pub fn kimi_code() -> Self {
        Self {
            id: "kimi-code",
            display: "Kimi Code",
            candidates: &[".kimi", "Library/Application Support/kimi", "AppData/Roaming/kimi"],
        }
    }
    pub fn openclaw() -> Self {
        Self {
            id: "openclaw",
            display: "OpenClaw",
            candidates: &[".openclaw", "Library/Application Support/openclaw"],
        }
    }
    pub fn hermes() -> Self {
        Self {
            id: "hermes",
            display: "Hermes",
            candidates: &[".hermes", "Library/Application Support/hermes"],
        }
    }
}

impl HarnessAdapter for PlaceholderAdapter {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        self.display
    }
    fn detect(&self, ctx: &DetectCtx) -> bool {
        self.candidates.iter().any(|c| ctx.join(c).is_dir())
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        self.candidates
            .iter()
            .map(|c| ctx.join(c))
            .filter(|p| p.is_dir())
            .collect()
    }
    fn enumerate(&self, _root: &Path, _ctx: &DetectCtx) -> Vec<RawRef> {
        Vec::new() // 格式待确认，由 GenericAdapter 兜底
    }
    fn parse(&self, _raw: &RawRef) -> Option<Session> {
        None
    }
    fn resume_spec(&self, _s: &Session) -> Option<ResumeSpec> {
        None
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

/// 兜底 adapter：对一组“长得像会话存储”的通用目录做启发式扫描。
pub struct GenericAdapter;

impl GenericAdapter {
    fn candidate_roots(ctx: &DetectCtx) -> Vec<PathBuf> {
        let rel = [
            ".claude-desktop",
            ".kimi/sessions",
            ".openclaw/sessions",
            ".hermes/sessions",
        ];
        rel.iter()
            .map(|r| ctx.join(r))
            .filter(|p| p.is_dir())
            .collect()
    }
}

impl HarnessAdapter for GenericAdapter {
    fn id(&self) -> &'static str {
        "generic"
    }
    fn name(&self) -> &'static str {
        "Generic（兜底）"
    }
    fn detect(&self, ctx: &DetectCtx) -> bool {
        !Self::candidate_roots(ctx).is_empty()
    }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
        Self::candidate_roots(ctx)
    }
    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> Vec<RawRef> {
        collect_files(root, &[".jsonl", ".json"])
            .into_iter()
            .filter_map(|p| file_raw_ref(&p))
            .collect()
    }
    fn parse(&self, raw: &RawRef) -> Option<Session> {
        // 启发式：尝试从文件头几行/顶层字段找 id、cwd、title；找不到就用文件名和 mtime
        let path = &raw.path;
        let mut id = file_stem(path);
        let mut title = String::new();
        let mut cwd = String::new();
        let mut first_ts: Option<i64> = None;
        let mut last_ts: Option<i64> = None;

        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let mut n = 0;
            for_each_jsonl_line(path, |v| {
                n += 1;
                if n > 200 {
                    return;
                }
                if let Some(s) = json_str(&v, "id").or_else(|| json_str(&v, "sessionId")) {
                    id = s.to_string();
                }
                if cwd.is_empty() {
                    if let Some(c) = json_str(&v, "cwd").or_else(|| json_str(&v, "projectPath")) {
                        cwd = c.to_string();
                    }
                }
                if let Some(ts) = json_str(&v, "timestamp").and_then(parse_iso_ms) {
                    if first_ts.is_none() {
                        first_ts = Some(ts);
                    }
                    last_ts = Some(ts);
                }
            });
        } else if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(s) = json_str(&v, "id").or_else(|| json_str(&v, "sessionId")) {
                    id = s.to_string();
                }
                if let Some(t) = json_str(&v, "title").or_else(|| json_str(&v, "summary")) {
                    title = t.to_string();
                }
                if let Some(c) = json_str(&v, "cwd").or_else(|| json_str(&v, "projectPath")) {
                    cwd = c.to_string();
                }
            }
        }

        Some(Session {
            session_id: id,
            harness_id: self.id().to_string(),
            project_path: cwd,
            title,
            started_at: first_ts.or(Some(raw.mtime_ms)),
            ended_at: last_ts.or(Some(raw.mtime_ms)),
            message_count: None,
            tokens_in: None,
            tokens_out: None,
            cost_usd: None,
            status: derive_status(raw.mtime_ms),
            raw_path: raw.path.to_string_lossy().into_owned(),
            source_format: "generic".to_string(),
            file_size: raw.size,
            file_mtime: raw.mtime_ms,
        })
    }
    fn resume_spec(&self, _s: &Session) -> Option<ResumeSpec> {
        None
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can_backup: true,
            ..Default::default()
        }
    }
}
