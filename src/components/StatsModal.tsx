import { SessionDto, StatsOverview } from "../api";
import { harnessBadge } from "./Sidebar";
import { fmtTokens } from "./SessionList";

interface Props {
  stats: StatsOverview;
  onClose: () => void;
  onSelect: (s: SessionDto) => void;
}

export default function StatsModal({ stats, onClose, onSelect }: Props) {
  const card = (label: string, value: string) => (
    <div className="bg-zinc-900 rounded-lg px-4 py-3 flex-1">
      <div className="text-[11px] text-zinc-500">{label}</div>
      <div className="text-xl font-semibold text-zinc-100 mt-1">{value}</div>
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-40 bg-black/60 flex items-center justify-center p-6"
      onClick={onClose}
    >
      <div
        className="bg-zinc-950 border border-zinc-800 rounded-xl w-full max-w-2xl max-h-[85vh] overflow-y-auto p-5 space-y-5"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold">用量统计</h2>
          <button onClick={onClose} className="text-zinc-500 hover:text-zinc-200 text-xl leading-none">
            ×
          </button>
        </div>

        <div className="flex gap-3">
          {card("会话总数", String(stats.total_sessions))}
          {card("总输入 Tokens", fmtTokens(stats.total_tokens_in))}
          {card("总输出 Tokens", fmtTokens(stats.total_tokens_out))}
          {card("总费用", stats.total_cost_usd > 0 ? `$${stats.total_cost_usd.toFixed(2)}` : "—")}
        </div>

        <div>
          <div className="text-xs text-zinc-500 mb-2">按 Harness</div>
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-[11px] text-zinc-500 border-b border-zinc-800">
                <th className="py-1.5 font-normal">Harness</th>
                <th className="py-1.5 font-normal text-right">会话</th>
                <th className="py-1.5 font-normal text-right">输入</th>
                <th className="py-1.5 font-normal text-right">输出</th>
                <th className="py-1.5 font-normal text-right">费用</th>
              </tr>
            </thead>
            <tbody>
              {stats.per_harness.map((h) => (
                <tr key={h.harness_id} className="border-b border-zinc-900">
                  <td className="py-1.5">
                    <span className={`text-[10px] px-1.5 py-0.5 rounded border ${harnessBadge(h.harness_id)}`}>
                      {h.harness_id}
                    </span>
                  </td>
                  <td className="py-1.5 text-right">{h.sessions}</td>
                  <td className="py-1.5 text-right">{fmtTokens(h.tokens_in)}</td>
                  <td className="py-1.5 text-right">{fmtTokens(h.tokens_out)}</td>
                  <td className="py-1.5 text-right">{h.cost_usd > 0 ? `$${h.cost_usd.toFixed(2)}` : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <div>
          <div className="text-xs text-zinc-500 mb-2">Token 消耗 Top {stats.top_sessions.length}</div>
          <div className="space-y-1">
            {stats.top_sessions.map((s) => (
              <button
                key={s.harness_id + "/" + s.session_id}
                onClick={() => onSelect(s)}
                className="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg hover:bg-zinc-900 text-left"
              >
                <span className={`shrink-0 text-[10px] px-1.5 py-0.5 rounded border ${harnessBadge(s.harness_id)}`}>
                  {s.harness_id}
                </span>
                <span className="truncate text-sm text-zinc-200 flex-1">{s.title || "(无标题)"}</span>
                <span className="shrink-0 text-xs text-zinc-500">
                  {fmtTokens((s.tokens_in ?? 0) + (s.tokens_out ?? 0))} tok
                </span>
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
