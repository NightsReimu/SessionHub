import { useCallback, useEffect, useRef, useState } from "react";
import { AdapterInfo, MessagePreview, SessionDto, api } from "../api";
import { fmtTime, fmtTokens } from "./SessionList";
import { CloseIcon, ExternalIcon, PlayIcon, StarIcon } from "./icons";
import MessageText from "./MessageText";
import { Toast } from "../App";

interface Props {
  session: SessionDto;
  adapters: AdapterInfo[];
  onPatch: (s: SessionDto) => void;
  onRemoved: (s: SessionDto) => void;
  onClose: () => void;
  toast: (kind: Toast["kind"], text: string) => void;
}

const btn =
  "inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs rounded-lg bg-zinc-800/80 hover:bg-zinc-700/80 text-zinc-300 hover:text-zinc-100 transition-colors disabled:opacity-40 disabled:hover:bg-zinc-800/80 disabled:hover:text-zinc-300";

export default function SessionDetail({ session, adapters, onPatch, onRemoved, onClose, toast }: Props) {
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

  // 会话切换：在同一个 effect 里完成重置和加载，
  // 避免「清空」与「请求」分布在两个 effect 导致的读到旧状态/不触发
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
    if (!session.meta.tags.includes(t)) saveMeta({ tags: [...session.meta.tags, t] });
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

  const totalTokens = (session.tokens_in ?? 0) + (session.tokens_out ?? 0);
  const displayTitle = session.meta.custom_title?.trim() || session.title || "(无标题)";

  const commitTitle = () => {
    const v = titleDraft.trim();
    setEditingTitle(false);
    const current = session.meta.custom_title?.trim() || session.title;
    if (v === current) return; // 无变化
    // 留空 = 恢复 harness 原标题（custom_title 置空）
    saveMeta({ custom_title: v || null });
  };

  const chip = (text: string) => (
    <span key={text} className="px-2 py-1 rounded-md bg-zinc-900/80 border border-zinc-800/60 text-zinc-400">
      {text}
    </span>
  );

  return (
    <div className="w-[520px] shrink-0 rounded-2xl border border-zinc-800/60 bg-zinc-900/40 flex flex-col overflow-hidden">
      {/* 头部：标题 + 操作 + 概要 */}
      <div className="px-5 pt-4 pb-3.5 border-b border-zinc-800/60 space-y-3">
        <div className="flex items-start gap-2">
          <button
            onClick={() => saveMeta({ favorite: !session.meta.favorite })}
            className="mt-0.5 shrink-0"
            title="收藏"
          >
            <StarIcon filled={session.meta.favorite} className="w-5 h-5" />
          </button>
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
                className="w-full bg-zinc-900/90 border border-indigo-500/50 rounded-lg px-2.5 py-1.5 text-[15px] font-medium text-zinc-100 outline-none"
              />
            ) : (
              <div className="group/title flex items-start gap-1.5">
                <h2 className="text-[15px] font-medium text-zinc-100 leading-snug break-words flex-1">
                  {displayTitle}
                </h2>
                <button
                  onClick={() => {
                    setTitleDraft(session.meta.custom_title?.trim() || session.title);
                    setEditingTitle(true);
                  }}
                  className="shrink-0 mt-1 p-0.5 text-zinc-600 opacity-0 group-hover/title:opacity-100 hover:text-zinc-300 transition-opacity"
                  title="编辑标题"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" className="w-3.5 h-3.5">
                    <path strokeLinecap="round" strokeLinejoin="round" d="M16.86 3.49a2.12 2.12 0 013 3L8.5 17.86 4 19l1.14-4.5L16.86 3.49z" />
                  </svg>
                </button>
              </div>
            )}
            <div className="flex items-center gap-2 mt-1">
              <div className="text-[11px] text-zinc-600 mono truncate">{session.session_id}</div>
              {session.meta.custom_title && !editingTitle && (
                <button
                  onClick={() => saveMeta({ custom_title: null })}
                  className="shrink-0 text-[10px] text-zinc-500 hover:text-zinc-300 underline underline-offset-2"
                  title="恢复 harness 原标题"
                >
                  重置标题
                </button>
              )}
            </div>
          </div>
          <button onClick={onClose} className="shrink-0 p-1 text-zinc-500 hover:text-zinc-200" title="关闭">
            <CloseIcon className="w-4 h-4" />
          </button>
        </div>

        <div className="flex flex-wrap gap-1.5">
          <button
            className={btn + " bg-indigo-600 hover:bg-indigo-500 text-white hover:text-white"}
            disabled={busy || !caps?.can_resume}
            title={caps?.can_resume ? "在终端中打开此对话" : "该 harness 暂不支持续接"}
            onClick={() => run(() => api.resume(session.harness_id, session.session_id), "已拉起终端")}
          >
            <PlayIcon className="w-3 h-3" />
            续接对话
          </button>
          <button
            className={btn}
            disabled={busy || !caps?.can_launch}
            title={caps?.can_launch ? "打开 harness 本身" : "该 harness 不支持直接打开"}
            onClick={() => run(() => api.launchHarness(session.harness_id, session.session_id), "已打开")}
          >
            <ExternalIcon className="w-3.5 h-3.5" />
            打开 Harness
          </button>
          <button
            className={btn}
            disabled={busy || !caps?.can_backup || !rawOk}
            title={!rawOk ? "独立存储目录缺失，无法备份" : "复制原始文件到 ~/SessionHub/backups"}
            onClick={() => run(() => api.backup(session.harness_id, session.session_id), "已备份到")}
          >
            备份
          </button>
          <button className={btn} disabled={busy} onClick={() => run(() => api.exportSession(session.harness_id, session.session_id, "md"), "已导出 Markdown 到")}>
            导出 MD
          </button>
          <button className={btn} disabled={busy} onClick={() => run(() => api.exportSession(session.harness_id, session.session_id, "jsonl"), "已导出 JSONL 到")}>
            导出 JSONL
          </button>
          <button
            className={btn + " text-red-400 hover:bg-red-950/60"}
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
            删除
          </button>
        </div>

        <div className="flex flex-wrap gap-1.5 text-[11px]">
          {chip(`开始 ${session.started_at ? new Date(session.started_at).toLocaleString("zh-CN", { hour12: false }) : "—"}`)}
          {chip(`活动 ${fmtTime(session.ended_at)}`)}
          {chip(session.message_count != null ? `${session.message_count} 条` : "— 条")}
          {totalTokens > 0 && chip(`${fmtTokens(totalTokens)} tok`)}
          {session.cost_usd != null && session.cost_usd > 0 && chip(`$${session.cost_usd.toFixed(3)}`)}
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          {session.meta.tags.map((t) => (
            <span key={t} className="px-2 py-0.5 rounded-full bg-zinc-800/80 text-[11px] text-zinc-300 flex items-center gap-1">
              {t}
              <button
                className="text-zinc-500 hover:text-red-400 leading-none"
                onClick={() => saveMeta({ tags: session.meta.tags.filter((x) => x !== t) })}
              >
                ×
              </button>
            </span>
          ))}
          <input
            value={tagInput}
            onChange={(e) => setTagInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addTag()}
            placeholder="+ 标签"
            className="w-16 bg-transparent text-[11px] outline-none border-b border-transparent focus:border-indigo-500 px-1 py-0.5 text-zinc-400"
          />
        </div>

        {!rawOk && (
          <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-200/90">
            该会话的独立存储目录缺失，raw 指向共享/全局文件——删除、备份、定位已禁用
          </div>
        )}
      </div>

      {/* 对话记录 */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto px-5 py-4">
        {msgLoading && messages === null ? (
          <div className="space-y-3">
            {[0, 1, 2].map((i) => (
              <div key={i} className="h-16 rounded-2xl bg-zinc-900/70 animate-pulse" />
            ))}
          </div>
        ) : msgError ? (
          <div className="text-center py-10 space-y-3">
            <div className="text-sm text-zinc-500">读取消息失败：{msgError}</div>
            <button className={btn} onClick={loadMessages}>
              重试
            </button>
          </div>
        ) : messages && messages.length > 0 ? (
          <div className="flex flex-col gap-2.5">
            {messages.map((m, i) => {
              const isUser = m.role === "user";
              return (
                <div
                  key={i}
                  className={`max-w-[92%] rounded-2xl border px-3.5 py-2.5 ${
                    isUser
                      ? "self-end bg-indigo-500/10 border-indigo-500/25"
                      : "self-start bg-zinc-900/80 border-zinc-800"
                  }`}
                >
                  <div className="text-[10px] uppercase tracking-wider mb-1 text-zinc-500 flex items-center gap-2">
                    <span>{isUser ? "你" : m.role === "assistant" ? "AI" : m.role}</span>
                    {m.timestamp && (
                      <span className="normal-case tracking-normal">
                        {new Date(m.timestamp).toLocaleString("zh-CN", { hour12: false })}
                      </span>
                    )}
                  </div>
                  <div className="text-[13px] leading-relaxed text-zinc-200 whitespace-pre-wrap break-words select-text">
                    <MessageText text={m.text} />
                  </div>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="text-center py-10 text-sm text-zinc-600">
            {canRead ? "该会话没有读取到消息内容" : "该 harness 暂不支持查看消息"}
          </div>
        )}
      </div>

      {/* 底部：详情与备注（折叠） */}
      <div className="border-t border-zinc-800/60">
        <button
          className="w-full px-5 py-2.5 text-[11px] text-zinc-500 hover:text-zinc-300 flex items-center justify-between"
          onClick={() => setShowInfo(!showInfo)}
        >
          <span>详情与备注</span>
          <span className="text-sm leading-none">{showInfo ? "−" : "+"}</span>
        </button>
        {showInfo && (
          <div className="px-5 pb-4 space-y-3">
            <div>
              <div className="text-[11px] text-zinc-600 mb-0.5">项目路径</div>
              <div className="text-xs mono text-zinc-400 break-all">{session.project_path || "—"}</div>
            </div>
            <div>
              <div className="text-[11px] text-zinc-600 mb-0.5">原始文件（{session.source_format}）</div>
              {rawOk ? (
                <button
                  onClick={() => run(() => api.reveal(session.harness_id, session.session_id), "已在文件管理器中显示")}
                  className="text-xs mono text-indigo-400 hover:text-indigo-300 break-all text-left"
                  disabled={busy}
                >
                  {session.raw_path}
                </button>
              ) : (
                <div className="text-xs mono text-zinc-500 break-all">{session.raw_path}</div>
              )}
            </div>
            <div>
              <div className="text-[11px] text-zinc-600 mb-0.5">备注（只写入 SessionHub 数据库，不动 harness 文件）</div>
              <textarea
                value={note}
                onChange={(e) => onNoteChange(e.target.value)}
                rows={3}
                className="w-full bg-zinc-900/80 border border-zinc-800 rounded-lg px-2.5 py-2 text-sm outline-none focus:border-indigo-500 resize-y"
                placeholder="记录这个会话在做什么…"
              />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
