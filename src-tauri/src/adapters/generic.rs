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
    fn enumerate(&self, _root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        (Vec::new(), 0) // 格式待确认，由 GenericAdapter 兜底
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
/// 另外支持免重编译的“配置插件”：在 ~/SessionHub/adapters.json 里写
/// `{ "generic_extra_roots": ["~/some/dir", "/abs/path"] }`，
/// 这些目录会被同样的启发式规则扫描。
pub struct GenericAdapter;

impl GenericAdapter {
    fn candidate_roots(ctx: &DetectCtx) -> Vec<PathBuf> {
        let rel = [
            ".claude-desktop",
            ".kimi/sessions",
            ".openclaw/sessions",
            ".hermes/sessions",
        ];
        let mut roots: Vec<PathBuf> = rel
            .iter()
            .map(|r| ctx.join(r))
            .filter(|p| p.is_dir())
            .collect();
        roots.extend(Self::custom_roots_from(&ctx.home.join("SessionHub/adapters.json")));
        roots.sort();
        roots.dedup();
        roots
    }

    /// 解析配置文件里的自定义根目录（支持 ~ 展开；文件缺失/损坏时静默忽略）
    fn custom_roots_from(cfg_path: &Path) -> Vec<PathBuf> {
        let Ok(text) = std::fs::read_to_string(cfg_path) else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        v.get("generic_extra_roots")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| {
                        if let Some(rest) = s.strip_prefix("~/") {
                            dirs::home_dir()
                                .map(|h| h.join(rest))
                                .unwrap_or_else(|| PathBuf::from(s))
                        } else {
                            PathBuf::from(s)
                        }
                    })
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
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
    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        let (files, mut errors) = collect_files(root, &[".jsonl", ".json"]);
        let mut out = Vec::new();
        for p in files {
            match file_raw_ref(&p) {
                Some(raw) => out.push(raw),
                None => errors += 1,
            }
        }
        (out, errors)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// adapters.json 自定义根目录：存在的目录被采纳、不存在/配置损坏静默忽略
    #[test]
    fn custom_roots_from_config() {
        let base = std::env::temp_dir().join(format!("sh-generic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let extra = base.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        let cfg = base.join("adapters.json");
        std::fs::write(
            &cfg,
            format!(r#"{{"generic_extra_roots":["{}", "/no/such/dir-xyz"]}}"#, extra.display()),
        )
        .unwrap();
        assert_eq!(GenericAdapter::custom_roots_from(&cfg), vec![extra.clone()]);

        std::fs::write(&cfg, "not json").unwrap();
        assert!(GenericAdapter::custom_roots_from(&cfg).is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }
}
