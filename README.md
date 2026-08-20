# SessionHub

一个**本地、只读优先、跨平台（macOS + Windows）**的桌面客户端，用「Adapter 插件」架构把
Claude Code、Codex、OpenCode、DeepSeek Harness、Zcode 等 AI coding harness 的会话
**统一发现 → 浏览 → 搜索 → 续接 → 备份 / 清理**。

![技术栈](https://img.shields.io/badge/Tauri_v2-Rust_%2B_React-blue)

## 五条定死的原则

1. **Read-only by default** —— 只读；删除一律走系统回收站，绝不硬删
2. **Adapter 化** —— 每个 harness 一个 adapter，新增 harness 不改核心
3. **本地优先** —— 索引存本地 SQLite（`~/SessionHub/sessionhub.db`），不联网
4. **防御式解析** —— 各家格式无文档且会变，单行/单文件解析失败只跳过，不崩溃
5. **流式读大文件** —— 几百 MB 的 JSONL 逐行解析，不全量载入内存

## 技术栈

- **壳**：Tauri v2（Rust 后端 + React/TypeScript 前端），安装包约 10MB
- **后端**：rusqlite（FTS5 全文搜索，不可用时降级 LIKE）、notify（文件监听）、trash（回收站）、walkdir、zstd（DSH 压缩会话）
- **前端**：React 18 + Tailwind CSS 4 + @tanstack/react-virtual（虚拟列表）

## 架构分层

```
UI (React) ──IPC──> Core (Rust) ──> 数据层
                     ├─ Registry   (adapter 注册/发现)
                     ├─ Scanner    (遍历/解析) / Watcher (notify 增量)
                     ├─ ActionExecutor (续接/备份/删除/导出)
                     └─ Adapters   (claude-code/codex/opencode/zcode/dsh/generic)
```

统一数据模型 `Session`（id / harnessId / projectPath / title / 起止时间 / 消息数 /
token / 费用 / 状态 / rawPath / sourceFormat），核心是一个 Rust trait：

```rust
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn detect(&self, ctx: &DetectCtx) -> bool;        // 是否安装
    fn roots(&self, ctx: &DetectCtx) -> Vec<PathBuf>; // 扫描根目录
    fn enumerate(&self, root: &Path, ctx: &DetectCtx) -> Vec<RawRef>;
    fn parse(&self, raw: &RawRef) -> Option<Session>; // 归一化（容错）
    fn resume_spec(&self, s: &Session) -> Option<ResumeSpec>;
    fn capabilities(&self) -> Capabilities;
}
```

新增 harness = 在 `src-tauri/src/adapters/` 加一个实现该 trait 的文件 + 在
`all_adapters()` 注册一行。

## 支持的 Harness（macOS 实测路径）

| Harness | 存储位置 | 格式 | 状态 |
|---|---|---|---|
| Claude Code | `~/.claude/projects/<编码项目>/<id>.jsonl` + `sessions-index.json` | JSONL | ✅ 完整支持 |
| Codex | `~/.codex/sessions/<年>/<月>/<日>/rollout-*.jsonl` + `archived_sessions/` | JSONL | ✅ 完整支持 |
| OpenCode | `~/.local/share/opencode/opencode.db`（只读打开） | SQLite | ✅ 完整支持 |
| Zcode | `~/.zcode/cli/db/db.sqlite`（与 OpenCode 同 schema） | SQLite | ✅ 完整支持 |
| DeepSeek Harness | `~/.dsh/storages/session_projcache.json` + `~/.dsh/sessions/*/session.jsonl.zstd` | JSON 索引 + zstd JSONL | ✅ 完整支持 |
| Claude Desktop / Kimi / OpenClaw / Hermes | 见 `adapters/generic.rs` | 待确认 | 🕐 占位探测 + Generic 兜底 |

> SQLite 型 harness（OpenCode/Zcode）的会话存在共享数据库里，因此**不支持删除单会话**；
> 备份/导出不受限。

## 动作

- **续接**：`claude --resume <id>` / `codex resume <id>` / `opencode --continue` /
  `zcode --continue` / `dsh`；macOS 经 `Terminal.app` 拉起，Windows 优先 `wt`（退回 `cmd /k`）
- **删除**：`trash` crate 送系统回收站；标签备注保留在 SessionHub 库中
- **备份**：复制到 `~/SessionHub/backups/<harness>/<日期>/`（目录自动打 zip）
- **导出**：Markdown（元数据 + 消息）或 JSONL，写入 `~/SessionHub/exports/`
- **标签 / 备注 / 收藏**：只写 SessionHub 自己的 SQLite，**完全不碰 harness 文件**
- **实时监听**：notify 监听各 harness 根目录，文件变化去抖 800ms 后增量重扫并推送前端
- **用量统计**：侧栏「📊 用量统计」按 harness 聚合 token/费用，列出消耗 Top 会话
- **配置插件**：`~/SessionHub/adapters.json` 的 `generic_extra_roots` 可让 GenericAdapter
  扫描任意自定义目录，免重编译；写正式 adapter 见 [docs/ADAPTERS.md](docs/ADAPTERS.md)

## 开发

```bash
npm install
npm run tauri dev        # 开发模式（热更新）
```

## 构建

```bash
# macOS 本地构建（跳过容易超时的 dmg 步骤，只出 .app）
npx tauri build --bundles app
scripts/make-dmg.sh      # 可选：手动打 .dmg

# Windows 上直接 npx tauri build 即可（.msi / .exe）
```

产物在 `src-tauri/target/release/bundle/`。

### GitHub Actions 跨平台发布

`.github/workflows/release.yml`：推 `v*` tag（或手动触发）即在 macOS(arm64 + Intel) 和
Windows 三个 runner 上并行构建，自动创建 Draft Release 并上传 `.dmg` / `.msi` / `.exe`：

```bash
git tag v0.1.0 && git push origin v0.1.0
```

> 注：tauri 内置的 dmg 打包步骤要调 Finder AppleScript 摆图标，在某些本地终端会话里会超时
> （`AppleEvent已超时 -1712`），CI runner 上无此问题；本地需要 dmg 时跑 `scripts/make-dmg.sh`。

### 签名说明

- macOS：本地 ad-hoc 签名即可运行；分发 `.dmg` 时用户需在「系统设置 → 安全性」放行
- Windows：无证书会触发 SmartScreen，建议先发 zip 版

### 测试

```bash
cd src-tauri
cargo test                     # 默认：prune 安全矩阵 / WAL 扫描戳 / DSH raw 防护 / 流式解析等单元回归测试
cargo test scan_real_machine_smoke -- --ignored --nocapture   # 可选：对本机真实 harness 目录的只读冒烟扫描
```

## Roadmap

- [x] M0 脚手架（Tauri + React + IPC + SQLite）
- [x] M1 核心 3 adapter：Claude Code / Codex / OpenCode
- [x] M2 DSH / Zcode + 续接 + 备份导出
- [x] M3 占位探测（Claude Desktop / Kimi / OpenClaw / Hermes）+ 标签备注收藏 + 实时状态
- [x] M4 配置插件（adapters.json 自定义根目录）+ [adapter 开发文档](docs/ADAPTERS.md) + 打包发布（GitHub Actions）
- [x] M5-lite token/费用统计面板（本地聚合，不联网）
- [ ] M5（可选）跨设备同步、AI 生成标题（需联网，与「本地优先」权衡后再定）
