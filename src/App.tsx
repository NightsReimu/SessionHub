import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, AdapterInfo, Counts, HubPaths, ScanProgress, SessionDto, StatsOverview } from "./api";
import Sidebar from "./components/Sidebar";
import SessionList from "./components/SessionList";
import SessionDetail from "./components/SessionDetail";
import StatsModal from "./components/StatsModal";
import MouseTrail from "./components/MouseTrail";

export interface Toast {
  id: number;
  kind: "ok" | "err" | "info";
  text: string;
}

let toastSeq = 1;

export default function App() {
  const [adapters, setAdapters] = useState<AdapterInfo[]>([]);
  const [counts, setCounts] = useState<Counts>({ total: 0, favorites: 0, per_harness: {} });
  const [sessions, setSessions] = useState<SessionDto[]>([]);
  const [filter, setFilter] = useState<string>("all"); // "all" | "fav" | harness id
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [selected, setSelected] = useState<SessionDto | null>(null);
  const [scanning, setScanning] = useState(false);
  const [watching, setWatching] = useState(false);
  const [hub, setHub] = useState<HubPaths | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [stats, setStats] = useState<StatsOverview | null>(null);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  // 请求序号：只应用最后一次请求的结果，防止乱序响应覆盖新结果
  const reqSeq = useRef(0);

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
        : await api.listSessions(
            filter === "all" ? null : filter === "fav" ? null : filter,
            filter === "fav"
          );
      if (seq === reqSeq.current) setSessions(data);
    } catch (e) {
      console.error(e);
    }
  }, [filter, debouncedQuery]);

  const runScan = useCallback(
    async (full: boolean) => {
      setScanning(true);
      setProgress(null);
      try {
        const report = await api.scan(full);
        toast("ok", `扫描完成：${report.total_sessions} 个会话，耗时 ${(report.duration_ms / 1000).toFixed(1)}s`);
        setAdapters(await api.listAdapters());
        await refreshCounts();
        await refreshList();
      } catch (e) {
        toast("err", `扫描失败：${String(e)}`);
      } finally {
        setScanning(false);
        setProgress(null);
      }
    },
    [toast, refreshCounts, refreshList]
  );

  useEffect(() => {
    (async () => {
      try {
        setAdapters(await api.listAdapters());
        setHub(await api.hubPaths());
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
    const t = window.setTimeout(() => setDebouncedQuery(query.trim()), 300);
    return () => window.clearTimeout(t);
  }, [query]);

  useEffect(() => {
    refreshList();
  }, [filter, debouncedQuery, refreshList]);

  useEffect(() => {
    const un = listen("scan-update", () => {
      setProgress(null);
      refreshCounts();
      refreshList();
    });
    const un2 = listen<ScanProgress>("scan-progress", (e) => {
      setProgress(e.payload);
    });
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, [refreshCounts, refreshList]);

  const onQueryChange = (q: string) => {
    setQuery(q);
  };

  const toggleWatcher = async () => {
    try {
      const on = watching ? await api.watcherStop() : await api.watcherStart();
      setWatching(on);
      toast("info", on ? "文件监听已开启" : "文件监听已停止");
    } catch (e) {
      toast("err", `监听切换失败：${String(e)}`);
    }
  };

  // 静默增量重扫（迁移后调用）：不弹 toast、不动进度条
  const quietRescan = useCallback(async () => {
    try {
      await api.scan(false);
      await refreshCounts();
      await refreshList();
    } catch (e) {
      console.error(e);
    }
  }, [refreshCounts, refreshList]);

  const openStats = async () => {
    try {
      setStats(await api.getStats());
    } catch (e) {
      toast("err", `统计加载失败：${String(e)}`);
    }
  };

  const patchSelected = (s: SessionDto) => {
    setSelected(s);
    setSessions((list) => list.map((x) => (x.harness_id === s.harness_id && x.session_id === s.session_id ? s : x)));
  };

  const removeFromList = (s: SessionDto) => {
    setSessions((list) => list.filter((x) => !(x.harness_id === s.harness_id && x.session_id === s.session_id)));
    if (selected?.session_id === s.session_id && selected?.harness_id === s.harness_id) setSelected(null);
    refreshCounts();
  };

  return (
    <div className="relative flex h-full gap-2.5 p-2.5 bg-page">
      {/* 底部彩色漂浮光斑：透过玻璃面板形成玻璃拟态 */}
      <div className="pointer-events-none absolute inset-0 overflow-hidden">
        <div className="absolute -top-32 -left-24 w-[480px] h-[480px] rounded-full bg-violet-600/15 blur-[110px] animate-blob1" />
        <div className="absolute top-1/3 -right-32 w-[420px] h-[420px] rounded-full bg-amber-500/13 blur-[110px] animate-blob2" />
        <div className="absolute -bottom-40 left-1/3 w-[460px] h-[460px] rounded-full bg-cyan-500/11 blur-[120px] animate-blob3" />
      </div>
      <MouseTrail />
      <Sidebar
        adapters={adapters}
        counts={counts}
        filter={filter}
        onFilter={(f) => {
          setFilter(f);
          setQuery("");
        }}
        watching={watching}
        onToggleWatcher={toggleWatcher}
        onOpenStats={openStats}
      />

      <div className="flex-1 flex flex-col gap-2.5 min-w-0">
        <div className="flex items-center gap-2.5 rounded-xl glass px-3.5 py-2.5">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" className="w-4 h-4 text-dim shrink-0">
            <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-4.35-4.35m1.6-4.4a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder="搜索标题 / 项目路径 / 标签 / 备注…"
            className="flex-1 bg-transparent text-sm outline-none placeholder:text-dim text-ink"
          />
          <button
            onClick={() => runScan(false)}
            disabled={scanning}
            className="px-3.5 py-1.5 text-xs font-medium rounded-lg bg-accent text-black hover:bg-accent-strong transition-colors disabled:opacity-50"
          >
            {scanning ? "扫描中…" : "扫描"}
          </button>
          <button
            onClick={() => runScan(true)}
            disabled={scanning}
            title="重新解析所有会话文件"
            className="px-3.5 py-1.5 text-xs rounded-lg border border-line text-mut hover:text-ink hover:bg-raise transition-colors disabled:opacity-50"
          >
            全量重扫
          </button>
        </div>

        {progress && (
          <div className="rounded-xl glass px-4 py-2.5">
            <div className="flex items-center justify-between text-[11px] text-mut mb-1.5">
              <span>
                正在扫描 <span className="text-ink">{progress.adapter_id}</span>
                <span className="text-dim">
                  （{progress.adapter_index + 1}/{progress.adapter_count}）
                </span>
              </span>
              <span className="text-dim">
                {progress.total > 0
                  ? `${progress.done}/${progress.total} · 解析 ${progress.parsed} · 跳过 ${progress.skipped}`
                  : "枚举文件…"}
                {progress.errors > 0 && <span className="text-amber-400"> · 错误 {progress.errors}</span>}
              </span>
            </div>
            <div className="h-1.5 rounded-full bg-raise overflow-hidden">
              <div
                className="h-full rounded-full bg-accent transition-all duration-200"
                style={{
                  width: `${
                    progress.adapter_count > 0
                      ? Math.min(
                          100,
                          ((progress.adapter_index +
                            (progress.total > 0 ? progress.done / progress.total : 0)) /
                            progress.adapter_count) *
                            100
                        )
                      : 0
                  }%`,
                }}
              />
            </div>
          </div>
        )}

        <div className="flex-1 flex gap-2.5 min-h-0">
          <div className="flex-1 rounded-xl glass overflow-hidden flex min-w-0">
            <SessionList sessions={sessions} selected={selected} onSelect={setSelected} />
          </div>
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
            <div className="w-[520px] shrink-0 rounded-xl glass flex flex-col items-center justify-center gap-3 text-dim">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.2" className="w-10 h-10 opacity-60">
                <path strokeLinecap="round" strokeLinejoin="round" d="M8 10h8m-8 4h5M21 12a9 9 0 01-13.2 7.9L3 21l1.1-4.8A9 9 0 1121 12z" />
              </svg>
              <div className="text-sm">选择左侧会话查看对话记录</div>
            </div>
          )}
        </div>
      </div>

      {stats && (
        <StatsModal
          stats={stats}
          onClose={() => setStats(null)}
          onSelect={(s) => {
            setSelected(s);
            setStats(null);
          }}
        />
      )}

      <div className="fixed bottom-4 right-4 flex flex-col gap-2 z-50 max-w-md">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`px-3.5 py-2.5 rounded-xl text-sm shadow-xl border break-all ${
              t.kind === "ok"
                ? "bg-emerald-950/95 border-emerald-700/60 text-emerald-200"
                : t.kind === "err"
                  ? "bg-red-950/95 border-red-700/60 text-red-200"
                  : "bg-zinc-900/95 border-zinc-700/60 text-zinc-200"
            }`}
          >
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
