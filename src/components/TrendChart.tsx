import { useEffect, useMemo, useRef, useState } from "react";
import { Granularity } from "../api";
import { harnessColor, harnessName } from "../lib/harness";

export interface TrendSeries {
  id: string;
  /** bucket 起点 → 数值 */
  values: Map<number, number>;
}

interface Props {
  buckets: number[];
  /** 已按固定槽位排好序（堆叠自下而上） */
  series: TrendSeries[];
  granularity: Granularity;
  format: (v: number) => string;
  /** 坐标轴刻度用的更紧凑格式 */
  formatAxis: (v: number) => string;
  height?: number;
}

const PAD = { top: 12, right: 8, bottom: 26, left: 52 };
const GAP = 2; // 堆叠段之间的表面色间隙
const RADIUS = 4;
const WEEK = ["日", "一", "二", "三", "四", "五", "六"];
const pad2 = (n: number) => String(n).padStart(2, "0");

function niceTicks(max: number): number[] {
  if (max <= 0) return [0];
  const rough = max / 4;
  const pow = Math.pow(10, Math.floor(Math.log10(rough)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * pow).find((s) => s >= rough) ?? 10 * pow;
  const ticks: number[] = [];
  for (let v = 0; v <= max + step * 0.001; v += step) ticks.push(v);
  if (ticks[ticks.length - 1] < max) ticks.push(ticks[ticks.length - 1] + step);
  return ticks;
}

export function bucketLabel(ms: number, g: Granularity, multiDay: boolean): string {
  const d = new Date(ms);
  const md = `${pad2(d.getMonth() + 1)}/${pad2(d.getDate())}`;
  switch (g) {
    case "hour":
      return multiDay ? `${md} ${pad2(d.getHours())}时` : `${pad2(d.getHours())}:00`;
    case "month":
      return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}`;
    default:
      return md;
  }
}

export function bucketTitle(ms: number, g: Granularity): string {
  const d = new Date(ms);
  const ymd = `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
  switch (g) {
    case "hour":
      return `${ymd} ${pad2(d.getHours())}:00 – ${pad2((d.getHours() + 1) % 24)}:00`;
    case "week": {
      const e = new Date(d);
      e.setDate(e.getDate() + 6);
      return `${ymd} – ${pad2(e.getMonth() + 1)}-${pad2(e.getDate())}`;
    }
    case "month":
      return `${d.getFullYear()} 年 ${d.getMonth() + 1} 月`;
    default:
      return `${ymd} 周${WEEK[d.getDay()]}`;
  }
}

/** 顶端圆角、底端方角的柱段路径 */
function capPath(x: number, y: number, w: number, h: number, r: number): string {
  const rr = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + rr}Q${x},${y} ${x + rr},${y}H${x + w - rr}Q${x + w},${y} ${x + w},${y + rr}V${y + h}Z`;
}

export default function TrendChart({ buckets, series, granularity, format, formatAxis, height = 260 }: Props) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(640);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = Math.floor(entries[0].contentRect.width);
      if (w > 0) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const totals = useMemo(
    () => buckets.map((b) => series.reduce((acc, s) => acc + (s.values.get(b) ?? 0), 0)),
    [buckets, series]
  );
  const max = Math.max(0, ...totals);
  const ticks = niceTicks(max);
  const yMax = ticks[ticks.length - 1] || 1;

  const innerW = Math.max(10, width - PAD.left - PAD.right);
  const innerH = height - PAD.top - PAD.bottom;
  const slot = buckets.length > 0 ? innerW / buckets.length : innerW;
  const barW = Math.max(2, Math.min(24, slot * 0.62));
  const y = (v: number) => PAD.top + innerH - (v / yMax) * innerH;

  const multiDay =
    buckets.length > 1 &&
    new Date(buckets[0]).toDateString() !== new Date(buckets[buckets.length - 1]).toDateString();
  const labelEvery = Math.max(1, Math.ceil(64 / slot));

  const hb = hover != null ? buckets[hover] : null;
  const tipLeft = hover != null ? PAD.left + slot * hover + slot / 2 : 0;
  const tipFlip = tipLeft > width * 0.62;

  return (
    <div ref={boxRef} className="relative w-full select-none" onMouseLeave={() => setHover(null)}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label="用量趋势堆叠柱状图"
        className="block overflow-visible"
      >
        {/* 网格线与 Y 轴刻度 */}
        {ticks.map((t) => (
          <g key={t}>
            <line
              x1={PAD.left}
              x2={width - PAD.right}
              y1={y(t)}
              y2={y(t)}
              stroke={t === 0 ? "var(--axis)" : "var(--grid)"}
              strokeWidth={1}
              shapeRendering="crispEdges"
            />
            <text
              x={PAD.left - 8}
              y={y(t)}
              dy="0.32em"
              textAnchor="end"
              fontSize={11}
              fill="var(--muted)"
              className="tnum"
            >
              {formatAxis(t)}
            </text>
          </g>
        ))}

        {/* 悬停列底色 */}
        {hover != null && (
          <rect
            x={PAD.left + slot * hover}
            y={PAD.top}
            width={slot}
            height={innerH}
            fill="var(--surface-2)"
          />
        )}

        {/* 堆叠柱 */}
        {buckets.map((b, i) => {
          const x = PAD.left + slot * i + (slot - barW) / 2;
          let acc = 0;
          const segs = series
            .map((s) => ({ id: s.id, v: s.values.get(b) ?? 0 }))
            .filter((s) => s.v > 0);
          return (
            <g key={b} opacity={hover == null || hover === i ? 1 : 0.55}>
              {segs.map((s, k) => {
                const y0 = y(acc);
                acc += s.v;
                const y1 = y(acc);
                const top = k === segs.length - 1;
                // 段与段之间留 2px 表面色间隙（底段贴基线）
                const h = Math.max(0, y0 - y1 - (k > 0 ? GAP : 0));
                if (h <= 0) return null;
                return top ? (
                  <path key={s.id} d={capPath(x, y1, barW, h, RADIUS)} fill={harnessColor(s.id)} />
                ) : (
                  <rect key={s.id} x={x} y={y1} width={barW} height={h} fill={harnessColor(s.id)} />
                );
              })}
            </g>
          );
        })}

        {/* X 轴标签 */}
        {buckets.map((b, i) =>
          i % labelEvery === 0 ? (
            <text
              key={b}
              x={PAD.left + slot * i + slot / 2}
              y={height - 8}
              textAnchor="middle"
              fontSize={11}
              fill="var(--muted)"
              className="tnum"
            >
              {bucketLabel(b, granularity, multiDay)}
            </text>
          ) : null
        )}

        {/* 命中区：整列，比柱子本身大得多 */}
        {buckets.map((b, i) => (
          <rect
            key={b}
            x={PAD.left + slot * i}
            y={PAD.top}
            width={slot}
            height={innerH}
            fill="transparent"
            onMouseEnter={() => setHover(i)}
          />
        ))}
      </svg>

      {hover != null && hb != null && (
        <div
          className="pointer-events-none absolute z-10 min-w-[180px] rounded-lg border border-line bg-surface px-3 py-2.5 shadow-[var(--shadow-pop)]"
          style={{
            top: PAD.top,
            left: tipFlip ? undefined : tipLeft + slot / 2 + 8,
            right: tipFlip ? width - tipLeft + slot / 2 + 8 : undefined,
          }}
        >
          <div className="text-[11px] text-muted mb-1.5 tnum">{bucketTitle(hb, granularity)}</div>
          <div className="flex items-baseline justify-between gap-4 mb-1.5">
            <span className="text-[15px] font-semibold text-ink tnum">{format(totals[hover])}</span>
            <span className="text-[11px] text-muted">合计</span>
          </div>
          <div className="space-y-1">
            {[...series]
              .reverse()
              .filter((s) => (s.values.get(hb) ?? 0) > 0)
              .map((s) => (
                <div key={s.id} className="flex items-center gap-2 text-xs">
                  <span className="w-2.5 h-[3px] rounded-full shrink-0" style={{ background: harnessColor(s.id) }} />
                  <span className="font-medium text-ink tnum">{format(s.values.get(hb) ?? 0)}</span>
                  <span className="text-muted truncate">{harnessName(s.id)}</span>
                </div>
              ))}
            {totals[hover] === 0 && <div className="text-xs text-muted">无用量</div>}
          </div>
        </div>
      )}
    </div>
  );
}
