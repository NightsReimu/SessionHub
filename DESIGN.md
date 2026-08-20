# SessionHub — 多 Agent Harness 统一 Session 管理器（设计方案）

> 目标：一个跨平台（macOS + Windows）的**本地桌面客户端**，把电脑上所有 AI coding agent
> （Claude Code、Claude Desktop、Codex、Zcode、OpenCode、DeepSeek Harness、Hermes、OpenClaw、KimiCode 等）
> 的 session 统一发现、浏览、搜索、续接、备份与清理。
>
> 定位：**只读优先、不侵入**。默认绝不改写各 harness 的内部存储，只做"读 + 安全动作"。

---

## 1. 设计原则（先定死，后面所有决策都服从它）

1. **Read-only by default**：扫描和解析只读。删除走系统回收站，不硬删。
2. **Adapter 化**：每个 harness 一个 adapter，新增 harness 不改核心代码。
3. **本地优先**：数据不出本机；索引存本地 SQLite，不联网（同步是可选项）。
4. **防御式解析**：存储格式无官方文档且会变，解析失败要降级而不是崩溃。
5. **流式读大文件**：JSONL 可能几百 MB，逐行解析，不全量载入内存。

---

## 2. 核心能力清单（Feature List）

### 2.1 必须（MVP）
- 自动发现本机已安装的 harness，并扫描其 session 目录
- 统一列表：按 harness / 项目 / 时间 分组、排序、过滤
- 全文搜索（标题 + 对话内容）
- Session 详情：对话时间线、消息数、token/费用（能取到的话）、涉及文件
- 一键续接（Resume）：调用对应 harness 的 resume 命令
- 删除到回收站、备份（复制/zip）
- 标签、备注、收藏（存在自己的 SQLite，不碰 harness 文件）

### 2.2 进阶
- 实时状态（运行中 / 空闲 / 已归档）—— 通过进程检测 + mtime
- 文件系统监听：新 session 自动出现，无需手动刷新
- 导出：单个/批量 → Markdown / JSON / JSONL / zip
- 跨 harness 上下文迁移（可选，接 PAXM 思路）

### 2.3 不做（明确排除）
- 不内置 AI 模型、不代理任何 harness 的请求
- 不替换各 harness 自己的 TUI/CLI（延续 sessions-cli 的设计哲学：不包一层前端）

---

## 3. 技术选型

### 3.1 主推荐：Tauri v2（Rust + Web 前端）

| 维度 | 说明 |
|---|---|
| 跨平台 | macOS（Intel + Apple Silicon）、Windows x64 一套代码 |
| 体积 | 约 10MB，远小于 Electron 的 150MB |
| 后端语言 | Rust：天然适合流式解析大 JSONL、notify 文件监听、rusqlite SQLite、trash 回收站 |
| 前端 | React + TypeScript + Tailwind + 虚拟列表（@tanstack/react-virtual） |
| 权限 | Tauri v2 有细粒度 capability，安全边界清晰 |
| 打包 | tauri-action + GitHub Actions，产出 .dmg / .msi / .exe |

### 3.2 备选：Electron（如果你是纯 JS 技术栈、想更快出活）
- 优点：生态熟、Node 直接读文件、better-sqlite3 / sql.js
- 缺点：体积大、内存占用高、文件 IO 性能弱于 Rust
- 结论：如果你主写 TS，用 Electron 也行；否则用 Tauri。

> 关键依赖（Tauri 路线）：rusqlite（bundled+fts5）、serde/serde_json、notify、trash、tauri-plugin-shell/open、walkdir/globset。

---

## 4. 总体架构（分层）

```
┌─────────────────────────────────────────────────────┐
│  UI 层 (React/TS)                                    │
│  Dashboard · Session 列表 · 详情 · 搜索 · 设置 · 备份  │
└──────────────────────┬──────────────────────────────┘
                       │ Tauri IPC (#[tauri::command])
┌──────────────────────▼──────────────────────────────┐
│  Core 服务层 (Rust)                                   │
│  Registry(adapter发现)  Scanner(遍历/解析)  Watcher(notify)  ActionExecutor │
│  Adapters：claude-code / claude-desktop / codex / opencode /              │
│            dsh / zcode / kimi / openclaw / hermes / generic              │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│  数据层：本地 SQLite 索引(metadata + FTS5) + 只读原始文件 │
└─────────────────────────────────────────────────────┘
```

---

## 5. 统一数据模型（Normalized Session）

所有 adapter 把各自的格式归一化成同一种结构，UI 只认这一种：

```ts
interface Session {
  id: string;            // harness 内唯一（文件 stem 或 db 主键）
  harnessId: string;     // "claude-code" | "codex" | "opencode" | "dsh" | "zcode" | ...
  projectKey: string;    // 解码后的项目路径 / 仓库名
  projectPath?: string;
  title: string;         // 首条用户消息，或自动摘要
  createdAt: number;
  updatedAt: number;
  messageCount: number;
  tokenIn?: number; tokenOut?: number; costUsd?: number;
  status: "idle" | "running" | "archived";
  tags: string[];
  note?: string;
  rawPath?: string;      // 源文件/目录绝对路径
  sourceFormat: "jsonl" | "sqlite" | "json" | "dir";
}
```

索引表（SQLite）：

```sql
CREATE TABLE sessions (
  id TEXT, harness_id TEXT, project_key TEXT, project_path TEXT,
  title TEXT, created_at INTEGER, updated_at INTEGER,
  message_count INTEGER, token_in INTEGER, token_out INTEGER, cost_usd REAL,
  status TEXT, raw_path TEXT, source_format TEXT, tags TEXT, note TEXT,
  PRIMARY KEY (harness_id, id)
);
CREATE VIRTUAL TABLE sessions_fts USING fts5(title, body, content='sessions', content_rowid='rowid');
```

> 全文只索引摘要 + 每条消息前 N 字（如 500 字），完整正文按需从原始文件实时读，避免索引库爆炸。

---

## 6. Adapter 抽象（整个系统的核心）

```rust
pub struct ResumeSpec {
    pub program: String,    // "claude" / "codex" / "opencode" / "dsh" ...
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub use_terminal: bool, // 是否在独立终端窗口里跑
}
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn detect(&self, ctx: &DetectCtx) -> bool;          // 是否安装
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf>;   // 要扫描的根目录（按平台）
    fn enumerate(&self, root: &Path) -> Vec<RawRef>;    // 找到 session 条目
    fn parse(&self, r: &RawRef) -> Option<Session>;     // 归一化（容错）
    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec>;
    fn capabilities(&self) -> Capabilities;
}
```

DetectCtx 提供：home_dir、os（macos/windows/linux）、data_dirs（~/Library/Application Support / %APPDATA% / ~/.local/share）。

**新增 harness = 只加一个实现了该 trait 的文件 + 注册一行**，核心零改动。

---

## 7. 各 Harness Adapter 规格（已实测，含真实路径）

> 下面是本机实测（macOS）拿到的真实路径，Windows 用对应等价路径。

| Harness | 存储位置 | 格式 | Resume 命令 |
|---|---|---|---|
| Claude Code (CLI) | ~/.claude/projects/<编码项目路径>/<session-id>.jsonl + sessions-index.json | 每行一个 JSON | claude --resume <id> |
| Claude Desktop | macOS ~/Library/Application Support/Claude/conversations/*.json；Windows %APPDATA%\Claude\conversations | JSON 一文件一会话 | 打开 App |
| Codex (CLI) | ~/.codex/sessions/<年>/<月>/rollout-<时间戳>-<uuid>.jsonl + ~/.codex/archived_sessions/*.jsonl | JSONL | codex resume <id> |
| Codex (Desktop) | ~/Library/Application Support/Codex / %APPDATA%\Codex | JSONL / 内部库 | 打开 App |
| OpenCode | ~/.local/share/opencode/opencode.db（Windows %USERPROFILE%\.local\share\opencode） | SQLite（session/message/part/todo/project/workspace） | opencode --continue / --session <id> |
| DeepSeek Harness (DSH) | ~/.dsh/sessions/<编码项目>/<session-dir>/；元数据 ~/.dsh/storages/session_projcache.json、workspace.json | 目录 + JSON（session_projcache.json 是现成索引入口） | dsh 打开 Web/headless |
| Zcode | ~/.zcode/cli/{db,rollout,exec,log,artifacts}（Codex 血统） | db/ SQLite + rollout/ JSONL | zcode（子命令待确认） |
| Kimi Code | ~/.kimi/（未安装，待确认） | JSONL/内部库（待确认） | kimi |
| OpenClaw | ~/.openclaw/（旧名 ~/.clawdbot、~/.moltbot） | 目录 + JSON（待确认） | openclaw |
| Hermes | 未知（若是模型而非 harness 则无 session） | — | 用 generic 兜底 |
| Generic（兜底） | 用户指定目录 + glob | 自动识别 JSONL/JSON/SQLite/NDJSON | 自定义命令模板 |

### 关键实现提示
1. DSH：优先解析 ~/.dsh/storages/session_projcache.json（已是"项目→session"索引），再落到 sessions/<编码项目>/<session>/ 读正文。
2. OpenCode：sqlite 只读打开 opencode.db，查 session + message 表，WAL 模式用只读连接也能读。
3. Codex：sessions/<年>/<月>/ 双层目录要递归；archived_sessions 平铺，单列一组"已归档"。
4. 通用：serde_json 流式 + 宽容模式，未知字段忽略、坏行跳过并计数（UI 显示"N 行解析失败"）。

---

## 8. 索引与搜索策略

- 冷启动：全量扫描一次（后台线程 + 进度条），写入 SQLite。
- 增量：notify 监听各 harness 根目录，debounce（800ms）后只重扫变化项。
- 缓存失效：每个 adapter 带 format_version，格式变化时强制重建对应索引。
- 运行中跳过：文件被写锁 / mtime 很近时跳过或标记"解析中"，稍后重试。

---

## 9. 动作（Action）设计

| 动作 | 实现 | 安全性 |
|---|---|---|
| 续接 | tauri-plugin-shell 执行 ResumeSpec；use_terminal 时 macOS 用 open -a Terminal，Windows 用 wt/start | 只读启动 |
| 删除 | trash crate 送回收站 | 可恢复 |
| 备份 | 复制到 ~/SessionHub/backups/<harness>/<日期>/<id> 或打包 zip | 不碰原文件 |
| 重命名/标签/备注/收藏 | 只写 app 自己的 SQLite | 完全安全 |
| 导出 | 单条/批量 → Markdown / JSON / JSONL / zip | 只读 |
| 实时状态 | 检测进程名 + 源文件 mtime | 只读 |

---

## 10. 目录结构（Tauri 路线）

```
sessionhub/
├── package.json
├── src/                    # React 前端
│   ├── App.tsx
│   ├── views/              # Dashboard / Sessions / Search / Settings / Backup
│   ├── components/         # SessionCard / TranscriptViewer / VirtualList ...
│   ├── stores/             # zustand / jotai
│   └── styles/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── icons/
│   └── src/
│       ├── main.rs
│       ├── commands.rs     # IPC 命令（list/search/resume/delete/backup/export）
│       ├── model.rs
│       ├── ctx.rs          # DetectCtx
│       ├── registry.rs     # adapter 注册表
│       ├── scanner.rs
│       ├── index.rs        # SQLite + FTS5
│       ├── watcher.rs
│       ├── actions.rs
│       └── adapters/
│           ├── mod.rs
│           ├── claude_code.rs / claude_desktop.rs / codex.rs / opencode.rs
│           ├── dsh.rs / zcode.rs / kimi.rs / openclaw.rs / hermes.rs / generic.rs
├── .github/workflows/release.yml
├── README.md
└── LICENSE
```

---

## 11. 里程碑（Roadmap）

| 阶段 | 目标 | 产出 | 预估 |
|---|---|---|---|
| M0 | 脚手架 | Tauri + React + IPC + SQLite 索引跑通 | 0.5–1 天 |
| M1 | 核心 3 个 adapter | Claude Code / Codex / OpenCode：列表、搜索、详情、回收站删除 | 2–3 天 |
| M2 | 补齐本地 | DSH / Zcode adapter + 续接 + 备份/导出 | 2 天 |
| M3 | 桌面与其它 | Claude Desktop / Kimi / OpenClaw + 标签备注收藏 + 实时状态 | 2 天 |
| M4 | 扩展与发布 | Generic adapter + 插件机制 + 设置页 + 打包发布 | 1–2 天 |
| M5 | 可选增强 | 跨设备同步、AI 生成标题、token/费用面板 | 后续 |

> 总计约 1.5–2 周可出可用 MVP 并发布到 GitHub Releases。

---

## 12. 风险与对策

| 风险 | 对策 |
|---|---|
| 存储格式无文档、随版本变化 | 防御式解析 + format_version 缓存失效 + 解析失败降级为 raw 查看器 |
| 大 JSONL 拖慢 UI | 流式逐行解析；全文只索引摘要+前 N 字；正文按需读 |
| 误改 harness 内部数据 | 全程只读；删除走回收站；标签备注存自己的库 |
| 并发写（agent 正在跑） | 跳过被锁文件；mtime 很近标记"解析中"；notify debounce |
| macOS 权限（Desktop/下载/文稿） | 首次扫描给明确授权提示（Tauri capability/entitlements） |
| Windows 路径差异 | 统一走 dirs crate（%APPDATA%/%USERPROFILE%/%LOCALAPPDATA%） |
| 隐私 | 全本地、无网络；提供"不索引某目录/字段"开关 |

---

## 13. GitHub 发布与 CI/CD

- 仓库：https://github.com/NightsReimu/<repo-name>（gh repo create 创建）
- License：MIT（或 Apache-2.0）
- README：徽章、截图、支持列表、安装说明
- Releases：GitHub Actions + tauri-apps/tauri-action，打 tag（v*）自动构建：
  - macOS：aarch64-apple-darwin + x86_64-apple-darwin（或 universal），产物 .dmg + .app
  - Windows：x86_64-pc-windows-msvc，产物 .msi + .exe
- 签名：macOS 本地 ad-hoc 签名即可；Windows 无证书会触发 SmartScreen，先提供 zip 版后续再签
- 自动更新：tauri-plugin-updater（可选，M4 之后）

---

## 14. 命名建议

SessionHub（推荐，直白）/ OmniSession / AgentVault / HarnessDeck / SessionDeck / CrossHarness

---

## 附：动手第一步

```bash
npm create tauri-app@latest sessionhub -- --template react-ts
cd sessionhub
npm install
npm run tauri dev

cargo add rusqlite --features bundled,fts5
cargo add serde serde_json notify trash dirs walkdir globset
cargo add tauri-plugin-shell tauri-plugin-dialog tauri-plugin-opener
# 先只实现 claude_code.rs 一个 adapter，跑通"扫描→索引→列表"
```
