import type { AuthStatus } from "./usage";

type GateAuthStatus = Exclude<AuthStatus, "connected">;

function LockIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none">
      <rect
        x={5}
        y={10.5}
        width={14}
        height={9.5}
        rx={2.2}
        stroke="currentColor"
        strokeWidth={1.6}
      />
      <path
        d="M8 10.5V8a4 4 0 0 1 8 0v2.5"
        stroke="currentColor"
        strokeWidth={1.6}
        strokeLinecap="round"
      />
      <circle cx={12} cy={14.6} r={1.25} fill="currentColor" />
      <path d="M12 15.4v1.9" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" />
    </svg>
  );
}

function ArrowUpRight() {
  return (
    <svg viewBox="0 0 24 24" fill="none">
      <path
        d="M8 16 16 8M9.5 8H16v6.5"
        stroke="currentColor"
        strokeWidth={1.9}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

interface GateProps {
  authStatus: GateAuthStatus;
  onRetry: () => void;
}

const COPY: Record<GateAuthStatus, { title: string; body: string; action: string; note: string }> = {
  missingToken: {
    title: "Log in to Claude Code first",
    body: "Open Claude Code, sign in, then come back to Tokn and retry.",
    action: "Retry",
    note: "Tokn uses Claude Code's Keychain login",
  },
  expiredToken: {
    title: "Claude Code session token expired",
    body: "Run any Claude Code command to refresh the token, then retry.",
    action: "Retry",
    note: "Tokn does not manage Claude Code login",
  },
  keychainDenied: {
    title: "Allow Keychain access",
    body: "macOS needs permission before Tokn can read Claude Code's Keychain login.",
    action: "Try again",
    note: "permission is handled by macOS",
  },
};

/** First-run gate: frosted scrim over the blurred meters, one crisp CTA. */
export function Gate({ authStatus, onRetry }: GateProps) {
  const copy = COPY[authStatus];

  return (
    <div className="tk-gate">
      <div className="lockchip">
        <LockIcon />
      </div>
      <div className="gtitle">{copy.title}</div>
      <div className="gsub">{copy.body}</div>
      <button className="tk-cta" onClick={onRetry}>
        <span>{copy.action}</span>
        <ArrowUpRight />
      </button>
      <div className="gnote">{copy.note}</div>
    </div>
  );
}
