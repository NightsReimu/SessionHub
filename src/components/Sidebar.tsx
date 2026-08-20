import { AdapterInfo, Counts, HubPaths } from "../api";

export const HARNESS_COLORS: Record<string, string> = {
  "claude-code": "bg-orange-500/20 text-orange-300 border-orange-500/40",
  codex: "bg-emerald-500/20 text-emerald-300 border-emerald-500/40",
  opencode: "bg-sky-500/20 text-sky-300 border-sky-500/40",
  dsh: "bg-violet-500/20 text-violet-300 border-violet-500/40",
  zcode: "bg-cyan-500/20 text-cyan-300 border-cyan-500/40",
  "claude-desktop": "bg-amber-500/20 text-amber-300 border-amber-500/40",
  generic: "bg-zinc-500/20 text-zinc-300 border-zinc-500/40",
};

export function harnessBadge(id: string) {
  return HARNESS_COLORS[id] ?? "bg-pink-500/20 text-pink-300 border-pink-500/40";
}

interface Props {
  adapters: AdapterInfo[];
  counts: Counts;
  filter: string;
  onFilter: (f: string) => void;
  watching: boolean;
  onToggleWatcher: () => void;
  hub: HubPaths | null;
  onOpenStats: () => void;
}

export default function Sidebar({ adapters, counts, filter, onFilter, watching, onToggleWatcher, hub, onOpenStats }: Props) {
  const itemCls = (active: boolean) =>
    `w-full flex items-center justify-between px-3 py-2 rounded-lg text-sm cursor-pointer ${
      active ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/60 hover:text-zinc-200"
    }`;

  return (
    <div className="w-60 shrink-0 border-r border-zinc-800 flex flex-col bg-zinc-950">
      <div className="px-4 py-4 border-b border-zinc-800">
        <div className="text-lg font-semibold tracking-tight">SessionHub</div>
        <div className="text-xs text-zinc-500 mt-0.5">统一 AI 会话管理</div>
      </div>

      <div className="flex-1 overflow-y-auto p-2 space-y-0.5">
        <button className={itemCls(filter === "all")} onClick={() => onFilter("all")}>
          <span>全部会话</span>
          <span className="text-xs text-zinc-500">{counts.total}</span>
        </button>
        <button className={itemCls(filter === "fav")} onClick={() => onFilter("fav")}>
          <span>⭐ 收藏</span>
          <span className="text-xs text-zinc-500">{counts.favorites}</span>
        </button>

        <div className="pt-3 pb-1 px-3 text-[11px] uppercase tracking-wider text-zinc-600">Harness</div>
        {adapters.map((a) => (
          <button
            key={a.id}
            className={itemCls(filter === a.id) + (a.detected ? "" : " opacity-45")}
            onClick={() => onFilter(a.id)}
            title={a.detected ? a.roots.join("\n") : "未检测到安装"}
          >
            <span className="flex items-center gap-2 min-w-0">
              <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${a.detected ? "bg-emerald-400" : "bg-zinc-600"}`} />
              <span className="truncate">{a.name}</span>
            </span>
            <span className="text-xs text-zinc-500">{a.detected ? (counts.per_harness[a.id] ?? 0) : "—"}</span>
          </button>
        ))}
      </div>

      <div className="p-3 border-t border-zinc-800 space-y-2">
        <button
          onClick={onOpenStats}
          className="w-full px-3 py-1.5 text-xs rounded-lg border border-zinc-700 bg-zinc-900 text-zinc-400 hover:text-zinc-200"
        >
          📊 用量统计
        </button>
        <button
          onClick={onToggleWatcher}
          className={`w-full px-3 py-1.5 text-xs rounded-lg border ${
            watching
              ? "border-emerald-600/50 bg-emerald-500/10 text-emerald-300"
              : "border-zinc-700 bg-zinc-900 text-zinc-400 hover:text-zinc-200"
          }`}
        >
          {watching ? "● 实时监听中" : "○ 开启实时监听"}
        </button>
        {hub && <div className="text-[10px] text-zinc-600 truncate" title={hub.hub_dir}>数据目录：{hub.hub_dir}</div>}
      </div>
    </div>
  );
}
