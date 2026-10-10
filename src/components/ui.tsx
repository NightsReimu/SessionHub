import type { ButtonHTMLAttributes, ReactNode } from "react";
import { harnessColor, harnessName } from "../lib/harness";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANT: Record<Variant, string> = {
  primary: "bg-primary text-on-primary hover:bg-primary-hover border border-transparent",
  secondary: "bg-surface text-ink border border-line hover:bg-surface-2 hover:border-line-strong",
  ghost: "text-ink-2 hover:text-ink hover:bg-surface-2 border border-transparent",
  danger: "bg-surface text-danger border border-line hover:bg-danger-soft hover:border-danger/40",
};

export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: "sm" | "md";
}) {
  const sz = size === "sm" ? "h-7 px-2.5 text-xs gap-1.5" : "h-8 px-3 text-[13px] gap-1.5";
  return (
    <button
      className={`inline-flex items-center justify-center rounded-lg font-medium whitespace-nowrap transition-colors disabled:opacity-45 disabled:pointer-events-none ${sz} ${VARIANT[variant]} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

export function IconButton({
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={`inline-flex items-center justify-center w-7 h-7 rounded-md text-muted hover:text-ink hover:bg-surface-2 transition-colors disabled:opacity-40 ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

/** 分段选择器（日期预设、指标切换） */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = "md",
}: {
  value: T;
  options: { value: T; label: ReactNode; title?: string }[];
  onChange: (v: T) => void;
  size?: "sm" | "md";
}) {
  return (
    <div className="inline-flex items-center p-0.5 rounded-lg bg-surface-2 border border-line">
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            title={o.title}
            onClick={() => onChange(o.value)}
            className={`${size === "sm" ? "h-6 px-2 text-xs" : "h-7 px-3 text-[13px]"} rounded-md whitespace-nowrap transition-colors ${
              active
                ? "bg-surface text-ink font-medium shadow-[var(--shadow)] border border-line"
                : "text-ink-2 hover:text-ink border border-transparent"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/** harness 身份：彩色圆点 + 中性文字（文字从不使用数据色） */
export function HarnessTag({ id, compact = false }: { id: string; compact?: boolean }) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 shrink-0 rounded-md bg-surface-2 border border-line text-ink-2 ${
        compact ? "h-5 px-1.5 text-[11px]" : "h-6 px-2 text-xs"
      }`}
    >
      <HarnessDot id={id} />
      {harnessName(id)}
    </span>
  );
}

export function HarnessDot({ id, size = 8 }: { id: string; size?: number }) {
  return (
    <span
      className="inline-block rounded-full shrink-0"
      style={{ width: size, height: size, background: harnessColor(id) }}
    />
  );
}

export function Spinner({ className = "w-4 h-4" }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={`${className} animate-spin-slow`} fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity="0.2" strokeWidth="2.5" />
      <path d="M21 12a9 9 0 00-9-9" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
    </svg>
  );
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: ReactNode;
  title: string;
  hint?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-2 text-center px-6 py-10">
      {icon && <div className="text-muted mb-1">{icon}</div>}
      <div className="text-[13px] font-medium text-ink-2">{title}</div>
      {hint && <div className="text-xs text-muted max-w-xs leading-relaxed">{hint}</div>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export interface Toast {
  id: number;
  kind: "ok" | "err" | "info";
  text: string;
}

export function Toasts({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  return (
    <div className="fixed bottom-4 right-4 flex flex-col gap-2 z-50 w-[380px] max-w-[calc(100vw-2rem)]">
      {toasts.map((t) => (
        <div
          key={t.id}
          role="status"
          onClick={() => onDismiss(t.id)}
          className="animate-in flex items-start gap-2.5 px-3.5 py-3 rounded-xl bg-surface border border-line shadow-[var(--shadow-pop)] text-[13px] text-ink break-all select-text"
        >
          <span
            className={`mt-1 w-2 h-2 rounded-full shrink-0 ${
              t.kind === "ok" ? "bg-ok" : t.kind === "err" ? "bg-danger" : "bg-accent"
            }`}
          />
          <span className="flex-1 whitespace-pre-wrap leading-relaxed">{t.text}</span>
        </div>
      ))}
    </div>
  );
}
