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
    launch_command: &'static str,
    source_format: &'static str,
    /// session 表没有 cost/tokens 列时，是否从 model_usage 表聚合（zcode）
    usage_pricing: bool,
}

const OPENCODE: SqliteConfig = SqliteConfig {
    id: "opencode",
    name: "OpenCode",
    db_rel: ".local/share/opencode/opencode.db",
    resume_command: "opencode --continue",
    launch_command: "opencode",
    source_format: "sqlite",
    usage_pricing: false,
};

const ZCODE: SqliteConfig = SqliteConfig {
    id: "zcode",
    name: "Zcode",
    db_rel: ".zcode/cli/db/db.sqlite",
    resume_command: "zcode --continue",
    launch_command: "zcode",
    source_format: "sqlite",
    usage_pricing: true,
};

/// 按模型族刊例价估算（美元/百万 token：输入, 输出, 缓存读；缓存写按输入价）。
/// 自定义 provider 的模型按同代公开价取近似档。
fn family_price(model: &str) -> (f64, f64, f64) {
    let m = model.to_lowercase();
    if m.contains("deepseek") {
        (0.27, 1.10, 0.07)
    } else if m.contains("kimi") {
        (0.60, 2.50, 0.15)
    } else if m.contains("grok") {
        (3.0, 15.0, 0.75)
    } else if m.contains("glm") {
        (0.60, 2.20, 0.15)
    } else if m.contains("qwen") {
        (0.20, 0.60, 0.05)
    } else {
        (1.0, 4.0, 0.25) // 未知模型的保守通用价
    }
}

/// 从 model_usage 按 session 聚合 token 与估算费用（zcode 的 session 表没有这些列）
fn aggregate_usage(
    conn: &Connection,
) -> std::collections::HashMap<String, (u64, u64, u64, u64, u64, f64)> {
    let mut map = std::collections::HashMap::new();
    if !table_exists(conn, "model_usage") {
        return map;
    }
    let sql = "SELECT session_id, model_id, \
               SUM(input_tokens), SUM(output_tokens), SUM(reasoning_tokens), \
               SUM(cache_creation_input_tokens), SUM(cache_read_input_tokens) \
               FROM model_usage GROUP BY session_id, model_id";
    let Ok(mut stmt) = conn.prepare(sql) else { return map };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
        ))
    });
    let Ok(rows) = rows else { return map };
    for r in rows.flatten() {
        let (sid, model, input, output, reasoning, cache_w, cache_r) = r;
        let (pi, po, pcr) = family_price(&model);
        let cost = (input.max(0) as f64 * pi
            + output.max(0) as f64 * po
            + reasoning.max(0) as f64 * po
            + cache_w.max(0) as f64 * pi
            + cache_r.max(0) as f64 * pcr)
            / 1e6;
        let e = map.entry(sid).or_insert((0, 0, 0, 0, 0, 0.0));
        e.0 += input.max(0) as u64;
        e.1 += output.max(0) as u64;
        e.2 += reasoning.max(0) as u64;
        e.3 += cache_w.max(0) as u64;
        e.4 += cache_r.max(0) as u64;
        e.5 += cost;
    }
    map
}

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

fn enumerate_db(cfg: &SqliteConfig, db_path: &Path) -> (Vec<RawRef>, usize) {
    let mut errors = 0usize;
    let Some(conn) = open_ro(db_path) else {
        return (Vec::new(), 1);
    };
    if !table_exists(&conn, "session") {
        // schema 对不上：无法区分“格式变了”和“被清空”，按错误处理防止误清索引
        return (Vec::new(), 1);
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
        return (Vec::new(), 1);
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

    // session 表没有用量列时（zcode），从 model_usage 聚合 token 与估算费用
    let usage = if cfg.usage_pricing {
        aggregate_usage(&conn)
    } else {
        std::collections::HashMap::new()
    };

    let mut out = Vec::new();
    // 扫描戳要感知 WAL：SQLite 的写入先进 *.db-wal，主 .db 的 size/mtime 可能长期不变
    fn file_stamp(p: &Path) -> (u64, i64) {
        std::fs::metadata(p)
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
            .unwrap_or((0, 0))
    }
    let (mut size, mut mtime) = file_stamp(db_path);
    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let (wal_size, wal_mtime) = file_stamp(&wal_path);
    size += wal_size;
    mtime = mtime.max(wal_mtime);

    match conn.prepare(&sql) {
        Ok(mut stmt) => {
            let col_names: Vec<String> = selected.iter().map(|s| s.to_string()).collect();
            match stmt.query_map([], |row| {
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
                Ok(rows) => {
                    for row in rows {
                        match row {
                            Ok(row) => {
                                let mut row = row;
                                let identity = json_str(&row, "id").map(|s| s.to_string());
                                if let Some(id) = identity.as_deref() {
                                    if let Some(c) = counts.get(id) {
                                        row.as_object_mut().map(|m| {
                                            m.insert("message_count".into(), (*c).into())
                                        });
                                    }
                                    // 注入聚合用量：仅在 session 行本身缺省时
                                    if let Some(u) = usage.get(id) {
                                        let missing = |k: &str| {
                                            row.get(k)
                                                .map(|v| v.is_null() || v.as_f64() == Some(0.0))
                                                .unwrap_or(true)
                                        };
                                        let mut updates: Vec<(&str, serde_json::Value)> = Vec::new();
                                        if missing("tokens_input") {
                                            updates.push(("tokens_input", u.0.into()));
                                        }
                                        if missing("tokens_output") {
                                            updates.push(("tokens_output", u.1.into()));
                                        }
                                        if missing("tokens_reasoning") {
                                            updates.push(("tokens_reasoning", u.2.into()));
                                        }
                                        if missing("tokens_cache_write") {
                                            updates.push(("tokens_cache_write", u.3.into()));
                                        }
                                        if missing("tokens_cache_read") {
                                            updates.push(("tokens_cache_read", u.4.into()));
                                        }
                                        if missing("cost") && u.5 > 0.0 {
                                            updates.push(("cost", serde_json::Value::from(u.5)));
                                        }
                                        if let Some(m) = row.as_object_mut() {
                                            for (k, v) in updates {
                                                m.insert(k.to_string(), v);
                                            }
                                        }
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
                            Err(_) => errors += 1,
                        }
                    }
                }
                Err(_) => errors += 1,
            }
        }
        Err(_) => errors += 1,
    }
    (out, errors)
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
            fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
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
            fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> {
                Some(ResumeSpec {
                    command: $cfg.launch_command.to_string(),
                    cwd: non_empty(&s.project_path),
                })
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    can_resume: true,
                    can_delete: false, // 共享数据库，绝不删除
                    can_backup: false,
                    can_read_messages: true,
                    can_launch: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WAL 感知：只写 .db-wal（主 .db 的 size/mtime 不变）也必须改变扫描戳
    #[test]
    fn wal_changes_scan_stamp() {
        let dir = std::env::temp_dir().join(format!("sh-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dbp = dir.join("opencode.db");
        {
            let conn = Connection::open(&dbp).unwrap();
            conn.execute_batch(
                "CREATE TABLE session(
                    id text primary key, directory text not null default '',
                    title text not null default '',
                    time_created integer not null default 0,
                    time_updated integer not null default 0);
                 INSERT INTO session(id, title) VALUES ('s1', 't1');",
            )
            .unwrap();
        }
        let (raws, errs) = enumerate_db(&OPENCODE, &dbp);
        assert_eq!(errs, 0);
        assert_eq!(raws.len(), 1);
        let base_size = raws[0].size;
        let base_mtime = raws[0].mtime_ms;

        // 模拟 SQLite WAL：写入只落在 wal 文件
        let walp = dir.join("opencode.db-wal");
        std::fs::write(&walp, vec![7u8; 4096]).unwrap();
        let wal_mtime = std::fs::metadata(&walp)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let (raws2, errs2) = enumerate_db(&OPENCODE, &dbp);
        assert_eq!(errs2, 0);
        assert_eq!(raws2[0].size, base_size + 4096, "扫描戳必须包含 wal 大小");
        assert!(
            raws2[0].mtime_ms >= wal_mtime.max(base_mtime),
            "扫描戳 mtime 必须反映 wal 的更新时间"
        );

        // 基本解析仍正常
        let adapter = OpenCodeAdapter;
        let s = adapter.parse(&raws2[0]).unwrap();
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.title, "t1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// zcode 模式：session 表无用量列时，从 model_usage 聚合 token 并按族刊例价估算费用
    #[test]
    fn usage_aggregated_from_model_usage() {
        let dir = std::env::temp_dir().join(format!("sh-zusage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dbp = dir.join("db.sqlite");
        {
            let conn = Connection::open(&dbp).unwrap();
            conn.execute_batch(
                "CREATE TABLE session(
                    id text primary key, directory text not null default '',
                    title text not null default '',
                    time_created integer not null default 0,
                    time_updated integer not null default 0);
                 CREATE TABLE model_usage(
                    id text primary key, session_id text not null,
                    model_id text not null,
                    input_tokens integer not null default 0,
                    output_tokens integer not null default 0,
                    reasoning_tokens integer not null default 0,
                    cache_creation_input_tokens integer not null default 0,
                    cache_read_input_tokens integer not null default 0);
                 INSERT INTO session(id, title) VALUES ('s1', 't1');
                 INSERT INTO model_usage(id, session_id, model_id, input_tokens, output_tokens, cache_read_input_tokens)
                 VALUES ('m1', 's1', 'deepseek-v4-flash', 1000000, 100000, 2000000),
                        ('m2', 's1', 'deepseek-v4-flash', 500000, 50000, 0);",
            )
            .unwrap();
        }
        let (raws, errs) = enumerate_db(&ZCODE, &dbp);
        assert_eq!(errs, 0);
        assert_eq!(raws.len(), 1);
        let adapter = ZcodeAdapter;
        let s = adapter.parse(&raws[0]).unwrap();
        assert_eq!(s.session_id, "s1");
        // tokens_in = 1.5M input + 2M cache_read；tokens_out = 150K
        assert_eq!(s.tokens_in, Some(3_500_000));
        assert_eq!(s.tokens_out, Some(150_000));
        // deepseek 刊例：1.5M×$0.27 + 150K×$1.10 + 2M×$0.07 = $0.71
        let cost = s.cost_usd.expect("应估算费用");
        assert!((cost - 0.71).abs() < 1e-6, "cost = {cost}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
