import { useCallback, useEffect, useRef, useState } from "react";
import { Gate } from "./Gate";
import { Gauge } from "./Gauge";
import { Mark } from "./Mark";
import { Spark } from "./Spark";
import { fetchUsage, fmtAgo, UsageSnapshot } from "./usage";
import "./App.css";

const REFRESH_INTERVAL_MS = 60_000;
const MIN_SPIN_MS = 720;

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
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [spinning, setSpinning] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
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

  const onRefreshClick = useCallback(() => {
    setSpinning(true);
    spinUntil.current = Date.now() + MIN_SPIN_MS;
    loadUsage().finally(() => {
      setTimeout(() => setSpinning(false), Math.max(0, spinUntil.current - Date.now()));
    });
  }, [loadUsage]);

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
      </header>

      {error && <div className="tk-error">{error}</div>}

      {snapshot && (
        <div className={connected ? "tk-body" : "tk-body gated"}>
          {/* key replays the count-up when the gate unlocks */}
          <div className="tk-gauges" key={connected ? "live" : "locked"}>
            <Gauge
              label={snapshot.session.label}
              usedPct={snapshot.session.usedPct}
              resetsAtMs={snapshot.session.resetsAtMs}
              nowMs={nowMs}
            />
            <Gauge
              label={snapshot.weekly.label}
              usedPct={snapshot.weekly.usedPct}
              resetsAtMs={snapshot.weekly.resetsAtMs}
              nowMs={nowMs}
            />
          </div>

          <Spark samples={snapshot.burnRate} />

          <footer className="tk-foot">
            <div className="tk-updated">
              <span className="tick" />
              <span>{fmtAgo(nowMs - snapshot.fetchedAtMs)}</span>
            </div>
            <button
              className={spinning ? "tk-refresh spinning" : "tk-refresh"}
              title="Refresh"
              onClick={onRefreshClick}
            >
              <RefreshIcon />
            </button>
          </footer>

          {lockedAuthStatus && <Gate authStatus={lockedAuthStatus} onRetry={refresh} />}
        </div>
      )}
    </main>
  );
}

export default App;
