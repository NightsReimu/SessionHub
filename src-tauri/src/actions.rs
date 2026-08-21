use std::path::{Path, PathBuf};
use std::process::Command;

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

/// 启动脚本唯一文件名：秒级时间戳 + 随机后缀。
/// 快速连续启动两个会话时绝不能互相覆盖（否则前一个终端会执行后一个会话的脚本）
fn launch_script_stem() -> String {
    format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

fn ts_suffix() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 清理 tmp 下超过 1 天的启动脚本，避免无限堆积
fn prune_old_launch_scripts(dir: &Path) {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("resume-") {
            continue;
        }
        let too_old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if too_old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 跨平台拉起终端执行 argv：macOS 写 zsh 脚本，Windows 写 PowerShell 脚本，
/// Linux 试常见终端。argv 逐元素按目标 shell 的引用规则转义 —— 会话 id、路径
/// 里的元字符（`&`、`|`、`;`、反引号等）不可能被解释成命令。
pub fn launch_in_terminal(argv: &[String], cwd: Option<&str>) -> Result<String, String> {
    if argv.is_empty() {
        return Err("启动命令为空".to_string());
    }
    let display = argv
        .iter()
        .map(|a| crate::models::posix_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let dir = hub_dir().join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    prune_old_launch_scripts(&dir);

    #[cfg(target_os = "macos")]
    {
        let quoted = argv
            .iter()
            .map(|a| crate::models::posix_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        let script = match cwd {
            Some(c) if !c.is_empty() => {
                format!(
                    "#!/bin/zsh\ncd {} && {}\n",
                    crate::models::posix_quote(c),
                    quoted
                )
            }
            _ => format!("#!/bin/zsh\n{}\n", quoted),
        };
        let path = dir.join(format!("resume-{}.command", launch_script_stem()));
        std::fs::write(&path, script).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }
        Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开 Terminal 失败：{e}"))?;
        Ok(display)
    }
    #[cfg(target_os = "windows")]
    {
        // cmd.exe 不把单引号当引用符，任何字符串拼接都可能被 `&` 之类截断；
        // 因此改为生成 PowerShell 脚本（单引号是真正的引用），只把我们自己
        // 生成的安全脚本路径交给 cmd。
        let quoted = argv
            .iter()
            .map(|a| crate::models::powershell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        let mut script = String::from("$ErrorActionPreference = 'Continue'\r\n");
        if let Some(c) = cwd {
            if !c.is_empty() {
                script.push_str(&format!(
                    "Set-Location -LiteralPath {}\r\n",
                    crate::models::powershell_quote(c)
                ));
            }
        }
        // 首元素是程序名，用 & 调用运算符执行，其余作为参数
        script.push_str(&format!("& {}\r\n", quoted));
        let path = dir.join(format!("resume-{}.ps1", launch_script_stem()));
        std::fs::write(&path, script).map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy().into_owned();
        let ps_args = [
            "-NoExit",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            path_str.as_str(),
        ];
        if Command::new("wt.exe")
            .arg("powershell")
            .args(ps_args)
            .spawn()
            .is_err()
        {
            Command::new("powershell")
                .args(ps_args)
                .spawn()
                .map_err(|e| format!("打开终端失败：{e}"))?;
        }
        Ok(display)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let quoted = argv
            .iter()
            .map(|a| crate::models::posix_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        let shell_cmd = match cwd {
            Some(c) if !c.is_empty() => {
                format!(
                    "cd {} && {}; exec $SHELL",
                    crate::models::posix_quote(c),
                    quoted
                )
            }
            _ => format!("{}; exec $SHELL", quoted),
        };
        for term in ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"] {
            if Command::new(term)
                .args(["-e", "bash", "-lc", &shell_cmd])
                .spawn()
                .is_ok()
            {
                return Ok(display);
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

/// 备份：文件直接复制；目录打包成 zip。dest_dir 为 None 时进默认 backups 目录
pub fn backup_raw(session: &Session, dest_dir: Option<&str>) -> Result<PathBuf, String> {
    let src = Path::new(&session.raw_path);
    if !src.exists() {
        return Err(format!("原始路径不存在：{}", session.raw_path));
    }
    let dir = match dest_dir {
        Some(d) => PathBuf::from(d),
        None => {
            let day = chrono::Local::now().format("%Y%m%d").to_string();
            hub_dir()
                .join("backups")
                .join(&session.harness_id)
                .join(day)
        }
    };
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
            zip.add_directory(format!("{rel}/"), options)
                .map_err(|e| e.to_string())?;
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

/// 导出：Markdown（元数据 + 消息）或 JSONL（原始文件复制 / 消息行）。
/// `complete=false` 表示消息可能被上限截断，必须在产物中标注。
pub fn export_session(
    session: &Session,
    messages: &[crate::models::MessagePreview],
    format: &str,
    dest_path: Option<&str>,
    complete: bool,
) -> Result<PathBuf, String> {
    let dir = hub_dir().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 用户在保存对话框里选了完整路径时直接使用（覆盖已有文件是用户的明确选择）
    let custom_dest: Option<PathBuf> = dest_path.map(|d| {
        let p = PathBuf::from(d);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        p
    });
    let safe_id: String = session
        .session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
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
                let dest = match custom_dest {
                    Some(d) => d,
                    None => {
                        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("jsonl");
                        unique_dest(&dir, &format!("{base}.{ext}"))
                    }
                };
                std::fs::copy(src, &dest).map_err(|e| format!("复制失败：{e}"))?;
                return Ok(dest);
            }
            if messages.is_empty() {
                return Err(
                    "没有可导出的内容：该会话的消息不可读取，且 raw 不是独立会话文件".to_string(),
                );
            }
            let dest = match custom_dest {
                Some(d) => d,
                None => unique_dest(&dir, &format!("{base}.jsonl")),
            };
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
            // 可能被截断时追加一条显式告警记录，不静默交付不完整的 JSONL
            if !complete {
                out.push_str(
                    &serde_json::json!({
                        "type": "sessionhub_truncation_warning",
                        "message": "该 harness 不支持完整消息读取，本文件可能不完整",
                        "exported_messages": messages.len(),
                    })
                    .to_string(),
                );
                out.push('\n');
            }
            std::fs::write(&dest, out).map_err(|e| e.to_string())?;
            Ok(dest)
        }
        _ => {
            let dest = match custom_dest {
                Some(d) => d,
                None => unique_dest(&dir, &format!("{base}.md")),
            };
            let mut md = String::new();
            md.push_str(&format!(
                "# {}\n\n",
                if session.title.is_empty() {
                    "(无标题)"
                } else {
                    &session.title
                }
            ));
            md.push_str(&format!("- Harness: `{}`\n", session.harness_id));
            md.push_str(&format!("- Session ID: `{}`\n", session.session_id));
            md.push_str(&format!("- 项目: `{}`\n", session.project_path));
            if let Some(t) = session.started_at {
                md.push_str(&format!(
                    "- 开始: {}\n",
                    chrono::DateTime::from_timestamp_millis(t)
                        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_default()
                ));
            }
            if let Some(t) = session.ended_at {
                md.push_str(&format!(
                    "- 最后活动: {}\n",
                    chrono::DateTime::from_timestamp_millis(t)
                        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_default()
                ));
            }
            md.push_str(&format!("- 原始文件: `{}`\n", session.raw_path));
            md.push_str(&format!("- 消息条数: {}\n", messages.len()));
            if !complete {
                md.push_str(
                    "- ⚠️ 该 harness 不支持完整消息读取，以下内容可能已被截断，不能视为完整备份\n",
                );
            }
            md.push_str("\n---\n\n");
            if messages.is_empty() {
                md.push_str("（该 harness 暂不支持读取消息内容）\n");
            } else {
                for m in messages {
                    let role = match m.role.as_str() {
                        "user" => "User",
                        "assistant" => "Assistant",
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
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
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
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
