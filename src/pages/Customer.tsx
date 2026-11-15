import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Building2, Circle, ListTodo } from "lucide-react";
import { ipc, type OpenActionItem } from "../lib/ipc";
import { byDue, dueColorVar, dueLabel, dueState } from "../lib/due";
import { useNotesStore } from "../lib/store";
import { indexById } from "../lib/noteList";
import { NoteCard } from "../components/NoteCard";

// Customer view (#customer): everything about one account in one place — the
// account's open follow-ups up top, then every meeting with them. The daily
// "prep for my call with Fisher" and "where did we land" surface. Reached from
// a note's client chip or anywhere a customer is named.

export function Customer() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const notes = useNotesStore((s) => s.notes);
  const folders = useNotesStore((s) => s.folders);
  const clients = useNotesStore((s) => s.clients);
  const recorded = useNotesStore((s) => s.recordedNoteIds);
  const [open, setOpen] = useState<OpenActionItem[]>([]);

  const client = useMemo(() => clients.find((c) => c.id === id), [clients, id]);
  const folderById = useMemo(() => indexById(folders), [folders]);
  const clientById = useMemo(() => indexById(clients), [clients]);

  const notesForClient = useMemo(
    () => notes.filter((n) => n.client_id === id).sort((a, b) => b.created_at - a.created_at),
    [notes, id],
  );

  useEffect(() => {
    ipc.actionItemsOpen()
      .then((rs) => setOpen((Array.isArray(rs) ? rs : []).filter((i) => i.client_id === id).sort(byDue)))
      .catch(() => setOpen([]));
  }, [id, notes.length]);

  const complete = (itemId: string) => {
    setOpen((cur) => cur.filter((i) => i.id !== itemId));
    void ipc.actionItemsUpdate(itemId, { done: true }).catch(() => {});
  };

  if (!client) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-[var(--color-text-muted)]">
        Customer not found.
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto nd-well">
      <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-24">
        <div className="flex items-center gap-2.5 mb-1 px-1">
          <Building2 size={22} strokeWidth={1.6} className="text-[var(--color-interactive)]" />
          <h1 className="nd-heading truncate">{client.name}</h1>
          <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
            {notesForClient.length} {notesForClient.length === 1 ? "meeting" : "meetings"}
          </span>
        </div>

        {/* Open follow-ups for this account */}
        {open.length > 0 && (
          <section className="mt-5 px-1">
            <h2 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-2">
              <ListTodo size={14} strokeWidth={1.7} />
              Open follow-ups
              <span className="font-normal">{open.length}</span>
            </h2>
            <ul className="flex flex-col max-w-2xl">
              {open.map((it) => (
                <li
                  key={it.id}
                  className="group flex items-start gap-3 py-1.5 border-b border-[var(--color-line)] last:border-0"
                >
                  <button
                    type="button"
                    aria-label="Mark done"
                    onClick={() => complete(it.id)}
                    className="mt-0.5 shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-success)] transition-colors"
                  >
                    <Circle size={16} strokeWidth={1.6} />
                  </button>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm leading-snug">{it.text}</p>
                    <div className="flex flex-wrap items-center gap-x-2.5 text-xs text-[var(--color-text-muted)] mt-0.5">
                      {it.owner && <span>{it.owner}</span>}
                      {it.due && (
                        <span style={{ color: dueColorVar(dueState(it.due)) }}>· {dueLabel(it.due)}</span>
                      )}
                      <button
                        type="button"
                        onClick={() => navigate(`/note/${it.note_id}`)}
                        className="hover:text-[var(--color-interactive)] hover:underline truncate max-w-[16rem]"
                        title={it.note_title}
                      >
                        · {it.note_title || "Untitled note"}
                      </button>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          </section>
        )}

        {/* The account's meetings */}
        <section className="mt-7 px-1">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-3">
            Meetings
          </h2>
          {notesForClient.length === 0 ? (
            <p className="text-sm text-[var(--color-text-muted)]">
              No meetings tagged to {client.name} yet.
            </p>
          ) : (
            <ul className="nd-notegrid">
              {notesForClient.map((n) => (
                <NoteCard
                  key={n.id}
                  note={n}
                  folder={n.folder_id ? folderById.get(n.folder_id) : undefined}
                  client={n.client_id ? clientById.get(n.client_id) : undefined}
                  recorded={recorded.has(n.id)}
                />
              ))}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
