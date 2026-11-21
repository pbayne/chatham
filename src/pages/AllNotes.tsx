import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Plus, Search, LayoutGrid, List as ListIcon, X } from "lucide-react";
import { ipc } from "../lib/ipc";
import { useNotesStore } from "../lib/store";
import { indexById, matchesFilter, noteExcerpt, NO_FILTER, type NoteFilter } from "../lib/noteList";
import { NoteCard } from "../components/NoteCard";
import { NoteFilters } from "../components/NoteFilters";
import { useCloudStore } from "../lib/cloud";
import { BulkActionBar } from "../components/BulkActionBar";
import { useNoteSelection } from "../lib/useNoteSelection";
import { Modal } from "./settings/components/Modal";
import { cn } from "../lib/cn";

// How the list is ordered. Persisted so the choice survives a reload.
type SortKey = "newest" | "oldest" | "title";
// How each note is drawn: tall cards in a grid, or compact full-width rows.
type ViewMode = "cards" | "rows";

const SORT_STORE = "allnotes.sort";
const VIEW_STORE = "allnotes.view";

function loadPref<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
  } catch {
    return fallback;
  }
}

// The text a search query is matched against: title + the note's excerpt
// (summary / body / transcript opening). Lowercased once per note per query.
function searchHaystack(n: { title: string } & Parameters<typeof noteExcerpt>[0]): string {
  return `${n.title} ${noteExcerpt(n)}`.toLowerCase();
}

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

  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>(() =>
    loadPref<SortKey>(SORT_STORE, ["newest", "oldest", "title"], "newest"),
  );
  const [view, setView] = useState<ViewMode>(() =>
    loadPref<ViewMode>(VIEW_STORE, ["cards", "rows"], "cards"),
  );
  useEffect(() => {
    try {
      localStorage.setItem(SORT_STORE, sort);
    } catch {
      /* private mode / disabled storage: the choice just won't persist */
    }
  }, [sort]);
  useEffect(() => {
    try {
      localStorage.setItem(VIEW_STORE, view);
    } catch {
      /* ignore */
    }
  }, [view]);

  const sorted = useMemo(() => {
    const copy = [...notes];
    copy.sort((a, b) => {
      if (sort === "oldest") return a.created_at - b.created_at;
      if (sort === "title") {
        return (a.title.trim() || "Untitled").localeCompare(
          b.title.trim() || "Untitled",
          undefined,
          { sensitivity: "base" },
        );
      }
      return b.created_at - a.created_at; // newest
    });
    return copy;
  }, [notes, sort]);

  const folderById = useMemo(() => indexById(folders), [folders]);
  const clientById = useMemo(() => indexById(clients), [clients]);

  const q = query.trim().toLowerCase();
  const filtered = useMemo(
    () =>
      sorted.filter((n) => {
        if (!matchesFilter(n, filter, { recorded: recorded.has(n.id), myId })) return false;
        if (q && !searchHaystack(n).includes(q)) return false;
        return true;
      }),
    [sorted, filter, recorded, myId, q],
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
      <div className="flex-1 overflow-y-auto nd-well flex flex-col">
        <div className="nd-well-bar shrink-0">
          <div className="max-w-[1180px] mx-auto w-full px-8 pt-14 pb-3">
            <div className="flex items-center gap-3 px-1">
              <h1 className="nd-heading truncate">All notes</h1>
              <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">
                {shown === total ? total : `${shown} of ${total}`}
              </span>
            </div>

            {total > 0 && (
              <div className="px-1 pt-3 flex flex-col gap-2.5">
                {/* Search + sort + view controls. */}
                <div className="flex items-center gap-2 flex-wrap">
                  <label className="relative flex-1 min-w-[180px]">
                    <Search
                      size={14}
                      strokeWidth={1.8}
                      aria-hidden
                      className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-text-disabled)] pointer-events-none"
                    />
                    <input
                      type="search"
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder="Search notes…"
                      aria-label="Search notes"
                      className="w-full text-[13.5px] pl-8 pr-8 py-1.5 rounded-[var(--radius-control)] border border-[var(--color-line-visible)] bg-transparent focus:outline-none focus:border-[var(--color-text-muted)]"
                    />
                    {query && (
                      <button
                        type="button"
                        onClick={() => setQuery("")}
                        aria-label="Clear search"
                        className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--color-text-disabled)] hover:text-[var(--color-text)]"
                      >
                        <X size={14} strokeWidth={2} />
                      </button>
                    )}
                  </label>

                  <select
                    value={sort}
                    onChange={(e) => setSort(e.target.value as SortKey)}
                    aria-label="Sort notes"
                    className="text-[13px] px-2.5 py-1.5 rounded-[var(--radius-control)] border border-[var(--color-line-visible)] bg-transparent focus:outline-none focus:border-[var(--color-text-muted)] cursor-pointer"
                  >
                    <option value="newest">Latest first</option>
                    <option value="oldest">Oldest first</option>
                    <option value="title">Title A–Z</option>
                  </select>

                  {/* Cards / rows toggle. */}
                  <div
                    role="group"
                    aria-label="View mode"
                    className="flex items-center rounded-[var(--radius-control)] border border-[var(--color-line-visible)] overflow-hidden"
                  >
                    <button
                      type="button"
                      aria-label="Card view"
                      aria-pressed={view === "cards"}
                      onClick={() => setView("cards")}
                      className={cn(
                        "px-2 py-1.5 transition-colors",
                        view === "cards"
                          ? "bg-[var(--color-accent-soft)] text-[var(--color-accent-text)]"
                          : "text-[var(--color-text-muted)] hover:text-[var(--color-text)]",
                      )}
                    >
                      <LayoutGrid size={15} strokeWidth={1.8} />
                    </button>
                    <button
                      type="button"
                      aria-label="Row view"
                      aria-pressed={view === "rows"}
                      onClick={() => setView("rows")}
                      className={cn(
                        "px-2 py-1.5 transition-colors border-l border-[var(--color-line-visible)]",
                        view === "rows"
                          ? "bg-[var(--color-accent-soft)] text-[var(--color-accent-text)]"
                          : "text-[var(--color-text-muted)] hover:text-[var(--color-text)]",
                      )}
                    >
                      <ListIcon size={15} strokeWidth={1.8} />
                    </button>
                  </div>
                </div>

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
            {q ? `No notes match “${query.trim()}”.` : "No notes match these filters."}
          </div>
        ) : (
          <div className="max-w-[1180px] mx-auto w-full px-8 pt-6 pb-24">
            <ul className={view === "rows" ? "nd-notelist" : "nd-notegrid"}>
              {filtered.map((n) => (
                <NoteCard
                  key={n.id}
                  note={n}
                  variant={view === "rows" ? "row" : "card"}
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
