import { AdapterInfo, Counts } from "../api";
import iconUrl from "../assets/icon.png";

export const HARNESS_COLORS: Record<string, string> = {
  "claude-code": "bg-orange-500/12 text-orange-300/90 border-orange-500/25",
  codex: "bg-emerald-500/12 text-emerald-300/90 border-emerald-500/25",
  opencode: "bg-sky-500/12 text-sky-300/90 border-sky-500/25",
  dsh: "bg-violet-500/12 text-violet-300/90 border-violet-500/25",
  zcode: "bg-cyan-500/12 text-cyan-300/90 border-cyan-500/25",
  "claude-desktop": "bg-amber-500/12 text-amber-300/90 border-amber-500/25",
  generic: "bg-zinc-500/12 text-zinc-400/90 border-zinc-500/25",
};

export function harnessBadge(id: string) {
  return HARNESS_COLORS[id] ?? "bg-pink-500/12 text-pink-300/90 border-pink-500/25";
}

interface Props {
  adapters: AdapterInfo[];
  counts: Counts;
  filter: string;
  onFilter: (f: string) => void;
  watching: boolean;
  onToggleWatcher: () => void;
  onOpenStats: () => void;
}

export default function Sidebar({ adapters, counts, filter, onFilter, watching, onToggleWatcher, onOpenStats }: Props) {
  const itemCls = (active: boolean) =>
    `w-full flex items-center justify-between px-3 py-[7px] rounded-lg text-[13px] cursor-pointer transition-all ${
      active
        ? "bg-white/10 text-ink shadow-[inset_2px_0_0_0_var(--color-accent)]"
        : "text-mut hover:bg-white/5 hover:text-ink"
    }`;

  return (
    <div className="w-64 shrink-0 rounded-xl glass flex flex-col overflow-hidden">
      <div className="flex items-center gap-2.5 px-4 pt-4 pb-3.5 border-b border-white/[0.06]">
        <img src={iconUrl} alt="SessionHub" className="w-9 h-9 rounded-[10px] shadow-lg shadow-black/40" />
        <div>
          <div className="text-[15px] font-semibold tracking-tight leading-tight">SessionHub</div>
          <div className="text-[11px] text-dim">统一 AI 会话管理</div>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-2.5 py-2.5 space-y-px">
        <button className={itemCls(filter === "all")} onClick={() => onFilter("all")}>
          <span>全部会话</span>
          <span className="text-xs text-dim">{counts.total}</span>
        </button>
        <button className={itemCls(filter === "fav")} onClick={() => onFilter("fav")}>
          <span className="flex items-center gap-2">
            <svg viewBox="0 0 24 24" strokeWidth="1.5" className="w-3.5 h-3.5 fill-amber-400 stroke-amber-400">
              <path strokeLinecap="round" strokeLinejoin="round" d="M11.48 3.499a.562.562 0 011.04 0l2.125 5.111a.563.563 0 00.475.345l5.518.442c.499.04.701.663.321.988l-4.204 3.602a.563.563 0 00-.182.557l1.285 5.385a.562.562 0 01-.84.61l-4.725-2.885a.563.563 0 00-.586 0l-4.725 2.885a.562.562 0 01-.84-.61l1.285-5.385a.563.563 0 00-.182-.557l-4.204-3.602a.562.562 0 01.321-.988l5.518-.442a.563.563 0 00.475-.345L11.48 3.5z" />
            </svg>
            收藏
          </span>
          <span className="text-xs text-dim">{counts.favorites}</span>
        </button>

        <div className="pt-4 pb-1.5 px-3 text-[10px] font-medium uppercase tracking-[0.14em] text-dim border-t border-white/[0.05] mt-3">
          Harness
        </div>
        {adapters.map((a) => (
          <button
            key={a.id}
            className={itemCls(filter === a.id) + (a.detected ? "" : " opacity-40")}
            onClick={() => onFilter(a.id)}
            title={a.detected ? a.roots.join("\n") : "未检测到安装"}
          >
            <span className="flex items-center gap-2.5 min-w-0">
              <span
                className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                  a.detected ? "bg-emerald-400/90 shadow-[0_0_6px_rgba(52,211,153,0.7)]" : "bg-zinc-600"
                }`}
              />
              <span className="truncate">{a.name}</span>
            </span>
            <span className="text-xs text-dim">{a.detected ? (counts.per_harness[a.id] ?? 0) : "—"}</span>
          </button>
        ))}
      </div>

      <div className="p-3 border-t border-white/[0.06] space-y-2">
        <button
          onClick={onOpenStats}
          className="w-full px-3 py-2 text-xs rounded-lg bg-white/[0.04] border border-white/[0.07] text-mut hover:text-ink hover:bg-white/[0.08] transition-colors"
        >
          用量统计
        </button>
        <button
          onClick={onToggleWatcher}
          className={`w-full px-3 py-2 text-xs rounded-lg border flex items-center justify-center gap-1.5 transition-colors ${
            watching
              ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-300"
              : "bg-white/[0.04] border-white/[0.07] text-mut hover:text-ink hover:bg-white/[0.08]"
          }`}
        >
          <span className={`w-1.5 h-1.5 rounded-full ${watching ? "bg-emerald-400 animate-pulse" : "bg-dim"}`} />
          {watching ? "实时监听中" : "开启实时监听"}
        </button>
      </div>
    </div>
  );
}
