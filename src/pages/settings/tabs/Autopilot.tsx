import { useCallback, useEffect, useState } from "react";
import { ipc, type AutopilotMode, type AutopilotStatus, type UpcomingMeeting } from "../../../lib/ipc";
import { Row, Section } from "../components/Section";

// Calendar auto-record ("Autopilot"). Connects a Google account (read-only
// calendar scope), picks a mode, and — as the read-only trust surface (PRD
// milestone M2) — shows exactly which upcoming meetings Autopilot would act on
// and why, before the user ever turns Auto on.
//
// Humla does NOT join meetings as a bot. In every mode it records the Mac's own
// mic + system audio; the calendar only decides WHEN to start and stop. That is
// stated plainly here because it changes what the user should expect.

const MODE_COPY: Record<AutopilotMode, { label: string; help: string }> = {
  off: {
    label: "Off",
    help: "Recording stays fully manual.",
  },
  ask: {
    label: "Ask",
    help: "A notification appears when a meeting starts, offering to record it.",
  },
  auto: {
    label: "Auto",
    help: "Meetings are recorded automatically and stopped when they end.",
  },
};

function fmtTime(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  });
}

export function AutopilotTab() {
  const [status, setStatus] = useState<AutopilotStatus | null>(null);
  const [upcoming, setUpcoming] = useState<UpcomingMeeting[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await ipc.autopilotStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshUpcoming = useCallback(async () => {
    try {
      setUpcoming(await ipc.autopilotUpcoming());
      setError(null);
    } catch (e) {
      // Not connected is the common, non-error case — leave the list empty.
      setUpcoming([]);
      if (!String(e).includes("not connected")) setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    if (status?.connected) void refreshUpcoming();
  }, [status?.connected, refreshUpcoming]);

  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      await ipc.autopilotConnectGoogle();
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    setBusy(true);
    try {
      await ipc.autopilotDisconnectGoogle();
      setUpcoming(null);
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const setMode = async (mode: AutopilotMode) => {
    try {
      await ipc.autopilotSetMode(mode);
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    }
  };

  const connected = status?.connected ?? false;
  const mode = status?.mode ?? "off";

  return (
    <Section title="Autopilot — auto-record meetings">
      <Row
        label="How it works"
        description="Humla reads your Google Calendar on this Mac to know when meetings start, and records the Mac's own audio — it does not join the call as a bot. Your calendar is used to decide when to start and stop, and to tag each recording with the meeting's title and attendees."
      />

      <Row
        label="Google Calendar"
        description={
          connected
            ? "Connected. Read-only access to your events."
            : "Connect to let Autopilot see your meetings. Read-only; opens your browser to sign in."
        }
        control={
          connected ? (
            <button
              type="button"
              onClick={disconnect}
              disabled={busy}
              className="text-sm px-3 py-1.5 rounded-[var(--radius-control)] border border-[var(--color-line-visible)] hover:bg-[var(--color-surface-hover)] disabled:opacity-50"
            >
              Disconnect
            </button>
          ) : (
            <button
              type="button"
              onClick={connect}
              disabled={busy}
              className="text-sm px-3 py-1.5 rounded-[var(--radius-control)] bg-[var(--color-interactive)] text-white hover:opacity-90 disabled:opacity-50"
            >
              {busy ? "Connecting…" : "Connect Google"}
            </button>
          )
        }
      />

      {connected && (
        <Row label="Mode">
          <div className="flex flex-col gap-2">
            {(Object.keys(MODE_COPY) as AutopilotMode[]).map((m) => (
              <label key={m} className="flex items-start gap-2.5 cursor-pointer">
                <input
                  type="radio"
                  name="autopilot-mode"
                  checked={mode === m}
                  onChange={() => void setMode(m)}
                  className="mt-0.5"
                />
                <span>
                  <span className="text-sm">{MODE_COPY[m].label}</span>
                  <span className="block text-xs text-[var(--color-text-muted)]">
                    {MODE_COPY[m].help}
                  </span>
                </span>
              </label>
            ))}
          </div>
        </Row>
      )}

      {connected && (
        <Row label="Upcoming meetings">
          <p className="text-xs text-[var(--color-text-muted)] mb-2">
            What Autopilot sees in the next 12 hours, and whether it would record
            each one. A meeting is skipped if it's all-day, declined, or has no
            other attendees and no video link.
          </p>
          {upcoming === null ? (
            <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
          ) : upcoming.length === 0 ? (
            <p className="text-xs text-[var(--color-text-muted)]">
              No meetings found in the window.
            </p>
          ) : (
            <ul className="flex flex-col gap-1.5">
              {upcoming.map((m) => (
                <li
                  key={m.id}
                  className="flex items-center justify-between gap-3 text-sm"
                >
                  <span className="min-w-0 truncate">
                    <span className="truncate">{m.title}</span>
                    <span className="text-xs text-[var(--color-text-muted)] ml-2">
                      {fmtTime(m.start)}
                      {m.attendee_count > 0 && ` · ${m.attendee_count} attendees`}
                    </span>
                  </span>
                  <span
                    className={
                      "shrink-0 text-xs px-1.5 py-0.5 rounded " +
                      (m.recordable
                        ? "text-[var(--color-success)]"
                        : "text-[var(--color-text-muted)]")
                    }
                  >
                    {m.recordable ? "will record" : "skip"}
                  </span>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            onClick={() => void refreshUpcoming()}
            className="mt-2 text-xs text-[var(--color-interactive)] hover:underline"
          >
            Refresh
          </button>
        </Row>
      )}

      {error && (
        <Row label="">
          <p className="text-xs text-[var(--color-danger)]">{error}</p>
        </Row>
      )}
    </Section>
  );
}
