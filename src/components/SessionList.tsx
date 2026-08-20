import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { SessionDto } from "../api";
import { harnessBadge } from "./Sidebar";
import { StarIcon } from "./icons";

export function fmtTokens(n: number | null): string {
  if (n == null) return "—";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

export function fmtTime(ms: number | null): string {
  if (!ms) return "—";
  const diff = Date.now() - ms;
  const abs = new Date(ms).toLocaleString("zh-CN", { hour12: false });
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  return abs.slice(0, 10);
}

export function shortPath(p: string): string {
  if (!p) return "—";
  const home = "/Users/";
  let s = p;
  if (s.startsWith(home)) {
    const parts = s.split("/");
    s = "~/" + parts.slice(3).join("/");
  }
  return s;
}

interface Props {
  sessions: SessionDto[];
  selected: SessionDto | null;
  onSelect: (s: SessionDto) => void;
}

export default function SessionList({ sessions, selected, onSelect }: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: sessions.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 62,
    overscan: 12,
  });

  if (sessions.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-2 text-zinc-600">
        <div className="text-sm">没有会话</div>
        <div className="text-xs text-zinc-700">点击上方「扫描」发现本地各 harness 的会话</div>
      </div>
    );
  }

  return (
    <div ref={parentRef} className="flex-1 overflow-y-auto min-w-0">
      <div style={{ height: v.getTotalSize(), position: "relative" }}>
        {v.getVirtualItems().map((row) => {
          const s = sessions[row.index];
          const active =
            selected?.harness_id === s.harness_id && selected?.session_id === s.session_id;
          const tokens = (s.tokens_in ?? 0) + (s.tokens_out ?? 0);
          return (
            <div
              key={s.harness_id + "/" + s.session_id}
              onClick={() => onSelect(s)}
              style={{ transform: `translateY(${row.start}px)` }}
              className={`absolute top-0 left-0 w-full h-[62px] px-4 py-2 cursor-pointer flex flex-col justify-center border-b border-zinc-900/80 border-l-2 transition-colors ${
                active
                  ? "bg-zinc-800/60 border-l-indigo-500"
                  : "border-l-transparent hover:bg-zinc-900/70"
              }`}
            >
              <div className="flex items-center gap-2 min-w-0">
                <span
                  className={`shrink-0 text-[10px] px-1.5 py-0.5 rounded border ${harnessBadge(s.harness_id)}`}
                >
                  {s.harness_id}
                </span>
                {s.meta.favorite && <StarIcon filled className="w-3 h-3 shrink-0" />}
                <span className="truncate text-sm text-zinc-100 flex-1">
                  {s.title || "(无标题)"}
                </span>
                <span className="shrink-0 text-xs text-zinc-500" title={s.ended_at ? new Date(s.ended_at).toLocaleString("zh-CN", { hour12: false }) : ""}>
                  {fmtTime(s.ended_at ?? s.started_at)}
                </span>
              </div>
              <div className="flex items-center gap-3 mt-1 text-[11px] text-zinc-500 min-w-0">
                <span className="truncate mono">{shortPath(s.project_path)}</span>
                <span className="shrink-0">{s.message_count != null ? `${s.message_count} 条` : "—"}</span>
                <span className="shrink-0">{tokens > 0 ? `${fmtTokens(tokens)} tok` : ""}</span>
                {s.cost_usd != null && s.cost_usd > 0 && (
                  <span className="shrink-0">${s.cost_usd.toFixed(3)}</span>
                )}
                {s.meta.tags.slice(0, 3).map((t) => (
                  <span key={t} className="shrink-0 px-1 rounded bg-zinc-800/80 text-zinc-500">
                    {t}
                  </span>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
