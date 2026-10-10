import type { ReactNode } from "react";
import { AdapterInfo, Counts } from "../api";
import iconUrl from "../assets/icon.png";
import { harnessColor } from "../lib/harness";
import { ThemeMode } from "../lib/theme";
import { ChartIcon, ChatIcon, EyeIcon, MonitorIcon, MoonIcon, StarIcon, SunIcon } from "./icons";

export type View = "sessions" | "usage";

interface Props {
  view: View;
  onView: (v: View) => void;
  adapters: AdapterInfo[];
  counts: Counts;
  filter: string;
  onFilter: (f: string) => void;
  watching: boolean;
  onToggleWatcher: () => void;
  theme: ThemeMode;
  onTheme: (m: ThemeMode) => void;
}

function NavItem({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-2.5 h-8 px-2.5 rounded-lg text-[13px] transition-colors ${
        active ? "bg-surface-3 text-ink font-medium" : "text-ink-2 hover:bg-surface-2 hover:text-ink"
      }`}
    >
      <span className={active ? "text-ink" : "text-muted"}>{icon}</span>
      {label}
    </button>
  );
}

export default function Sidebar({
  view,
  onView,
  adapters,
  counts,
  filter,
  onFilter,
  watching,
  onToggleWatcher,
  theme,
  onTheme,
}: Props) {
  const sourceCls = (active: boolean, dim = false) =>
    `w-full flex items-center justify-between gap-2 h-[30px] px-2.5 rounded-lg text-[13px] transition-colors ${
      active && view === "sessions"
        ? "bg-surface-3 text-ink font-medium"
        : "text-ink-2 hover:bg-surface-2 hover:text-ink"
    } ${dim ? "opacity-45" : ""}`;

  const pick = (f: string) => {
    onFilter(f);
    onView("sessions");
  };

  // 有会话的在前，未安装的沉底
  const sorted = [...adapters].sort((a, b) => {
    const ca = counts.per_harness[a.id] ?? 0;
    const cb = counts.per_harness[b.id] ?? 0;
    if (a.detected !== b.detected) return a.detected ? -1 : 1;
    return cb - ca;
  });

  return (
    <aside className="w-[220px] shrink-0 flex flex-col bg-surface border-r border-line">
      <div className="flex items-center gap-2.5 px-4 h-14 shrink-0">
        <img src={iconUrl} alt="" className="w-7 h-7 rounded-[8px]" />
        <div className="min-w-0">
          <div className="text-[14px] font-semibold tracking-tight leading-tight">SessionHub</div>
          <div className="text-[11px] text-muted leading-tight">AI 会话管理</div>
        </div>
      </div>

      <nav className="px-2.5 space-y-0.5">
        <NavItem
          active={view === "sessions"}
          icon={<ChatIcon />}
          label="会话"
          onClick={() => onView("sessions")}
        />
        <NavItem
          active={view === "usage"}
          icon={<ChartIcon />}
          label="用量统计"
          onClick={() => onView("usage")}
        />
      </nav>

      <div className="mt-5 px-4 pb-1.5 text-[11px] font-medium text-muted">来源</div>
      <div className="flex-1 overflow-y-auto px-2.5 pb-3 space-y-0.5">
        <button className={sourceCls(filter === "all")} onClick={() => pick("all")}>
          <span>全部会话</span>
          <span className="text-xs text-muted tnum">{counts.total}</span>
        </button>
        <button className={sourceCls(filter === "fav")} onClick={() => pick("fav")}>
          <span className="flex items-center gap-2">
            <StarIcon filled className="w-3.5 h-3.5" />
            收藏
          </span>
          <span className="text-xs text-muted tnum">{counts.favorites}</span>
        </button>
        <div className="h-2" />
        {sorted.map((a) => (
          <button
            key={a.id}
            className={sourceCls(filter === a.id, !a.detected)}
            onClick={() => pick(a.id)}
            title={a.detected ? a.roots.join("\n") : "未检测到安装"}
          >
            <span className="flex items-center gap-2 min-w-0">
              <span
                className="w-2 h-2 rounded-full shrink-0"
                style={{ background: a.detected ? harnessColor(a.id) : "var(--line-strong)" }}
              />
              <span className="truncate">{a.name}</span>
            </span>
            <span className="text-xs text-muted tnum">
              {a.detected ? (counts.per_harness[a.id] ?? 0) : "—"}
            </span>
          </button>
        ))}
      </div>

      <div className="p-2.5 border-t border-line space-y-2">
        <button
          onClick={onToggleWatcher}
          title="监听各 harness 的会话目录，有变化时自动增量更新"
          className={`w-full flex items-center gap-2 h-8 px-2.5 rounded-lg text-[13px] transition-colors ${
            watching ? "text-ink bg-surface-2" : "text-ink-2 hover:bg-surface-2 hover:text-ink"
          }`}
        >
          <EyeIcon className="w-4 h-4 text-muted" />
          <span className="flex-1 text-left">实时监听</span>
          <span
            className={`relative w-7 h-4 rounded-full transition-colors ${watching ? "bg-ok" : "bg-line-strong"}`}
          >
            <span
              className={`absolute top-0.5 w-3 h-3 rounded-full bg-white shadow transition-[left] ${
                watching ? "left-3.5" : "left-0.5"
              }`}
            />
          </span>
        </button>
        <div className="flex items-center gap-1 p-0.5 rounded-lg bg-surface-2 border border-line">
          {(
            [
              ["light", <SunIcon key="l" className="w-3.5 h-3.5" />, "浅色"],
              ["dark", <MoonIcon key="d" className="w-3.5 h-3.5" />, "深色"],
              ["system", <MonitorIcon key="s" className="w-3.5 h-3.5" />, "跟随系统"],
            ] as [ThemeMode, ReactNode, string][]
          ).map(([m, icon, label]) => (
            <button
              key={m}
              title={label}
              onClick={() => onTheme(m)}
              className={`flex-1 h-6 flex items-center justify-center rounded-md transition-colors ${
                theme === m ? "bg-surface text-ink shadow-[var(--shadow)]" : "text-muted hover:text-ink"
              }`}
            >
              {icon}
            </button>
          ))}
        </div>
      </div>
    </aside>
  );
}
