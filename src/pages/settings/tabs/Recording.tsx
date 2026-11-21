import { useEffect, useState } from "react";
import { Permissions } from "../../../components/Permissions";
import { ipc, type StoredAudioStats, type SilenceStatus } from "../../../lib/ipc";
import { formatBytes, s as plural } from "../components/format";
import { HotkeyField } from "../components/HotkeyField";
import { LaunchAtLoginField } from "../components/LaunchAtLoginField";
import { Row, Section } from "../components/Section";
import { Segmented } from "../components/Segmented";
import { Toggle } from "../components/Toggle";
import type { SettingsHook } from "../useSettings";

// Recording section: capture permissions, audio retention, and menu-bar mode
// (#21) — the triggers that start a recording from outside the window.
export function RecordingSection({
  s,
  update,
}: Pick<SettingsHook, "s" | "update">) {
  const keeping = s.keep_audio === "true";
  const manualTranscription = s.transcribe_manually === "true";
  const closeToTray = s.close_to_tray === "true";
  const keepAwake = s.keep_awake !== "false";
  const captureShots = s.capture_screenshots_enabled === "true";
  const captureClip = s.capture_clipboard_enabled === "true";
  return (
    <>
      <Section title="Permissions">
        <div className="py-3.5">
          <Permissions />
        </div>
      </Section>

      <Section title="Audio retention">
        <Row
          label="Keep recorded audio"
          // The copy states which regime is in force rather than describing
          // the knob (#24). Before that issue this setting was a half-truth:
          // the mixed playback.wav was written either way, so "off keeps
          // nothing" was false. It's now literal, which is also why the off
          // text names the cost — no playback, no re-detection.
          description={
            keeping
              ? "Keep recordings for playback and speaker re-detection. Roughly 1 MB per minute per channel."
              : "No audio is stored on this Mac — transcript only; playback and speaker re-detection unavailable."
          }
          control={
            <Toggle
              label="Keep recorded audio"
              checked={keeping}
              onChange={(on) => update("keep_audio", on ? "true" : "false")}
            />
          }
        />
        {/* Progressive disclosure, not a disabled control (#146): deferring
            transcription only makes sense when the audio survives the
            recording, so with retention off there is nothing here to explain
            — and a greyed-out switch would invite the user to wonder why.
            `keep_audio` also gates it in the backend
            (`sessions::defer_transcription`), so turning retention off doesn't
            silently leave a regime running that the UI no longer shows. */}
        {keeping && (
          <Row
            label="Transcribe manually"
            description="Skip transcription while recording and run it later with a Transcribe button instead. Lighter on your Mac."
            control={
              <Toggle
                label="Transcribe manually"
                checked={manualTranscription}
                onChange={(on) =>
                  update("transcribe_manually", on ? "true" : "false")
                }
              />
            }
          />
        )}
        <StoredAudioCleanup />
        <AudioQualityRow s={s} update={update} />
        <PastCaptureCompress />
      </Section>

      <Section title="Power">
        <Row
          label="Keep Mac awake while recording"
          description="Stops your Mac from going to sleep during a recording. The display can still turn off, and a closed lid still sleeps."
          control={
            <Toggle
              label="Keep Mac awake while recording"
              checked={keepAwake}
              onChange={(on) => update("keep_awake", on ? "true" : "false")}
            />
          }
        />
      </Section>

      <SilenceAutoStopSection />

      <Section title="Captures">
        <Row
          label="Capture screenshots during recording"
          description="While recording, any screenshot you take (⌘⇧4, etc.) is automatically attached to the note. Watches your Mac's screenshot folder — your normal screenshot habit is unchanged."
          control={
            <Toggle
              label="Capture screenshots during recording"
              checked={captureShots}
              onChange={(on) =>
                update("capture_screenshots_enabled", on ? "true" : "false")
              }
            />
          }
        />
        <Row
          label="Capture clipboard during recording"
          description="While recording, anything you copy (text or images) is attached to the note. Items marked private by password managers are always skipped. Everything stays on this Mac."
          control={
            <Toggle
              label="Capture clipboard during recording"
              checked={captureClip}
              onChange={(on) =>
                update("capture_clipboard_enabled", on ? "true" : "false")
              }
            />
          }
        />
      </Section>

      <Section title="Menu bar">
        <Row
          label="Open at login"
          description="Chatham starts in the menu bar when you log in, without opening its window. If you added Chatham to Login Items in System Settings, remove it there."
          control={<LaunchAtLoginField />}
        />
        <Row
          label="Keep running in the menu bar"
          // Off by default, so closing the window still quits until the user
          // opts in — and the copy states which of the two regimes is in force
          // rather than describing the switch.
          description={
            closeToTray
              ? "Closing the window hides Chatham in the menu bar and drops its Dock icon. Quit from the menu-bar icon."
              : "Closing the window quits Chatham. The menu-bar icon is still there while it runs."
          }
          control={
            <Toggle
              label="Keep running in the menu bar"
              checked={closeToTray}
              onChange={(on) => update("close_to_tray", on ? "true" : "false")}
            />
          }
        />
        <Row
          label="Record shortcut"
          description="Starts and stops a recording from any app. With a note open it records that note; otherwise it starts a new one."
          control={<HotkeyField />}
        />
      </Section>
    </>
  );
}

// Silence auto-stop (silence.rs). Backend-backed state (not the settings map).
// Stops a recording after N minutes with no detected speech.
function SilenceAutoStopSection() {
  const [status, setStatus] = useState<SilenceStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc.silenceStatus()
      .then((next) => {
        if (!cancelled) setStatus(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const enabled = status?.enabled ?? false;
  const minutes = status?.minutes ?? 5;

  const setEnabled = async (on: boolean) => {
    await ipc.silenceSetEnabled(on);
    setStatus((s) => (s ? { ...s, enabled: on } : s));
  };
  const setMinutes = async (m: number) => {
    const clamped = await ipc.silenceSetMinutes(m);
    setStatus((s) => (s ? { ...s, minutes: clamped } : s));
  };

  return (
    <Section title="Silence auto-stop">
      <Row
        label="Stop recording after silence"
        description="Automatically stop a recording when no one has spoken for a while — handy when a call ends but the recording is left running."
        control={
          <Toggle
            label="Stop recording after silence"
            checked={enabled}
            onChange={(on) => void setEnabled(on)}
          />
        }
      />
      {enabled && (
        <Row
          label="Stop after"
          description="How long the room can stay quiet before Chatham stops and summarizes."
          control={
            <div className="flex items-center gap-2">
              <input
                type="number"
                min={1}
                max={120}
                value={minutes}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  if (Number.isFinite(n)) void setMinutes(n);
                }}
                className="w-16 text-sm px-2 py-1 rounded-[var(--radius-control)] border border-[var(--color-line-visible)] bg-transparent text-right"
              />
              <span className="text-sm text-[var(--color-text-muted)]">min</span>
            </div>
          }
        />
      )}
    </Section>
  );
}

// Audio compression quality (#audio-compress). Standard is transparent for
// voice; Small / Tiny use HE-AAC to trade fidelity for disk. All three stay
// natively playable + re-diarizable (no Opus decoder needed). New recordings
// compress to the chosen tier; "Compress old recordings" re-encodes to it too.
const AUDIO_QUALITY_OPTIONS: { value: string; label: string }[] = [
  { value: "standard", label: "Standard" },
  { value: "small", label: "Small" },
  { value: "tiny", label: "Tiny" },
];

function AudioQualityRow({
  s,
  update,
}: Pick<SettingsHook, "s" | "update">) {
  const quality = s.audio_quality || "standard";
  const desc =
    quality === "tiny"
      ? "Tiny: ~16 kbps HE-AAC — smallest files, rougher voice. Good for archiving."
      : quality === "small"
        ? "Small: ~24 kbps HE-AAC — clearly smaller, still clean for speech."
        : "Standard: 32 kbps AAC — transparent for voice. The default.";
  return (
    <Row
      label="Audio quality"
      description={desc}
      control={
        <Segmented
          label="Audio quality"
          value={quality}
          onChange={(v) => update("audio_quality", v)}
          options={AUDIO_QUALITY_OPTIONS}
        />
      }
    />
  );
}

// Batch-compress old captures' leftover playback.wav → .m4a (#audio-compress).
// New recordings compress automatically after transcription; this backfills
// anything recorded before that, reclaiming disk with no quality loss for
// speech (32 kbps mono AAC) and leaving transcripts/labels/timelines intact.
function PastCaptureCompress() {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const r = await ipc.compressPastCaptures();
      if (r.converted === 0) {
        setResult("Nothing to shrink — every recording is already at or below this quality.");
      } else {
        const saved = formatBytes(r.bytes_saved);
        const failed = r.failed > 0 ? ` (${r.failed} could not be converted)` : "";
        setResult(
          `Re-encoded ${r.converted} stream${plural(r.converted)}, reclaimed ${saved}${failed}.`,
        );
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Row
      label="Compress old recordings"
      description="Re-encode all previously recorded audio down to the Audio quality selected above. Only shrinks streams larger than that tier; transcripts, speaker labels and timelines are untouched."
      control={
        <div className="flex flex-col items-end gap-1">
          <button type="button" className="nd-btn" onClick={run} disabled={busy}>
            {busy ? "Compressing…" : "Compress now"}
          </button>
          {result && (
            <p className="text-xs text-[var(--color-text-muted)]">{result}</p>
          )}
          {error && (
            <p className="text-xs text-[var(--color-danger)]">{error}</p>
          )}
        </div>
      }
    />
  );
}

// audio, since silently deleting a user's recordings on a settings change
// would be worse than the dishonesty #24 fixes. This is the explicit action
// for what's already on disk.
//
// Two-step inline confirm: Tauri's webview no-ops window.confirm (it would
// deadlock the main thread), and the deletion is irreversible, so the armed
// state spells out the count.
function StoredAudioCleanup() {
  const [stats, setStats] = useState<StoredAudioStats | null>(null);
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc.storedAudioStats()
      .then((next) => {
        if (!cancelled) setStats(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await ipc.deleteStoredAudio();
      setDone(true);
      setArmed(false);
      setStats(await ipc.storedAudioStats().catch(() => null));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  // Nothing stored → no dangling affordance, except right after a sweep where
  // the confirmation is the whole point of having clicked.
  if (!stats || stats.notes === 0) {
    if (!done) return null;
    return (
      <Row label="Stored audio">
        <p className="text-xs text-[var(--color-text-muted)]">
          No audio stored — transcripts and speaker labels are untouched.
        </p>
      </Row>
    );
  }

  return (
    <Row
      label="Stored audio"
      description={`${stats.notes} note${plural(stats.notes)} · ${formatBytes(stats.bytes)}. Deleting keeps every transcript, speaker label and timeline — only the audio goes.`}
      control={
        <div className="flex flex-col items-end gap-1">
          {armed ? (
            <div className="flex items-center gap-2">
              <button
                type="button"
                className="nd-btn"
                onClick={() => setArmed(false)}
                disabled={busy}
              >
                Cancel
              </button>
              {/* Inline style, not a text-* utility: `.nd-btn` is unlayered, so
                  it wins the cascade over Tailwind's layered utilities and the
                  destructive red silently doesn't render. Same pattern as the
                  Sidebar's delete confirm. */}
              <button
                type="button"
                className="nd-btn"
                style={{ color: "var(--color-danger)" }}
                onClick={remove}
                disabled={busy}
              >
                {busy ? "Deleting…" : `Delete ${stats.files} files`}
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="nd-btn"
              onClick={() => setArmed(true)}
            >
              Delete stored audio…
            </button>
          )}
          {error && (
            <p className="text-xs text-[var(--color-danger)]">{error}</p>
          )}
        </div>
      }
    />
  );
}
