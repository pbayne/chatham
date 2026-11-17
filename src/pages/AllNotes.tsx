import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Plus } from "lucide-react";
import { ipc } from "../lib/ipc";
import { useNotesStore } from "../lib/store";
import { indexById, matchesFilter, NO_FILTER, type NoteFilter } from "../lib/noteList";
import { NoteCard } from "../components/NoteCard";
import { NoteFilters } from "../components/NoteFilters";
import { useCloudStore } from "../lib/cloud";
import { BulkActionBar } from "../components/BulkActionBar";
import { useNoteSelection } from "../lib/useNoteSelection";
import { Modal } from "./settings/components/Modal";

export function AllNotes() {
  const navigate = useNavigate();
  const notes = useNotesStore((s) => s.notes);
  const folders = useNotesStore((s) => s.folders);
  const clients = useNotesStore((s) => s.clients);
  const upsert = useNotesStore((s) => s.upsertLocal);
  const [filter, setFilter] = useState<NoteFilter>(NO_FILTER);
  const myId = useCloudStore((s) => s.status.user?.id ?? null);
  const recorded = useNotesStore((s) => s.recordedNoteIds);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const sorted = useMemo(
    () => [...notes].sort((a, b) => b.created_at - a.created_at),
    [notes],
  );
  const folderById = useMemo(() => indexById(folders), [folders]);
  const clientById = useMemo(() => indexById(clients), [clients]);

  const filtered = useMemo(
    () =>
      sorted.filter((n) =>
        matchesFilter(n, filter, { recorded: recorded.has(n.id), myId }),
      ),
    [sorted, filter, recorded, myId],
  );
  const orderedIds = useMemo(() => filtered.map((n) => n.id), [filtered]);
  const total = sorted.length;
  const shown = filtered.length;

  const {
    selected,
    busy,
    onSelectRow,
    clearSelection,
    doDelete,
    doMove,
    doTranscribe,
    doSummarize,
  } = useNoteSelection(orderedIds, notes);

  async function newNote() {
    const note = await ipc.createNote();
    upsert(note);
    navigate(`/note/${note.id}`);
  }

  async function confirmedDelete() {
    await doDelete();
    setConfirmDelete(false);
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* One scrolling well: the grid runs under a translucent title bar. New
          note + theme toggle live in the floating TopBar (top-right). */}
      <div className="flex-1 overflow-y-auto nd-well flex flex-col">
        <div className="nd-well-bar shrink-0">
          <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-3">
            <div className="flex items-center gap-3 px-1">
              <h1 className="nd-heading truncate">All notes</h1>
              {/* The count follows the filter: with one on, the total behind
                  it is what says how much is being hidden. */}
              <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
                {shown === total ? total : `${shown} of ${total}`}
              </span>
            </div>
            {total > 0 && (
              <div className="px-1 pt-2">
                <NoteFilters
                  value={filter}
                  onChange={setFilter}
                  clients={clients}
                  showUnfiled={folders.length > 0}
                />
              </div>
            )}
          </div>
        </div>

        {total === 0 ? (
          <div className="flex-1 flex flex-col items-center justify-center gap-4 text-center px-12">
            <div className="text-[var(--color-text-muted)] flex items-center gap-2">
              <span>Press</span>
              <kbd
                className="px-2 py-0.5 border border-[var(--color-line-visible)] rounded text-xs"
                style={{ fontFamily: "var(--font-code)" }}
              >
                ⌘N
              </kbd>
              <span>to start a new note</span>
            </div>
            <button onClick={newNote} className="nd-btn nd-btn-primary no-drag">
              <Plus size={15} strokeWidth={1.8} />
              New note
            </button>
          </div>
        ) : shown === 0 ? (
          <div className="max-w-[1180px] mx-auto w-full px-8 pt-16 text-center text-sm text-[var(--color-text-muted)]">
            No notes match these filters.
          </div>
        ) : (
          // The grid clears the title bar's fade before the first row.
          <div className="max-w-[1180px] mx-auto w-full px-8 pt-6 pb-24">
            <ul className="nd-notegrid">
              {filtered.map((n) => (
                <NoteCard
                  key={n.id}
                  note={n}
                  folder={n.folder_id ? folderById.get(n.folder_id) : undefined}
                  client={n.client_id ? clientById.get(n.client_id) : undefined}
                  recorded={recorded.has(n.id)}
                  selected={selected.has(n.id)}
                  selectionActive={selected.size > 0}
                  onSelect={(e) => onSelectRow(n.id, e)}
                />
              ))}
            </ul>
          </div>
        )}
      </div>

      {/* Bar appears at the first selection: with an explicit checkbox affordance,
          a single pick with no visible action would read as a dead end. */}
      {selected.size >= 1 && (
        <BulkActionBar
          count={selected.size}
          folders={folders}
          busy={busy}
          onDelete={() => setConfirmDelete(true)}
          onMove={doMove}
          onTranscribe={doTranscribe}
          onSummarize={doSummarize}
          onCancel={clearSelection}
        />
      )}

      <Modal
        open={confirmDelete}
        onClose={() => {
          if (!busy) setConfirmDelete(false);
        }}
        title="Delete notes"
      >
        <p className="text-[14px] text-[var(--color-text)]">
          Move {selected.size} {selected.size === 1 ? "note" : "notes"} to Trash? You can restore
          {selected.size === 1 ? " it" : " them"} later.
        </p>
        <div className="flex justify-end gap-2 mt-5">
          <button className="nd-btn" onClick={() => setConfirmDelete(false)} disabled={busy}>
            Cancel
          </button>
          <button
            className="nd-btn"
            style={{ color: "var(--color-danger)" }}
            onClick={confirmedDelete}
            disabled={busy}
          >
            Delete
          </button>
        </div>
      </Modal>
    </div>
  );
}
