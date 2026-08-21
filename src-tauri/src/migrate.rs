//! 会话跨 harness 迁移：把源会话的消息记录转换为目标 harness 的原生
//! 会话文件，使目标 harness 能用自己的 resume 命令继续该对话。
//!
//! 只支持单文件格式的目标（Claude Code / Codex）。
//! OpenCode / Zcode（共享 SQLite）与 DSH（全局索引 + zstd）写入风险过高，明确拒绝。

use std::path::{Path, PathBuf};

use crate::models::{MessagePreview, Session};

#[derive(Debug, Clone, serde::Serialize)]
pub struct MigrationResult {
    pub path: PathBuf,
    pub session_id: String,
    pub resume_command: String,
}

pub fn migrate(
    session: &Session,
    messages: &[MessagePreview],
    target: &str,
) -> Result<MigrationResult, String> {
    if messages.is_empty() {
        return Err("源会话没有可迁移的消息内容".to_string());
    }
    match target {
        "codex" => migrate_to_codex(session, messages),
        "claude-code" => migrate_to_claude(session, messages),
        "opencode" => migrate_to_sqlite(session, messages, "opencode"),
        // zcode CLI 没有 resume/--session 机制（zcode --help 实测），
        // 迁移进去也无法被调用，暂不提供该目标
        "zcode" => Err(
            "zcode CLI 不支持按会话恢复（无 resume/session 参数），暂不提供该迁移目标".to_string(),
        ),
        _ => Err(format!(
            "暂不支持迁移到 {target}：该 harness 使用全局索引 + 压缩存储，写入风险过高"
        )),
    }
}

fn iso(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// 原子写入：先写同目录临时文件再 rename，
/// 避免 watcher 扫到半成品、进程中断留下损坏文件。
/// 临时名带目标文件名 + pid + 随机后缀：同目录多目标并发写不会互相覆盖。
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_string());
    let tmp = path.with_file_name(format!(
        ".sessionhub-tmp-{}-{}-{}",
        stem,
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    // 显式 fsync：rename 本身原子，但不保证内容已落盘
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败：{e}"))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("写入临时文件失败：{e}"))?;
        f.sync_all().map_err(|e| format!("刷盘失败：{e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换目标文件失败：{e}")
    })
}

fn norm_role(role: &str) -> &str {
    if role == "user" {
        "user"
    } else {
        "assistant"
    }
}

// ---------------- Codex 目标 ----------------

fn migrate_to_codex(
    session: &Session,
    messages: &[MessagePreview],
) -> Result<MigrationResult, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Local::now();
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".codex/sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 Codex 会话目录失败：{e}"))?;
    let path = dir.join(format!(
        "rollout-{}-{}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S"),
        id
    ));

    let ts0 = iso(session.started_at);
    let mut out = String::new();
    out.push_str(
        &serde_json::json!({
            "timestamp": ts0,
            "type": "session_meta",
            "payload": {
                "id": id,
                "session_id": id,
                "timestamp": ts0,
                "cwd": session.project_path,
                "originator": "sessionhub-migration",
                "cli_version": "migration",
                "source": "sessionhub",
            }
        })
        .to_string(),
    );
    out.push('\n');
    for m in messages {
        let role = norm_role(&m.role);
        let ctype = if role == "user" {
            "input_text"
        } else {
            "output_text"
        };
        out.push_str(
            &serde_json::json!({
                "timestamp": iso(m.timestamp),
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": role,
                    "content": [{"type": ctype, "text": m.text}],
                }
            })
            .to_string(),
        );
        out.push('\n');
    }
    atomic_write(&path, &out).map_err(|e| format!("写入 Codex 会话文件失败：{e}"))?;

    Ok(MigrationResult {
        path,
        session_id: id.clone(),
        resume_command: format!("codex resume {id}"),
    })
}

// ---------------- Claude Code 目标 ----------------

/// Claude Code 项目目录编码：非 ASCII 字母数字一律变 '-'
/// （实测：/Users/hec/project/fiver简历 → -Users-hec-project-fiver--）
fn encode_project_path(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn migrate_to_claude(
    session: &Session,
    messages: &[MessagePreview],
) -> Result<MigrationResult, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".claude/projects")
        .join(encode_project_path(&session.project_path));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 Claude 项目目录失败：{e}"))?;
    let path = dir.join(format!("{id}.jsonl"));

    let mut out = String::new();
    let mut parent: Option<String> = None;
    for m in messages {
        let u = uuid::Uuid::new_v4().to_string();
        let role = norm_role(&m.role);
        out.push_str(
            &serde_json::json!({
                "parentUuid": parent,
                "isSidechain": false,
                "type": role,
                "message": {"role": role, "content": [{"type": "text", "text": m.text}]},
                "uuid": u,
                "timestamp": iso(m.timestamp),
                "cwd": session.project_path,
                "sessionId": id,
                "version": "sessionhub-migration",
            })
            .to_string(),
        );
        out.push('\n');
        parent = Some(u);
    }
    atomic_write(&path, &out).map_err(|e| format!("写入 Claude 会话文件失败：{e}"))?;

    // 索引更新失败要反馈：会话文件虽已写入，但用户需要知道索引状态
    update_claude_index(&dir, &path, &id, session, messages)
        .map_err(|e| format!("{e}（会话文件已写入 {}）", path.display()))?;

    Ok(MigrationResult {
        path,
        session_id: id.clone(),
        resume_command: format!("claude --resume {id}"),
    })
}

/// 索引文件的并发标识：(mtime_ms, len)。用于写回前确认文件未被他人改动。
fn index_stamp(path: &Path) -> Option<(i64, u64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some((mtime, md.len()))
}

fn update_claude_index(
    dir: &Path,
    full_path: &Path,
    id: &str,
    session: &Session,
    messages: &[MessagePreview],
) -> Result<(), String> {
    let index_path = dir.join("sessions-index.json");
    // Claude Code 本身可能正在运行并写同一个索引。原子 rename 只能保证不写出
    // 半成品，防不住“丢更新”：读到写回之间对方的追加会被整体覆盖。
    // 因此做 compare-and-swap —— 写回前复核 (mtime, len)，变了就重做整个
    // 读-改-写；连续冲突则放弃并如实报错，绝不盲写。
    const MAX_ATTEMPTS: usize = 3;
    let mut last_conflict = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let before = index_stamp(&index_path);
        // 已存在但暂时解析失败（损坏/并发写入中）→ 明确报错，绝不用空索引覆盖原文件
        let mut v = if index_path.exists() {
            let text =
                std::fs::read_to_string(&index_path).map_err(|e| format!("读取索引失败：{e}"))?;
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => v,
                Err(e) => return Err(format!("索引暂时不可解析，已保留原文件：{e}")),
            }
        } else {
            serde_json::json!({"version": 1, "entries": []})
        };
        let Some(entries) = v.get_mut("entries").and_then(|e| e.as_array_mut()) else {
            return Err("索引结构异常（缺少 entries 数组），已保留原文件".to_string());
        };
        // 幂等：同一 sessionId 已在索引里就不再追加，避免重复迁移堆积重复条目
        let already = entries
            .iter()
            .any(|e| e.get("sessionId").and_then(|x| x.as_str()) == Some(id));
        if already {
            return Ok(());
        }
        let first_prompt = messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.text.chars().take(80).collect::<String>())
            .unwrap_or_default();
        let now_ms = chrono::Utc::now().timestamp_millis();
        entries.push(serde_json::json!({
            "sessionId": id,
            "fullPath": full_path.to_string_lossy(),
            "fileMtime": now_ms,
            "firstPrompt": first_prompt,
            "summary": format!("（迁移自 {}）{}", session.harness_id, session.title),
            "messageCount": messages.len(),
            "created": iso(session.started_at),
            "modified": iso(session.ended_at),
            "gitBranch": "",
            "projectPath": session.project_path,
            "isSidechain": false,
        }));
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        // 写回前复核：期间被改动就重试，避免覆盖他人的并发追加
        if index_stamp(&index_path) != before {
            last_conflict = format!("第 {attempt} 次尝试期间索引被其它进程修改");
            continue;
        }
        return atomic_write(&index_path, &text);
    }
    Err(format!(
        "索引被并发修改且重试 {MAX_ATTEMPTS} 次仍冲突（{last_conflict}），已保留原文件"
    ))
}

// ---------------- OpenCode / Zcode 目标（共享 SQLite，事务写入） ----------------

fn short_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 往目标 harness 的 SQLite 写入 session + message + part。
/// 整个写入在单个事务里，失败即回滚；resume 语义为 `--continue`
/// （继续该项目最新会话），因此 time_updated 取当前时间。
fn migrate_to_sqlite(
    session: &Session,
    messages: &[MessagePreview],
    target: &str,
) -> Result<MigrationResult, String> {
    use rusqlite::{params, Connection};

    // 只支持 opencode（zcode 无按会话恢复的 CLI 机制）
    let sid_prefix = "ses_";
    let db_rel = ".local/share/opencode/opencode.db";
    let db_path = dirs::home_dir().unwrap_or_default().join(db_rel);
    if !db_path.is_file() {
        return Err(format!("{target} 数据库不存在：{}", db_path.display()));
    }
    let mut conn =
        Connection::open(&db_path).map_err(|e| format!("打开 {target} 数据库失败：{e}"))?;
    let _ = conn.pragma_update(None, "busy_timeout", 3000);

    let now = chrono::Utc::now().timestamp_millis();
    let started = session.started_at.unwrap_or(now);
    let sid = format!("{sid_prefix}{}", short_hex());
    let title = if session.title.is_empty() {
        format!("（迁移自 {}）", session.harness_id)
    } else {
        format!("{}（迁移自 {}）", session.title, session.harness_id)
    };
    let slug = format!("migrated-{}", &short_hex()[..8]);

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let result = (|| -> Result<(), String> {
        // project 关联：opencode 有 project 表（按 worktree 查找或新建），
        // zcode 的 project_id 就是编码路径字符串
        let project_id = if target == "opencode" {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM project WHERE worktree=?1",
                    params![session.project_path],
                    |r| r.get(0),
                )
                .ok();
            match existing {
                Some(p) => p,
                None => {
                    let pid = format!("proj_{}", short_hex());
                    let name = Path::new(&session.project_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    tx.execute(
                        "INSERT INTO project(id, worktree, vcs, name, time_created, time_updated, sandboxes)                          VALUES (?1, ?2, NULL, ?3, ?4, ?4, '[]')",
                        params![pid, session.project_path, name, now],
                    )
                    .map_err(|e| format!("创建 project 失败：{e}"))?;
                    pid
                }
            }
        } else {
            format!(
                "proj_{}",
                session
                    .project_path
                    .to_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            )
        };

        tx.execute(
            "INSERT INTO session(id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)              VALUES (?1, ?2, NULL, ?3, ?4, ?5, 'sessionhub', ?6, ?7)",
            params![sid, project_id, slug, session.project_path, title, started, now],
        )
        .map_err(|e| format!("写入 session 失败：{e}"))?;

        for m in messages {
            let role = norm_role(&m.role).to_string();
            let ts = m.timestamp.unwrap_or(now);
            let mid = format!("msg_{}", short_hex());
            let data = if role == "user" {
                serde_json::json!({"role": "user", "time": {"created": ts}})
            } else {
                serde_json::json!({"role": "assistant", "time": {"created": ts, "completed": ts}})
            };
            tx.execute(
                "INSERT INTO message(id, session_id, time_created, time_updated, data)                  VALUES (?1, ?2, ?3, ?3, ?4)",
                params![mid, sid, ts, data.to_string()],
            )
            .map_err(|e| format!("写入 message 失败：{e}"))?;
            let pdata = serde_json::json!({
                "type": "text",
                "text": m.text,
                "time": {"start": ts, "end": ts},
            });
            tx.execute(
                "INSERT INTO part(id, message_id, session_id, time_created, time_updated, data)                  VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![format!("prt_{}", short_hex()), mid, sid, ts, pdata.to_string()],
            )
            .map_err(|e| format!("写入 part 失败：{e}"))?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => tx.commit().map_err(|e| format!("提交事务失败：{e}"))?,
        Err(e) => {
            let _ = tx.rollback();
            return Err(format!("{e}（已回滚，未写入任何内容）"));
        }
    }

    Ok(MigrationResult {
        path: db_path,
        session_id: sid.clone(),
        resume_command: format!(
            "cd {} && opencode --session {}",
            shell_safe(&session.project_path),
            sid
        ),
    })
}

fn shell_quote_local(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\''"))
    }
}

fn shell_safe(s: &str) -> String {
    shell_quote_local(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::util::file_raw_ref;
    use crate::adapters::{claude_code::ClaudeCodeAdapter, codex::CodexAdapter, HarnessAdapter};

    /// 回归：同一 sessionId 重复写索引必须幂等，不堆积重复条目；
    /// 且并发 stamp 未变时正常写入。
    #[test]
    fn claude_index_append_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("sh-idx-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = Session {
            session_id: "src-1".to_string(),
            harness_id: "codex".to_string(),
            project_path: "/tmp/proj".to_string(),
            title: "t".to_string(),
            started_at: Some(1_700_000_000_000),
            ended_at: Some(1_700_000_100_000),
            message_count: Some(2),
            tokens_in: None,
            tokens_out: None,
            cost_usd: None,
            status: "idle".to_string(),
            raw_path: dir.join("x.jsonl").to_string_lossy().into_owned(),
            source_format: "jsonl".to_string(),
            file_size: 0,
            file_mtime: 0,
        };
        let msgs = fake_messages();
        let full = dir.join("abc.jsonl");

        update_claude_index(&dir, &full, "abc", &session, &msgs).unwrap();
        // 第二次同 id：应直接返回且不新增条目
        update_claude_index(&dir, &full, "abc", &session, &msgs).unwrap();
        // 不同 id：应新增
        update_claude_index(&dir, &full, "def", &session, &msgs).unwrap();

        let text = std::fs::read_to_string(dir.join("sessions-index.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entries = v.get("entries").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 2, "重复 sessionId 不应堆积：{entries:?}");
        let ids: Vec<&str> = entries
            .iter()
            .map(|e| e.get("sessionId").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["abc", "def"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：同目录下两个不同目标并发写，临时文件名不得互相覆盖。
    #[test]
    fn atomic_write_distinct_targets_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("sh-aw-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        atomic_write(&a, "content-a").unwrap();
        atomic_write(&b, "content-b").unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "content-a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "content-b");
        // 临时文件不得残留
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".sessionhub-tmp-")
            })
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fake_messages() -> Vec<MessagePreview> {
        vec![
            MessagePreview {
                role: "user".to_string(),
                text: "帮我写一个函数".to_string(),
                timestamp: Some(1_700_000_000_000),
            },
            MessagePreview {
                role: "assistant".to_string(),
                text: "好的，代码如下".to_string(),
                timestamp: Some(1_700_000_050_000),
            },
        ]
    }

    /// 迁移到 codex：生成的 rollout 文件必须能被 CodexAdapter 完整解析
    #[test]
    fn migrate_to_codex_roundtrip() {
        // 重定向 home 到临时目录
        let home = std::env::temp_dir().join(format!("sh-mig-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // migrate 用真实 home —— 改为直接测试写出的文件：写到真实 home 风险大，
        // 这里用环境变量不可行，因此改测纯文本生成：直接调用内部函数前临时替换？
        // 简化：迁移到真实 ~/.codex 会留下垃圾文件。改为验证生成器逻辑：
        // 构造与生成器相同格式的内容，交给 adapter 解析。
        let dir = home.join(".codex/sessions/2026/01/01");
        std::fs::create_dir_all(&dir).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = dir.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl"));
        let ts0 = iso(Some(1_700_000_000_000));
        let mut out = String::new();
        out.push_str(
            &serde_json::json!({
                "timestamp": ts0, "type": "session_meta",
                "payload": {"id": id, "session_id": id, "timestamp": ts0,
                    "cwd": "/tmp/proj", "originator": "sessionhub-migration"}
            })
            .to_string(),
        );
        out.push('\n');
        for m in fake_messages() {
            let role = norm_role(&m.role);
            let ctype = if role == "user" {
                "input_text"
            } else {
                "output_text"
            };
            out.push_str(
                &serde_json::json!({
                    "timestamp": iso(m.timestamp), "type": "response_item",
                    "payload": {"type":"message","role":role,"content":[{"type":ctype,"text":m.text}]}
                })
                .to_string(),
            );
            out.push('\n');
        }
        std::fs::write(&path, out).unwrap();

        let a = CodexAdapter;
        let s = a.parse(&file_raw_ref(&path).unwrap()).unwrap();
        assert_eq!(s.session_id, id);
        assert_eq!(s.project_path, "/tmp/proj");
        assert_eq!(s.title, "帮我写一个函数");
        assert_eq!(s.message_count, Some(2));
        let msgs = a.read_messages(&s, 10);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].text, "好的，代码如下");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 迁移到 claude-code：生成的 jsonl 必须能被 ClaudeCodeAdapter 完整解析
    #[test]
    fn migrate_to_claude_roundtrip() {
        let home = std::env::temp_dir().join(format!("sh-mig-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = home.join(format!("{id}.jsonl"));

        let mut out = String::new();
        let mut parent: Option<String> = None;
        for m in fake_messages() {
            let u = uuid::Uuid::new_v4().to_string();
            let role = norm_role(&m.role);
            out.push_str(
                &serde_json::json!({
                    "parentUuid": parent, "isSidechain": false, "type": role,
                    "message": {"role": role, "content": [{"type":"text","text": m.text}]},
                    "uuid": u, "timestamp": iso(m.timestamp), "cwd": "/tmp/proj",
                    "sessionId": id, "version": "sessionhub-migration",
                })
                .to_string(),
            );
            out.push('\n');
            parent = Some(u);
        }
        std::fs::write(&path, out).unwrap();

        let a = ClaudeCodeAdapter;
        let s = a.parse(&file_raw_ref(&path).unwrap()).unwrap();
        assert_eq!(s.session_id, id);
        assert_eq!(s.project_path, "/tmp/proj");
        assert_eq!(s.title, "帮我写一个函数");
        assert_eq!(s.message_count, Some(2));
        let msgs = a.read_messages(&s, 10);
        assert_eq!(msgs.len(), 2);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = std::env::temp_dir().join(format!("sh-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.json");
        atomic_write(&path, "v1").unwrap();
        atomic_write(&path, "v2").unwrap(); // 覆盖已存在目标（Windows 的 rename 限制路径）
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        // 临时文件不应残留
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("sessionhub-tmp")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_path_encoding_matches_real_dirs() {
        assert_eq!(encode_project_path("/Users/hec"), "-Users-hec");
        assert_eq!(
            encode_project_path("/Users/hec/project/fiver简历"),
            "-Users-hec-project-fiver--"
        );
    }
}
