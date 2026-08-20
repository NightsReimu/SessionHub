use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::models::RawRef;

/// 构造文件型 RawRef；读不到元数据时返回 None
pub fn file_raw_ref(path: &Path) -> Option<RawRef> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mtime_ms = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(RawRef {
        path: path.to_path_buf(),
        size: md.len(),
        mtime_ms,
        inline: None,
        identity: Some(file_stem(path)),
    })
}

/// 流式逐行读取 JSONL，对每一行调用 f；单行解析失败只跳过，不中断。
/// 返回是否成功打开了文件。
pub fn for_each_jsonl_line<F: FnMut(serde_json::Value)>(path: &Path, mut f: F) -> bool {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let reader = BufReader::with_capacity(1 << 20, file);
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            f(v);
        }
    }
    true
}

pub fn parse_iso_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// 从 Claude Code 风格的 message.content 里提取纯文本
pub fn extract_claude_text(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let mut out = String::new();
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        if !out.is_empty() {
                            out.push(' ');
                        }
                        out.push_str(t);
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 文件名（不含扩展名）
pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

pub fn json_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    v.get(key)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

pub fn json_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key)
        .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f.max(0.0) as u64)))
}

/// 根据文件 mtime 粗略判断“活跃”：15 分钟内有写入视为 recent
pub fn derive_status(mtime_ms: i64) -> String {
    let now = chrono::Utc::now().timestamp_millis();
    if mtime_ms > 0 && now - mtime_ms < 15 * 60 * 1000 {
        "recent".to_string()
    } else {
        "idle".to_string()
    }
}

/// 递归收集目录下匹配扩展名的文件。
/// 返回 (文件列表, 错误数)：根目录不存在或遍历失败都计入错误数，
/// 让扫描器能区分“源文件真的全删了”和“目录暂时不可读”。
pub fn collect_files(root: &Path, exts: &[&str]) -> (Vec<PathBuf>, usize) {
    collect_files_depth(root, exts, usize::MAX)
}

/// 限制最大深度的 collect_files（用于 Application Support 这类大目录）
pub fn collect_files_depth(root: &Path, exts: &[&str], max_depth: usize) -> (Vec<PathBuf>, usize) {
    let mut out = Vec::new();
    let mut errors = 0usize;
    if !root.exists() {
        return (out, 1);
    }
    for entry in walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
    {
        match entry {
            Ok(e) => {
                if !e.file_type().is_file() {
                    continue;
                }
                let name = e.file_name().to_string_lossy();
                if exts.iter().any(|x| name.ends_with(x)) {
                    out.push(e.path().to_path_buf());
                }
            }
            Err(_) => errors += 1,
        }
    }
    (out, errors)
}
