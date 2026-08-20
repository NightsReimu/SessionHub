use std::path::{Path, PathBuf};
use std::process::Command;

use crate::adapters::claude_code::shell_quote;
use crate::models::Session;

pub fn hub_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SessionHub")
}

pub fn ensure_hub_dirs() -> std::io::Result<()> {
    let hub = hub_dir();
    std::fs::create_dir_all(hub.join("backups"))?;
    std::fs::create_dir_all(hub.join("exports"))?;
    std::fs::create_dir_all(hub.join("tmp"))?;
    Ok(())
}

fn ts_suffix() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 跨平台拉起终端执行续接命令：macOS 用 Terminal.app，Windows 优先 wt，Linux 试常见终端。
pub fn launch_in_terminal(command: &str, cwd: Option<&str>) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let script = match cwd {
            Some(c) if !c.is_empty() => format!("#!/bin/zsh\ncd {} && {}\n", shell_quote(c), command),
            _ => format!("#!/bin/zsh\n{}\n", command),
        };
        let dir = hub_dir().join("tmp");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("resume-{}.command", ts_suffix()));
        std::fs::write(&path, script).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开 Terminal 失败：{e}"))?;
        return Ok(command.to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let cwd_owned = cwd.map(|c| c.to_string());
        let try_wt = Command::new("wt.exe")
            .args({
                let mut a: Vec<String> = Vec::new();
                if let Some(c) = &cwd_owned {
                    a.push("-d".into());
                    a.push(c.clone());
                }
                a.push("cmd".into());
                a.push("/k".into());
                a.push(command.to_string());
                a
            })
            .spawn();
        if try_wt.is_err() {
            Command::new("cmd")
                .args(["/c", "start", "cmd", "/k", command])
                .spawn()
                .map_err(|e| format!("打开终端失败：{e}"))?;
        }
        return Ok(command.to_string());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let shell_cmd = match cwd {
            Some(c) if !c.is_empty() => format!("cd {} && {}; exec $SHELL", shell_quote(c), command),
            _ => format!("{}; exec $SHELL", command),
        };
        for term in ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"] {
            if Command::new(term)
                .args(["-e", "bash", "-lc", &shell_cmd])
                .spawn()
                .is_ok()
            {
                return Ok(command.to_string());
            }
        }
        Err("未找到可用终端模拟器".to_string())
    }
}

/// 删除 = 送回收站，绝不硬删
pub fn trash_raw(session: &Session) -> Result<(), String> {
    let path = Path::new(&session.raw_path);
    if !path.exists() {
        return Err(format!("原始路径不存在：{}", session.raw_path));
    }
    trash::delete(path).map_err(|e| format!("移入回收站失败：{e}"))
}

fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let dest = dir.join(name);
    if !dest.exists() {
        return dest;
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "backup".to_string());
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    dir.join(format!("{}-{}{}", stem, ts_suffix(), ext))
}

/// 备份：文件直接复制；目录打包成 zip
pub fn backup_raw(session: &Session) -> Result<PathBuf, String> {
    let src = Path::new(&session.raw_path);
    if !src.exists() {
        return Err(format!("原始路径不存在：{}", session.raw_path));
    }
    let day = chrono::Local::now().format("%Y%m%d").to_string();
    let dir = hub_dir().join("backups").join(&session.harness_id).join(day);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    if src.is_file() {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session.bin".to_string());
        let dest = unique_dest(&dir, &name);
        std::fs::copy(src, &dest).map_err(|e| format!("复制失败：{e}"))?;
        return Ok(dest);
    }

    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".to_string());
    let dest = unique_dest(&dir, &format!("{name}.zip"));
    let file = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        let rel = p
            .strip_prefix(src)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        if p.is_dir() {
            zip.add_directory(format!("{rel}/"), options).map_err(|e| e.to_string())?;
        } else {
            zip.start_file(rel, options).map_err(|e| e.to_string())?;
            let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
            use std::io::Write;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(dest)
}

/// 导出：Markdown（元数据 + 消息）或 JSONL（原始文件复制 / 消息行）
pub fn export_session(session: &Session, messages: &[crate::models::MessagePreview], format: &str) -> Result<PathBuf, String> {
    let dir = hub_dir().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe_id: String = session
        .session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let base = format!("{}-{}-{}", session.harness_id, safe_id, ts_suffix());

    match format {
        "jsonl" => {
            let src = Path::new(&session.raw_path);
            // raw 是独立会话文件（jsonl/json/generic）时直接复制原文件；
            // 共享数据库或全局索引文件绝不当成“单会话导出”复制出去
            let per_session_raw = src.is_file()
                && matches!(session.source_format.as_str(), "jsonl" | "json" | "generic");
            if per_session_raw {
                let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("jsonl");
                let dest = unique_dest(&dir, &format!("{base}.{ext}"));
                std::fs::copy(src, &dest).map_err(|e| format!("复制失败：{e}"))?;
                return Ok(dest);
            }
            if messages.is_empty() {
                return Err(
                    "没有可导出的内容：该会话的消息不可读取，且 raw 不是独立会话文件".to_string(),
                );
            }
            let dest = unique_dest(&dir, &format!("{base}.jsonl"));
            let mut out = String::new();
            for m in messages {
                out.push_str(
                    &serde_json::json!({
                        "role": m.role, "text": m.text, "timestamp": m.timestamp,
                    })
                    .to_string(),
                );
                out.push('\n');
            }
            std::fs::write(&dest, out).map_err(|e| e.to_string())?;
            Ok(dest)
        }
        _ => {
            let dest = unique_dest(&dir, &format!("{base}.md"));
            let mut md = String::new();
            md.push_str(&format!("# {}\n\n", if session.title.is_empty() { "(无标题)" } else { &session.title }));
            md.push_str(&format!("- Harness: `{}`\n", session.harness_id));
            md.push_str(&format!("- Session ID: `{}`\n", session.session_id));
            md.push_str(&format!("- 项目: `{}`\n", session.project_path));
            if let Some(t) = session.started_at {
                md.push_str(&format!("- 开始: {}\n", chrono::DateTime::from_timestamp_millis(t).map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()));
            }
            if let Some(t) = session.ended_at {
                md.push_str(&format!("- 最后活动: {}\n", chrono::DateTime::from_timestamp_millis(t).map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()));
            }
            md.push_str(&format!("- 原始文件: `{}`\n\n---\n\n", session.raw_path));
            if messages.is_empty() {
                md.push_str("（该 harness 暂不支持读取消息内容）\n");
            } else {
                for m in messages {
                    let role = match m.role.as_str() {
                        "user" => "🧑 User",
                        "assistant" => "🤖 Assistant",
                        r => r,
                    };
                    md.push_str(&format!("## {}\n\n{}\n\n", role, m.text));
                }
            }
            std::fs::write(&dest, md).map_err(|e| e.to_string())?;
            Ok(dest)
        }
    }
}

/// 在系统文件管理器中显示原始文件
pub fn reveal_raw(session: &Session) -> Result<(), String> {
    let path = Path::new(&session.raw_path);
    if !path.exists() {
        return Err(format!("原始路径不存在：{}", session.raw_path));
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        Command::new("xdg-open").arg(dir).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}
