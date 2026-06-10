import { useEffect, useMemo, useRef } from "react";

const W = 288;
const H = 38;
const PAD = 2;

interface SparkProps {
  samples: number[];
}

/** Burn-rate sparkline: ink line, dashed baseline, draw-in animation. */
export function Spark({ samples }: SparkProps) {
  const lineRef = useRef<SVGPathElement>(null);
  const areaRef = useRef<SVGPathElement>(null);
  const dotRef = useRef<SVGCircleElement>(null);

  const { linePath, areaPath, dotX, dotY, value } = useMemo(() => {
    const pts = samples.length >= 2 ? samples : [0.5, 0.5];
    const max = Math.max(...pts);
    const min = Math.min(...pts);
    const xs = (i: number) => PAD + (i / (pts.length - 1)) * (W - PAD * 2);
    const ys = (v: number) => H - 2 - ((v - min) / (max - min || 1)) * (H - 6);
    let d = "";
    pts.forEach((v, i) => {
      d += (i ? "L" : "M") + xs(i).toFixed(1) + " " + ys(v).toFixed(1) + " ";
    });
    const avg = pts.reduce((a, b) => a + b, 0) / pts.length;
    return {
      linePath: d,
      areaPath: d + `L ${xs(pts.length - 1).toFixed(1)} ${H} L ${xs(0).toFixed(1)} ${H} Z`,
      dotX: +xs(pts.length - 1).toFixed(1),
      dotY: +ys(pts[pts.length - 1]).toFixed(1),
      value: (pts[pts.length - 1] / (avg || 1)).toFixed(2),
    };
  }, [samples]);

  useEffect(() => {
    const line = lineRef.current;
    const area = areaRef.current;
    const dot = dotRef.current;
    if (!line || !area || !dot) return;
    const len = line.getTotalLength();
    line.style.transition = "none";
    line.style.strokeDasharray = `${len}`;
    line.style.strokeDashoffset = `${len}`;
    area.style.transition = "none";
    area.style.opacity = "0";
    dot.style.transition = "none";
    dot.style.opacity = "0";
    line.getBoundingClientRect();
    line.style.transition = "stroke-dashoffset 0.9s var(--cx-ease)";
    line.style.strokeDashoffset = "0";
    const t1 = setTimeout(() => {
      area.style.transition = "opacity 0.5s ease";
      area.style.opacity = "0.07";
    }, 520);
    const t2 = setTimeout(() => {
      dot.style.transition = "opacity 0.3s ease";
      dot.style.opacity = "1";
    }, 820);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
    };
  }, [linePath]);

  return (
    <div className="tk-spark">
      <div className="row">
        <span className="l">
          <span className="em">—</span> burn rate
        </span>
        <span className="v">
          <b>{value}×</b> avg · 6h
        </span>
      </div>
      <div className="chart">
        <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
          <line className="base" x1={PAD} y1={H - 2} x2={W - PAD} y2={H - 2} />
          <path ref={areaRef} className="area" d={areaPath} />
          <path ref={lineRef} className="line" d={linePath} />
          <circle ref={dotRef} className="dot" cx={dotX} cy={dotY} r={2.2} />
        </svg>
      </div>
    </div>
  );
}
