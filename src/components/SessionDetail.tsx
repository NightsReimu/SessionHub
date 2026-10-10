import { memo, useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { AdapterInfo, MessagePreview, SessionDto, api } from "../api";
import { fmtDateTime, fmtTime, fmtTokens, fmtUsd, shortPath } from "../lib/format";
import { harnessName } from "../lib/harness";
import {
  ArchiveIcon,
  ChatIcon,
  CloseIcon,
  DownloadIcon,
  ExternalIcon,
  FolderIcon,
  PencilIcon,
  PlayIcon,
  StarIcon,
  SwapIcon,
  TrashIcon,
} from "./icons";
import MessageText from "./MessageText";
import { Button, EmptyState, HarnessTag, IconButton, Toast } from "./ui";

interface Props {
  session: SessionDto;
  adapters: AdapterInfo[];
  onPatch: (s: SessionDto) => void;
  onRemoved: (s: SessionDto) => void;
  onClose: () => void;
  onMigrated: () => void;
  toast: (kind: Toast["kind"], text: string) => void;
}

const MIGRATE_TARGETS = ["claude-code", "codex", "opencode"];

/** 对话气泡列表：只随消息数组变化重渲染（扫描进度等父级更新不会波及） */
const MessageList = memo(function MessageList({ messages }: { messages: MessagePreview[] }) {
  return (
    <div className="flex flex-col gap-3">
      {messages.map((m, i) => {
        const isUser = m.role === "user";
        return (
          <div key={i} className={`msg-bubble flex flex-col ${isUser ? "items-end" : "items-start"}`}>
            <div className="flex items-center gap-2 mb-1 px-1 text-[11px] text-muted">
              <span className="font-medium text-ink-2">
                {isUser ? "你" : m.role === "assistant" ? "AI" : m.role}
              </span>
              {m.timestamp && <span className="tnum">{fmtDateTime(m.timestamp)}</span>}
            </div>
            <div
              className={`max-w-[92%] rounded-2xl px-3.5 py-2.5 text-[13px] leading-relaxed whitespace-pre-wrap break-words select-text ${
                isUser
                  ? "bg-accent-soft text-ink rounded-tr-md"
                  : "bg-surface-2 text-ink border border-line rounded-tl-md"
              }`}
            >
              <MessageText text={m.text} />
            </div>
          </div>
        );
      })}
    </div>
  );
});

function Stat({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="min-w-0" title={title}>
      <div className="text-[11px] text-muted">{label}</div>
      <div className="text-[13px] font-medium text-ink tnum truncate mt-0.5">{value}</div>
    </div>
  );
}

export default function SessionDetail({ session, adapters, onPatch, onRemoved, onClose, onMigrated, toast }: Props) {
  const [tagInput, setTagInput] = useState("");
  const [note, setNote] = useState(session.meta.note);
  const [messages, setMessages] = useState<MessagePreview[] | null>(null);
  const [msgLoading, setMsgLoading] = useState(false);
  const [msgError, setMsgError] = useState<string | null>(null);
  const [showInfo, setShowInfo] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const msgSeq = useRef(0);
  const noteTimer = useRef<number | null>(null);
  const metaRef = useRef(session.meta);
  const savedMetaRef = useRef(session.meta);
  const scrollRef = useRef<HTMLDivElement>(null);

  const adapter = adapters.find((a) => a.id === session.harness_id);
  const caps = adapter?.capabilities;
  const rawOk = session.raw_usable;
  const canRead = !!caps?.can_read_messages;

  useEffect(() => {
    setNote(session.meta.note);
    metaRef.current = session.meta;
    savedMetaRef.current = session.meta;
    setTagInput("");
    setMessages(null);
    setMsgError(null);
    setEditingTitle(false);
    const harnessId = session.harness_id;
    const sessionId = session.session_id;
    return () => {
      if (noteTimer.current) {
        window.clearTimeout(noteTimer.current);
        noteTimer.current = null;
      }
      const pending = metaRef.current;
      if (pending.note !== savedMetaRef.current.note) {
        api.setMeta(harnessId, sessionId, pending).catch((e) => console.error("切换时保存备注失败", e));
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.harness_id, session.session_id]);

  const loadMessages = useCallback(() => {
    // 序号守卫：只有最后一次请求的结果能落地，会话快速切换时
    // 旧会话的响应永远不会覆盖新会话的内容
    const seq = ++msgSeq.current;
    setMsgLoading(true);
    setMsgError(null);
    api
      .getMessages(session.harness_id, session.session_id, 400)
      .then((data) => {
        if (seq === msgSeq.current) setMessages(data);
      })
      .catch((e) => {
        if (seq === msgSeq.current) setMsgError(String(e));
      })
      .finally(() => {
        if (seq === msgSeq.current) setMsgLoading(false);
      });
  }, [session.harness_id, session.session_id]);

  // 会话切换：在同一个 effect 里完成重置和加载
  useEffect(() => {
    setMessages(null);
    setMsgError(null);
    if (canRead) loadMessages();
  }, [session.harness_id, session.session_id, canRead, loadMessages]);

  // 新消息加载后滚到底部（最近的对话在末尾）
  useEffect(() => {
    if (messages && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  const saveMeta = async (patch: Partial<SessionDto["meta"]>) => {
    const meta = { ...metaRef.current, ...patch };
    metaRef.current = meta;
    onPatch({ ...session, meta });
    try {
      await api.setMeta(session.harness_id, session.session_id, meta);
      savedMetaRef.current = meta;
    } catch (e) {
      toast("err", `保存失败：${String(e)}`);
    }
  };

  const onNoteChange = (v: string) => {
    setNote(v);
    metaRef.current = { ...metaRef.current, note: v };
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => saveMeta({ note: v }), 600);
  };

  const addTag = () => {
    const t = tagInput.trim().replace(/^#/, "");
    if (!t) return;
    if (!metaRef.current.tags.includes(t)) saveMeta({ tags: [...metaRef.current.tags, t] });
    setTagInput("");
  };

  const run = async (fn: () => Promise<string>, okPrefix: string) => {
    setBusy(true);
    try {
      const r = await fn();
      toast("ok", `${okPrefix}：${r}`);
    } catch (e) {
      toast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  const exportAs = (format: "md" | "jsonl") => {
    setBusy(true);
    (async () => {
      // 先弹系统保存对话框，用户取消则什么也不做
      const dest = await saveDialog({
        defaultPath: `${session.harness_id}-${session.session_id.slice(0, 8)}.${format}`,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });
      if (!dest) return;
      const r = await api.exportSession(session.harness_id, session.session_id, format, dest);
      toast("ok", `已导出到：${r}`);
    })()
      .catch((e) => toast("err", String(e)))
      .finally(() => setBusy(false));
  };

  const backupTo = () => {
    setBusy(true);
    (async () => {
      const dir = await openDialog({ directory: true, title: "选择备份目录" });
      if (!dir || Array.isArray(dir)) return;
      const r = await api.backup(session.harness_id, session.session_id, dir);
      toast("ok", `已备份到：${r}`);
    })()
      .catch((e) => toast("err", String(e)))
      .finally(() => setBusy(false));
  };

  const onDelete = async () => {
    if (!window.confirm(`把该会话的原始文件移入回收站？\n${session.raw_path}\n（标签和备注会保留）`)) return;
    setBusy(true);
    try {
      await api.deleteSession(session.harness_id, session.session_id);
      toast("ok", "已移入回收站");
      onRemoved(session);
    } catch (e) {
      toast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  const displayTitle = session.meta.custom_title?.trim() || session.title || "(无标题)";

  const commitTitle = () => {
    const v = titleDraft.trim();
    setEditingTitle(false);
    const current = session.meta.custom_title?.trim() || session.title;
    if (v === current) return;
    // 留空 = 恢复 harness 原标题（custom_title 置空）
    saveMeta({ custom_title: v || null });
  };

  const tokensIn = session.tokens_in ?? 0;
  const tokensOut = session.tokens_out ?? 0;

  return (
    <section className="w-[540px] shrink-0 panel flex flex-col overflow-hidden">
      {/* 头部 */}
      <div className="px-5 pt-4 pb-4 border-b border-line space-y-3.5">
        <div className="flex items-start gap-2">
          <IconButton
            onClick={() => saveMeta({ favorite: !metaRef.current.favorite })}
            className="-ml-1.5 mt-[-2px]"
            title={session.meta.favorite ? "取消收藏" : "收藏"}
          >
            <StarIcon filled={session.meta.favorite} className="w-4 h-4" />
          </IconButton>
          <div className="flex-1 min-w-0">
            {editingTitle ? (
              <input
                autoFocus
                value={titleDraft}
                onChange={(e) => setTitleDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitTitle();
                  if (e.key === "Escape") setEditingTitle(false);
                }}
                onBlur={commitTitle}
                placeholder="输入自定义标题，留空恢复默认"
                className="w-full bg-surface border border-accent rounded-lg px-2.5 py-1 text-[15px] font-semibold text-ink outline-none"
              />
            ) : (
              <div className="group/title flex items-start gap-1">
                <h2 className="text-[15px] font-semibold text-ink leading-snug break-words flex-1 select-text">
                  {displayTitle}
                </h2>
                <IconButton
                  onClick={() => {
                    setTitleDraft(session.meta.custom_title?.trim() || session.title);
                    setEditingTitle(true);
                  }}
                  className="opacity-0 group-hover/title:opacity-100 w-6 h-6"
                  title="编辑标题"
                >
                  <PencilIcon className="w-3.5 h-3.5" />
                </IconButton>
              </div>
            )}
            <div className="flex items-center gap-2 mt-2 min-w-0">
              <HarnessTag id={session.harness_id} compact />
              <span className="text-[11px] text-muted mono truncate select-text" title={session.session_id}>
                {session.session_id}
              </span>
              {session.meta.custom_title && !editingTitle && (
                <button
                  onClick={() => saveMeta({ custom_title: null })}
                  className="shrink-0 text-[11px] text-muted hover:text-ink underline underline-offset-2"
                  title="恢复 harness 原标题"
                >
                  重置标题
                </button>
              )}
            </div>
          </div>
          <IconButton onClick={onClose} title="关闭" className="-mr-1.5 mt-[-2px]">
            <CloseIcon className="w-4 h-4" />
          </IconButton>
        </div>

        <div className="grid grid-cols-4 gap-3 px-3 py-2.5 rounded-lg bg-surface-2 border border-line">
          <Stat label="最后活动" value={fmtTime(session.ended_at)} title={fmtDateTime(session.ended_at)} />
          <Stat label="消息" value={session.message_count != null ? `${session.message_count} 条` : "—"} />
          <Stat
            label="Tokens"
            value={tokensIn + tokensOut > 0 ? fmtTokens(tokensIn + tokensOut) : "—"}
            title={`输入（含缓存）${fmtTokens(tokensIn)} · 输出 ${fmtTokens(tokensOut)}`}
          />
          <Stat label="估算费用" value={session.cost_usd ? fmtUsd(session.cost_usd) : "—"} />
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            variant="primary"
            size="sm"
            disabled={busy || !caps?.can_resume}
            title={caps?.can_resume ? "在终端中继续这个对话" : "该 harness 暂不支持续接"}
            onClick={() => run(() => api.resume(session.harness_id, session.session_id), "已拉起终端")}
          >
            <PlayIcon className="w-3 h-3" />
            续接对话
          </Button>
          <Button
            size="sm"
            disabled={busy || !caps?.can_launch}
            title={caps?.can_launch ? `在项目目录打开 ${harnessName(session.harness_id)}` : "该 harness 不支持直接打开"}
            onClick={() => run(() => api.launchHarness(session.harness_id, session.session_id), "已打开")}
          >
            <ExternalIcon className="w-3.5 h-3.5" />
            打开
          </Button>
          <Button
            size="sm"
            disabled={busy || !caps?.can_backup || !rawOk}
            title={!rawOk ? "独立存储目录缺失，无法备份" : "复制原始文件到指定目录"}
            onClick={backupTo}
          >
            <ArchiveIcon className="w-3.5 h-3.5" />
            备份
          </Button>
          <label
            className={`relative inline-flex items-center gap-1.5 h-7 px-2.5 text-xs font-medium rounded-lg border border-line bg-surface text-ink hover:bg-surface-2 transition-colors ${
              busy ? "opacity-45 pointer-events-none" : "cursor-pointer"
            }`}
            title="导出完整对话"
          >
            <DownloadIcon className="w-3.5 h-3.5" />
            导出
            <select
              className="absolute inset-0 opacity-0 cursor-pointer"
              disabled={busy}
              value=""
              onChange={(e) => {
                const f = e.target.value as "md" | "jsonl" | "";
                e.target.value = "";
                if (f) exportAs(f);
              }}
            >
              <option value="">导出为…</option>
              <option value="md">Markdown (.md)</option>
              <option value="jsonl">JSONL (.jsonl)</option>
            </select>
          </label>
          {canRead && (
            <label
              className={`relative inline-flex items-center gap-1.5 h-7 px-2.5 text-xs font-medium rounded-lg border border-line bg-surface text-ink hover:bg-surface-2 transition-colors ${
                busy ? "opacity-45 pointer-events-none" : "cursor-pointer"
              }`}
              title="把会话转换成目标 harness 的原生格式，然后可以在目标 harness 里续接"
            >
              <SwapIcon className="w-3.5 h-3.5" />
              迁移
              <select
                className="absolute inset-0 opacity-0 cursor-pointer"
                disabled={busy}
                value=""
                onChange={(e) => {
                  const target = e.target.value;
                  e.target.value = "";
                  if (!target) return;
                  run(async () => {
                    const r = await api.migrate(session.harness_id, session.session_id, target);
                    onMigrated();
                    return `${r.path}\n在 ${harnessName(target)} 中运行：${r.resume_command}`;
                  }, "已迁移");
                }}
              >
                <option value="">迁移到…</option>
                {MIGRATE_TARGETS.filter((t) => t !== session.harness_id).map((t) => (
                  <option key={t} value={t}>
                    {harnessName(t)}
                  </option>
                ))}
              </select>
            </label>
          )}
          <span className="flex-1" />
          <Button
            variant="danger"
            size="sm"
            disabled={busy || !caps?.can_delete || !rawOk}
            title={
              !caps?.can_delete
                ? "该 harness 存储为共享数据库，不支持删除单个会话"
                : !rawOk
                  ? "独立存储目录缺失，删除会误伤共享文件"
                  : "原始文件移入回收站"
            }
            onClick={onDelete}
          >
            <TrashIcon className="w-3.5 h-3.5" />
            删除
          </Button>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          {session.meta.tags.map((t) => (
            <span
              key={t}
              className="h-6 pl-2 pr-1 rounded-md bg-surface-2 border border-line text-xs text-ink-2 flex items-center gap-1"
            >
              {t}
              <button
                className="w-4 h-4 flex items-center justify-center rounded text-muted hover:text-danger"
                onClick={() => saveMeta({ tags: metaRef.current.tags.filter((x) => x !== t) })}
                title="移除标签"
              >
                <CloseIcon className="w-3 h-3" />
              </button>
            </span>
          ))}
          <input
            value={tagInput}
            onChange={(e) => setTagInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addTag()}
            placeholder="+ 添加标签"
            className="w-24 h-6 bg-transparent text-xs outline-none rounded-md px-1.5 text-ink-2 placeholder:text-muted focus:bg-surface-2"
          />
        </div>

        {!rawOk && (
          <div className="rounded-lg border border-warn/30 bg-warn-soft px-3 py-2 text-xs text-warn">
            该会话的独立存储目录缺失，raw 指向共享/全局文件——删除、备份、定位已禁用
          </div>
        )}
      </div>

      {/* 对话记录 */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto px-5 py-4 bg-bg/40">
        {msgLoading && messages === null ? (
          <div className="space-y-3">
            {[0, 1, 2].map((i) => (
              <div
                key={i}
                className={`h-16 rounded-2xl bg-surface-2 animate-pulse ${i % 2 ? "ml-auto w-2/3" : "w-4/5"}`}
              />
            ))}
          </div>
        ) : msgError ? (
          <EmptyState
            title="读取消息失败"
            hint={msgError}
            action={
              <Button size="sm" onClick={loadMessages}>
                重试
              </Button>
            }
          />
        ) : messages && messages.length > 0 ? (
          <MessageList messages={messages} />
        ) : (
          <EmptyState
            icon={<ChatIcon className="w-7 h-7" />}
            title={canRead ? "没有读到消息内容" : "该 harness 暂不支持查看消息"}
          />
        )}
      </div>

      {/* 底部：详情与备注（折叠） */}
      <div className="border-t border-line">
        <button
          className="w-full px-5 h-9 text-xs text-ink-2 hover:text-ink flex items-center justify-between"
          onClick={() => setShowInfo(!showInfo)}
        >
          <span>详情与备注{session.meta.note ? " · 有备注" : ""}</span>
          <span className="text-sm leading-none text-muted">{showInfo ? "−" : "+"}</span>
        </button>
        {showInfo && (
          <div className="px-5 pb-4 space-y-3 animate-in">
            <div className="grid grid-cols-2 gap-3">
              <div>
                <div className="text-[11px] text-muted mb-0.5">开始时间</div>
                <div className="text-xs text-ink-2 tnum">{fmtDateTime(session.started_at)}</div>
              </div>
              <div>
                <div className="text-[11px] text-muted mb-0.5">输入 / 输出 Tokens</div>
                <div className="text-xs text-ink-2 tnum">
                  {fmtTokens(tokensIn)} / {fmtTokens(tokensOut)}
                </div>
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted mb-0.5">项目路径</div>
              <div className="text-xs mono text-ink-2 break-all select-text">{shortPath(session.project_path)}</div>
            </div>
            <div>
              <div className="text-[11px] text-muted mb-0.5">原始文件（{session.source_format}）</div>
              {rawOk ? (
                <button
                  onClick={() => run(() => api.reveal(session.harness_id, session.session_id), "已在文件管理器中显示")}
                  className="text-xs mono text-accent hover:underline break-all text-left inline-flex items-start gap-1"
                  disabled={busy}
                >
                  <FolderIcon className="w-3.5 h-3.5 shrink-0 mt-px" />
                  {session.raw_path}
                </button>
              ) : (
                <div className="text-xs mono text-muted break-all">{session.raw_path}</div>
              )}
            </div>
            <div>
              <div className="text-[11px] text-muted mb-1">备注（只写入 SessionHub 自己的数据库）</div>
              <textarea
                value={note}
                onChange={(e) => onNoteChange(e.target.value)}
                rows={3}
                className="w-full bg-surface-2 border border-line rounded-lg px-2.5 py-2 text-[13px] text-ink outline-none focus:border-accent resize-y placeholder:text-muted select-text"
                placeholder="记录这个会话在做什么…"
              />
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
