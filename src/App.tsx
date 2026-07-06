import { useCallback, useEffect, useRef, useState } from "react";
import { Gate } from "./Gate";
import { Gauge } from "./Gauge";
import { Mark } from "./Mark";
import { Spark } from "./Spark";
import { fetchUsage, fmtAgo, fmtCountdown, UsageSnapshot } from "./usage";
import { useZoom } from "./zoom";
import "./App.css";

const REFRESH_INTERVAL_MS = 60_000;
const MIN_SPIN_MS = 720;
/** Manual-refresh cooldown; matches the backend's minimum fetch spacing. */
const REFRESH_COOLDOWN_MS = 30_000;

/** Compact countdown that fits inside the 30px refresh button. */
function fmtShort(ms: number): string {
  const s = Math.ceil(Math.max(0, ms) / 1000);
  return s < 60 ? `${s}s` : `${Math.ceil(s / 60)}m`;
}

function useTheme(): "dark" | "light" {
  const [theme, setTheme] = useState<"dark" | "light">(() =>
    window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"
  );
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = (e: MediaQueryListEvent) => setTheme(e.matches ? "light" : "dark");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return theme;
}

function RefreshIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none">
      <path
        d="M20 11a8 8 0 1 0-1.6 5.6"
        stroke="currentColor"
        strokeWidth={1.7}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M20 5v6h-6"
        stroke="currentColor"
        strokeWidth={1.7}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function App() {
  const theme = useTheme();
  const zoom = useZoom();
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [spinning, setSpinning] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [cooldownUntil, setCooldownUntil] = useState(0);
  const spinUntil = useRef(0);

  const loadUsage = useCallback(() => {
    return fetchUsage()
      .then((s) => {
        setSnapshot(s);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const refresh = useCallback(() => {
    void loadUsage();
  }, [loadUsage]);

  const rateLimitedUntil = snapshot?.rateLimitedUntilMs ?? 0;
  const rateLimited = rateLimitedUntil > nowMs;
  const placeholder = snapshot?.placeholder ?? false;
  const refreshBlockedUntil = Math.max(cooldownUntil, rateLimitedUntil);
  const refreshBlocked = refreshBlockedUntil > nowMs;

  // Refetch as soon as the rate-limit window passes instead of waiting for
  // the next 60s tick — otherwise stale/placeholder data lingers banner-less.
  const wasRateLimited = useRef(false);
  useEffect(() => {
    if (wasRateLimited.current && !rateLimited) refresh();
    wasRateLimited.current = rateLimited;
  }, [rateLimited, refresh]);

  const onRefreshClick = () => {
    if (refreshBlocked || spinning) return;
    setSpinning(true);
    spinUntil.current = Date.now() + MIN_SPIN_MS;
    setCooldownUntil(Date.now() + REFRESH_COOLDOWN_MS);
    loadUsage().finally(() => {
      setTimeout(() => setSpinning(false), Math.max(0, spinUntil.current - Date.now()));
    });
  };

  useEffect(() => {
    refresh();
    const fetchTimer = setInterval(refresh, REFRESH_INTERVAL_MS);
    const clockTimer = setInterval(() => setNowMs(Date.now()), 1000);
    return () => {
      clearInterval(fetchTimer);
      clearInterval(clockTimer);
    };
  }, [refresh]);

  const lockedAuthStatus =
    snapshot?.authStatus === "connected" ? null : snapshot?.authStatus ?? null;
  const connected = !lockedAuthStatus;

  return (
    <main className="tk" data-theme={theme}>
      <header className="tk-head">
        <Mark />
        <span className="tk-word">Tokn</span>
        <span className="tk-tagline">claude code usage</span>
      </header>

      {error && <div className="tk-error">{error}</div>}
      {rateLimited && (
        <div className="tk-ratelimit">
          {placeholder ? "rate limited · first data in " : "rate limited · retrying in "}
          {fmtCountdown(rateLimitedUntil - nowMs)}
        </div>
      )}

      {snapshot && (
        <div className={connected ? "tk-body" : "tk-body gated"}>
          {/* key replays the count-up when the gate unlocks */}
          <div className="tk-gauges" key={connected ? "live" : "locked"}>
            <Gauge
              label={snapshot.session.label}
              usedPct={snapshot.session.usedPct}
              resetsAtMs={snapshot.session.resetsAtMs}
              nowMs={nowMs}
              placeholder={placeholder}
            />
            <Gauge
              label={snapshot.weekly.label}
              usedPct={snapshot.weekly.usedPct}
              resetsAtMs={snapshot.weekly.resetsAtMs}
              nowMs={nowMs}
              placeholder={placeholder}
            />
          </div>

          <Spark samples={snapshot.burnRate} />

          <footer className="tk-foot">
            <div className="tk-updated">
              <span className="tick" />
              <span>{fmtAgo(nowMs - snapshot.fetchedAtMs)}</span>
            </div>
            <div className="tk-foot-actions">
              <div className="tk-zoom" role="group" aria-label="Zoom">
                <button
                  className="zbtn"
                  onClick={zoom.zoomOut}
                  disabled={!zoom.canOut}
                  title="Zoom out (⌘−)"
                  aria-label="Zoom out"
                >
                  −
                </button>
                <button
                  className="zlvl"
                  onClick={zoom.reset}
                  title="Reset zoom (⌘0)"
                  aria-label="Reset zoom"
                >
                  {Math.round(zoom.zoom * 100)}%
                </button>
                <button
                  className="zbtn"
                  onClick={zoom.zoomIn}
                  disabled={!zoom.canIn}
                  title="Zoom in (⌘+)"
                  aria-label="Zoom in"
                >
                  +
                </button>
              </div>
              <button
                className={spinning ? "tk-refresh spinning" : "tk-refresh"}
                title={
                  refreshBlocked
                    ? `Refresh available in ${fmtCountdown(refreshBlockedUntil - nowMs)}`
                    : "Refresh"
                }
                onClick={onRefreshClick}
                disabled={refreshBlocked}
              >
                {refreshBlocked && !spinning ? (
                  <span className="count">{fmtShort(refreshBlockedUntil - nowMs)}</span>
                ) : (
                  <RefreshIcon />
                )}
              </button>
            </div>
          </footer>

          {lockedAuthStatus && <Gate authStatus={lockedAuthStatus} onRetry={refresh} />}
        </div>
      )}
    </main>
  );
}

export default App;
