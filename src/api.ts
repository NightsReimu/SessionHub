import { invoke } from "@tauri-apps/api/core";

export interface Capabilities {
  can_resume: boolean;
  can_delete: boolean;
  can_backup: boolean;
  can_read_messages: boolean;
  can_launch: boolean;
}

export interface AdapterInfo {
  id: string;
  name: string;
  detected: boolean;
  roots: string[];
  capabilities: Capabilities;
}

export interface Session {
  session_id: string;
  harness_id: string;
  project_path: string;
  title: string;
  started_at: number | null;
  ended_at: number | null;
  message_count: number | null;
  tokens_in: number | null;
  tokens_out: number | null;
  cost_usd: number | null;
  status: string;
  raw_path: string;
  source_format: string;
  file_size: number;
  file_mtime: number;
}

export interface SessionMeta {
  tags: string[];
  note: string;
  favorite: boolean;
  custom_title?: string | null;
}

export interface SessionDto extends Session {
  meta: SessionMeta;
  raw_usable: boolean;
}

export interface HarnessStat {
  harness_id: string;
  sessions: number;
  tokens_in: number;
  tokens_out: number;
  cost_usd: number;
}

export interface StatsOverview {
  total_sessions: number;
  total_tokens_in: number;
  total_tokens_out: number;
  total_cost_usd: number;
  per_harness: HarnessStat[];
  top_sessions: SessionDto[];
}

export interface AdapterScanStat {
  adapter_id: string;
  detected: boolean;
  scanned: number;
  parsed: number;
  errors: number;
}

export interface ScanReport {
  adapters: AdapterScanStat[];
  total_sessions: number;
  duration_ms: number;
}

export interface MessagePreview {
  role: string;
  text: string;
  timestamp: number | null;
}

export interface Counts {
  total: number;
  favorites: number;
  per_harness: Record<string, number>;
}

export interface HubPaths {
  hub_dir: string;
  backups_dir: string;
  exports_dir: string;
  db_path: string;
}

// 元数据保存全局串行化：快速连续修改标签/收藏/备注时，
// 保证请求按发起顺序落库，旧快照不可能覆盖新状态
let metaSaveChain: Promise<unknown> = Promise.resolve();

export const api = {
  listAdapters: () => invoke<AdapterInfo[]>("list_adapters"),
  scan: (full: boolean) => invoke<ScanReport>("scan_sessions", { full }),
  listSessions: (harness: string | null, favoritesOnly: boolean, limit = 800, offset = 0) =>
    invoke<SessionDto[]>("list_sessions", { harness, favoritesOnly, limit, offset }),
  searchSessions: (query: string) => invoke<SessionDto[]>("search_sessions", { query }),
  getMessages: (harnessId: string, sessionId: string, limit = 300) =>
    invoke<MessagePreview[]>("get_session_messages", { harnessId, sessionId, limit }),
  resume: (harnessId: string, sessionId: string) =>
    invoke<string>("resume_session", { harnessId, sessionId }),
  launchHarness: (harnessId: string, sessionId: string) =>
    invoke<string>("launch_harness", { harnessId, sessionId }),
  deleteSession: (harnessId: string, sessionId: string) =>
    invoke<string>("delete_session", { harnessId, sessionId }),
  backup: (harnessId: string, sessionId: string) =>
    invoke<string>("backup_session", { harnessId, sessionId }),
  exportSession: (harnessId: string, sessionId: string, format: "md" | "jsonl") =>
    invoke<string>("export_session", { harnessId, sessionId, format }),
  reveal: (harnessId: string, sessionId: string) =>
    invoke<string>("reveal_raw", { harnessId, sessionId }),
  setMeta: (harnessId: string, sessionId: string, meta: SessionMeta) => {
    const p = metaSaveChain.then(() =>
      invoke<void>("set_session_meta", { harnessId, sessionId, meta })
    );
    // 单个失败不阻塞后续保存，但调用方仍能感知本次失败
    metaSaveChain = p.catch(() => undefined);
    return p;
  },
  counts: () => invoke<Counts>("get_counts"),
  getStats: () => invoke<StatsOverview>("get_stats"),
  hubPaths: () => invoke<HubPaths>("get_hub_paths"),
  watcherStart: () => invoke<boolean>("watcher_start"),
  watcherStop: () => invoke<boolean>("watcher_stop"),
  watcherStatus: () => invoke<boolean>("watcher_status"),
};
