import { invoke } from "@tauri-apps/api/core";

export interface UsageWindow {
  label: string;
  usedPct: number;
  resetsAtMs: number;
}

export type Provider = "claude" | "codex";

export type AuthStatus =
  | "connected"
  | "missingToken"
  | "expiredToken"
  | "keychainDenied"
  | "codexNotFound";

export interface UsageSnapshot {
  authStatus: AuthStatus;
  session: UsageWindow;
  weekly: UsageWindow;
  /** Burn-rate velocity samples for the sparkline, oldest first (~6h). */
  burnRate: number[];
  fetchedAtMs: number;
  /**
   * Set while the usage endpoint is rate limiting us (429): when the next
   * fetch is allowed. The rest of the snapshot is the last good data.
   */
  rateLimitedUntilMs?: number | null;
  /** True when no real data has ever been fetched — windows are filler. */
  placeholder?: boolean;
}

export function fetchUsage(provider: Provider = "claude"): Promise<UsageSnapshot> {
  return invoke<UsageSnapshot>(provider === "codex" ? "codex_snapshot" : "usage_snapshot");
}

export type Band = "ok" | "warn" | "crit";

/* green < 50 · amber 50–85 · red > 85 */
export function bandOf(pct: number): Band {
  if (pct > 85) return "crit";
  if (pct >= 50) return "warn";
  return "ok";
}

export function fmtCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const d = Math.floor(total / 86400);
  const h = Math.floor((total % 86400) / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function fmtAgo(ms: number): string {
  const s = Math.floor(Math.max(0, ms) / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  return `${Math.floor(m / 60)}h ago`;
}

/**
 * Absolute reset moment, smart-relative to now: time-only when it lands
 * today (e.g. "5:00 PM"), weekday + time within the week ("Wed 5:00 PM"),
 * and month/day + time beyond that ("Jul 14, 5:00 PM"). Pairs with the
 * countdown so users see both "how long" and "when".
 */
export function fmtResetAt(atMs: number, nowMs: number): string {
  const at = new Date(atMs);
  const now = new Date(nowMs);
  const time = at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const sameDay =
    at.getFullYear() === now.getFullYear() &&
    at.getMonth() === now.getMonth() &&
    at.getDate() === now.getDate();
  if (sameDay) return time;
  if (atMs - nowMs < 6 * 86400_000) {
    return `${at.toLocaleDateString([], { weekday: "short" })} ${time}`;
  }
  return `${at.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}
