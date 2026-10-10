/** 紧凑 token 数：1.2K / 3.4M / 5.6B */
export function fmtTokens(n: number | null | undefined): string {
  if (n == null) return "—";
  const abs = Math.abs(n);
  if (abs >= 1e9) return (n / 1e9).toFixed(abs >= 1e10 ? 1 : 2) + "B";
  if (abs >= 1e6) return (n / 1e6).toFixed(abs >= 1e7 ? 1 : 2) + "M";
  if (abs >= 1e3) return (n / 1e3).toFixed(1) + "K";
  return String(Math.round(n));
}

/** 带千分位的完整数字 */
export function fmtInt(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** 美元：大额取整数，小额保留到分 / 厘 */
export function fmtUsd(n: number | null | undefined, opts: { compact?: boolean } = {}): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n === 0) return "$0";
  const abs = Math.abs(n);
  if (opts.compact && abs >= 1e4) return "$" + (n / 1e3).toFixed(1) + "K";
  if (abs >= 1000) return "$" + n.toLocaleString("en-US", { maximumFractionDigits: 0 });
  if (abs >= 1) return "$" + n.toFixed(2);
  if (abs >= 0.01) return "$" + n.toFixed(3);
  return "<$0.01";
}

export function fmtPct(x: number, digits = 1): string {
  if (!Number.isFinite(x)) return "—";
  return (x * 100).toFixed(digits) + "%";
}

/** 相对时间：刚刚 / N 分钟前 / N 小时前 / N 天前 / 日期 */
export function fmtTime(ms: number | null | undefined): string {
  if (!ms) return "—";
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  return fmtDate(ms);
}

const pad = (n: number) => String(n).padStart(2, "0");

export function fmtDate(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function fmtDateTime(ms: number | null | undefined): string {
  if (!ms) return "—";
  const d = new Date(ms);
  return `${fmtDate(ms)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 把 /Users/<name>/ 或 C:\Users\<name>\ 前缀缩成 ~ */
export function shortPath(p: string): string {
  if (!p) return "—";
  const m = p.match(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)(.*)$/);
  return m ? "~" + m[2] : p;
}

/** 路径最后一段（项目名） */
export function baseName(p: string): string {
  if (!p) return "";
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
