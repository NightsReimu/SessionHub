pub mod claude_code;
pub mod codex;
pub mod dsh;
pub mod generic;
pub mod sqlite_sessions;
pub mod util;

use std::path::{Path, PathBuf};

use crate::models::{Capabilities, MessagePreview, RawRef, ResumeSpec, Session};

pub struct DetectCtx {
    pub home: PathBuf,
}

impl DetectCtx {
    pub fn new() -> Self {
        Self {
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        }
    }
    pub fn join(&self, p: &str) -> PathBuf {
        self.home.join(p)
    }
}

/// 核心抽象：每个 harness 一个 adapter，加新 harness = 实现这个 trait + 注册一行。
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// 是否安装（根目录存在）
    fn detect(&self, ctx: &DetectCtx) -> bool;
    /// 扫描根目录
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf>;
    /// 枚举根目录下的原始对象
    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> Vec<RawRef>;
    /// 归一化（容错：解析失败返回 None，不 panic）
    fn parse(&self, raw: &RawRef) -> Option<Session>;
    /// 续接命令；None 表示不支持
    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec>;
    fn capabilities(&self) -> Capabilities;
    /// 读取消息预览（用于详情面板和导出），不支持则返回空
    fn read_messages(&self, _s: &Session, _limit: usize) -> Vec<MessagePreview> {
        Vec::new()
    }
}

pub fn all_adapters() -> Vec<Box<dyn HarnessAdapter>> {
    vec![
        Box::new(claude_code::ClaudeCodeAdapter),
        Box::new(codex::CodexAdapter),
        Box::new(sqlite_sessions::OpenCodeAdapter),
        Box::new(sqlite_sessions::ZcodeAdapter),
        Box::new(dsh::DshAdapter),
        // 未安装/未确认的桌面端：标记“待确认”，命中目录时走 generic 兜底
        Box::new(generic::PlaceholderAdapter::claude_desktop()),
        Box::new(generic::PlaceholderAdapter::kimi_code()),
        Box::new(generic::PlaceholderAdapter::openclaw()),
        Box::new(generic::PlaceholderAdapter::hermes()),
        Box::new(generic::GenericAdapter),
    ]
}
