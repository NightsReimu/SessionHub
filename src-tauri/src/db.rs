use std::collections::{HashMap, HashSet};
use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Deserialize;

use crate::models::{
    Counts, Session, SessionDto, SessionMeta, UsageEvent, UsageGroup, UsageReport, UsageSessionRow,
    UsageSums, UsageTrendPoint,
};

type DbResult<T> = Result<T, rusqlite::Error>;

/// 用量明细入库前按 15 分钟对齐压缩：任何时区（含 :30/:45 偏移）的小时/日桶
/// 都由整 15 分钟拼成，聚合结果与逐条明细完全一致，行数却少一两个数量级
const USAGE_SLOT_MS: i64 = 15 * 60 * 1000;

/// 用量统计查询参数（前端按本地时区算好区间边界）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UsageQuery {
    /// 区间起点（含），ms epoch；None = 不限
    pub start_ms: Option<i64>,
    /// 区间终点（不含），ms epoch；None = 不限
    pub end_ms: Option<i64>,
    /// 只看某个 harness
    pub harness: Option<String>,
    /// hour | day | week | month；None = 按区间长度自动选
    pub granularity: Option<String>,
}

/// SessionHub 自己的索引库（~/SessionHub/sessionhub.db）。
/// 用户数据（标签/备注/收藏）与 harness 索引分表存放，扫描重建不会覆盖用户数据。
///
/// 读写分两条连接：WAL 模式下读连接不会被扫描的写事务阻塞，
/// 扫描期间列表/搜索/统计照常秒回，界面不卡。
pub struct Db {
    conn: Mutex<Connection>,
    read: Mutex<Connection>,
    fts_ok: bool,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions(
                harness_id   TEXT NOT NULL,
                session_id   TEXT NOT NULL,
                project_path TEXT NOT NULL DEFAULT '',
                title        TEXT NOT NULL DEFAULT '',
                started_at   INTEGER,
                ended_at     INTEGER,
                message_count INTEGER,
                tokens_in    INTEGER,
                tokens_out   INTEGER,
                cost_usd     REAL,
                status       TEXT NOT NULL DEFAULT '',
                raw_path     TEXT NOT NULL DEFAULT '',
                source_format TEXT NOT NULL DEFAULT '',
                file_size    INTEGER NOT NULL DEFAULT 0,
                file_mtime   INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(harness_id, session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_time ON sessions(ended_at DESC);
            CREATE TABLE IF NOT EXISTS session_meta(
                harness_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tags     TEXT NOT NULL DEFAULT '[]',
                note     TEXT NOT NULL DEFAULT '',
                favorite INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(harness_id, session_id)
            );
            CREATE TABLE IF NOT EXISTS settings(
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            -- 用量账本：按会话整体替换，会话从 harness 里被清理/删除后仍保留
            -- （用量已经实际发生，历史统计不应因源文件被清理而缩水）
            CREATE TABLE IF NOT EXISTS usage_events(
                harness_id  TEXT NOT NULL,
                session_id  TEXT NOT NULL,
                ts          INTEGER NOT NULL,
                model       TEXT NOT NULL DEFAULT '',
                input       INTEGER NOT NULL DEFAULT 0,
                output      INTEGER NOT NULL DEFAULT 0,
                cache_read  INTEGER NOT NULL DEFAULT 0,
                cache_write INTEGER NOT NULL DEFAULT 0,
                cost_usd    REAL NOT NULL DEFAULT 0,
                requests    INTEGER NOT NULL DEFAULT 0,
                priced      INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_usage_ts ON usage_events(ts);
            CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_events(harness_id, session_id);",
        )?;
        // 迁移：旧库补 custom_title 列
        let has_custom_title = conn
            .prepare("PRAGMA table_info(session_meta)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .flatten()
            .any(|c| c == "custom_title");
        if !has_custom_title {
            conn.execute_batch("ALTER TABLE session_meta ADD COLUMN custom_title TEXT")?;
        }
        // FTS5 不可用时降级为 LIKE 搜索
        let fts_ok = conn
            .execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(title, project_path, tags, note);",
            )
            .is_ok();
        let read = Connection::open(path)?;
        read.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self {
            conn: Mutex::new(conn),
            read: Mutex::new(read),
            fts_ok,
        })
    }

    fn row_to_session(r: &rusqlite::Row) -> rusqlite::Result<Session> {
        Ok(Session {
            harness_id: r.get(0)?,
            session_id: r.get(1)?,
            project_path: r.get(2)?,
            title: r.get(3)?,
            started_at: r.get(4)?,
            ended_at: r.get(5)?,
            message_count: r.get(6)?,
            tokens_in: r.get(7)?,
            tokens_out: r.get(8)?,
            cost_usd: r.get(9)?,
            status: r.get(10)?,
            raw_path: r.get(11)?,
            source_format: r.get(12)?,
            file_size: r.get::<_, i64>(13)? as u64,
            file_mtime: r.get(14)?,
            ..Default::default()
        })
    }

    /// 会话列 + meta 列（LEFT JOIN 一次取齐，不再逐行补查 meta）
    fn row_to_dto(r: &rusqlite::Row) -> rusqlite::Result<SessionDto> {
        let session = Self::row_to_session(r)?;
        let tags: Option<String> = r.get(15)?;
        let meta = SessionMeta {
            tags: tags
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default(),
            note: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
            favorite: r.get::<_, Option<i64>>(17)?.unwrap_or(0) != 0,
            custom_title: r.get(18)?,
        };
        Ok(SessionDto {
            session,
            meta,
            raw_usable: true,
        })
    }

    const SESSION_COLS: &'static str =
        "harness_id, session_id, project_path, title, started_at, ended_at, message_count, \
         tokens_in, tokens_out, cost_usd, status, raw_path, source_format, file_size, file_mtime";

    /// `SELECT s.…, m.tags, m.note, m.favorite, m.custom_title FROM sessions s LEFT JOIN session_meta m`
    fn dto_select() -> String {
        let cols = Self::SESSION_COLS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "SELECT {cols}, m.tags, m.note, m.favorite, m.custom_title FROM sessions s \
             LEFT JOIN session_meta m ON m.harness_id=s.harness_id AND m.session_id=s.session_id"
        )
    }

    fn fts_sync(&self, conn: &Connection, rowid: i64) {
        if !self.fts_ok {
            return;
        }
        let _ = conn.execute("DELETE FROM sessions_fts WHERE rowid=?1", params![rowid]);
        let _ = conn.execute(
            "INSERT INTO sessions_fts(rowid, title, project_path, tags, note)
             SELECT s.rowid,
                    s.title || ' ' || COALESCE(m.custom_title, ''),
                    s.project_path, COALESCE(m.tags,''), COALESCE(m.note,'')
             FROM sessions s LEFT JOIN session_meta m
               ON m.harness_id=s.harness_id AND m.session_id=s.session_id
             WHERE s.rowid=?1",
            params![rowid],
        );
    }

    pub fn get_setting(&self, key: &str) -> Option<String> {
        let conn = self.read.lock();
        conn.query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> DbResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 增量扫描用：已存在且 (size, mtime) 未变则跳过解析
    #[cfg(test)]
    pub fn stamp(&self, harness: &str, id: &str) -> Option<(u64, i64)> {
        self.stamps(harness).ok()?.get(id).copied()
    }

    /// 一次取出某 harness 全部会话的扫描戳：并行解析的 worker 查内存表，
    /// 不再每个文件都去抢数据库锁
    pub fn stamps(&self, harness: &str) -> DbResult<HashMap<String, (u64, i64)>> {
        let conn = self.read.lock();
        let mut stmt = conn.prepare(
            "SELECT session_id, file_size, file_mtime FROM sessions WHERE harness_id=?1",
        )?;
        let rows = stmt.query_map(params![harness], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?),
            ))
        })?;
        Ok(rows.flatten().collect())
    }

    #[cfg(test)]
    pub fn upsert_session(&self, s: &Session) -> DbResult<()> {
        self.upsert_sessions(std::slice::from_ref(s))
    }

    /// 批量写入（单个事务）：会话行 + FTS + 整体替换该会话的用量明细
    pub fn upsert_sessions(&self, sessions: &[Session]) -> DbResult<()> {
        if sessions.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for s in sessions {
            tx.execute(
                "INSERT INTO sessions(harness_id, session_id, project_path, title, started_at, ended_at,
                    message_count, tokens_in, tokens_out, cost_usd, status, raw_path, source_format, file_size, file_mtime)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(harness_id, session_id) DO UPDATE SET
                    project_path=excluded.project_path, title=excluded.title,
                    started_at=excluded.started_at, ended_at=excluded.ended_at,
                    message_count=excluded.message_count, tokens_in=excluded.tokens_in,
                    tokens_out=excluded.tokens_out, cost_usd=excluded.cost_usd,
                    status=excluded.status, raw_path=excluded.raw_path,
                    source_format=excluded.source_format, file_size=excluded.file_size,
                    file_mtime=excluded.file_mtime",
                params![
                    s.harness_id, s.session_id, s.project_path, s.title, s.started_at, s.ended_at,
                    s.message_count, s.tokens_in.map(|v| v as i64), s.tokens_out.map(|v| v as i64),
                    s.cost_usd, s.status, s.raw_path, s.source_format,
                    s.file_size as i64, s.file_mtime,
                ],
            )?;
            let rowid: i64 = tx.query_row(
                "SELECT rowid FROM sessions WHERE harness_id=?1 AND session_id=?2",
                params![s.harness_id, s.session_id],
                |r| r.get(0),
            )?;
            self.fts_sync(&tx, rowid);

            tx.execute(
                "DELETE FROM usage_events WHERE harness_id=?1 AND session_id=?2",
                params![s.harness_id, s.session_id],
            )?;
            if !s.usage.is_empty() {
                // 没有时间戳的明细（极少见）落到会话最后活动时间，不让它掉到 1970 年
                let fallback_ts = s.ended_at.or(s.started_at).unwrap_or(s.file_mtime);
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO usage_events(harness_id, session_id, ts, model, input, output,
                        cache_read, cache_write, cost_usd, requests, priced)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                )?;
                for e in compact_usage(&s.usage, fallback_ts) {
                    stmt.execute(params![
                        s.harness_id,
                        s.session_id,
                        e.ts,
                        e.model,
                        e.input as i64,
                        e.output as i64,
                        e.cache_read as i64,
                        e.cache_write as i64,
                        e.cost_usd,
                        e.requests as i64,
                        e.priced as i64,
                    ])?;
                }
            }
        }
        tx.commit()
    }

    pub fn list_sessions(
        &self,
        harness: Option<&str>,
        favorites_only: bool,
        limit: usize,
        offset: usize,
    ) -> DbResult<Vec<SessionDto>> {
        let conn = self.read.lock();
        let mut sql = Self::dto_select();
        let mut conds: Vec<&str> = Vec::new();
        let mut binds: Vec<&dyn rusqlite::ToSql> = Vec::new();
        let harness_bind = harness;
        if let Some(h) = harness_bind.as_ref() {
            // 绑定参数而非字符串拼接：harness 来自前端，不手写转义
            conds.push("s.harness_id = ?");
            binds.push(h);
        }
        if favorites_only {
            conds.push("m.favorite = 1");
        }
        if !conds.is_empty() {
            sql.push_str(&format!(" WHERE {}", conds.join(" AND ")));
        }
        sql.push_str(&format!(
            " ORDER BY COALESCE(s.ended_at, s.started_at, 0) DESC LIMIT {limit} OFFSET {offset}"
        ));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(binds.as_slice(), Self::row_to_dto)?;
        Ok(rows.flatten().collect())
    }

    pub fn get_session(&self, harness: &str, id: &str) -> Option<SessionDto> {
        let conn = self.read.lock();
        Self::get_session_conn(&conn, harness, id)
    }

    fn get_session_conn(conn: &Connection, harness: &str, id: &str) -> Option<SessionDto> {
        let sql = format!(
            "{} WHERE s.harness_id=?1 AND s.session_id=?2",
            Self::dto_select()
        );
        conn.query_row(&sql, params![harness, id], Self::row_to_dto)
            .ok()
    }

    pub fn search(&self, query: &str, limit: usize) -> DbResult<Vec<SessionDto>> {
        let conn = self.read.lock();
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        if self.fts_ok && !tokens.is_empty() {
            let m = tokens.join(" AND ");
            let sql = format!(
                "{} JOIN sessions_fts f ON f.rowid = s.rowid \
                 WHERE sessions_fts MATCH ?1 ORDER BY rank LIMIT {limit}",
                Self::dto_select()
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map(params![m], Self::row_to_dto) {
                    return Ok(rows.flatten().collect());
                }
            }
        }
        let like = format!("%{}%", query.replace('%', ""));
        let sql = format!(
            "{} WHERE s.title LIKE ?1 OR s.project_path LIKE ?1 \
             OR m.note LIKE ?1 OR m.tags LIKE ?1 OR m.custom_title LIKE ?1 \
             ORDER BY COALESCE(s.ended_at, s.started_at, 0) DESC LIMIT {limit}",
            Self::dto_select()
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![like], Self::row_to_dto)?;
        Ok(rows.flatten().collect())
    }

    pub fn set_meta(&self, harness: &str, id: &str, meta: &SessionMeta) -> DbResult<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO session_meta(harness_id, session_id, tags, note, favorite, custom_title)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(harness_id, session_id) DO UPDATE SET
                tags=excluded.tags, note=excluded.note, favorite=excluded.favorite,
                custom_title=excluded.custom_title",
            params![
                harness,
                id,
                serde_json::to_string(&meta.tags).unwrap_or_else(|_| "[]".to_string()),
                meta.note,
                if meta.favorite { 1 } else { 0 },
                meta.custom_title
                    .as_deref()
                    .filter(|t| !t.trim().is_empty()),
            ],
        )?;
        if let Ok(rowid) = conn.query_row::<i64, _, _>(
            "SELECT rowid FROM sessions WHERE harness_id=?1 AND session_id=?2",
            params![harness, id],
            |r| r.get(0),
        ) {
            self.fts_sync(&conn, rowid);
        }
        Ok(())
    }

    pub fn delete_session_row(&self, harness: &str, id: &str) -> DbResult<()> {
        let conn = self.conn.lock();
        if self.fts_ok {
            let _ = conn.execute(
                "DELETE FROM sessions_fts WHERE rowid IN \
                 (SELECT rowid FROM sessions WHERE harness_id=?1 AND session_id=?2)",
                params![harness, id],
            );
        }
        conn.execute(
            "DELETE FROM sessions WHERE harness_id=?1 AND session_id=?2",
            params![harness, id],
        )?;
        Ok(())
    }

    /// 全量扫描后清理该 harness 已不存在的会话（用量账本保留）
    pub fn prune_not_in(&self, harness: &str, keep: &[String]) -> DbResult<usize> {
        let mut conn = self.conn.lock();
        let existing: Vec<String> = {
            let mut stmt = conn.prepare("SELECT session_id FROM sessions WHERE harness_id=?1")?;
            let rows = stmt.query_map(params![harness], |r| r.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        let keep_set: HashSet<&String> = keep.iter().collect();
        let tx = conn.transaction()?;
        let mut removed = 0;
        for id in existing {
            if !keep_set.contains(&id) {
                if self.fts_ok {
                    let _ = tx.execute(
                        "DELETE FROM sessions_fts WHERE rowid IN \
                         (SELECT rowid FROM sessions WHERE harness_id=?1 AND session_id=?2)",
                        params![harness, id],
                    );
                }
                tx.execute(
                    "DELETE FROM sessions WHERE harness_id=?1 AND session_id=?2",
                    params![harness, id],
                )?;
                removed += 1;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    pub fn counts(&self) -> DbResult<Counts> {
        let conn = self.read.lock();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        // 只统计仍在索引里的会话的收藏，删除会话后遗留的孤立 meta 不算
        let favorites: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session_meta m \
             JOIN sessions s ON s.harness_id=m.harness_id AND s.session_id=m.session_id \
             WHERE m.favorite=1",
            [],
            |r| r.get(0),
        )?;
        let mut per_harness = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT harness_id, COUNT(*) FROM sessions GROUP BY harness_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for r in rows.flatten() {
            per_harness.insert(r.0, r.1 as usize);
        }
        Ok(Counts {
            total: total as usize,
            favorites: favorites as usize,
            per_harness,
        })
    }

    /// 用量统计：区间内的汇总、趋势（本地时区分桶）、按 harness / 模型分组、Top 会话
    pub fn usage_report(&self, q: &UsageQuery, top_n: usize) -> DbResult<UsageReport> {
        let conn = self.read.lock();
        let start = q.start_ms.unwrap_or(i64::MIN);
        let end = q.end_ms.unwrap_or(i64::MAX);

        let (first_ts, last_ts): (Option<i64>, Option<i64>) = match q.harness.as_deref() {
            Some(h) => conn.query_row(
                "SELECT MIN(ts), MAX(ts) FROM usage_events WHERE harness_id=?1",
                params![h],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?,
            None => conn.query_row("SELECT MIN(ts), MAX(ts) FROM usage_events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?,
        };

        let span_start = q.start_ms.or(first_ts).unwrap_or(0);
        let span_end = q.end_ms.or(last_ts).unwrap_or(span_start);
        let granularity = match q.granularity.as_deref() {
            Some(g @ ("hour" | "day" | "week" | "month")) => g.to_string(),
            _ => auto_granularity(span_end - span_start).to_string(),
        };

        let mut sql = String::from(
            "SELECT harness_id, session_id, ts, model, input, output, cache_read, cache_write, \
             cost_usd, requests, priced FROM usage_events WHERE ts >= ?1 AND ts < ?2",
        );
        if q.harness.is_some() {
            sql.push_str(" AND harness_id = ?3");
        }
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                UsageEvent {
                    ts: r.get(2)?,
                    model: r.get(3)?,
                    input: r.get::<_, i64>(4)?.max(0) as u64,
                    output: r.get::<_, i64>(5)?.max(0) as u64,
                    cache_read: r.get::<_, i64>(6)?.max(0) as u64,
                    cache_write: r.get::<_, i64>(7)?.max(0) as u64,
                    cost_usd: r.get(8)?,
                    requests: r.get::<_, i64>(9)?.max(0) as u32,
                    priced: r.get::<_, i64>(10)? != 0,
                },
            ))
        };
        let rows: Vec<(String, String, UsageEvent)> = match q.harness.as_deref() {
            Some(h) => stmt
                .query_map(params![start, end, h], map_row)?
                .flatten()
                .collect(),
            None => stmt
                .query_map(params![start, end], map_row)?
                .flatten()
                .collect(),
        };

        let mut totals = UsageSums::default();
        let mut trend: HashMap<(i64, String), UsageSums> = HashMap::new();
        let mut bucket_cache: HashMap<i64, i64> = HashMap::new();
        let mut by_harness: HashMap<String, (UsageSums, HashSet<String>)> = HashMap::new();
        let mut by_model: HashMap<String, ModelAcc> = HashMap::new();
        let mut by_session: HashMap<(String, String), UsageSums> = HashMap::new();

        for (h, sid, e) in &rows {
            totals.add(e);
            let b = *bucket_cache
                .entry(e.ts)
                .or_insert_with(|| bucket_start(e.ts, &granularity));
            trend.entry((b, h.clone())).or_default().add(e);
            let hg = by_harness.entry(h.clone()).or_default();
            hg.0.add(e);
            hg.1.insert(sid.clone());
            let model_key = if e.model.is_empty() {
                "(未知模型)".to_string()
            } else {
                e.model.clone()
            };
            let mg = by_model.entry(model_key).or_default();
            mg.0.add(e);
            mg.1.insert((h.clone(), sid.clone()));
            mg.2.insert(h.clone());
            by_session
                .entry((h.clone(), sid.clone()))
                .or_default()
                .add(e);
        }

        let mut trend: Vec<UsageTrendPoint> = trend
            .into_iter()
            .map(|((start, harness_id), sums)| UsageTrendPoint {
                start,
                harness_id,
                sums,
            })
            .collect();
        trend.sort_by(|a, b| a.start.cmp(&b.start).then(a.harness_id.cmp(&b.harness_id)));

        let rank = |a: &UsageSums, b: &UsageSums| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| tokens_of(b).cmp(&tokens_of(a)))
        };

        let mut by_harness: Vec<UsageGroup> = by_harness
            .into_iter()
            .map(|(key, (sums, sessions))| UsageGroup {
                harnesses: vec![key.clone()],
                key,
                sessions: sessions.len(),
                priced: sums.unpriced_tokens == 0,
                sums,
            })
            .collect();
        by_harness.sort_by(|a, b| rank(&a.sums, &b.sums));

        let mut by_model: Vec<UsageGroup> = by_model
            .into_iter()
            .map(|(key, (sums, sessions, harnesses))| {
                let mut harnesses: Vec<String> = harnesses.into_iter().collect();
                harnesses.sort();
                UsageGroup {
                    key,
                    harnesses,
                    sessions: sessions.len(),
                    priced: sums.unpriced_tokens == 0,
                    sums,
                }
            })
            .collect();
        by_model.sort_by(|a, b| rank(&a.sums, &b.sums));

        let session_count = by_session.len();
        let mut sessions: Vec<((String, String), UsageSums)> = by_session.into_iter().collect();
        sessions.sort_by(|a, b| rank(&a.1, &b.1));
        let top_sessions = sessions
            .into_iter()
            .take(top_n)
            .map(|((harness_id, session_id), sums)| UsageSessionRow {
                session: Self::get_session_conn(&conn, &harness_id, &session_id),
                harness_id,
                session_id,
                sums,
            })
            .collect();

        Ok(UsageReport {
            granularity,
            totals,
            sessions: session_count,
            trend,
            by_harness,
            by_model,
            top_sessions,
            first_ts,
            last_ts,
        })
    }
}

/// 按模型聚合：(用量, 涉及的会话, 涉及的 harness)
type ModelAcc = (UsageSums, HashSet<(String, String)>, HashSet<String>);

fn tokens_of(s: &UsageSums) -> u64 {
    s.input + s.output + s.cache_read + s.cache_write
}

/// 按 15 分钟槽 + 模型合并明细
fn compact_usage(events: &[UsageEvent], fallback_ts: i64) -> Vec<UsageEvent> {
    let mut slots: HashMap<(i64, &str, bool), UsageEvent> = HashMap::new();
    for e in events {
        let ts = if e.ts > 0 { e.ts } else { fallback_ts };
        let slot = ts.div_euclid(USAGE_SLOT_MS) * USAGE_SLOT_MS;
        let acc = slots
            .entry((slot, e.model.as_str(), e.priced))
            .or_insert_with(|| UsageEvent {
                ts: slot,
                model: e.model.clone(),
                priced: e.priced,
                ..Default::default()
            });
        acc.input += e.input;
        acc.output += e.output;
        acc.cache_read += e.cache_read;
        acc.cache_write += e.cache_write;
        acc.cost_usd += e.cost_usd;
        acc.requests += e.requests;
    }
    let mut out: Vec<UsageEvent> = slots.into_values().collect();
    out.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.model.cmp(&b.model)));
    out
}

/// 区间长度 → 默认粒度：两天内按小时，约四个月内按天，两年内按周，更长按月
fn auto_granularity(span_ms: i64) -> &'static str {
    const DAY: i64 = 86_400_000;
    if span_ms <= 2 * DAY {
        "hour"
    } else if span_ms <= 120 * DAY {
        "day"
    } else if span_ms <= 730 * DAY {
        "week"
    } else {
        "month"
    }
}

/// 时间戳所在桶的起点（按本地时区对齐；周从周一开始）
fn bucket_start(ts: i64, granularity: &str) -> i64 {
    use chrono::{Datelike, Local, NaiveTime, TimeZone, Timelike};
    let Some(dt) = Local.timestamp_millis_opt(ts).single() else {
        return ts;
    };
    let local_midnight = |d: chrono::NaiveDate| {
        Local
            .from_local_datetime(&d.and_time(NaiveTime::MIN))
            .earliest()
            .map(|x| x.timestamp_millis())
            .unwrap_or(ts)
    };
    match granularity {
        "hour" => dt
            .with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .map(|d| d.timestamp_millis())
            .unwrap_or(ts),
        "week" => {
            let d = dt.date_naive();
            local_midnight(d - chrono::Days::new(d.weekday().num_days_from_monday() as u64))
        }
        "month" => local_midnight(dt.date_naive().with_day(1).unwrap_or(dt.date_naive())),
        _ => local_midnight(dt.date_naive()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Session, SessionMeta};

    fn stub_session(harness: &str, id: &str) -> Session {
        Session {
            session_id: id.to_string(),
            harness_id: harness.to_string(),
            project_path: "/tmp/p".to_string(),
            title: format!("title-{id}"),
            started_at: Some(1),
            ended_at: Some(2),
            message_count: Some(1),
            status: "idle".to_string(),
            raw_path: format!("/tmp/{id}.jsonl"),
            source_format: "test".to_string(),
            file_size: 10,
            file_mtime: 20,
            ..Default::default()
        }
    }

    fn temp_db(tag: &str) -> (Db, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "sessionhub-ut-db-{}-{}.db",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        (Db::open(&p).unwrap(), p)
    }

    fn cleanup(p: &std::path::PathBuf) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(p.with_extension("db-wal"));
        let _ = std::fs::remove_file(p.with_extension("db-shm"));
    }

    #[test]
    fn upsert_and_stamp_roundtrip() {
        let (db, path) = temp_db("stamp");
        let s = stub_session("h", "s1");
        assert_eq!(db.stamp("h", "s1"), None);
        db.upsert_session(&s).unwrap();
        assert_eq!(db.stamp("h", "s1"), Some((10, 20)));
        let mut s2 = s.clone();
        s2.file_mtime = 21;
        db.upsert_session(&s2).unwrap();
        assert_eq!(db.stamp("h", "s1"), Some((10, 21)));
        cleanup(&path);
    }

    /// 收藏计数不得统计已删除会话遗留的孤立 meta
    #[test]
    fn favorites_ignore_orphan_meta() {
        let (db, path) = temp_db("fav");
        db.upsert_session(&stub_session("h", "s1")).unwrap();
        db.set_meta(
            "h",
            "s1",
            &SessionMeta {
                tags: vec![],
                note: String::new(),
                favorite: true,
                custom_title: None,
            },
        )
        .unwrap();
        assert_eq!(db.counts().unwrap().favorites, 1);
        assert_eq!(db.list_sessions(None, true, 10, 0).unwrap().len(), 1);

        db.delete_session_row("h", "s1").unwrap();
        assert_eq!(
            db.counts().unwrap().favorites,
            0,
            "删除会话后孤立 meta 不应计入收藏"
        );
        assert!(db.list_sessions(None, true, 10, 0).unwrap().is_empty());
        cleanup(&path);
    }

    /// 自定义标题：写入/读取/清空往返；列表 JOIN 带出 meta
    #[test]
    fn custom_title_roundtrip() {
        let (db, path) = temp_db("title");
        db.upsert_session(&stub_session("h", "s1")).unwrap();
        assert_eq!(db.get_session("h", "s1").unwrap().meta.custom_title, None);

        db.set_meta(
            "h",
            "s1",
            &SessionMeta {
                custom_title: Some("我的标题".to_string()),
                tags: vec!["a".to_string()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.get_session("h", "s1")
                .unwrap()
                .meta
                .custom_title
                .as_deref(),
            Some("我的标题")
        );
        let listed = db.list_sessions(Some("h"), false, 10, 0).unwrap();
        assert_eq!(listed[0].meta.tags, vec!["a".to_string()]);

        // 空白标题视为清除
        db.set_meta(
            "h",
            "s1",
            &SessionMeta {
                custom_title: Some("   ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.get_session("h", "s1").unwrap().meta.custom_title, None);
        cleanup(&path);
    }

    #[test]
    fn search_falls_back_or_fts_works() {
        let (db, path) = temp_db("search");
        db.upsert_session(&stub_session("h", "s1")).unwrap();
        let hits = db.search("title-s1", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(db.search("不存在的词xyz", 10).unwrap().is_empty());
        cleanup(&path);
    }

    fn ev(ts: i64, model: &str, input: u64, output: u64, cost: f64) -> UsageEvent {
        UsageEvent {
            ts,
            model: model.to_string(),
            input,
            output,
            cost_usd: cost,
            priced: cost > 0.0,
            requests: 1,
            ..Default::default()
        }
    }

    /// 用量统计：区间过滤、按 harness / 模型聚合、Top 按费用排序；
    /// 重扫同一会话整体替换明细（不累加），会话被清理后账本保留
    #[test]
    fn usage_report_filters_range_and_groups() {
        let (db, path) = temp_db("usage");
        const H: i64 = 3_600_000;
        let day0 = 1_767_225_600_000; // 2026-01-01T00:00:00Z
        let mut a = stub_session("h1", "a");
        a.usage = vec![
            ev(day0 + H, "m-x", 100, 50, 0.5),
            ev(day0 + H + 60_000, "m-x", 100, 50, 0.5),
            ev(day0 + 30 * H, "m-y", 10, 0, 0.0),
        ];
        let mut b = stub_session("h2", "b");
        b.usage = vec![ev(day0 + 2 * H, "m-x", 1000, 0, 2.0)];
        db.upsert_session(&a).unwrap();
        db.upsert_session(&b).unwrap();
        // 重扫：明细整体替换
        db.upsert_session(&a).unwrap();

        let all = db.usage_report(&UsageQuery::default(), 5).unwrap();
        assert_eq!(all.totals.input, 1210);
        assert_eq!(all.totals.requests, 4);
        assert!((all.totals.cost_usd - 3.0).abs() < 1e-9);
        assert_eq!(all.totals.unpriced_tokens, 10);
        assert_eq!(all.sessions, 2);
        assert_eq!(all.by_harness[0].key, "h2", "按费用排序");
        assert_eq!(all.by_model[0].key, "m-x");
        assert_eq!(all.by_model[0].harnesses, vec!["h1", "h2"]);
        assert!(!all.by_model[1].priced);
        assert_eq!(all.top_sessions[0].session_id, "b");
        assert!(all.top_sessions[0].session.is_some());

        // 只看第一天
        let first_day = db
            .usage_report(
                &UsageQuery {
                    start_ms: Some(day0),
                    end_ms: Some(day0 + 24 * H),
                    granularity: Some("hour".to_string()),
                    ..Default::default()
                },
                5,
            )
            .unwrap();
        assert_eq!(first_day.totals.input, 1200);
        assert_eq!(first_day.granularity, "hour");
        assert!(first_day.trend.iter().all(|p| p.start >= day0 - 14 * H));

        // 只看 h1
        let h1 = db
            .usage_report(
                &UsageQuery {
                    harness: Some("h1".to_string()),
                    ..Default::default()
                },
                5,
            )
            .unwrap();
        assert_eq!(h1.totals.input, 210);

        // 源文件被清理（prune）后，历史用量仍在
        db.prune_not_in("h1", &[]).unwrap();
        assert!(db.get_session("h1", "a").is_none());
        let after = db.usage_report(&UsageQuery::default(), 5).unwrap();
        assert_eq!(after.totals.input, 1210);
        cleanup(&path);
    }

    #[test]
    fn compact_merges_same_slot_and_model() {
        let base = 1_767_225_600_000;
        let out = compact_usage(
            &[
                ev(base + 1_000, "m", 1, 1, 0.1),
                ev(base + 2_000, "m", 2, 2, 0.2),
                ev(base + 1_000, "n", 5, 0, 0.0),
                ev(0, "m", 7, 0, 0.0),
            ],
            base + USAGE_SLOT_MS + 5,
        );
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].input, out[0].requests), (3, 2));
        assert_eq!(out[2].ts, base + USAGE_SLOT_MS, "无时间戳落到兜底时间");
    }
}
