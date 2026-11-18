// Inbox Triage board (/inbox). Turns the unread inbox into a to-do board:
// four columns (Action Needed / Follow-ups / FYI / Noise), each card showing
// the email, the LLM-extracted action items, and — for actionable mail — a
// pre-written draft reply the user edits and approves. Approving stages a
// Gmail DRAFT (never auto-sent), reusing the same compose path the follow-up
// feature uses. Triage runs through the local LLM by default (free, private).

import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  CheckCircle2,
  Inbox as InboxIcon,
  Loader2,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  ipc,
  type ActionItem,
  type InboxStatus,
  type TriageCard,
  type TriageCategory,
} from "../lib/ipc";
import { cn } from "../lib/cn";

const COLUMNS: { key: TriageCategory; label: string; hint: string }[] = [
  { key: "action_needed", label: "Action Needed", hint: "You need to do something" },
  { key: "follow_up", label: "Follow-ups", hint: "You owe a reply or a nudge" },
  { key: "fyi", label: "FYI", hint: "Worth reading, no action" },
  { key: "noise", label: "Noise", hint: "Newsletters, receipts, promos" },
];

export function Inbox() {
  const [status, setStatus] = useState<InboxStatus | null>(null);
  const [cards, setCards] = useState<TriageCard[]>([]);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastRun, setLastRun] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, c] = await Promise.all([ipc.inboxStatus(), ipc.inboxList()]);
      setStatus(s);
      setCards(c);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function triageNow() {
    setRunning(true);
    setError(null);
    try {
      const n = await ipc.inboxTriageNow();
      setLastRun(n === 0 ? "No new unread mail to triage." : `Triaged ${n} new message${n === 1 ? "" : "s"}.`);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function clearAll() {
    await ipc.inboxClear();
    await refresh();
  }

  const connected = status?.connected ?? false;

  return (
    <div className="flex h-full flex-col">
      {/* Header bar */}
      <div className="nd-well-bar shrink-0">
        <div className="max-w-[1400px] mx-auto w-full px-8 pt-14 pb-3">
          <div className="flex items-center gap-3 px-1">
            <InboxIcon size={22} strokeWidth={1.6} className="text-[var(--color-text-muted)]" />
            <h1 className="nd-heading truncate">Inbox</h1>
            <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
              {cards.length}
            </span>
            <div className="flex-1" />
            {lastRun && (
              <span className="text-[13px] text-[var(--color-text-muted)] mr-2">{lastRun}</span>
            )}
            <ProviderPill
              provider={status?.provider ?? "local"}
              onChange={async (p) => {
                await ipc.inboxSetProvider(p);
                await refresh();
              }}
            />
            <AuthModePill
              mode={status?.auth_mode ?? "oauth"}
              onChange={async (m) => {
                await ipc.inboxSetAuthMode(m);
                await refresh();
              }}
            />
            {cards.length > 0 && (
              <button
                onClick={clearAll}
                title="Clear the board"
                className="no-drag nd-navrow px-3"
              >
                <Trash2 size={15} strokeWidth={1.6} className="opacity-85" />
                <span>Clear</span>
              </button>
            )}
            <button
              onClick={triageNow}
              disabled={running || !connected}
              className={cn(
                "no-drag nd-navrow px-3 is-active font-medium",
                (running || !connected) && "opacity-60 cursor-not-allowed",
              )}
              title={connected ? "Fetch unread and triage" : "Connect Google first"}
            >
              {running ? (
                <Loader2 size={15} className="animate-spin" />
              ) : (
                <RefreshCw size={15} strokeWidth={1.8} />
              )}
              <span>{running ? "Triaging…" : "Triage now"}</span>
            </button>
          </div>
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-[1400px] mx-auto w-full px-8 py-6">
          {!connected && <NotConnected />}
          {error && (
            <div className="mb-4 rounded-[var(--radius)] border border-[var(--color-danger,#c0392b)] bg-[var(--color-surface)] px-4 py-3 text-sm text-[var(--color-text)]">
              {error}
            </div>
          )}
          {connected && cards.length === 0 && !error && (
            <div className="px-2 py-16 text-center text-[var(--color-text-muted)]">
              <p className="text-[15px]">No triaged mail yet.</p>
              <p className="text-[13px] mt-1 text-[var(--color-text-disabled)]">
                Click “Triage now” to classify your unread inbox.
              </p>
            </div>
          )}

          {cards.length > 0 && (
            <div className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-4 gap-4">
              {COLUMNS.map((col) => {
                const colCards = cards.filter((c) => c.category === col.key);
                return (
                  <div key={col.key} className="flex flex-col gap-3">
                    <div className="px-1">
                      <div className="flex items-center gap-2">
                        <span className="text-[13px] font-semibold text-[var(--color-text)]">
                          {col.label}
                        </span>
                        <span className="text-[11px] text-[var(--color-text-disabled)] tabular-nums">
                          {colCards.length}
                        </span>
                      </div>
                      <div className="text-[11px] text-[var(--color-text-disabled)]">{col.hint}</div>
                    </div>
                    {colCards.map((card) => (
                      <Card key={card.message.id} card={card} onChange={refresh} />
                    ))}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function ProviderPill({
  provider,
  onChange,
}: {
  provider: string;
  onChange: (p: "local" | "openai" | "anthropic") => void;
}) {
  return (
    <select
      value={provider}
      onChange={(e) => onChange(e.target.value as "local" | "openai" | "anthropic")}
      title="Which model triages your inbox"
      className="no-drag text-[12px] bg-[var(--color-surface)] border border-[var(--color-line-visible)] rounded-[var(--radius)] px-2 py-1 text-[var(--color-text-muted)] mr-2"
    >
      <option value="local">Local (free)</option>
      <option value="openai">OpenAI</option>
      <option value="anthropic">Anthropic</option>
    </select>
  );
}

function AuthModePill({
  mode,
  onChange,
}: {
  mode: string;
  onChange: (m: "oauth" | "gcloud") => void;
}) {
  return (
    <select
      value={mode}
      onChange={(e) => onChange(e.target.value as "oauth" | "gcloud")}
      title="How Chatham reaches Gmail. gcloud uses your machine's authorized Google Cloud credentials — works on corp accounts that block third-party OAuth apps."
      className="no-drag text-[12px] bg-[var(--color-surface)] border border-[var(--color-line-visible)] rounded-[var(--radius)] px-2 py-1 text-[var(--color-text-muted)] mr-2"
    >
      <option value="oauth">Auth: OAuth</option>
      <option value="gcloud">Auth: gcloud</option>
    </select>
  );
}

function NotConnected() {
  const navigate = useNavigate();
  return (
    <div className="rounded-[var(--radius)] border border-[var(--color-line-visible)] bg-[var(--color-surface)] px-5 py-4 mb-5">
      <p className="text-[15px] font-medium text-[var(--color-text)]">Connect Google to triage your inbox</p>
      <p className="text-[13px] text-[var(--color-text-muted)] mt-1">
        Inbox reads your unread mail (read-only) and stages draft replies you review before sending.
        Nothing is sent or deleted automatically.
      </p>
      <button
        onClick={() => navigate("/settings")}
        className="no-drag nd-navrow is-active font-medium px-3 mt-3 inline-flex"
      >
        Open Settings
      </button>
    </div>
  );
}

function Card({ card, onChange }: { card: TriageCard; onChange: () => void }) {
  const actionable = card.category === "action_needed" || card.category === "follow_up";
  const [expanded, setExpanded] = useState(actionable);
  const [to, setTo] = useState(extractEmail(card.message.from));
  const [reply, setReply] = useState(card.suggested_reply);
  const [staging, setStaging] = useState(false);
  const [staged, setStaged] = useState<string | null>(card.draft_id ? "done" : null);
  const [err, setErr] = useState<string | null>(null);

  // Editable to-do list. Seeded from the card; every mutation persists the
  // full list to the backend, then refreshes the board.
  const [items, setItems] = useState<ActionItem[]>(card.action_items);
  const [newItem, setNewItem] = useState("");

  async function persist(next: ActionItem[]) {
    setItems(next);
    try {
      await ipc.inboxSetActionItems(card.message.id, next);
    } catch (e) {
      setErr(String(e));
    }
  }
  function toggle(i: number) {
    persist(items.map((it, idx) => (idx === i ? { ...it, done: !it.done } : it)));
  }
  function editText(i: number, text: string) {
    setItems(items.map((it, idx) => (idx === i ? { ...it, text } : it)));
  }
  function remove(i: number) {
    persist(items.filter((_, idx) => idx !== i));
  }
  function addItem() {
    const text = newItem.trim();
    if (!text) return;
    persist([...items, { text, done: false }]);
    setNewItem("");
  }

  async function approve() {
    setStaging(true);
    setErr(null);
    try {
      const subject = card.message.subject.toLowerCase().startsWith("re:")
        ? card.message.subject
        : `Re: ${card.message.subject}`;
      const url = await ipc.inboxApproveDraft(card.message.id, to, subject, reply);
      setStaged(url);
      onChange();
    } catch (e) {
      setErr(String(e));
    } finally {
      setStaging(false);
    }
  }

  async function dismiss() {
    await ipc.inboxClear(card.message.id);
    onChange();
  }

  return (
    <div className="rounded-[var(--radius)] border border-[var(--color-line-visible)] bg-[var(--color-surface)] overflow-hidden">
      <button
        onClick={() => setExpanded((v) => !v)}
        className="w-full text-left px-3.5 py-3 hover:bg-[var(--color-pill-hover)] transition-colors"
      >
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[13px] font-medium text-[var(--color-text)] truncate">
              {card.message.subject || "(no subject)"}
            </div>
            <div className="text-[11px] text-[var(--color-text-muted)] truncate">
              {displayName(card.message.from)}
            </div>
          </div>
          {card.draft_id && (
            <CheckCircle2 size={15} className="shrink-0 text-[var(--color-success,#1e8e3e)]" />
          )}
        </div>
        {card.reason && (
          <div className="text-[11px] text-[var(--color-text-disabled)] mt-1 line-clamp-2">
            {card.reason}
          </div>
        )}
      </button>

      {expanded && (
        <div className="px-3.5 pb-3.5 pt-1 border-t border-[var(--color-line)]">
          <div className="mt-2">
            <div className="text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wide mb-1">
              To do
            </div>
            <ul className="space-y-1">
              {items.map((item, i) => (
                <li key={i} className="group flex items-start gap-1.5 text-[12px]">
                  <input
                    type="checkbox"
                    checked={item.done}
                    onChange={() => toggle(i)}
                    className="mt-[3px] shrink-0 accent-[var(--color-interactive,#2563eb)]"
                  />
                  <input
                    value={item.text}
                    onChange={(e) => editText(i, e.target.value)}
                    onBlur={() => persist(items)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    }}
                    className={cn(
                      "flex-1 min-w-0 bg-transparent border-none outline-none text-[var(--color-text)]",
                      item.done && "line-through text-[var(--color-text-disabled)]",
                    )}
                  />
                  <button
                    onClick={() => remove(i)}
                    title="Delete"
                    className="shrink-0 opacity-0 group-hover:opacity-100 text-[var(--color-text-disabled)] hover:text-[var(--color-danger,#c0392b)] transition-opacity"
                  >
                    <Trash2 size={12} />
                  </button>
                </li>
              ))}
              {items.length === 0 && (
                <li className="text-[11px] text-[var(--color-text-disabled)] italic">
                  No action items — add one below.
                </li>
              )}
            </ul>
            <div className="flex items-center gap-1.5 mt-1.5">
              <input
                value={newItem}
                onChange={(e) => setNewItem(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") addItem();
                }}
                placeholder="Add an action item…"
                className="flex-1 text-[12px] bg-[var(--color-canvas)] border border-[var(--color-line-visible)] rounded px-2 py-1"
              />
              <button
                onClick={addItem}
                disabled={!newItem.trim()}
                className={cn(
                  "no-drag nd-navrow px-2 text-[12px]",
                  !newItem.trim() && "opacity-50 cursor-not-allowed",
                )}
              >
                <Plus size={13} /> Add
              </button>
            </div>
          </div>

          {card.message.snippet && (
            <div className="text-[12px] text-[var(--color-text-muted)] mt-2 line-clamp-3">
              {card.message.snippet}
            </div>
          )}

          {actionable && (
            <div className="mt-3">
              <div className="text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wide mb-1">
                Draft reply
              </div>
              <input
                value={to}
                onChange={(e) => setTo(e.target.value)}
                placeholder="To"
                className="w-full text-[12px] bg-[var(--color-canvas)] border border-[var(--color-line-visible)] rounded px-2 py-1 mb-1.5"
              />
              <textarea
                value={reply}
                onChange={(e) => setReply(e.target.value)}
                rows={5}
                placeholder="Write a reply…"
                className="w-full text-[12px] bg-[var(--color-canvas)] border border-[var(--color-line-visible)] rounded px-2 py-1.5 resize-y"
              />
              {err && <div className="text-[11px] text-[var(--color-danger,#c0392b)] mt-1">{err}</div>}
              <div className="flex items-center gap-2 mt-2">
                {staged && staged !== "done" ? (
                  <a
                    href={staged}
                    target="_blank"
                    rel="noreferrer"
                    className="no-drag nd-navrow is-active font-medium px-3 text-[12px]"
                  >
                    <CheckCircle2 size={14} /> View draft in Gmail
                  </a>
                ) : (
                  <button
                    onClick={approve}
                    disabled={staging}
                    className={cn(
                      "no-drag nd-navrow is-active font-medium px-3 text-[12px]",
                      staging && "opacity-60 cursor-not-allowed",
                    )}
                  >
                    {staging ? <Loader2 size={14} className="animate-spin" /> : <CheckCircle2 size={14} />}
                    {card.draft_id ? "Re-draft" : "Approve → draft"}
                  </button>
                )}
                <button onClick={dismiss} className="no-drag nd-navrow px-3 text-[12px]">
                  Dismiss
                </button>
              </div>
            </div>
          )}

          {!actionable && (
            <div className="mt-2">
              <button onClick={dismiss} className="no-drag nd-navrow px-3 text-[12px]">
                <Trash2 size={13} /> Dismiss
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// "Brad Lucas <brad@x.com>" → "Brad Lucas"; falls back to the raw string.
function displayName(from: string): string {
  const m = from.match(/^\s*"?([^"<]+?)"?\s*</);
  return m ? m[1].trim() : from.trim();
}

// Pull the bare email out of a "Name <email>" header for the reply To field.
function extractEmail(from: string): string {
  const m = from.match(/<([^>]+)>/);
  return m ? m[1].trim() : from.trim();
}
