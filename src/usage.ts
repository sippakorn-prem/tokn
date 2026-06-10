import { invoke } from "@tauri-apps/api/core";

export interface UsageWindow {
  label: string;
  usedPct: number;
  resetsAtMs: number;
}

export type AuthStatus = "connected" | "missingToken" | "expiredToken" | "keychainDenied";

export interface UsageSnapshot {
  authStatus: AuthStatus;
  session: UsageWindow;
  weekly: UsageWindow;
  /** Burn-rate velocity samples for the sparkline, oldest first (~6h). */
  burnRate: number[];
  fetchedAtMs: number;
}

export function fetchUsage(): Promise<UsageSnapshot> {
  return invoke<UsageSnapshot>("usage_snapshot");
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
