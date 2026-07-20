import type { Provider } from "./usage";

const PROVIDERS: { id: Provider; label: string }[] = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
];

interface ProviderToggleProps {
  value: Provider;
  onChange: (p: Provider) => void;
}

/** Segmented Claude/Codex switch; the gauges and tray follow the selection. */
export function ProviderToggle({ value, onChange }: ProviderToggleProps) {
  return (
    <div className="tk-provider" role="tablist" aria-label="Provider">
      {PROVIDERS.map((p) => (
        <button
          key={p.id}
          role="tab"
          aria-selected={value === p.id}
          className={value === p.id ? "pv active" : "pv"}
          onClick={() => value !== p.id && onChange(p.id)}
        >
          {p.label}
        </button>
      ))}
    </div>
  );
}
