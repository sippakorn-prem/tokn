import { useEffect, useMemo, useRef, useState } from "react";
import { bandOf, fmtCountdown, fmtResetAt } from "./usage";

const TICKS = 40;
const START = 135;
const SWEEP = 270;
const C = 62;
const R_IN = 44;
const R_OUT = 54;

const TICK_GEOMETRY = Array.from({ length: TICKS }, (_, i) => {
  const a = ((START + (SWEEP * i) / (TICKS - 1)) * Math.PI) / 180;
  return {
    x1: +(C + R_IN * Math.cos(a)).toFixed(2),
    y1: +(C + R_IN * Math.sin(a)).toFixed(2),
    x2: +(C + R_OUT * Math.cos(a)).toFixed(2),
    y2: +(C + R_OUT * Math.sin(a)).toFixed(2),
  };
});

const prefersReducedMotion = () =>
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Count-up toward the target over 900ms with cubic ease-out. */
function useCountUp(target: number): number {
  const [shown, setShown] = useState(target);
  const raf = useRef(0);
  useEffect(() => {
    if (prefersReducedMotion()) {
      setShown(target);
      return;
    }
    const dur = 900;
    const t0 = performance.now();
    const step = (now: number) => {
      const k = Math.min(1, (now - t0) / dur);
      const e = 1 - Math.pow(1 - k, 3);
      setShown(e * target);
      if (k < 1) raf.current = requestAnimationFrame(step);
    };
    setShown(0);
    raf.current = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf.current);
  }, [target]);
  return shown;
}

interface GaugeProps {
  label: string;
  usedPct: number;
  resetsAtMs: number;
  nowMs: number;
  /** No real data behind these numbers yet — render dashes, not zeros. */
  placeholder?: boolean;
}

export function Gauge({ label, usedPct, resetsAtMs, nowMs, placeholder }: GaugeProps) {
  const pct = Math.min(100, Math.max(0, usedPct));
  const shown = useCountUp(pct);
  const lit = Math.round((shown / 100) * TICKS);

  // The absolute reset label only changes at minute granularity, so keep it
  // off the 1s clock tick that re-renders this component for the countdown.
  const nowMin = Math.floor(nowMs / 60_000);
  const resetAt = useMemo(() => fmtResetAt(resetsAtMs, nowMin * 60_000), [resetsAtMs, nowMin]);

  return (
    <div className="tk-gauge" data-status={bandOf(pct)}>
      <div className="tk-ring">
        <svg viewBox="0 0 124 124">
          {TICK_GEOMETRY.map((t, i) => (
            <line key={i} {...t} className={!placeholder && i < lit ? "tk-tick on" : "tk-tick"} />
          ))}
        </svg>
        <div className="ctr">
          <div className="tk-pct">
            {placeholder ? (
              <span className="dash">–</span>
            ) : (
              <>
                <span>{Math.round(shown)}</span>
                <span className="u">%</span>
              </>
            )}
          </div>
        </div>
      </div>
      <div className="lbl">{label}</div>
      <div className="reset">
        {placeholder ? (
          <span>waiting for data</span>
        ) : resetsAtMs > 0 ? (
          <>
            <span>
              resets in <b>{fmtCountdown(resetsAtMs - nowMs)}</b>
            </span>
            <span className="at">{resetAt}</span>
          </>
        ) : (
          <span className="at">no active session</span>
        )}
      </div>
    </div>
  );
}
