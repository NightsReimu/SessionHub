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
    /// 枚举根目录下的原始对象。
    /// 返回 (对象列表, 错误数)：目录遍历/读取失败必须计入错误数，
    /// 扫描器据此决定全量扫描后是否安全清理索引——
    /// 静默跳过会让“目录暂时不可读”被误判成“会话全被删了”。
    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> (Vec<RawRef>, usize);
    /// 归一化（容错：解析失败返回 None，不 panic）
    fn parse(&self, raw: &RawRef) -> Option<Session>;
    /// 续接命令（打开该对话）；None 表示不支持
    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec>;
    /// 打开 harness 本身（不定位到具体会话）；None 表示不支持
    fn launch_spec(&self, _s: &Session) -> Option<ResumeSpec> {
        None
    }
    fn capabilities(&self) -> Capabilities;
    /// raw_path 是否指向该会话的独立存储（删除/备份/定位前必须检查）。
    /// 为 false 说明 raw_path 回退到了共享/全局文件，操作会误伤其它会话。
    fn can_use_raw_path(&self, s: &Session) -> bool {
        let _ = s;
        true
    }
    /// 缺失的“必需根目录”数量（>0 时全量扫描不得 prune）。
    /// 多根 adapter 用它区分「主目录在 detect 之后消失」（错误，保护索引）
    /// 与「可选目录从未创建」（正常，见 roots() 的过滤）。
    fn required_roots_missing(&self, _ctx: &DetectCtx) -> usize {
        0
    }
    /// 会话级删除许可：capabilities 允许 且 raw_path 指向独立存储
    fn can_delete_session(&self, s: &Session) -> bool {
        self.capabilities().can_delete && self.can_use_raw_path(s)
    }
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
