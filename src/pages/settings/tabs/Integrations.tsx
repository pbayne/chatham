import { useEffect, useState } from "react";
import { ipc } from "../../../lib/ipc";
import { CommandSnippet } from "../../../components/CommandSnippet";
import { Row, Section } from "../components/Section";
import { Toggle } from "../components/Toggle";
import type { EditableKey } from "../types";
import { WebhookRows } from "./Webhook";

// The ways the user's own tools reach their notes: the MCP server (#172), which
// they read from, and the webhook (#213, `WebhookRows`), which a note is sent to.
//
// The MCP integration: one switch, and a config snippet per client.
//
// Off by default and explicitly opt-in — this hands an outside agent the
// contents of real meetings, so installing an update must never quietly open
// them. The switch writes `mcp_enabled`, which the `chatham-mcp` process reads on
// every call, so turning it off takes effect on the next tool call rather than
// at the client's next restart.
//
// Snippets rather than a one-click install: `.mcpb` bundles target the GUI
// Claude apps and don't work with Claude Code in the terminal, and a Claude Code
// plugin would serve only one of the two clients this is for.
export function IntegrationsSection({
  s,
  update,
}: {
  s: Record<EditableKey, string>;
  update: (key: EditableKey, value: string) => void;
}) {
  const enabled = s.mcp_enabled === "true";
  const [path, setPath] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .mcpServerPath()
      .then((p) => {
        if (!cancelled) setPath(p);
      })
      .catch(() => {
        /* the snippet section simply stays hidden */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Quoted, because an .app lives under a path with a space in it.
  const claudeCode = path ? `claude mcp add --scope user chatham -- "${path}"` : "";
  const codex = path
    ? `[mcp_servers.chatham]\ncommand = "${path}"`
    : "";

  return (
    <Section title="Integrations">
      <Row
        label="Model Context Protocol server"
        description="Let Claude Code, Codex or any other MCP client search and read your notes. Read-only: nothing can change or delete a note, and recorded audio is never reachable."
        control={
          <Toggle
            checked={enabled}
            onChange={(next) => update("mcp_enabled", next ? "true" : "false")}
            label="Enable the MCP server"
          />
        }
      />
      {enabled && path && (
        <>
          <Row label="Claude Code">
            <p className="text-xs text-[var(--color-text-muted)] mb-2">
              Run this once in a terminal.
            </p>
            {/* Wrapped, not truncated: the path is the part that differs per
                install, and it sits at the END of the line — truncating hides
                exactly the thing worth reading before pasting. */}
            <CommandSnippet block command={claudeCode} ariaLabel="Copy the Claude Code command" />
          </Row>
          <Row label="Codex">
            <p className="text-xs text-[var(--color-text-muted)] mb-2">
              Add this to <code style={{ fontFamily: "var(--font-code)" }}>~/.codex/config.toml</code>.
            </p>
            <CommandSnippet block command={codex} ariaLabel="Copy the Codex configuration" />
          </Row>
          <Row label="Anything else">
            <p className="text-xs text-[var(--color-text-muted)]">
              Any MCP client that speaks stdio can run this command directly. It
              works whether or not Chatham is open, and reads whichever workspace
              you're currently in.
            </p>
          </Row>
        </>
      )}
      <WebhookRows s={s} update={update} />
    </Section>
  );
}

// Obsidian vault sync (obsidian.rs). A one-way mirror of notes into a vault
// folder as Markdown — own state because it's driven by backend commands, not
// the settings map. Off by default; picking a folder + enabling triggers a
// backfill of every existing note.
export function ObsidianSection() {
  const [status, setStatus] = useState<import("../../../lib/ipc").ObsidianStatus | null>(null);
  const [autoTag, setAutoTag] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus(await ipc.obsidianStatus());
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    void refresh();
    ipc.autoCustomerTaggingStatus().then(setAutoTag).catch(() => {});
  }, []);

  const pickVault = async () => {
    setError(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false, title: "Choose your Obsidian vault folder" });
      if (typeof picked === "string") {
        setBusy(true);
        await ipc.obsidianSetVault(picked);
        await refresh();
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = async (next: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await ipc.obsidianSetEnabled(next);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const enabled = status?.enabled ?? false;
  const vault = status?.vault_path ?? "";

  return (
    <Section title="Obsidian sync">
      <Row
        label="Auto-tag the customer from each meeting"
        description="After a recording is summarized, Chatham reads it to identify which customer the meeting was with and tags the note — learning your account list as you go. Client-tagged notes are filed under the customer's name in your vault."
        control={
          <Toggle
            checked={autoTag ?? true}
            onChange={(next) => {
              setAutoTag(next);
              void ipc.autoCustomerTaggingSet(next);
            }}
            label="Auto-tag the customer from each meeting"
          />
        }
      />
      <Row
        label="Mirror notes to an Obsidian vault"
        description="Write each note (summary + your notes, no transcript) into a vault folder as Markdown, organised by folder. One-way: edits you make in Obsidian are never read back, and a note you've edited there is never overwritten."
        control={
          <Toggle
            checked={enabled}
            onChange={(next) => void toggle(next)}
            disabled={busy || !vault}
            label="Enable Obsidian sync"
          />
        }
      />
      <Row
        label="Vault folder"
        description={vault ? vault : "Pick the folder Chatham should write notes into (your vault, or a subfolder of it)."}
        control={
          <button
            type="button"
            onClick={() => void pickVault()}
            disabled={busy}
            className="text-sm px-3 py-1.5 rounded-[var(--control-radius)] border border-[var(--color-line-visible)] hover:bg-[var(--color-pill-hover)] disabled:opacity-50"
          >
            {vault ? "Change…" : "Choose folder…"}
          </button>
        }
      />
      {enabled && vault && (
        <Row label="Status">
          <p className="text-xs text-[var(--color-text-muted)]">
            {status?.synced_count ?? 0} note{(status?.synced_count ?? 0) === 1 ? "" : "s"} synced
            {status && status.conflict_count > 0
              ? ` · ${status.conflict_count} skipped (edited in Obsidian)`
              : ""}
          </p>
          <button
            type="button"
            onClick={async () => {
              setBusy(true);
              try {
                await ipc.obsidianBackfill();
                await refresh();
              } catch (e) {
                setError(String(e));
              } finally {
                setBusy(false);
              }
            }}
            disabled={busy}
            className="mt-2 text-xs text-[var(--color-interactive)] hover:underline disabled:opacity-50"
          >
            Re-sync all notes
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
