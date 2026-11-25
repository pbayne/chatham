import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Building2, Circle, ListTodo, Users } from "lucide-react";
import { useNotesStore } from "../lib/store";
import { indexById } from "../lib/noteList";
import { extractSpeakerLabels, isNamedPerson } from "../lib/speakers";
import { NoteCard } from "../components/NoteCard";
import { ipc, type Note, type OpenActionItem } from "../lib/ipc";
import { dueColorVar, dueLabel, dueState } from "../lib/due";
import { initials, nameHue, personActivity } from "../lib/person";
import { SPEAKER_COLORS } from "../components/SpeakerLabels";

// People directory (#people): everyone who's spoken across your meetings builds
// into a self-organizing contact list. Reuses the speaker labels already in
// each transcript — rename "Speaker 1" → "Hege" anywhere and she appears here.
// Placeholders ("Speaker 2", "You") are filtered until named. Parallels the
// Customer view: a page per person listing the meetings they were in.

type PersonEntry = { name: string; notes: Note[] };

function buildPeople(notes: Note[]): PersonEntry[] {
  const byName = new Map<string, Note[]>();
  for (const n of notes) {
    if (!n.transcript?.trim()) continue;
    for (const label of extractSpeakerLabels(n.transcript)) {
      if (!isNamedPerson(label)) continue;
      const key = label.trim();
      if (!byName.has(key)) byName.set(key, []);
      byName.get(key)!.push(n);
    }
  }
  return [...byName.entries()]
    .map(([name, ns]) => ({ name, notes: ns.sort((a, b) => b.created_at - a.created_at) }))
    .sort((a, b) => b.notes.length - a.notes.length || a.name.localeCompare(b.name));
}

export function People() {
  const navigate = useNavigate();
  const notes = useNotesStore((s) => s.notes);
  const people = useMemo(() => buildPeople(notes), [notes]);

  return (
    <div className="h-full overflow-y-auto nd-well">
      <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-24">
        <div className="flex items-center gap-2.5 mb-1 px-1">
          <Users size={22} strokeWidth={1.6} className="text-[var(--color-interactive)]" />
          <h1 className="nd-heading truncate">People</h1>
          <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
            {people.length}
          </span>
        </div>
        <p className="text-sm text-[var(--color-text-muted)] mb-6 px-1">
          {people.length === 0
            ? "People you've named in transcripts show up here. Rename a “Speaker 1” to someone's name and they'll appear."
            : "Everyone across your meetings — click to see every conversation with them."}
        </p>

        {people.length > 0 && (
          <ul className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2 px-1">
            {people.map((p) => (
              <li key={p.name}>
                <button
                  type="button"
                  onClick={() => navigate(`/person/${encodeURIComponent(p.name)}`)}
                  className="nd-personcard w-full flex items-center gap-3 p-3 text-left"
                >
                  <Avatar name={p.name} />
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm font-medium truncate">{p.name}</span>
                    <span className="block text-xs text-[var(--color-text-muted)] tabular-nums">
                      {p.notes.length} {p.notes.length === 1 ? "meeting" : "meetings"}
                    </span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export function Person() {
  const { label } = useParams<{ label: string }>();
  const name = label ? decodeURIComponent(label) : "";
  const navigate = useNavigate();
  const notes = useNotesStore((s) => s.notes);
  const folders = useNotesStore((s) => s.folders);
  const clients = useNotesStore((s) => s.clients);
  const recorded = useNotesStore((s) => s.recordedNoteIds);
  const folderById = useMemo(() => indexById(folders), [folders]);
  const clientById = useMemo(() => indexById(clients), [clients]);

  const [open, setOpen] = useState<OpenActionItem[]>([]);
  const load = useCallback(() => {
    ipc.actionItemsOpen()
      .then((rs) => setOpen(Array.isArray(rs) ? rs : []))
      .catch(() => setOpen([]));
  }, []);
  useEffect(load, [load, notes.length]);

  const activity = useMemo(() => personActivity(name, notes, open, clients), [name, notes, open, clients]);
  const theirNotes = activity.notes;

  // Optimistic, and reloads on failure so a write that didn't land comes back
  // (the Follow-ups view's rule).
  const complete = (itemId: string) => {
    setOpen((cur) => cur.filter((i) => i.id !== itemId));
    void ipc.actionItemsUpdate(itemId, { done: true }).catch(load);
  };

  return (
    <div className="h-full overflow-y-auto nd-well">
      <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-24">
        <div className="flex items-center gap-2.5 mb-1 px-1">
          <button
            type="button"
            onClick={() => navigate("/people")}
            className="shrink-0 rounded-full hover:opacity-80 transition-opacity"
            title="All people"
          >
            <Avatar name={name} size={36} />
          </button>
          <h1 className="nd-heading truncate">{name}</h1>
          <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
            {theirNotes.length} {theirNotes.length === 1 ? "meeting" : "meetings"}
          </span>
        </div>

        {/* Who they are to you: last met, and under which customers. */}
        {(activity.lastSeen !== null || activity.clients.length > 0) && (
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-[var(--color-text-muted)] px-1 mt-1">
            {activity.lastSeen !== null && (
              <span>Last met {new Date(activity.lastSeen).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" })}</span>
            )}
            {activity.clients.map((c) => (
              <button
                key={c.id}
                type="button"
                onClick={() => navigate(`/client/${c.id}`)}
                className="inline-flex items-center gap-1 hover:text-[var(--color-interactive)] transition-colors"
                title={`${c.meetings} ${c.meetings === 1 ? "meeting" : "meetings"} with ${c.name}`}
              >
                <Building2 size={13} strokeWidth={1.7} />
                {c.name}
              </button>
            ))}
          </p>
        )}

        {/* What they owe — open action items whose owner is this person. */}
        {activity.commitments.length > 0 && (
          <section className="mt-6 px-1">
            <h2 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-2">
              <ListTodo size={14} strokeWidth={1.7} />
              Open commitments
              <span className="font-normal">{activity.commitments.length}</span>
            </h2>
            <ul className="flex flex-col max-w-2xl">
              {activity.commitments.map((it) => (
                <li
                  key={it.id}
                  className="flex items-start gap-3 py-1.5 border-b border-[var(--color-line)] last:border-0"
                >
                  <button
                    type="button"
                    aria-label="Mark done"
                    onClick={() => complete(it.id)}
                    className="mt-0.5 text-[var(--color-text-muted)] hover:text-[var(--color-success)] transition-colors shrink-0"
                  >
                    <Circle size={16} strokeWidth={1.6} />
                  </button>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm leading-snug">{it.text}</p>
                    <div className="flex flex-wrap items-center gap-x-2.5 text-xs text-[var(--color-text-muted)] mt-0.5">
                      {it.due && <span style={{ color: dueColorVar(dueState(it.due)) }}>{dueLabel(it.due)}</span>}
                      <button
                        type="button"
                        onClick={() => navigate(`/note/${it.note_id}`)}
                        className="hover:text-[var(--color-interactive)] hover:underline truncate max-w-[16rem]"
                        title={it.note_title}
                      >
                        {it.due ? "· " : ""}{it.note_title || "Untitled note"}
                      </button>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          </section>
        )}

        <section className="mt-6 px-1">
          {activity.commitments.length > 0 && theirNotes.length > 0 && (
            <h2 className="text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-3">Meetings</h2>
          )}
          {theirNotes.length === 0 ? (
            <p className="text-sm text-[var(--color-text-muted)]">No meetings with {name}.</p>
          ) : (
            <ul className="nd-notegrid">
              {theirNotes.map((n) => (
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

/** Initials on a soft tint of the person's speaker colour. The colour is keyed
    on the name (nameHue), so it is the same on People, Person and every page
    that adopts it — a transcript's per-note order can't give that. */
function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  const color = SPEAKER_COLORS[nameHue(name, SPEAKER_COLORS.length)];
  return (
    <span
      aria-hidden
      className="shrink-0 rounded-full flex items-center justify-center font-semibold tabular-nums select-none"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.38),
        color,
        background: `color-mix(in srgb, ${color} 16%, var(--color-surface))`,
      }}
    >
      {initials(name)}
    </span>
  );
}
