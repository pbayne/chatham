import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ListTodo, Plus } from "lucide-react";
import { ipc, type OpenActionItem } from "../lib/ipc";
import { dueColorVar, dueState } from "../lib/due";
import { useNotesStore } from "../lib/store";
import { NemotronOffer } from "../components/NemotronOffer";
import { ReleaseNotes } from "../components/ReleaseNotes";
import humlaIcon from "../../src-tauri/icons/icon.png";

// Relative "2h ago" / "3d ago" for a unix-ms timestamp. Compact, for the
// recent-notes list.
function ago(ms: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d ago`;
  return new Date(ms).toLocaleDateString();
}

export function Home() {
  const navigate = useNavigate();
  const notes = useNotesStore((s) => s.notes);
  const upsert = useNotesStore((s) => s.upsertLocal);
  const [open, setOpen] = useState<OpenActionItem[]>([]);

  // Open follow-ups across all meetings — drives both the "open items" count
  // and the "customers needing attention" roll-up.
  useEffect(() => {
    ipc.actionItemsOpen().then((rs) => setOpen(Array.isArray(rs) ? rs : [])).catch(() => setOpen([]));
  }, [notes.length]);

  const { total, summarized } = useMemo(() => {
    let summarized = 0;
    for (const n of notes) if (n.summary.trim()) summarized += 1;
    return { total: notes.length, summarized };
  }, [notes]);

  const recent = useMemo(
    () => [...notes].sort((a, b) => b.created_at - a.created_at).slice(0, 5),
    [notes],
  );

  // Customers ranked by open follow-up count — "who needs attention".
  const customers = useMemo(() => {
    const byClient = new Map<string, { id: string; name: string; count: number }>();
    for (const it of open) {
      if (!it.client_id) continue;
      const key = it.client_id;
      const name = it.client_name ?? "Customer";
      const e = byClient.get(key) ?? { id: key, name, count: 0 };
      e.count += 1;
      byClient.set(key, e);
    }
    return [...byClient.values()].sort((a, b) => b.count - a.count).slice(0, 6);
  }, [open]);

  const notesLabel = total === 1 ? "1 note" : `${total} notes`;
  const overdue = useMemo(() => open.filter((i) => dueState(i.due) === "overdue").length, [open]);

  async function newNote() {
    const note = await ipc.createNote();
    upsert(note);
    navigate(`/note/${note.id}`);
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className="home-content">
        <div className="home-welcome">
          <div className="flex flex-col items-center gap-3">
            <img
              src={humlaIcon}
              alt="Parley"
              width={72}
              height={72}
              style={{ filter: "drop-shadow(0 6px 20px rgba(0, 0, 0, 0.16))" }}
            />
            <h1 className="text-4xl font-semibold tracking-tight leading-none">Parley</h1>
          </div>
          <p className="text-sm text-[var(--color-text-muted)] tabular-nums">
            {notesLabel}, {summarized} summarized
          </p>
          <button onClick={newNote} className="nd-btn nd-btn-primary no-drag mt-1">
            <Plus size={15} strokeWidth={1.8} />
            New note
          </button>
        </div>

        {/* Today dashboard: recent meetings, open follow-ups, customers needing
            attention. Only shown once there's something to show so a brand-new
            install keeps the clean welcome screen. */}
        {(recent.length > 0 || open.length > 0) && (
          <div className="w-full max-w-2xl mx-auto mt-8 grid grid-cols-1 sm:grid-cols-2 gap-5">
            {/* Recent meetings */}
            <section className="rounded-[var(--radius-panel)] border border-[var(--color-line)] p-4">
              <h2 className="text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-2.5">
                Recent
              </h2>
              <ul className="flex flex-col">
                {recent.map((n) => (
                  <li key={n.id}>
                    <button
                      type="button"
                      onClick={() => navigate(`/note/${n.id}`)}
                      className="w-full text-left py-1.5 border-b border-[var(--color-line)] last:border-0 hover:text-[var(--color-interactive)] transition-colors"
                    >
                      <div className="text-sm truncate">{n.title?.trim() || "Untitled note"}</div>
                      <div className="text-xs text-[var(--color-text-muted)] tabular-nums">{ago(n.created_at)}</div>
                    </button>
                  </li>
                ))}
              </ul>
            </section>

            {/* Follow-ups summary */}
            <section className="rounded-[var(--radius-panel)] border border-[var(--color-line)] p-4">
              <div className="flex items-center justify-between mb-2.5">
                <h2 className="text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)]">
                  Follow-ups
                </h2>
                <button
                  type="button"
                  onClick={() => navigate("/follow-ups")}
                  className="text-xs text-[var(--color-interactive)] hover:underline inline-flex items-center gap-1"
                >
                  <ListTodo size={13} strokeWidth={1.7} />
                  {open.length} open
                  {overdue > 0 && (
                    <span style={{ color: dueColorVar("overdue") }}>· {overdue} overdue</span>
                  )}
                </button>
              </div>
              {open.length === 0 ? (
                <p className="text-sm text-[var(--color-text-muted)]">You're all caught up.</p>
              ) : customers.length > 0 ? (
                <ul className="flex flex-col">
                  {customers.map((c) => (
                    <li
                      key={c.name}
                      className="flex items-center justify-between py-1.5 border-b border-[var(--color-line)] last:border-0"
                    >
                      <button
                        type="button"
                        onClick={() => navigate(`/client/${c.id}`)}
                        className="text-sm truncate text-left hover:text-[var(--color-interactive)] transition-colors"
                      >
                        {c.name}
                      </button>
                      <span className="text-xs text-[var(--color-text-muted)] tabular-nums shrink-0 ml-2">
                        {c.count}
                      </span>
                    </li>
                  ))}
                </ul>
              ) : (
                <button
                  type="button"
                  onClick={() => navigate("/follow-ups")}
                  className="text-sm text-[var(--color-interactive)] hover:underline"
                >
                  View all follow-ups
                </button>
              )}
            </section>
          </div>
        )}

        <NemotronOffer />
        <ReleaseNotes />
      </div>
    </div>
  );
}
