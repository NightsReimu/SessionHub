use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection};

use crate::models::{Counts, Session, SessionDto, SessionMeta};

type DbResult<T> = Result<T, rusqlite::Error>;

/// SessionHub 自己的索引库（~/SessionHub/sessionhub.db）。
/// 用户数据（标签/备注/收藏）与 harness 索引分表存放，扫描重建不会覆盖用户数据。
pub struct Db {
    conn: Mutex<Connection>,
    fts_ok: bool,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
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
            );",
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
        Ok(Self {
            conn: Mutex::new(conn),
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
        })
    }

    const SESSION_COLS: &'static str =
        "harness_id, session_id, project_path, title, started_at, ended_at, message_count, \
         tokens_in, tokens_out, cost_usd, status, raw_path, source_format, file_size, file_mtime";

    fn get_meta_conn(conn: &Connection, harness: &str, id: &str) -> SessionMeta {
        conn.query_row(
            "SELECT tags, note, favorite, custom_title FROM session_meta WHERE harness_id=?1 AND session_id=?2",
            params![harness, id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .map(|(tags, note, fav, custom_title)| SessionMeta {
            tags: serde_json::from_str(&tags).unwrap_or_default(),
            note,
            favorite: fav != 0,
            custom_title,
        })
        .unwrap_or_default()
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
        let conn = self.conn.lock();
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
    pub fn stamp(&self, harness: &str, id: &str) -> Option<(u64, i64)> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT file_size, file_mtime FROM sessions WHERE harness_id=?1 AND session_id=?2",
            params![harness, id],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)),
        )
        .ok()
    }

    pub fn upsert_session(&self, s: &Session) -> DbResult<()> {
        let conn = self.conn.lock();
        conn.execute(
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
        let rowid: i64 = conn.query_row(
            "SELECT rowid FROM sessions WHERE harness_id=?1 AND session_id=?2",
            params![s.harness_id, s.session_id],
            |r| r.get(0),
        )?;
        self.fts_sync(&conn, rowid);
        Ok(())
    }

    pub fn list_sessions(
        &self,
        harness: Option<&str>,
        favorites_only: bool,
        limit: usize,
        offset: usize,
    ) -> DbResult<Vec<SessionDto>> {
        let conn = self.conn.lock();
        // 声明在 binds 之前，保证借用存活到 query_map
        let harness_bind = harness;
        let mut sql = format!("SELECT {} FROM sessions", Self::SESSION_COLS);
        let mut conds: Vec<String> = Vec::new();
        let mut binds: Vec<&dyn rusqlite::ToSql> = Vec::new();
        if let Some(h) = harness_bind.as_ref() {
            // 绑定参数而非字符串拼接：harness 来自前端，不手写转义
            conds.push("harness_id = ?".to_string());
            binds.push(h);
        }
        if favorites_only {
            conds.push(
                "EXISTS(SELECT 1 FROM session_meta m WHERE m.harness_id=sessions.harness_id \
                 AND m.session_id=sessions.session_id AND m.favorite=1)"
                    .to_string(),
            );
        }
        if !conds.is_empty() {
            sql.push_str(&format!(" WHERE {}", conds.join(" AND ")));
        }
        sql.push_str(&format!(
            " ORDER BY COALESCE(ended_at, started_at, 0) DESC LIMIT {} OFFSET {}",
            limit, offset
        ));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(binds.as_slice(), Self::row_to_session)?;
        let mut out = Vec::new();
        for s in rows.flatten() {
            let meta = Self::get_meta_conn(&conn, &s.harness_id, &s.session_id);
            out.push(SessionDto {
                session: s,
                meta,
                raw_usable: true,
            });
        }
        Ok(out)
    }

    pub fn get_session(&self, harness: &str, id: &str) -> Option<SessionDto> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT {} FROM sessions WHERE harness_id=?1 AND session_id=?2",
            Self::SESSION_COLS
        );
        let s = conn
            .query_row(&sql, params![harness, id], Self::row_to_session)
            .ok()?;
        let meta = Self::get_meta_conn(&conn, &s.harness_id, &s.session_id);
        Some(SessionDto {
            session: s,
            meta,
            raw_usable: true,
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> DbResult<Vec<SessionDto>> {
        let conn = self.conn.lock();
        let mut sessions: Vec<Session> = Vec::new();
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        let mut matched = false;
        if self.fts_ok && !tokens.is_empty() {
            let m = tokens.join(" AND ");
            let sql = format!(
                "SELECT {} FROM sessions JOIN sessions_fts f ON f.rowid = sessions.rowid \
                 WHERE sessions_fts MATCH ?1 ORDER BY rank LIMIT {}",
                Self::SESSION_COLS
                    .split(", ")
                    .map(|c| format!("sessions.{c}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                limit
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map(params![m], Self::row_to_session) {
                    for s in rows.flatten() {
                        sessions.push(s);
                    }
                    matched = true;
                }
            }
        }
        if !matched {
            let like = format!("%{}%", query.replace('%', ""));
            let sql = format!(
                "SELECT {cols} FROM sessions WHERE title LIKE ?1 OR project_path LIKE ?1 \
                 OR EXISTS(SELECT 1 FROM session_meta m WHERE m.harness_id=sessions.harness_id \
                    AND m.session_id=sessions.session_id AND (m.note LIKE ?1 OR m.tags LIKE ?1 \
                    OR m.custom_title LIKE ?1)) \
                 ORDER BY COALESCE(ended_at, started_at, 0) DESC LIMIT {limit}",
                cols = Self::SESSION_COLS
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![like], Self::row_to_session)?;
            for s in rows.flatten() {
                sessions.push(s);
            }
        }
        Ok(sessions
            .into_iter()
            .map(|s| {
                let meta = Self::get_meta_conn(&conn, &s.harness_id, &s.session_id);
                SessionDto {
                    session: s,
                    meta,
                    raw_usable: true,
                }
            })
            .collect())
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

    /// 全量扫描后清理该 harness 已不存在的会话
    pub fn prune_not_in(&self, harness: &str, keep: &[String]) -> DbResult<usize> {
        let conn = self.conn.lock();
        let existing: Vec<String> = {
            let mut stmt = conn.prepare("SELECT session_id FROM sessions WHERE harness_id=?1")?;
            let rows = stmt.query_map(params![harness], |r| r.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        let keep_set: std::collections::HashSet<&String> = keep.iter().collect();
        let mut removed = 0;
        for id in existing {
            if !keep_set.contains(&id) {
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
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn counts(&self) -> DbResult<Counts> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        // 只统计仍在索引里的会话的收藏，删除会话后遗留的孤立 meta 不算
        let favorites: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session_meta m \
             JOIN sessions s ON s.harness_id=m.harness_id AND s.session_id=m.session_id \
             WHERE m.favorite=1",
            [],
            |r| r.get(0),
        )?;
        let mut per_harness = std::collections::HashMap::new();
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

    /// token / 费用统计：按 harness 聚合 + 消耗最高的会话
    pub fn stats(&self, top_n: usize) -> DbResult<crate::models::StatsOverview> {
        let conn = self.conn.lock();
        let mut per_harness = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT harness_id, COUNT(*), COALESCE(SUM(tokens_in),0), \
                        COALESCE(SUM(tokens_out),0), COALESCE(SUM(cost_usd),0.0) \
                 FROM sessions GROUP BY harness_id ORDER BY 2 DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(crate::models::HarnessStat {
                    harness_id: r.get::<_, String>(0)?,
                    sessions: r.get::<_, i64>(1)? as usize,
                    tokens_in: r.get::<_, i64>(2)? as u64,
                    tokens_out: r.get::<_, i64>(3)? as u64,
                    cost_usd: r.get::<_, f64>(4)?,
                })
            })?;
            for r in rows.flatten() {
                per_harness.push(r);
            }
        }
        let total_sessions = per_harness.iter().map(|h| h.sessions).sum();
        let total_tokens_in = per_harness.iter().map(|h| h.tokens_in).sum();
        let total_tokens_out = per_harness.iter().map(|h| h.tokens_out).sum();
        let total_cost_usd = per_harness.iter().map(|h| h.cost_usd).sum();

        let sql = format!(
            "SELECT {} FROM sessions \
             ORDER BY (COALESCE(tokens_in,0) + COALESCE(tokens_out,0)) DESC LIMIT {}",
            Self::SESSION_COLS,
            top_n
        );
        let mut top_sessions = Vec::new();
        {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], Self::row_to_session)?;
            for s in rows.flatten() {
                let meta = Self::get_meta_conn(&conn, &s.harness_id, &s.session_id);
                top_sessions.push(SessionDto {
                    session: s,
                    meta,
                    raw_usable: true,
                });
            }
        }
        Ok(crate::models::StatsOverview {
            total_sessions,
            total_tokens_in,
            total_tokens_out,
            total_cost_usd,
            per_harness,
            top_sessions,
        })
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
            tokens_in: None,
            tokens_out: None,
            cost_usd: None,
            status: "idle".to_string(),
            raw_path: format!("/tmp/{id}.jsonl"),
            source_format: "test".to_string(),
            file_size: 10,
            file_mtime: 20,
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

        db.delete_session_row("h", "s1").unwrap();
        assert_eq!(
            db.counts().unwrap().favorites,
            0,
            "删除会话后孤立 meta 不应计入收藏"
        );
        assert!(db.list_sessions(None, true, 10, 0).unwrap().is_empty());
        cleanup(&path);
    }

    /// 自定义标题：写入/读取/清空往返
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

    /// 统计：按 harness 聚合 token/费用，top 按总消耗排序
    #[test]
    fn stats_aggregates_tokens_and_cost() {
        let (db, path) = temp_db("stats");
        let mut a = stub_session("h1", "a");
        a.tokens_in = Some(100);
        a.tokens_out = Some(50);
        a.cost_usd = Some(0.5);
        let mut b = stub_session("h1", "b");
        b.tokens_in = Some(300);
        b.tokens_out = Some(200);
        let c = stub_session("h2", "c");
        db.upsert_session(&a).unwrap();
        db.upsert_session(&b).unwrap();
        db.upsert_session(&c).unwrap();

        let s = db.stats(5).unwrap();
        assert_eq!(s.total_sessions, 3);
        assert_eq!(s.total_tokens_in, 400);
        assert_eq!(s.total_tokens_out, 250);
        assert!((s.total_cost_usd - 0.5).abs() < 1e-9);
        assert_eq!(s.per_harness[0].harness_id, "h1");
        assert_eq!(s.per_harness[0].sessions, 2);
        assert_eq!(s.top_sessions[0].session.session_id, "b");
        cleanup(&path);
    }
}
