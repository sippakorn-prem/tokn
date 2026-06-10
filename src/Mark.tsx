const N = 16;
const START = 135;
const SWEEP = 270;
const C = 12;
const R_IN = 6.3;
const R_OUT = 9.6;

/** The Tokn brand mark — segmented 270° ring with a gradual fade. */
export function Mark() {
  return (
    <svg className="tk-mark" viewBox="0 0 24 24" fill="none">
      {Array.from({ length: N }, (_, i) => {
        const f = i / (N - 1);
        const a = ((START + SWEEP * f) * Math.PI) / 180;
        return (
          <line
            key={i}
            x1={+(C + R_IN * Math.cos(a)).toFixed(2)}
            y1={+(C + R_IN * Math.sin(a)).toFixed(2)}
            x2={+(C + R_OUT * Math.cos(a)).toFixed(2)}
            y2={+(C + R_OUT * Math.sin(a)).toFixed(2)}
            stroke="currentColor"
            strokeWidth={1.8}
            strokeLinecap="round"
            opacity={+(1 - 0.74 * f).toFixed(3)}
          />
        );
      })}
    </svg>
  );
}
