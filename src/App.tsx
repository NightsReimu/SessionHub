import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, AdapterInfo, Counts, ScanProgress, SessionDto } from "./api";
import Sidebar, { View } from "./components/Sidebar";
import SessionList, { sessionKey } from "./components/SessionList";
import SessionDetail from "./components/SessionDetail";
import UsagePage from "./pages/UsagePage";
import { Button, EmptyState, Spinner, Toast, Toasts } from "./components/ui";
import { ChatIcon, RefreshIcon, SearchIcon } from "./components/icons";
import { harnessName } from "./lib/harness";
import { useTheme } from "./lib/theme";

export type { Toast };

let toastSeq = 1;

/** 扫描进度条：自己订阅进度事件，进度更新只重渲染这一小块 */
function ScanProgressBar({ active }: { active: boolean }) {
  const [p, setP] = useState<ScanProgress | null>(null);

  useEffect(() => {
    let idle: number | undefined;
    const un = listen<ScanProgress>("scan-progress", (e) => {
      const q = e.payload;
      // 最后一个 adapter 处理完即视为结束；另外 3 秒没有新进度也收起（监听触发的扫描没有结束信号）
      const finished = q.adapter_index + 1 >= q.adapter_count && q.total > 0 && q.done >= q.total;
      setP(finished ? null : q);
      window.clearTimeout(idle);
      idle = window.setTimeout(() => setP(null), 3000);
    });
    const un2 = listen("scan-update", () => setP(null));
    return () => {
      window.clearTimeout(idle);
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!active) setP(null);
  }, [active]);

  if (!active && !p) return null;
  const pct =
    p && p.adapter_count > 0
      ? Math.min(100, ((p.adapter_index + (p.total > 0 ? p.done / p.total : 0)) / p.adapter_count) * 100)
      : null;
  return (
    <div className="px-4 pb-3 -mt-1">
      <div className="flex items-center justify-between text-[11px] text-muted mb-1.5 tnum">
        <span>
          {p ? (
            <>
              正在扫描 <span className="text-ink-2">{harnessName(p.adapter_id)}</span>（{p.adapter_index + 1}/
              {p.adapter_count}）
            </>
          ) : (
            "准备扫描…"
          )}
        </span>
        <span>
          {p && p.total > 0 ? `${p.done}/${p.total} · 更新 ${p.parsed} · 未变 ${p.skipped}` : "枚举文件…"}
          {p && p.errors > 0 && <span className="text-warn"> · 错误 {p.errors}</span>}
        </span>
      </div>
      <div className="h-1 rounded-full bg-surface-3 overflow-hidden relative">
        {pct == null ? (
          <div className="absolute inset-y-0 w-2/5 rounded-full bg-accent progress-indeterminate" />
        ) : (
          <div className="h-full rounded-full bg-accent transition-[width] duration-200" style={{ width: `${pct}%` }} />
        )}
      </div>
    </div>
  );
}

export default function App() {
  const [theme, setTheme] = useTheme();
  const [view, setView] = useState<View>("sessions");
  const [adapters, setAdapters] = useState<AdapterInfo[]>([]);
  const [counts, setCounts] = useState<Counts>({ total: 0, favorites: 0, per_harness: {} });
  const [sessions, setSessions] = useState<SessionDto[]>([]);
  const [listLoading, setListLoading] = useState(true);
  const [filter, setFilter] = useState<string>("all"); // "all" | "fav" | harness id
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [selected, setSelected] = useState<SessionDto | null>(null);
  const [scanning, setScanning] = useState(false);
  const [watching, setWatching] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [dataVersion, setDataVersion] = useState(0);
  // 请求序号：只应用最后一次请求的结果，防止乱序响应覆盖新结果
  const reqSeq = useRef(0);
  const searchRef = useRef<HTMLInputElement>(null);

  const toast = useCallback((kind: Toast["kind"], text: string) => {
    const id = toastSeq++;
    setToasts((t) => [...t, { id, kind, text }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 6000);
  }, []);

  const refreshCounts = useCallback(async () => {
    try {
      setCounts(await api.counts());
    } catch (e) {
      console.error(e);
    }
  }, []);

  const refreshList = useCallback(async () => {
    const seq = ++reqSeq.current;
    try {
      const data = debouncedQuery
        ? await api.searchSessions(debouncedQuery)
        : await api.listSessions(filter === "all" || filter === "fav" ? null : filter, filter === "fav");
      if (seq === reqSeq.current) setSessions(data);
    } catch (e) {
      console.error(e);
    } finally {
      if (seq === reqSeq.current) setListLoading(false);
    }
  }, [filter, debouncedQuery]);

  const runScan = useCallback(
    async (full: boolean) => {
      setScanning(true);
      try {
        const report = await api.scan(full);
        const changed = report.adapters.reduce((a, x) => a + x.parsed, 0);
        toast(
          "ok",
          `扫描完成：共 ${report.total_sessions} 个会话，更新 ${changed} 个，用时 ${(report.duration_ms / 1000).toFixed(1)}s`
        );
        setAdapters(await api.listAdapters());
        await refreshCounts();
        await refreshList();
        setDataVersion((v) => v + 1);
      } catch (e) {
        toast("err", `扫描失败：${String(e)}`);
      } finally {
        setScanning(false);
      }
    },
    [toast, refreshCounts, refreshList]
  );

  useEffect(() => {
    (async () => {
      try {
        setAdapters(await api.listAdapters());
        setWatching(await api.watcherStatus());
      } catch (e) {
        console.error(e);
      }
      await refreshCounts();
      await refreshList();
      // 首次启动自动做一次增量扫描
      runScan(false);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const t = window.setTimeout(() => setDebouncedQuery(query.trim()), 250);
    return () => window.clearTimeout(t);
  }, [query]);

  useEffect(() => {
    refreshList();
  }, [filter, debouncedQuery, refreshList]);

  // 实时监听触发的增量扫描完成
  useEffect(() => {
    const un = listen("scan-update", () => {
      refreshCounts();
      refreshList();
      setDataVersion((v) => v + 1);
    });
    return () => {
      un.then((f) => f());
    };
  }, [refreshCounts, refreshList]);

  // ⌘K / Ctrl+K 聚焦搜索，Esc 关闭详情
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setView("sessions");
        window.setTimeout(() => searchRef.current?.focus(), 0);
      } else if (e.key === "Escape" && document.activeElement === searchRef.current) {
        setQuery("");
        searchRef.current?.blur();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const toggleWatcher = async () => {
    try {
      const on = watching ? await api.watcherStop() : await api.watcherStart();
      setWatching(on);
      toast("info", on ? "实时监听已开启：会话有变化会自动更新" : "实时监听已关闭");
    } catch (e) {
      toast("err", `监听切换失败：${String(e)}`);
    }
  };

  // 静默增量重扫（迁移后调用）：不弹 toast
  const quietRescan = useCallback(async () => {
    try {
      await api.scan(false);
      await refreshCounts();
      await refreshList();
      setDataVersion((v) => v + 1);
    } catch (e) {
      console.error(e);
    }
  }, [refreshCounts, refreshList]);

  const patchSelected = useCallback((s: SessionDto) => {
    setSelected(s);
    setSessions((list) => list.map((x) => (sessionKey(x) === sessionKey(s) ? s : x)));
  }, []);

  const removeFromList = useCallback(
    (s: SessionDto) => {
      setSessions((list) => list.filter((x) => sessionKey(x) !== sessionKey(s)));
      setSelected((cur) => (cur && sessionKey(cur) === sessionKey(s) ? null : cur));
      refreshCounts();
    },
    [refreshCounts]
  );

  const openFromUsage = useCallback((s: SessionDto) => {
    setView("sessions");
    setFilter("all");
    setQuery("");
    setSelected(s);
  }, []);

  const filterLabel =
    filter === "all" ? "全部会话" : filter === "fav" ? "收藏" : (adapters.find((a) => a.id === filter)?.name ?? filter);
  const harnessesWithData = Object.keys(counts.per_harness);

  return (
    <div className="flex h-full bg-bg text-ink">
      <Sidebar
        view={view}
        onView={setView}
        adapters={adapters}
        counts={counts}
        filter={filter}
        onFilter={(f) => {
          setFilter(f);
          setQuery("");
        }}
        watching={watching}
        onToggleWatcher={toggleWatcher}
        theme={theme}
        onTheme={setTheme}
      />

      {view === "usage" ? (
        <UsagePage harnesses={harnessesWithData} refreshKey={dataVersion} onOpenSession={openFromUsage} />
      ) : (
        <main className="flex-1 min-w-0 flex flex-col">
          <header className="shrink-0">
            <div className="flex items-center gap-3 px-4 h-14">
              <div className="min-w-0">
                <div className="text-[15px] font-semibold text-ink truncate">
                  {debouncedQuery ? "搜索结果" : filterLabel}
                </div>
              </div>
              <span className="text-xs text-muted tnum">{sessions.length} 个</span>
              <div className="flex-1" />
              <label className="flex items-center gap-2 w-[340px] h-8 px-2.5 rounded-lg border border-line bg-surface focus-within:border-accent transition-colors">
                <SearchIcon className="w-4 h-4 text-muted shrink-0" />
                <input
                  ref={searchRef}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="搜索标题、项目路径、标签、备注"
                  className="flex-1 min-w-0 bg-transparent text-[13px] outline-none placeholder:text-muted text-ink"
                />
                <kbd className="text-[10px] text-muted border border-line rounded px-1 py-px">⌘K</kbd>
              </label>
              <Button
                variant="primary"
                onClick={() => runScan(false)}
                disabled={scanning}
                title="增量扫描：只解析有变化的会话"
              >
                {scanning ? <Spinner className="w-3.5 h-3.5" /> : <RefreshIcon className="w-3.5 h-3.5" />}
                {scanning ? "扫描中" : "扫描"}
              </Button>
              <Button onClick={() => runScan(true)} disabled={scanning} title="重新解析所有会话文件，并清理已删除的会话">
                全量重扫
              </Button>
            </div>
            <ScanProgressBar active={scanning} />
          </header>

          <div className="flex-1 flex gap-3 min-h-0 px-4 pb-4">
            <section className="flex-1 min-w-0 panel flex overflow-hidden">
              <SessionList
                sessions={sessions}
                selectedKey={selected ? sessionKey(selected) : null}
                onSelect={setSelected}
                loading={listLoading}
                searching={!!debouncedQuery}
              />
            </section>
            {selected ? (
              <SessionDetail
                session={selected}
                adapters={adapters}
                onPatch={patchSelected}
                onRemoved={removeFromList}
                onClose={() => setSelected(null)}
                onMigrated={quietRescan}
                toast={toast}
              />
            ) : (
              <section className="w-[540px] shrink-0 panel flex">
                <EmptyState
                  icon={<ChatIcon className="w-8 h-8" />}
                  title="选择一个会话"
                  hint="在左侧列表中点选会话，查看完整对话、续接、导出或迁移"
                />
              </section>
            )}
          </div>
        </main>
      )}

      <Toasts toasts={toasts} onDismiss={(id) => setToasts((t) => t.filter((x) => x.id !== id))} />
    </div>
  );
}
