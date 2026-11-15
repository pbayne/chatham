import { useEffect, useMemo, useRef, useState } from "react";
import { ipc, type Note } from "./ipc";
import { useNotesStore, useRecordingStore } from "./store";
import type { SelectIntent } from "../components/NoteCard";

// Multi-select + bulk actions for the note grids (All notes, Folder). Extracted
// so both views share one implementation of range-select, Esc-to-clear, and the
// delete / move / transcribe / summarize batches (issue #19 + bulk processing).
//
// `orderedIds` is the visual order of the currently-rendered cards — the source
// of truth for Shift-click range selection, so the caller passes whatever its
// filter produced. `allNotes` is the full note list (for title lookups + the
// summarize "has anything to summarize" guard).
export function useNoteSelection(orderedIds: string[], allNotes: Note[]) {
  const upsert = useNotesStore((s) => s.upsertLocal);
  const removeLocal = useNotesStore((s) => s.removeLocal);
  const recorded = useNotesStore((s) => s.recordedNoteIds);
  const pushError = useRecordingStore((s) => s.pushError);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const anchorRef = useRef<string | null>(null);
  const [busy, setBusy] = useState(false);

  const orderedKey = orderedIds.join(",");
  const orderIndex = useMemo(() => {
    const m = new Map<string, number>();
    orderedIds.forEach((id, i) => m.set(id, i));
    return m;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [orderedKey]);

  function clearSelection() {
    setSelected(new Set());
    anchorRef.current = null;
  }

  function onSelectRow(id: string, e: SelectIntent) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (e.shiftKey && anchorRef.current) {
        const a = orderIndex.get(anchorRef.current);
        const b = orderIndex.get(id);
        if (a !== undefined && b !== undefined) {
          const [lo, hi] = a < b ? [a, b] : [b, a];
          for (let i = lo; i <= hi; i++) next.add(orderedIds[i]);
          return next; // anchor unchanged — repeated shift-clicks pivot on it
        }
      }
      if (next.has(id)) next.delete(id);
      else next.add(id);
      anchorRef.current = id;
      return next;
    });
  }

  // Esc clears the selection. Bubble phase so a modal's capture-phase handler
  // preempts it (Esc closes the modal first, leaving the selection intact).
  useEffect(() => {
    if (selected.size === 0) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") clearSelection();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected.size]);

  function titleFor(id: string) {
    return allNotes.find((n) => n.id === id)?.title.trim() || "Untitled";
  }

  function dropFromSelection(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
  }

  // Bulk soft-delete. Each note fires its own tombstone via the single-note
  // command. Stop-and-report: on the first failure we stop (never silently
  // skip); notes already done are dropped from the selection as we go.
  async function doDelete() {
    setBusy(true);
    try {
      for (const id of [...selected]) {
        try {
          await ipc.deleteNote(id);
          removeLocal(id);
          dropFromSelection(id);
        } catch (e) {
          pushError({ noteId: id, message: `Couldn't delete "${titleFor(id)}": ${String(e)}` });
          return;
        }
      }
      clearSelection();
    } finally {
      setBusy(false);
    }
  }

  async function doMove(folderId: string | null) {
    setBusy(true);
    try {
      for (const id of [...selected]) {
        try {
          await ipc.moveNote(id, folderId);
          const note = allNotes.find((n) => n.id === id);
          if (note) upsert({ ...note, folder_id: folderId });
          dropFromSelection(id);
        } catch (e) {
          pushError({ noteId: id, message: `Couldn't move "${titleFor(id)}": ${String(e)}` });
          return;
        }
      }
      clearSelection();
    } finally {
      setBusy(false);
    }
  }

  // Bulk transcribe pending audio. Skips notes with no audio on this device.
  // "pending" scope only touches takes that haven't been transcribed yet.
  async function doTranscribe() {
    setBusy(true);
    try {
      for (const id of [...selected]) {
        if (!recorded.has(id)) continue;
        try {
          await ipc.transcribeNote(id, "pending");
        } catch (e) {
          pushError({ noteId: id, message: `Couldn't transcribe "${titleFor(id)}": ${String(e)}` });
          return;
        }
      }
      clearSelection();
    } finally {
      setBusy(false);
    }
  }

  // Bulk summarize. Skips notes with nothing to summarize (no transcript/body).
  async function doSummarize() {
    setBusy(true);
    try {
      for (const id of [...selected]) {
        const note = allNotes.find((n) => n.id === id);
        if (note && !note.transcript.trim() && !note.body.trim()) continue;
        try {
          await ipc.summarizeNote(id);
        } catch (e) {
          pushError({ noteId: id, message: `Couldn't summarize "${titleFor(id)}": ${String(e)}` });
          return;
        }
      }
      clearSelection();
    } finally {
      setBusy(false);
    }
  }

  return {
    selected,
    busy,
    onSelectRow,
    clearSelection,
    doDelete,
    doMove,
    doTranscribe,
    doSummarize,
  };
}
