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
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDto {
    #[serde(flatten)]
    pub session: Session,
    pub meta: SessionMeta,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Capabilities {
    pub can_resume: bool,
    pub can_delete: bool,
    pub can_backup: bool,
    pub can_read_messages: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResumeSpec {
    /// 在 shell 中执行的完整命令，例如 `claude --resume <id>`
    pub command: String,
    pub cwd: Option<String>,
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
