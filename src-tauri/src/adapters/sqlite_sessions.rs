use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::claude_code::non_empty;
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};

/// OpenCode 与 Zcode 共用同一套 drizzle SQLite schema（session/message/part，data 为 JSON 列）。
struct SqliteConfig {
    id: &'static str,
    name: &'static str,
    db_rel: &'static str,
    resume_command: &'static str,
    source_format: &'static str,
}

const OPENCODE: SqliteConfig = SqliteConfig {
    id: "opencode",
    name: "OpenCode",
    db_rel: ".local/share/opencode/opencode.db",
    resume_command: "opencode --continue",
    source_format: "sqlite",
};

const ZCODE: SqliteConfig = SqliteConfig {
    id: "zcode",
    name: "Zcode",
    db_rel: ".zcode/cli/db/db.sqlite",
    resume_command: "zcode --continue",
    source_format: "sqlite",
};

pub struct OpenCodeAdapter;
pub struct ZcodeAdapter;

fn open_ro(db: &Path) -> Option<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = match conn.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn enumerate_db(_cfg: &SqliteConfig, db_path: &Path) -> Vec<RawRef> {
    let Some(conn) = open_ro(db_path) else { return Vec::new() };
    if !table_exists(&conn, "session") {
        return Vec::new();
    }
    let cols = columns_of(&conn, "session");
    let has = |c: &str| cols.iter().any(|x| x == c);
    // 防御式：只选存在的列，schema 演进也不会崩
    let wanted = [
        "id", "directory", "title", "time_created", "time_updated", "cost", "tokens_input",
        "tokens_output", "tokens_reasoning", "tokens_cache_read", "tokens_cache_write", "parent_id",
    ];
    let selected: Vec<&str> = wanted.into_iter().filter(|c| has(c)).collect();
    if !selected.contains(&"id") {
        return Vec::new();
    }
    let sql = format!("SELECT {} FROM session", selected.join(", "));

    // 每个会话的消息数（单列聚合查询，失败就放弃该字段）
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    if table_exists(&conn, "message") {
        if let Ok(mut st) = conn.prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id") {
            if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for r in rows.flatten() {
                    counts.insert(r.0, r.1.max(0) as u32);
                }
            }
        }
    }

    let mut out = Vec::new();
    let (size, mtime) = std::fs::metadata(db_path)
        .ok()
        .and_then(|md| {
            let m = md
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as i64;
            Some((md.len(), m))
        })
        .unwrap_or((0, 0));

    if let Ok(mut stmt) = conn.prepare(&sql) {
        let col_names: Vec<String> = selected.iter().map(|s| s.to_string()).collect();
        if let Ok(rows) = stmt.query_map([], |row| {
            let mut map = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                let v = row.get_ref(i)?;
                let jv = match v {
                    rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => serde_json::Value::from(n),
                    rusqlite::types::ValueRef::Real(f) => serde_json::Value::from(f),
                    rusqlite::types::ValueRef::Text(t) => {
                        serde_json::Value::from(String::from_utf8_lossy(t).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(_) => serde_json::Value::Null,
                };
                map.insert(name.clone(), jv);
            }
            Ok(serde_json::Value::Object(map))
        }) {
            for row in rows.flatten() {
                let mut row = row;
                let identity = json_str(&row, "id").map(|s| s.to_string());
                if let Some(id) = identity.as_deref() {
                    if let Some(c) = counts.get(id) {
                        row.as_object_mut().map(|m| m.insert("message_count".into(), (*c).into()));
                    }
                }
                out.push(RawRef {
                    path: db_path.to_path_buf(),
                    size,
                    mtime_ms: mtime,
                    inline: Some(row),
                    identity,
                });
            }
        }
    }
    out
}

fn parse_row(cfg: &SqliteConfig, raw: &RawRef) -> Option<Session> {
    let row = raw.inline.as_ref()?;
    let id = json_str(row, "id")?.to_string();
    let created = json_i64(row, "time_created");
    let updated = json_i64(row, "time_updated").or(created);
    let tokens_in = json_u64(row, "tokens_input").unwrap_or(0)
        + json_u64(row, "tokens_cache_read").unwrap_or(0)
        + json_u64(row, "tokens_cache_write").unwrap_or(0);
    let tokens_out =
        json_u64(row, "tokens_output").unwrap_or(0) + json_u64(row, "tokens_reasoning").unwrap_or(0);
    let cost = row.get("cost").and_then(|c| c.as_f64()).filter(|c| *c > 0.0);
    let title = json_str(row, "title").unwrap_or("").to_string();
    let is_sub = row.get("parent_id").and_then(|p| p.as_str()).is_some();

    Some(Session {
        session_id: id,
        harness_id: cfg.id.to_string(),
        project_path: json_str(row, "directory").unwrap_or("").to_string(),
        title: if is_sub && title.is_empty() {
            "(子代理会话)".to_string()
        } else {
            title
        },
        started_at: created,
        ended_at: updated,
        message_count: json_u64(row, "message_count").map(|n| n as u32),
        tokens_in: if tokens_in > 0 { Some(tokens_in) } else { None },
        tokens_out: if tokens_out > 0 { Some(tokens_out) } else { None },
        cost_usd: cost,
        status: derive_status(updated.unwrap_or(raw.mtime_ms)),
        raw_path: raw.path.to_string_lossy().into_owned(),
        source_format: cfg.source_format.to_string(),
        file_size: raw.size,
        file_mtime: raw.mtime_ms,
    })
}

fn read_messages_db(db_path: &Path, session_id: &str, limit: usize) -> Vec<MessagePreview> {
    let Some(conn) = open_ro(db_path) else { return Vec::new() };
    if !table_exists(&conn, "message") || !table_exists(&conn, "part") {
        return Vec::new();
    }
    let sql = "SELECT m.data, p.data, p.time_created FROM part p \
               JOIN message m ON p.message_id = m.id \
               WHERE p.session_id = ?1 ORDER BY p.time_created ASC";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let rows = stmt.query_map([session_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    });
    let mut msgs: Vec<MessagePreview> = Vec::new();
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            let (m_data, p_data, ts) = row;
            let (Ok(m), Ok(p)) = (
                serde_json::from_str::<serde_json::Value>(&m_data),
                serde_json::from_str::<serde_json::Value>(&p_data),
            ) else {
                continue;
            };
            if json_str(&p, "type") != Some("text") {
                continue;
            }
            let Some(text) = json_str(&p, "text") else { continue };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let role = json_str(&m, "role").unwrap_or("?").to_string();
            msgs.push(MessagePreview {
                role,
                text: truncate(text, 2000),
                timestamp: Some(ts),
            });
        }
    }
    if msgs.len() > limit {
        msgs = msgs.split_off(msgs.len() - limit);
    }
    msgs
}

macro_rules! sqlite_adapter {
    ($ty:ident, $cfg:expr) => {
        impl HarnessAdapter for $ty {
            fn id(&self) -> &'static str {
                $cfg.id
            }
            fn name(&self) -> &'static str {
                $cfg.name
            }
            fn detect(&self, ctx: &DetectCtx) -> bool {
                ctx.join($cfg.db_rel).is_file()
            }
            fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> {
                vec![ctx.join($cfg.db_rel)]
            }
            fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> Vec<RawRef> {
                enumerate_db(&$cfg, root)
            }
            fn parse(&self, raw: &RawRef) -> Option<Session> {
                parse_row(&$cfg, raw)
            }
            fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
                Some(ResumeSpec {
                    command: $cfg.resume_command.to_string(),
                    cwd: non_empty(&s.project_path),
                })
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    can_resume: true,
                    can_delete: false, // 共享数据库，绝不删除
                    can_backup: false,
                    can_read_messages: true,
                }
            }
            fn read_messages(&self, s: &Session, limit: usize) -> Vec<MessagePreview> {
                read_messages_db(Path::new(&s.raw_path), &s.session_id, limit)
            }
        }
    };
}

sqlite_adapter!(OpenCodeAdapter, OPENCODE);
sqlite_adapter!(ZcodeAdapter, ZCODE);
