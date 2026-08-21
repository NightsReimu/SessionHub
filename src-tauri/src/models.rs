use serde::{Deserialize, Serialize};

/// 归一化后的统一会话模型。所有 adapter 都把各自格式转成这个结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub harness_id: String,
    pub project_path: String,
    pub title: String,
    /// ms epoch
    pub started_at: Option<i64>,
    /// ms epoch，最后一次活动时间
    pub ended_at: Option<i64>,
    pub message_count: Option<u32>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub cost_usd: Option<f64>,
    pub status: String,
    pub raw_path: String,
    pub source_format: String,
    pub file_size: u64,
    pub file_mtime: i64,
}

/// 只写入 SessionHub 自己数据库的用户数据，绝不触碰 harness 文件。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMeta {
    pub tags: Vec<String>,
    pub note: String,
    pub favorite: bool,
    /// 用户自定义标题；None 时显示 harness 解析出的原标题
    #[serde(default)]
    pub custom_title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDto {
    #[serde(flatten)]
    pub session: Session,
    pub meta: SessionMeta,
    /// raw_path 是否指向该会话的独立存储（false = 回退到共享/全局文件，
    /// 前端据此禁用删除/备份/定位按钮）
    pub raw_usable: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Capabilities {
    pub can_resume: bool,
    pub can_delete: bool,
    pub can_backup: bool,
    pub can_read_messages: bool,
    pub can_launch: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResumeSpec {
    /// 仅用于展示/回显的命令文本，例如 `claude --resume <id>`。
    /// 绝不能把它交给 shell 执行——执行一律走 `argv`。
    pub command: String,
    /// 真正执行用的 argv（program + 参数）。由调用方逐元素按目标平台引用规则
    /// 转义，避免会话 id、路径中的 shell 元字符被解释成命令。
    pub argv: Vec<String>,
    pub cwd: Option<String>,
}

impl ResumeSpec {
    /// 由 argv 构造；`command` 逐元素 POSIX 引用后拼出，仅供展示。
    pub fn new(argv: Vec<String>, cwd: Option<String>) -> Self {
        let command = argv
            .iter()
            .map(|a| posix_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        Self { command, argv, cwd }
    }
}

/// POSIX shell 单引号转义：安全字符原样输出，其余整体单引号包裹并转义内部单引号。
pub fn posix_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// PowerShell 单引号转义：单引号字符串内除 `'` 本身外无转义语义，`'` 写作 `''`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn powershell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 一个待解析的原始对象。对文件型 harness 就是磁盘上的文件；
/// 对 SQLite/索引型 harness，`inline` 直接携带行数据（防御式：避免二次查询）。
#[derive(Debug, Clone)]
pub struct RawRef {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    pub inline: Option<serde_json::Value>,
    /// 廉价 identity（文件 stem / 行 id），用于增量扫描时跳过昂贵 parse
    pub identity: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdapterInfo {
    pub id: String,
    pub name: String,
    pub detected: bool,
    pub roots: Vec<String>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdapterScanStat {
    pub adapter_id: String,
    pub detected: bool,
    pub scanned: usize,
    pub parsed: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub adapters: Vec<AdapterScanStat>,
    pub total_sessions: usize,
    pub duration_ms: u128,
}

/// 扫描进度事件（scan-progress）：先枚举得到 total，再按 done 推进
#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub adapter_id: String,
    /// 第几个可扫描 adapter（0 起）
    pub adapter_index: usize,
    /// 可扫描 adapter 总数
    pub adapter_count: usize,
    /// 当前 adapter 已处理条数（0 且 total=0 表示仍在枚举）
    pub done: usize,
    /// 当前 adapter 待处理总数
    pub total: usize,
    pub parsed: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessagePreview {
    pub role: String,
    pub text: String,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Counts {
    pub total: usize,
    pub favorites: usize,
    pub per_harness: std::collections::HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubPaths {
    pub hub_dir: String,
    pub backups_dir: String,
    pub exports_dir: String,
    pub db_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessStat {
    pub harness_id: String,
    pub sessions: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsOverview {
    pub total_sessions: usize,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_cost_usd: f64,
    pub per_harness: Vec<HarnessStat>,
    /// 按 token 消耗排序的会话（含 meta，可直接跳转）
    pub top_sessions: Vec<SessionDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quote_neutralizes_metacharacters() {
        // 安全字符原样保留
        assert_eq!(posix_quote("ses_abc-123.jsonl"), "ses_abc-123.jsonl");
        assert_eq!(posix_quote("/tmp/a/b"), "/tmp/a/b");
        // 空串必须被引用，否则会在命令行里消失
        assert_eq!(posix_quote(""), "''");
        // shell 元字符一律被单引号包裹
        for raw in ["a&calc", "a|b", "a;b", "a$(id)", "a`id`", "a b", "a>b"] {
            let q = posix_quote(raw);
            assert!(
                q.starts_with('\'') && q.ends_with('\''),
                "未引用：{raw} -> {q}"
            );
        }
        // 内部单引号按 POSIX 规则转义
        assert_eq!(posix_quote("it's"), r#"'it'\''s'"#);
    }

    #[test]
    fn powershell_quote_escapes_single_quotes() {
        assert_eq!(powershell_quote("plain"), "'plain'");
        // cmd.exe 会把 & 当分隔符；PowerShell 单引号内它是普通字符
        assert_eq!(powershell_quote("a&calc"), "'a&calc'");
        assert_eq!(powershell_quote("it's"), "'it''s'");
    }

    /// 回归：会话 id 含 shell 元字符时，argv 必须逐元素保留原值，
    /// 且展示用 command 必须是已引用的形式（绝不能拼出可注入的裸串）。
    #[test]
    fn resume_spec_keeps_argv_verbatim_and_quotes_display() {
        let evil = "abc&calc";
        let spec = ResumeSpec::new(
            vec![
                "claude".to_string(),
                "--resume".to_string(),
                evil.to_string(),
            ],
            Some("/tmp/proj dir".to_string()),
        );
        // argv 是执行的唯一来源：原值不被改写、不被合并
        assert_eq!(spec.argv, vec!["claude", "--resume", evil]);
        // 展示串里元字符必须处于引号内
        assert_eq!(spec.command, "claude --resume 'abc&calc'");
        assert!(!spec.command.contains("resume abc&calc"));
    }
}
