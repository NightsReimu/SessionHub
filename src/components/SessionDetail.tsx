import { useEffect, useRef, useState } from "react";
import { AdapterInfo, MessagePreview, SessionDto, api } from "../api";
import { fmtTime, fmtTokens } from "./SessionList";
import { Toast } from "../App";

interface Props {
  session: SessionDto;
  adapters: AdapterInfo[];
  onPatch: (s: SessionDto) => void;
  onRemoved: (s: SessionDto) => void;
  toast: (kind: Toast["kind"], text: string) => void;
}

export default function SessionDetail({ session, adapters, onPatch, onRemoved, toast }: Props) {
  const [tagInput, setTagInput] = useState("");
  const [note, setNote] = useState(session.meta.note);
  const [messages, setMessages] = useState<MessagePreview[] | null>(null);
  const [busy, setBusy] = useState(false);
  const noteTimer = useRef<number | null>(null);

  const adapter = adapters.find((a) => a.id === session.harness_id);
  const caps = adapter?.capabilities;

  useEffect(() => {
    setNote(session.meta.note);
    setTagInput("");
    setMessages(null);
  }, [session.harness_id, session.session_id]);

  const saveMeta = async (patch: Partial<SessionDto["meta"]>) => {
    const meta = { tags: session.meta.tags, note, favorite: session.meta.favorite, ...patch };
    const next = { ...session, meta };
    onPatch(next);
    try {
      await api.setMeta(session.harness_id, session.session_id, meta);
    } catch (e) {
      toast("err", `保存失败：${String(e)}`);
    }
  };

  const onNoteChange = (v: string) => {
    setNote(v);
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

  const loadMessages = async () => {
    setBusy(true);
    try {
      setMessages(await api.getMessages(session.harness_id, session.session_id));
    } catch (e) {
      toast("err", `读取消息失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const stat = (label: string, value: string) => (
    <div className="bg-zinc-900 rounded-lg px-3 py-2">
      <div className="text-[10px] text-zinc-500">{label}</div>
      <div className="text-sm text-zinc-200 mt-0.5">{value}</div>
    </div>
  );

  const btn =
    "px-2.5 py-1.5 text-xs rounded-lg bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 disabled:hover:bg-zinc-800";

  return (
    <div className="w-[380px] shrink-0 border-l border-zinc-800 flex flex-col bg-zinc-950">
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        <div>
          <div className="flex items-start justify-between gap-2">
            <h2 className="text-base font-medium text-zinc-100 break-all">{session.title || "(无标题)"}</h2>
            <button
              onClick={() => saveMeta({ favorite: !session.meta.favorite })}
              className="text-lg shrink-0 hover:scale-110 transition-transform"
              title="收藏"
            >
              {session.meta.favorite ? "⭐" : "☆"}
            </button>
          </div>
          <div className="text-xs text-zinc-500 mono mt-1 break-all">{session.session_id}</div>
        </div>

        <div className="grid grid-cols-2 gap-2">
          {stat("Harness", session.harness_id)}
          {stat("状态", session.status)}
          {stat("开始", session.started_at ? new Date(session.started_at).toLocaleString("zh-CN", { hour12: false }) : "—")}
          {stat("最后活动", fmtTime(session.ended_at))}
          {stat("消息数", session.message_count != null ? String(session.message_count) : "—")}
          {stat("Tokens", `${fmtTokens(session.tokens_in)} 入 / ${fmtTokens(session.tokens_out)} 出`)}
          {stat("费用", session.cost_usd != null && session.cost_usd > 0 ? `$${session.cost_usd.toFixed(4)}` : "—")}
          {stat("大小", session.file_size > 0 ? `${(session.file_size / 1024 / 1024).toFixed(1)} MB` : "—")}
        </div>

        <div>
          <div className="text-[11px] text-zinc-500 mb-1">项目路径</div>
          <div className="text-xs mono text-zinc-300 break-all">{session.project_path || "—"}</div>
          <div className="text-[11px] text-zinc-500 mt-2 mb-1">原始文件（{session.source_format}）</div>
          <button
            onClick={() => run(() => api.reveal(session.harness_id, session.session_id), "已在文件管理器中显示")}
            className="text-xs mono text-indigo-400 hover:text-indigo-300 break-all text-left"
            disabled={busy}
          >
            {session.raw_path}
          </button>
        </div>

        <div>
          <div className="text-[11px] text-zinc-500 mb-1">标签</div>
          <div className="flex flex-wrap gap-1.5">
            {session.meta.tags.map((t) => (
              <span key={t} className="px-2 py-0.5 rounded-full bg-zinc-800 text-xs text-zinc-300 flex items-center gap-1">
                #{t}
                <button
                  className="text-zinc-500 hover:text-red-400"
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
              placeholder="+ 加标签"
              className="w-20 bg-transparent text-xs outline-none border-b border-zinc-800 focus:border-indigo-500 px-1 py-0.5"
            />
          </div>
        </div>

        <div>
          <div className="text-[11px] text-zinc-500 mb-1">备注（只写入 SessionHub 数据库，不动 harness 文件）</div>
          <textarea
            value={note}
            onChange={(e) => onNoteChange(e.target.value)}
            rows={3}
            className="w-full bg-zinc-900 border border-zinc-800 rounded-lg px-2.5 py-2 text-sm outline-none focus:border-indigo-500 resize-y"
            placeholder="记录这个会话在做什么…"
          />
        </div>

        <div className="flex flex-wrap gap-1.5">
          <button
            className={btn + " bg-indigo-600 hover:bg-indigo-500"}
            disabled={busy || !caps?.can_resume}
            title={caps?.can_resume ? "在终端中续接此会话" : "该 harness 暂不支持续接"}
            onClick={() => run(() => api.resume(session.harness_id, session.session_id), "已拉起终端")}
          >
            ▶ 续接
          </button>
          <button className={btn} disabled={busy || !caps?.can_backup} onClick={() => run(() => api.backup(session.harness_id, session.session_id), "已备份到")}>
            备份
          </button>
          <button className={btn} disabled={busy} onClick={() => run(() => api.exportSession(session.harness_id, session.session_id, "md"), "已导出 Markdown 到")}>
            导出 MD
          </button>
          <button className={btn} disabled={busy} onClick={() => run(() => api.exportSession(session.harness_id, session.session_id, "jsonl"), "已导出 JSONL 到")}>
            导出 JSONL
          </button>
          <button
            className={btn + " text-red-400 hover:bg-red-950"}
            disabled={busy || !caps?.can_delete}
            title={caps?.can_delete ? "原始文件移入回收站" : "该 harness 存储为共享数据库，不支持删除单个会话"}
            onClick={onDelete}
          >
            删除
          </button>
          {caps?.can_read_messages && (
            <button className={btn} disabled={busy} onClick={loadMessages}>
              {messages ? "刷新消息" : "查看消息"}
            </button>
          )}
        </div>

        {messages && (
          <div className="space-y-2">
            <div className="text-[11px] text-zinc-500">最近 {messages.length} 条消息</div>
            {messages.map((m, i) => (
              <div key={i} className="bg-zinc-900 rounded-lg px-3 py-2">
                <div className="text-[10px] text-zinc-500 mb-0.5">
                  {m.role}
                  {m.timestamp ? ` · ${new Date(m.timestamp).toLocaleString("zh-CN", { hour12: false })}` : ""}
                </div>
                <div className="text-xs text-zinc-300 whitespace-pre-wrap break-words select-text">{m.text}</div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
