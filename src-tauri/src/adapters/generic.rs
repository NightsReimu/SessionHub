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
            candidates: &[
                ".kimi",
                "Library/Application Support/kimi",
                "AppData/Roaming/kimi",
            ],
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
    /// FNV-1a 64：无依赖、跨进程/跨版本稳定的哈希
    fn fnv1a64(s: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// 内容里找不到 id 时的兜底：文件名 + 全路径哈希做命名空间，
    /// 避免两个配置目录里的同名 session.jsonl 互相覆盖（主键是 (harness, id)）
    fn fallback_id(path: &Path) -> String {
        format!(
            "{}-{:016x}",
            file_stem(path),
            Self::fnv1a64(&path.to_string_lossy())
        )
    }

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
        roots.extend(Self::custom_roots_from(
            &ctx.home.join("SessionHub/adapters.json"),
        ));
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
                Some(mut raw) => {
                    raw.identity = Some(Self::fallback_id(&p));
                    out.push(raw);
                }
                None => errors += 1,
            }
        }
        (out, errors)
    }
    fn parse(&self, raw: &RawRef) -> Option<Session> {
        // 启发式提取 cwd/title/时间；id 一律用路径命名空间的稳定值——
        // 内容里的 id 无法保证跨目录唯一，且必须与 enumerate 的 identity 一致，
        // 否则增量扫描永远命中不了扫描戳
        let path = &raw.path;
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
                if let Some(t) = json_str(&v, "title").or_else(|| json_str(&v, "summary")) {
                    title = t.to_string();
                }
                if let Some(c) = json_str(&v, "cwd").or_else(|| json_str(&v, "projectPath")) {
                    cwd = c.to_string();
                }
            }
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        Some(Session {
            session_id: Self::fallback_id(path),
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
            source_format: if ext == "jsonl" { "jsonl" } else { "json" }.to_string(),
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
            format!(
                r#"{{"generic_extra_roots":["{}", "/no/such/dir-xyz"]}}"#,
                extra.display()
            ),
        )
        .unwrap();
        assert_eq!(GenericAdapter::custom_roots_from(&cfg), vec![extra.clone()]);

        std::fs::write(&cfg, "not json").unwrap();
        assert!(GenericAdapter::custom_roots_from(&cfg).is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 两个目录里的同名 session.jsonl（内容无 id）必须得到不同的 session_id
    #[test]
    fn same_stem_files_get_namespaced_ids() {
        let base = std::env::temp_dir().join(format!("sh-generic-ns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (d1, d2) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        let (f1, f2) = (d1.join("session.jsonl"), d2.join("session.jsonl"));
        std::fs::write(&f1, "{\"timestamp\":\"2026-01-01T00:00:00Z\"}\n").unwrap();
        std::fs::write(&f2, "{\"timestamp\":\"2026-01-01T00:00:00Z\"}\n").unwrap();

        let a = GenericAdapter;
        let s1 = a.parse(&file_raw_ref(&f1).unwrap()).unwrap();
        let s2 = a.parse(&file_raw_ref(&f2).unwrap()).unwrap();
        assert_ne!(
            s1.session_id, s2.session_id,
            "同名无 ID 文件不得共享 session_id"
        );
        assert_eq!(s1.raw_path, f1.to_string_lossy());
        assert_eq!(s2.raw_path, f2.to_string_lossy());

        // 同一文件 id 稳定（重扫描不会产生新会话）
        let s1b = a.parse(&file_raw_ref(&f1).unwrap()).unwrap();
        assert_eq!(s1.session_id, s1b.session_id);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 即使内容里写了相同的 id，跨目录也必须得到不同 session_id；
    /// 且 session_id 与 enumerate 的 identity 一致（增量扫描命中的前提）
    #[test]
    fn content_ids_are_not_trusted() {
        let base = std::env::temp_dir().join(format!("sh-generic-cid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (d1, d2) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        let body = "{\"id\":\"same-id\",\"timestamp\":\"2026-01-01T00:00:00Z\"}\n";
        let (f1, f2) = (d1.join("x.jsonl"), d2.join("x.jsonl"));
        std::fs::write(&f1, body).unwrap();
        std::fs::write(&f2, body).unwrap();

        let a = GenericAdapter;
        let raw1 = file_raw_ref(&f1).unwrap();
        let s1 = a.parse(&raw1).unwrap();
        let s2 = a.parse(&file_raw_ref(&f2).unwrap()).unwrap();
        assert_ne!(s1.session_id, s2.session_id, "内容 id 相同也必须按路径区分");

        // identity 一致性：enumerate 产出的 identity 必须等于 parse 的 session_id
        let (raws, _) = a.enumerate(&base, &DetectCtx { home: base.clone() });
        for raw in &raws {
            let s = a.parse(raw).unwrap();
            assert_eq!(raw.identity.as_deref(), Some(s.session_id.as_str()));
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
