# 新增 Harness Adapter 指南

SessionHub 的所有 harness 支持都通过 `HarnessAdapter` trait 接入。新增一个 harness
**不需要改核心代码**：在 `src-tauri/src/adapters/` 加一个文件，再在
`adapters/mod.rs` 的 `all_adapters()` 里注册一行。

## 不想写代码？先试配置插件

如果只是想让某个目录被兜底规则扫描，编辑 `~/SessionHub/adapters.json`：

```json
{
  "generic_extra_roots": ["~/Library/Application Support/SomeApp/sessions", "/abs/path"]
}
```

GenericAdapter 会对这些目录做启发式解析（`.jsonl` / `.json`，自动找 id/cwd/title/时间戳），
无需重新编译。启发式不够用时，再按下面的步骤写正式 adapter。

## 生命周期

```
detect(ctx) ──> roots(ctx) ──> enumerate(root) ──> parse(raw) ──> 入库
     │                                  │
     │                                  └─ 决定「删除/备份/定位」是否允许（can_use_raw_path）
     └─ 为 false 时扫描器整体跳过本 adapter（也不会 prune 索引）
```

## Trait 契约（含必须遵守的安全规则）

```rust
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;          // 全局唯一，小写连字符
    fn name(&self) -> &'static str;        // UI 显示名
    fn detect(&self, ctx: &DetectCtx) -> bool;
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf>;
    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> (Vec<RawRef>, usize);
    fn parse(&self, raw: &RawRef) -> Option<Session>;
    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec>;
    /// 打开 harness 本体（不定位到具体会话）
    fn launch_spec(&self, s: &Session) -> Option<ResumeSpec> { None }
    fn capabilities(&self) -> Capabilities;
    fn can_use_raw_path(&self, s: &Session) -> bool { true }
    fn can_delete_session(&self, s: &Session) -> bool { ... } // 默认 = can_delete && can_use_raw_path
    fn read_messages(&self, s: &Session, limit: usize) -> Vec<MessagePreview> { vec![] }
}
```

### 规则 1：enumerate 必须如实上报错误

返回 `(raws, error_count)`。目录遍历失败、文件不可读、索引损坏都要 `errors += 1`。
**这是 prune 安全护栏的数据来源**：全量扫描只在 `errors == 0` 时清理索引。
静默跳过 = 把「目录暂时不可读」谎报成「会话全被删了」= 索引被清空。

### 规则 2：可选根目录在 roots() 里过滤

像 Codex 的 `archived_sessions` 这种可能从未创建的目录，在 `roots()` 里用
`.filter(|p| p.is_dir())` 过滤掉，而不是留给 enumerate 报错——否则它会永远
阻断 prune（参见 codex.rs）。`detect()` 则要求「至少一个根存在」。

### 规则 3：parse 只许容错，不许 panic

格式无文档且会变。单行解析失败跳过该字段；整体无法解析返回 `None`；
扫描器还会在外层 `catch_unwind` 兜底，但 adapter 自己也不应 panic。

### 规则 4：raw_path 回退到共享文件时，覆盖 can_use_raw_path

如果某些会话的 `raw_path` 不得不指向共享/全局文件（如 DSH 回退到
`session_projcache.json`），必须覆写：

```rust
fn can_use_raw_path(&self, s: &Session) -> bool {
    !s.raw_path.ends_with("session_projcache.json")
}
```

删除 / 备份 / 定位三个动作共用这个检查，前端也会据此禁用按钮。

### 规则 5：大文件必须流式

JSONL 逐行（`util::for_each_jsonl_line`）、压缩流（zstd Decoder + BufReader）、
消息预览用环形缓冲限制内存（参考 dsh.rs）。不要 `read_to_string` 整文件。

### 规则 6：提供廉价 identity

`RawRef.identity`（文件 stem / 行 id）让增量扫描跳过未变化文件的昂贵 parse。
SQLite 型 adapter 的扫描戳记得把 `*.db-wal` 的 size/mtime 算进去（参考
sqlite_sessions.rs）。

## 模板

```rust
use std::path::{Path, PathBuf};
use super::util::*;
use super::{DetectCtx, HarnessAdapter};
use crate::models::{Capabilities, RawRef, ResumeSpec, Session};

pub struct MyHarnessAdapter;

impl HarnessAdapter for MyHarnessAdapter {
    fn id(&self) -> &'static str { "my-harness" }
    fn name(&self) -> &'static str { "My Harness" }
    fn detect(&self, ctx: &DetectCtx) -> bool { ctx.join(".myharness/sessions").is_dir() }
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf> { vec![ctx.join(".myharness/sessions")] }

    fn enumerate(&self, root: &Path, _ctx: &DetectCtx) -> (Vec<RawRef>, usize) {
        let (files, mut errors) = collect_files(root, &[".jsonl"]);
        let mut out = Vec::new();
        for p in files {
            match file_raw_ref(&p) {
                Some(raw) => out.push(raw),
                None => errors += 1,
            }
        }
        (out, errors)
    }

    fn parse(&self, raw: &RawRef) -> Option<Session> {
        // 流式逐行读，容错提取 id/cwd/title/时间/token……
        None
    }

    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec> {
        Some(ResumeSpec {
            command: format!("myharness resume {}", s.session_id),
            cwd: None,
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { can_resume: true, can_delete: true, can_backup: true, can_read_messages: false }
    }
}
```

然后 `adapters/mod.rs`：

```rust
pub mod my_harness;
// all_adapters() 里加一行：
Box::new(my_harness::MyHarnessAdapter),
```

### 规则 7：resume 必须真实可达

`resume_spec` 返回 `None` 就是「不支持」，CLI 没有按会话恢复机制时（如 zcode/DSH）
把 `can_resume` 设为 `false`、只保留 `launch_spec` 打开本体。
`ResumeSpec` 的 `command` 仅用于展示，执行一律走 `argv`（逐元素平台转义），
绝不把拼接字符串交给 shell。

### 规则 8：完整读取必须诚实

`read_messages_full` 用于迁移/导出：不截断、不限条数；
源文件打不开或读取中途失败时返回 `None`（调用方会明确报错），
绝不返回空数组假装「这个会话没有消息」。预览接口 `read_messages` 可以保持宽容。

## 验收清单

- [ ] `cargo test` 全绿（至少加一个 parse 单元测试，参考 claude_code.rs 的 tests）
- [ ] `enumerate` 错误数如实上报（手动 chmod 000 一个目录验证全量扫描不丢索引）
- [ ] `cargo test scan_real_machine_smoke -- --ignored --nocapture` 本机验证
- [ ] UI 里 adapter 显示为已检测，会话能搜索/续接/导出
