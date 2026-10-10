import { ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, Granularity, SessionDto, totalTokens, UsageReport, UsageSums } from "../api";
import TrendChart, { TrendSeries } from "../components/TrendChart";
import { Button, EmptyState, HarnessDot, HarnessTag, Segmented, Spinner } from "../components/ui";
import { CalendarIcon, ChartIcon, InfoIcon, RefreshIcon } from "../components/icons";
import { baseName, fmtDate, fmtInt, fmtPct, fmtTokens, fmtUsd } from "../lib/format";
import { bySlot, harnessColor, harnessName } from "../lib/harness";

type Preset = "today" | "7d" | "30d" | "90d" | "month" | "all" | "custom";
type Metric = "cost" | "tokens";

const PRESETS: { value: Preset; label: string }[] = [
  { value: "today", label: "今天" },
  { value: "7d", label: "近 7 天" },
  { value: "30d", label: "近 30 天" },
  { value: "90d", label: "近 90 天" },
  { value: "month", label: "本月" },
  { value: "all", label: "全部" },
  { value: "custom", label: "自定义" },
];

const DAY = 86_400_000;

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

function parseDateInput(s: string): Date | null {
  const m = s.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  return m ? new Date(+m[1], +m[2] - 1, +m[3]) : null;
}

/** 预设 → 本地时区的 [start, end) */
function presetRange(p: Preset, from: string, to: string): { start: number | null; end: number | null } {
  const today = startOfDay(new Date());
  const tomorrow = addDays(today, 1).getTime();
  switch (p) {
    case "today":
      return { start: today.getTime(), end: tomorrow };
    case "7d":
      return { start: addDays(today, -6).getTime(), end: tomorrow };
    case "30d":
      return { start: addDays(today, -29).getTime(), end: tomorrow };
    case "90d":
      return { start: addDays(today, -89).getTime(), end: tomorrow };
    case "month":
      return { start: new Date(today.getFullYear(), today.getMonth(), 1).getTime(), end: tomorrow };
    case "custom": {
      const a = parseDateInput(from);
      const b = parseDateInput(to);
      if (!a || !b) return { start: null, end: null };
      const [lo, hi] = a <= b ? [a, b] : [b, a];
      return { start: lo.getTime(), end: addDays(hi, 1).getTime() };
    }
    default:
      return { start: null, end: null };
  }
}

/** 与后端一致的本地时区桶对齐（周从周一开始） */
function alignBucket(ms: number, g: Granularity): number {
  const d = new Date(ms);
  switch (g) {
    case "hour":
      return new Date(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours()).getTime();
    case "week": {
      const back = (d.getDay() + 6) % 7;
      return new Date(d.getFullYear(), d.getMonth(), d.getDate() - back).getTime();
    }
    case "month":
      return new Date(d.getFullYear(), d.getMonth(), 1).getTime();
    default:
      return startOfDay(d).getTime();
  }
}

function nextBucket(ms: number, g: Granularity): number {
  const d = new Date(ms);
  switch (g) {
    case "hour":
      return new Date(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours() + 1).getTime();
    case "week":
      return addDays(d, 7).getTime();
    case "month":
      return new Date(d.getFullYear(), d.getMonth() + 1, 1).getTime();
    default:
      return addDays(d, 1).getTime();
  }
}

function makeBuckets(start: number, end: number, g: Granularity): number[] {
  const out: number[] = [];
  for (let b = alignBucket(start, g); b < end && out.length < 2000; b = nextBucket(b, g)) out.push(b);
  return out;
}

/** 某区间允许的粒度（避免 90 天按小时画 2000 根柱子） */
function allowedGranularities(span: number): Granularity[] {
  const out: Granularity[] = [];
  if (span <= 7 * DAY) out.push("hour");
  if (span <= 400 * DAY) out.push("day");
  if (span >= 14 * DAY) out.push("week");
  if (span >= 60 * DAY) out.push("month");
  return out;
}

const G_LABEL: Record<Granularity, string> = { hour: "按小时", day: "按天", week: "按周", month: "按月" };

function Card({
  title,
  right,
  children,
  className = "",
}: {
  title: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel ${className}`}>
      <header className="flex items-center justify-between gap-3 px-4 h-11 border-b border-line">
        <h3 className="text-[13px] font-semibold text-ink">{title}</h3>
        {right}
      </header>
      {children}
    </section>
  );
}

function Tile({ label, value, sub, warn }: { label: string; value: string; sub?: ReactNode; warn?: ReactNode }) {
  return (
    <div className="panel px-4 py-3.5 min-w-0">
      <div className="text-xs text-muted">{label}</div>
      <div className="mt-1.5 text-[22px] leading-7 font-semibold tracking-tight text-ink truncate">{value}</div>
      {sub && <div className="mt-1 text-xs text-ink-2 truncate tnum">{sub}</div>}
      {warn && <div className="mt-1 text-[11px] text-warn truncate">{warn}</div>}
    </div>
  );
}

/** 水平占比条（单系列，不需要图例） */
function Meter({ ratio, color }: { ratio: number; color: string }) {
  return (
    <div className="h-1.5 rounded-full bg-surface-3 overflow-hidden">
      <div
        className="h-full rounded-full"
        style={{ width: `${Math.max(ratio > 0 ? 1.5 : 0, Math.min(100, ratio * 100))}%`, background: color }}
      />
    </div>
  );
}

const metricOf = (m: Metric) => (s: UsageSums) => (m === "cost" ? s.cost_usd : totalTokens(s));

interface Props {
  harnesses: string[];
  /** 扫描完成后 +1，触发重新拉取 */
  refreshKey: number;
  onOpenSession: (s: SessionDto) => void;
}

export default function UsagePage({ harnesses, refreshKey, onOpenSession }: Props) {
  const [preset, setPreset] = useState<Preset>("30d");
  const [from, setFrom] = useState(() => fmtDate(addDays(new Date(), -13).getTime()));
  const [to, setTo] = useState(() => fmtDate(Date.now()));
  const [harness, setHarness] = useState<string | null>(null);
  const [metric, setMetric] = useState<Metric>("cost");
  const [gran, setGran] = useState<Granularity | null>(null);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const range = useMemo(() => presetRange(preset, from, to), [preset, from, to]);
  const span = range.start != null && range.end != null ? range.end - range.start : null;

  // 区间变了，手动粒度不再合法时回到自动
  useEffect(() => {
    if (gran && span != null && !allowedGranularities(span).includes(gran)) setGran(null);
  }, [span, gran]);

  const load = useCallback(() => {
    if (preset === "custom" && range.start == null) return;
    const id = ++seq.current;
    setLoading(true);
    setError(null);
    api
      .usageReport({ start_ms: range.start, end_ms: range.end, harness, granularity: gran })
      .then((r) => {
        if (id === seq.current) setReport(r);
      })
      .catch((e) => {
        if (id === seq.current) setError(String(e));
      })
      .finally(() => {
        if (id === seq.current) setLoading(false);
      });
  }, [range.start, range.end, harness, gran, preset]);

  useEffect(() => {
    load();
  }, [load, refreshKey]);

  const g = report?.granularity ?? "day";
  const buckets = useMemo(() => {
    if (!report) return [];
    const start = range.start ?? report.first_ts;
    const end = range.end ?? (report.last_ts != null ? report.last_ts + 1 : null);
    if (start == null || end == null) return [];
    return makeBuckets(start, end, report.granularity);
  }, [report, range.start, range.end]);

  const series: TrendSeries[] = useMemo(() => {
    if (!report) return [];
    const pick = metricOf(metric);
    const map = new Map<string, Map<number, number>>();
    for (const p of report.trend) {
      let m = map.get(p.harness_id);
      if (!m) map.set(p.harness_id, (m = new Map()));
      m.set(p.start, (m.get(p.start) ?? 0) + pick(p));
    }
    return [...map.keys()].sort(bySlot).map((id) => ({ id, values: map.get(id)! }));
  }, [report, metric]);

  const totals = report?.totals;
  const tokens = totals ? totalTokens(totals) : 0;
  const inputSide = totals ? totals.input + totals.cache_read + totals.cache_write : 0;
  const hitRate = totals && inputSide > 0 ? totals.cache_read / inputSide : NaN;
  const days =
    span != null
      ? Math.max(1, Math.round(span / DAY))
      : report?.first_ts != null && report.last_ts != null
        ? Math.max(1, Math.ceil((report.last_ts - report.first_ts) / DAY))
        : 1;

  const fmtMetric = metric === "cost" ? (v: number) => fmtUsd(v) : (v: number) => fmtTokens(v);
  const fmtAxis =
    metric === "cost"
      ? (v: number) => (v >= 1000 ? `$${(v / 1000).toFixed(v >= 1e4 ? 0 : 1)}K` : `$${+v.toFixed(2)}`)
      : (v: number) => fmtTokens(v);

  const pickMetric = metricOf(metric);
  const harnessTotal = report ? report.by_harness.reduce((a, h) => a + pickMetric(h), 0) : 0;
  const granOptions = span != null ? allowedGranularities(span) : (["day", "week", "month"] as Granularity[]);
  const empty = report != null && tokens === 0 && (totals?.cost_usd ?? 0) === 0;

  const rangeText =
    preset === "all"
      ? report?.first_ts != null && report.last_ts != null
        ? `${fmtDate(report.first_ts)} 至 ${fmtDate(report.last_ts)}`
        : "全部时间"
      : range.start != null && range.end != null
        ? range.end - range.start <= DAY
          ? fmtDate(range.start)
          : `${fmtDate(range.start)} 至 ${fmtDate(range.end - 1)}`
        : "";

  return (
    <div className="flex-1 min-w-0 overflow-y-auto">
      <div className="max-w-[1240px] mx-auto px-6 py-5 space-y-4">
        {/* 标题 */}
        <div className="flex items-end justify-between gap-4">
          <div>
            <h1 className="text-[20px] font-semibold tracking-tight text-ink">用量统计</h1>
            <div className="mt-1 text-xs text-muted flex items-center gap-1.5">
              <CalendarIcon className="w-3.5 h-3.5" />
              <span className="tnum">{rangeText}</span>
              {harness && <span>· 仅 {harnessName(harness)}</span>}
            </div>
          </div>
          <Button size="sm" onClick={load} disabled={loading} title="重新统计">
            {loading ? <Spinner className="w-3.5 h-3.5" /> : <RefreshIcon className="w-3.5 h-3.5" />}
            刷新
          </Button>
        </div>

        {/* 筛选：一行，作用于下方全部内容 */}
        <div className="flex flex-wrap items-center gap-2">
          <Segmented value={preset} options={PRESETS} onChange={setPreset} />
          {preset === "custom" && (
            <div className="flex items-center gap-1.5 h-8 px-2 rounded-lg border border-line bg-surface text-[13px]">
              <input
                type="date"
                value={from}
                max={to}
                onChange={(e) => setFrom(e.target.value)}
                className="bg-transparent outline-none tnum"
              />
              <span className="text-muted">至</span>
              <input
                type="date"
                value={to}
                min={from}
                onChange={(e) => setTo(e.target.value)}
                className="bg-transparent outline-none tnum"
              />
            </div>
          )}
          <div className="relative">
            <select
              value={harness ?? ""}
              onChange={(e) => setHarness(e.target.value || null)}
              className="h-8 pl-3 pr-8 rounded-lg border border-line bg-surface text-[13px] text-ink outline-none appearance-none hover:border-line-strong"
            >
              <option value="">全部工具</option>
              {[...harnesses].sort(bySlot).map((h) => (
                <option key={h} value={h}>
                  {harnessName(h)}
                </option>
              ))}
            </select>
            <span className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-muted text-[10px]">
              ▼
            </span>
          </div>
        </div>

        {error && (
          <div className="rounded-lg border border-danger/30 bg-danger-soft px-3 py-2 text-xs text-danger">
            统计失败：{error}
          </div>
        )}

        {!report ? (
          <div className="grid grid-cols-5 gap-3">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="panel h-[104px] animate-pulse" />
            ))}
          </div>
        ) : (
          <div className={`space-y-4 transition-opacity ${loading ? "opacity-60" : ""}`}>
            {/* 指标卡 */}
            <div className="grid grid-cols-5 gap-3">
              <Tile
                label="估算费用"
                value={fmtUsd(totals!.cost_usd)}
                sub={days > 1 ? `日均 ${fmtUsd(totals!.cost_usd / days)}` : undefined}
                warn={
                  totals!.unpriced_tokens > 0
                    ? `${fmtTokens(totals!.unpriced_tokens)} tokens 来自未定价模型`
                    : undefined
                }
              />
              <Tile
                label="Tokens 总量"
                value={fmtTokens(tokens)}
                sub={`输入 ${fmtTokens(inputSide)} · 输出 ${fmtTokens(totals!.output)}`}
              />
              <Tile
                label="请求次数"
                value={fmtInt(totals!.requests)}
                sub={totals!.requests > 0 ? `平均每次 ${fmtTokens(tokens / totals!.requests)} tokens` : undefined}
              />
              <Tile
                label="缓存命中率"
                value={Number.isFinite(hitRate) ? fmtPct(hitRate) : "—"}
                sub={`命中 ${fmtTokens(totals!.cache_read)} tokens`}
              />
              <Tile
                label="活跃会话"
                value={fmtInt(report.sessions)}
                sub={`${report.by_harness.length} 个工具 · ${report.by_model.length} 个模型`}
              />
            </div>

            {empty ? (
              <div className="panel">
                <EmptyState
                  icon={<ChartIcon className="w-8 h-8" />}
                  title="这个时间段没有用量记录"
                  hint="换一个时间范围，或先到「会话」页扫描一次本机会话"
                />
              </div>
            ) : (
              <>
                {/* 趋势 */}
                <Card
                  title="用量趋势"
                  right={
                    <div className="flex items-center gap-2">
                      <Segmented
                        size="sm"
                        value={gran ?? "auto"}
                        options={[
                          { value: "auto", label: "自动" },
                          ...granOptions.map((x) => ({ value: x, label: G_LABEL[x] })),
                        ]}
                        onChange={(v) => setGran(v === "auto" ? null : (v as Granularity))}
                      />
                      <Segmented
                        size="sm"
                        value={metric}
                        options={[
                          { value: "cost", label: "费用" },
                          { value: "tokens", label: "Tokens" },
                        ]}
                        onChange={setMetric}
                      />
                    </div>
                  }
                >
                  <div className="px-4 pt-3 pb-2">
                    {series.length > 1 && (
                      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 mb-2">
                        {series.map((s) => (
                          <span key={s.id} className="flex items-center gap-1.5 text-xs text-ink-2">
                            <span className="w-2.5 h-2.5 rounded-[3px]" style={{ background: harnessColor(s.id) }} />
                            {harnessName(s.id)}
                          </span>
                        ))}
                      </div>
                    )}
                    <TrendChart
                      buckets={buckets}
                      series={series}
                      granularity={g}
                      format={fmtMetric}
                      formatAxis={fmtAxis}
                    />
                  </div>
                </Card>

                <div className="grid grid-cols-[3fr_2fr] gap-4">
                  {/* 按工具 */}
                  <Card title="按工具" right={<span className="text-xs text-muted">点击只看该工具</span>}>
                    <div className="divide-y divide-line">
                      {report.by_harness.map((h) => {
                        const v = pickMetric(h);
                        const active = harness === h.key;
                        return (
                          <button
                            key={h.key}
                            onClick={() => setHarness(active ? null : h.key)}
                            className={`w-full text-left px-4 py-3 grid grid-cols-[150px_1fr_auto] items-center gap-4 transition-colors ${
                              active ? "bg-accent-soft" : "hover:bg-surface-2"
                            }`}
                          >
                            <span className="flex items-center gap-2 min-w-0">
                              <HarnessDot id={h.key} />
                              <span className="text-[13px] text-ink truncate">{harnessName(h.key)}</span>
                            </span>
                            <span className="min-w-0">
                              <Meter ratio={harnessTotal > 0 ? v / harnessTotal : 0} color={harnessColor(h.key)} />
                              <span className="block mt-1 text-[11px] text-muted tnum">
                                {h.sessions} 个会话 · {fmtInt(h.requests)} 次请求 ·{" "}
                                {metric === "cost" ? fmtTokens(totalTokens(h)) + " tokens" : fmtUsd(h.cost_usd)}
                              </span>
                            </span>
                            <span className="text-right">
                              <span className="block text-[13px] font-semibold text-ink tnum">{fmtMetric(v)}</span>
                              <span className="block text-[11px] text-muted tnum">
                                {harnessTotal > 0 ? fmtPct(v / harnessTotal) : "—"}
                              </span>
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </Card>

                  {/* Token 构成 */}
                  <Card title="Token 构成">
                    <div className="px-4 py-3 space-y-3">
                      {(
                        [
                          ["输入（未命中缓存）", totals!.input],
                          ["缓存读取", totals!.cache_read],
                          ["缓存写入", totals!.cache_write],
                          ["输出（含推理）", totals!.output],
                        ] as [string, number][]
                      ).map(([label, v]) => (
                        <div key={label}>
                          <div className="flex items-baseline justify-between text-xs mb-1">
                            <span className="text-ink-2">{label}</span>
                            <span className="tnum">
                              <span className="font-semibold text-ink">{fmtTokens(v)}</span>
                              <span className="text-muted ml-1.5">{tokens > 0 ? fmtPct(v / tokens) : "—"}</span>
                            </span>
                          </div>
                          <Meter ratio={tokens > 0 ? v / tokens : 0} color="var(--accent)" />
                        </div>
                      ))}
                      <p className="flex gap-1.5 pt-1 text-[11px] leading-relaxed text-muted">
                        <InfoIcon className="w-3.5 h-3.5 shrink-0 mt-px" />
                        缓存读取按约 1/10 输入价计费，命中率越高越省钱。
                      </p>
                    </div>
                  </Card>
                </div>

                {/* 按模型 */}
                <Card title="按模型" right={<span className="text-xs text-muted">{report.by_model.length} 个模型</span>}>
                  <div className="overflow-x-auto">
                    <table className="w-full text-[13px] tnum">
                      <thead>
                        <tr className="text-left text-[11px] text-muted border-b border-line">
                          <th className="font-medium px-4 py-2">模型</th>
                          <th className="font-medium px-2 py-2">来源</th>
                          <th className="font-medium px-2 py-2 text-right">请求</th>
                          <th className="font-medium px-2 py-2 text-right">输入</th>
                          <th className="font-medium px-2 py-2 text-right">缓存读</th>
                          <th className="font-medium px-2 py-2 text-right">缓存写</th>
                          <th className="font-medium px-2 py-2 text-right">输出</th>
                          <th className="font-medium px-4 py-2 text-right">费用</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-line">
                        {report.by_model.map((m) => (
                          <tr key={m.key} className="hover:bg-surface-2">
                            <td className="px-4 py-2 mono text-[12px] text-ink max-w-[260px] truncate" title={m.key}>
                              {m.key}
                            </td>
                            <td className="px-2 py-2">
                              <span className="flex items-center gap-1">
                                {m.harnesses.map((h) => (
                                  <span key={h} title={harnessName(h)}>
                                    <HarnessDot id={h} />
                                  </span>
                                ))}
                              </span>
                            </td>
                            <td className="px-2 py-2 text-right text-ink-2">{fmtInt(m.requests)}</td>
                            <td className="px-2 py-2 text-right text-ink-2">{fmtTokens(m.input)}</td>
                            <td className="px-2 py-2 text-right text-ink-2">{fmtTokens(m.cache_read)}</td>
                            <td className="px-2 py-2 text-right text-ink-2">{fmtTokens(m.cache_write)}</td>
                            <td className="px-2 py-2 text-right text-ink-2">{fmtTokens(m.output)}</td>
                            <td className="px-4 py-2 text-right font-medium text-ink">
                              {m.priced ? (
                                fmtUsd(m.cost_usd)
                              ) : (
                                <span className="text-[11px] font-normal text-warn" title="价格表里没有这个模型，费用未计入">
                                  未定价
                                </span>
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </Card>

                {/* Top 会话 */}
                <Card title="消耗最高的会话" right={<span className="text-xs text-muted">按费用排序</span>}>
                  <div className="divide-y divide-line">
                    {report.top_sessions.map((r, i) => {
                      const s = r.session;
                      const title = s ? s.meta.custom_title?.trim() || s.title || "(无标题)" : r.session_id;
                      return (
                        <button
                          key={r.harness_id + "/" + r.session_id}
                          disabled={!s}
                          onClick={() => s && onOpenSession(s)}
                          title={s ? "打开会话" : "该会话的源文件已被清理，仅保留用量记录"}
                          className="w-full text-left px-4 py-2.5 flex items-center gap-3 hover:bg-surface-2 disabled:hover:bg-transparent"
                        >
                          <span className="w-5 text-xs text-muted tnum text-right shrink-0">{i + 1}</span>
                          <span className="flex-1 min-w-0">
                            <span className={`block truncate text-[13px] ${s ? "text-ink" : "text-muted"}`}>
                              {title}
                            </span>
                            <span className="flex items-center gap-2 mt-0.5 text-[11px] text-muted">
                              <HarnessTag id={r.harness_id} compact />
                              {s && <span className="mono truncate">{baseName(s.project_path)}</span>}
                              {!s && <span>源文件已清理</span>}
                            </span>
                          </span>
                          <span className="text-right shrink-0 tnum">
                            <span className="block text-[13px] font-semibold text-ink">{fmtUsd(r.cost_usd)}</span>
                            <span className="block text-[11px] text-muted">{fmtTokens(totalTokens(r))} tokens</span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </Card>
              </>
            )}

            <p className="flex gap-1.5 text-[11px] leading-relaxed text-muted pb-2">
              <InfoIcon className="w-3.5 h-3.5 shrink-0 mt-px" />
              <span>
                数据来自本机各 harness 的会话日志，按请求时间归入日期。费用按各模型公开刊例价估算（OpenCode
                使用其记录的实际费用）；订阅制套餐的实际花费会低于这里的估算。Claude Code 已按
                message.id + requestId 去重，Codex 按累计计数的增量统计；harness 自动清理的旧会话仍保留历史用量。
              </span>
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
