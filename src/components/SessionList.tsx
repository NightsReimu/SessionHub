import { memo, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { SessionDto } from "../api";
import { baseName, fmtDateTime, fmtTime, fmtTokens, fmtUsd, shortPath } from "../lib/format";
import { harnessName } from "../lib/harness";
import { InboxIcon, StarIcon } from "./icons";
import { EmptyState, HarnessDot } from "./ui";

interface Props {
  sessions: SessionDto[];
  selectedKey: string | null;
  onSelect: (s: SessionDto) => void;
  loading: boolean;
  searching: boolean;
}

export const sessionKey = (s: { harness_id: string; session_id: string }) =>
  s.harness_id + "/" + s.session_id;

const ROW_H = 66;

const Row = memo(function Row({
  s,
  active,
  onSelect,
}: {
  s: SessionDto;
  active: boolean;
  onSelect: (s: SessionDto) => void;
}) {
  const tokens = (s.tokens_in ?? 0) + (s.tokens_out ?? 0);
  const title = s.meta.custom_title?.trim() || s.title || "(无标题)";
  const project = baseName(s.project_path);
  return (
    <div
      onClick={() => onSelect(s)}
      className={`relative h-full rounded-lg px-3 py-2.5 flex flex-col justify-center gap-1.5 transition-colors ${
        active ? "bg-accent-soft" : "hover:bg-surface-2"
      }`}
    >
      {active && <span className="absolute left-0 top-2.5 bottom-2.5 w-[3px] rounded-full bg-accent" />}
      <div className="flex items-center gap-2 min-w-0">
        {s.meta.favorite && <StarIcon filled className="w-3.5 h-3.5 shrink-0" />}
        <span className="truncate text-[13px] font-medium text-ink flex-1">{title}</span>
        <span
          className="shrink-0 text-[11px] text-muted tnum"
          title={fmtDateTime(s.ended_at ?? s.started_at)}
        >
          {fmtTime(s.ended_at ?? s.started_at)}
        </span>
      </div>
      <div className="flex items-center gap-2.5 text-[11.5px] text-muted min-w-0">
        <span className="flex items-center gap-1.5 shrink-0 text-ink-2">
          <HarnessDot id={s.harness_id} size={7} />
          {harnessName(s.harness_id)}
        </span>
        {project && (
          <span className="truncate mono text-[11px]" title={shortPath(s.project_path)}>
            {project}
          </span>
        )}
        <span className="flex-1" />
        {s.message_count != null && <span className="shrink-0 tnum">{s.message_count} 条</span>}
        {tokens > 0 && <span className="shrink-0 tnum">{fmtTokens(tokens)}</span>}
        {s.cost_usd != null && s.cost_usd > 0 && (
          <span className="shrink-0 tnum text-ink-2">{fmtUsd(s.cost_usd)}</span>
        )}
      </div>
    </div>
  );
});

export default function SessionList({ sessions, selectedKey, onSelect, loading, searching }: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: sessions.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 10,
  });

  if (sessions.length === 0) {
    return loading ? (
      <div className="flex-1 p-3 space-y-2">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="h-[58px] rounded-lg bg-surface-2 animate-pulse" />
        ))}
      </div>
    ) : (
      <EmptyState
        icon={<InboxIcon className="w-8 h-8" />}
        title={searching ? "没有匹配的会话" : "还没有会话"}
        hint={searching ? "换个关键词试试，支持标题、项目路径、标签和备注" : "点击右上角「扫描」发现本机各 harness 的会话"}
      />
    );
  }

  return (
    <div ref={parentRef} className="flex-1 overflow-y-auto min-w-0 px-2 py-2">
      <div style={{ height: v.getTotalSize(), position: "relative" }}>
        {v.getVirtualItems().map((row) => {
          const s = sessions[row.index];
          const key = sessionKey(s);
          return (
            <div
              key={key}
              style={{ transform: `translateY(${row.start}px)`, height: ROW_H }}
              className="absolute top-0 left-0 w-full py-[2px]"
            >
              <Row s={s} active={key === selectedKey} onSelect={onSelect} />
            </div>
          );
        })}
      </div>
    </div>
  );
}
