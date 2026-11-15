import { useMemo } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Users, User } from "lucide-react";
import { useNotesStore } from "../lib/store";
import { indexById } from "../lib/noteList";
import { extractSpeakerLabels, isNamedPerson } from "../lib/speakers";
import { NoteCard } from "../components/NoteCard";
import type { Note } from "../lib/ipc";

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
                  className="w-full flex items-center gap-3 rounded-[var(--radius-panel)] border border-[var(--color-line)] p-3 text-left hover:bg-[var(--color-surface-hover)] transition-colors"
                >
                  <span className="shrink-0 w-8 h-8 rounded-full bg-[var(--color-surface-sunken)] flex items-center justify-center text-[var(--color-text-muted)]">
                    <User size={16} strokeWidth={1.7} />
                  </span>
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

  const theirNotes = useMemo(
    () =>
      notes
        .filter((n) => n.transcript?.trim() && extractSpeakerLabels(n.transcript).some((l) => l.trim() === name))
        .sort((a, b) => b.created_at - a.created_at),
    [notes, name],
  );

  return (
    <div className="h-full overflow-y-auto nd-well">
      <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-24">
        <div className="flex items-center gap-2.5 mb-1 px-1">
          <button
            type="button"
            onClick={() => navigate("/people")}
            className="shrink-0 w-9 h-9 rounded-full bg-[var(--color-surface-sunken)] flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-interactive)] transition-colors"
            title="All people"
          >
            <User size={18} strokeWidth={1.7} />
          </button>
          <h1 className="nd-heading truncate">{name}</h1>
          <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
            {theirNotes.length} {theirNotes.length === 1 ? "meeting" : "meetings"}
          </span>
        </div>

        <section className="mt-6 px-1">
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
