import { type MouseEvent, type KeyboardEvent, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { Link } from "react-router-dom";
import { Building2, Check, Folder as FolderIcon, Lock, Users, FileText, AudioLines, Captions, Sparkles, type LucideIcon } from "lucide-react";
import { type Client, type Folder, type Note } from "../lib/ipc";
import { formatNoteDate, noteExcerpt, noteState, noteFacets, NOTE_STATE_LABEL, NOTE_FACET_LABEL, NOTE_FACET_COLOR, NOTE_FACET_ORDER, type NoteState, type NoteFacet } from "../lib/noteList";
import { cn } from "../lib/cn";
import { noteActivity } from "../lib/noteActivity";
import { useRecordingStore } from "../lib/store";
import { formatTime, useCaptureElapsed } from "../lib/captureClock";

// What a note holds, as a glyph: notes body, audio, transcript, summary.
// The colours come from NOTE_FACET_COLOR so the icon and the (card) chip agree.
const FACET_ICON: Record<NoteFacet, LucideIcon> = {
  notes: FileText,
  audio: AudioLines,
  transcribed: Captions,
  summarized: Sparkles,
};

// A row of facet glyphs — the at-a-glance "what's in this note" readout.
// `title` on each gives the hover label; the set is also announced for a11y.
function FacetIcons({ facets, size = 13 }: { facets: Set<NoteFacet>; size?: number }) {
  const present = NOTE_FACET_ORDER.filter((f) => facets.has(f));
  if (present.length === 0) return null;
  return (
    <span className="inline-flex items-center gap-1.5 shrink-0" aria-label={`Contains: ${present.map((f) => NOTE_FACET_LABEL[f]).join(", ")}`}>
      {present.map((f) => {
        const Icon = FACET_ICON[f];
        return (
          <span key={f} title={NOTE_FACET_LABEL[f]} className="inline-flex">
            <Icon
              size={size}
              strokeWidth={1.8}
              aria-hidden
              style={{ color: NOTE_FACET_COLOR[f] }}
            />
          </span>
        );
      })}
    </span>
  );
}


// Selection intent reported to the parent. Only the shift flag matters (range
// vs toggle); works for a modifier-click, the checkbox, and the keyboard toggle.
export type SelectIntent = { shiftKey: boolean };

// The card carries no legend, so the colour is named rather than left to
// stand on its own.
// Recorded is the one state naming work left to do — audio with no text yet —
// so it takes the warning colour.
const STATE_COLOR: Record<NoteState, string> = {
  summarized: "var(--color-accent-text)",
  transcribed: "var(--color-interactive)",
  recorded: "var(--color-warning)",
  notes: "var(--color-text-muted)",
  empty: "var(--color-text-disabled)",
};

// One note in a grid view (All notes, Folder). Nothing above the grid says
// when, so the card carries its own full date.
//
// A modifier-click selects instead of navigating: Cmd toggles, Shift extends a
// range. Ctrl is excluded — on macOS ctrl-click is the OS secondary click, so
// treating it as a selection modifier triple-fires. The checkbox is a sibling
// of the <Link>, never nested inside the anchor, so toggling it cannot
// navigate; Space on a focused card toggles too, leaving Enter to the link.
// Its own component so only the card whose capture this is pays the tick.
function LiveElapsed() {
  const elapsed = useCaptureElapsed();
  return <span className="tabular-nums opacity-80">{formatTime(elapsed)}</span>;
}

export function NoteCard({
  note,
  folder,
  client,
  showFolder = true,
  recorded = false,
  selected = false,
  selectionActive = false,
  variant = "card",
  onSelect,
}: {
  note: Note;
  folder?: Folder;
  client?: Client;
  /** False inside a folder view, where the note's folder is the view itself. */
  showFolder?: boolean;
  /** This note holds at least one recording session. */
  recorded?: boolean;
  selected?: boolean;
  /** True when any note in the view is selected. Forces every checkbox visible. */
  selectionActive?: boolean;
  /** "card" = tall grid cell; "row" = compact full-width line. */
  variant?: "card" | "row";
  onSelect?: (e: SelectIntent) => void;
}) {
  const title = note.title.trim() || "Untitled";
  const excerpt = noteExcerpt(note);
  const key = noteState(note, recorded);
  const state = { label: NOTE_STATE_LABEL[key], color: STATE_COLOR[key] };
  // Facet chips: what the note HAS (independent of the single state dot).
  const facets = noteFacets(note, recorded);
  // `useShallow` because the helper builds a fresh object each call: without it
  // every card re-renders on every store tick.
  const activity = useRecordingStore(useShallow((s) => noteActivity(note.id, s)));
  const shown = activity ?? state;
  const checkboxShown = selected || selectionActive;
  // The checkbox's toggle fires via onChange (which carries no modifier flags),
  // so we stash the click's shift state here for the onChange to read.
  const shiftRef = useRef(false);

  function handleClick(e: MouseEvent) {
    if (onSelect && (e.metaKey || e.shiftKey)) {
      e.preventDefault();
      onSelect({ shiftKey: e.shiftKey });
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (onSelect && e.key === " ") {
      e.preventDefault();
      onSelect({ shiftKey: e.shiftKey });
    }
  }

  function handleCheckboxClick(e: MouseEvent) {
    shiftRef.current = e.shiftKey;
  }

  function handleCheckboxChange() {
    onSelect?.({ shiftKey: shiftRef.current });
    shiftRef.current = false;
  }

  // Row variant: a compact full-width line — date, title, state dot, and the
  // same checkbox — for scanning many notes at once. Reuses the .nd-notecard
  // surface (padding is tightened by .nd-notelist .nd-notecard) and all the
  // same selection behaviour as the card.
  if (variant === "row") {
    return (
      <li className="nd-notecard group relative" data-selected={selected ? "true" : undefined}>
        <Link
          to={`/note/${note.id}`}
          onClick={handleClick}
          onKeyDown={handleKeyDown}
          aria-selected={selected}
          data-selected={selected ? "true" : undefined}
          className="flex items-center gap-3 min-w-0 focus:outline-none pr-8"
        >
          <span
            aria-hidden
            className={cn(
              "w-[7px] h-[7px] rounded-full shrink-0",
              activity?.animated && "rec-dot",
            )}
            style={{ background: shown.color }}
            title={shown.label}
          />
          <span className="text-[15px] font-medium leading-tight text-[var(--color-text)] truncate min-w-0 flex-1">
            {title}
          </span>
          <FacetIcons facets={facets} size={13} />
          {client && (
            <span className="hidden sm:inline-flex items-center gap-1 text-[11.5px] text-[var(--color-text-muted)] shrink-0 max-w-[10rem]">
              <Building2 size={11} strokeWidth={1.7} aria-hidden className="opacity-70 shrink-0" />
              <span className="truncate">{client.name}</span>
            </span>
          )}
          {showFolder && folder && (
            <span className="hidden sm:inline-flex items-center gap-1 text-[11.5px] text-[var(--color-text-muted)] shrink-0 max-w-[10rem]">
              <FolderIcon size={11} strokeWidth={1.7} aria-hidden className="opacity-70 shrink-0" />
              <span className="truncate">{folder.name}</span>
            </span>
          )}
          <span className="text-[11.5px] text-[var(--color-text-disabled)] tabular-nums shrink-0 w-[92px] text-right">
            {activity?.timed ? <LiveElapsed /> : formatNoteDate(note.created_at)}
          </span>
        </Link>
        {onSelect && (
          <div
            className={cn(
              "absolute top-1/2 -translate-y-1/2 right-3 w-[24px] h-[24px] grid place-items-center transition-opacity duration-150",
              checkboxShown ? "opacity-100" : "opacity-0 group-hover:opacity-100 focus-within:opacity-100",
            )}
          >
            <input
              type="checkbox"
              checked={selected}
              aria-label={`Select ${title}`}
              onClick={handleCheckboxClick}
              onChange={handleCheckboxChange}
              className="peer absolute inset-0 m-0 w-full h-full appearance-none opacity-0 cursor-pointer"
            />
            <span
              aria-hidden
              className={cn(
                "w-[17px] h-[17px] rounded-full border transition-colors pointer-events-none",
                "border-[var(--color-line-visible)] bg-[var(--color-surface)] peer-hover:border-[var(--color-text-muted)]",
                "peer-checked:border-[var(--color-accent-text)] peer-checked:bg-[var(--color-accent-text)]",
              )}
            />
            <Check
              aria-hidden
              size={11}
              strokeWidth={3}
              className="absolute opacity-0 peer-checked:opacity-100 text-[var(--color-canvas)] pointer-events-none"
            />
          </div>
        )}
      </li>
    );
  }

  return (
    <li className="nd-notecard group relative" data-selected={selected ? "true" : undefined}>
      <Link
        to={`/note/${note.id}`}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        aria-selected={selected}
        data-selected={selected ? "true" : undefined}
        className="flex-1 flex flex-col min-w-0 focus:outline-none"
      >
        <div className="text-[11.5px] text-[var(--color-text-disabled)] tabular-nums">
          {formatNoteDate(note.created_at)}
        </div>
        {/* Room for the checkbox in the corner, so a long title can't run under it. */}
        <h3 className="mt-1.5 pr-7 text-[16.5px] font-semibold leading-[1.28] tracking-[-0.015em] text-[var(--color-text)] line-clamp-2">
          {title}
        </h3>
        {excerpt && (
          <p className="mt-2 text-[13.5px] leading-[1.55] text-[var(--color-text-muted)] line-clamp-4">
            {excerpt}
          </p>
        )}
        <div className="mt-auto pt-4 flex items-center gap-x-2.5 gap-y-1.5 flex-wrap text-[11.5px]">
          <span
            className="inline-flex items-center gap-1.5 whitespace-nowrap"
            style={{ color: shown.color }}
          >
            <span
              aria-hidden
              className={cn(
                "w-[6px] h-[6px] rounded-full shrink-0",
                activity?.animated && "rec-dot",
              )}
              style={{ background: shown.color }}
            />
            {shown.label}
            {activity?.timed && <LiveElapsed />}
          </span>
          {/* Facet glyphs: color-coded icons for what the note holds (body,
              audio, transcript, summary), so you can tell at a glance what's
              done vs still to do. */}
          <FacetIcons facets={facets} size={14} />
          {note.private && note.workspace_id && (
            // Only inside a workspace: every Personal note is private, so the
            // glyph would say nothing there and appear on every card (#191).
            <span className="inline-flex items-center gap-1.5 min-w-0 text-[var(--color-text-muted)]">
              {note.shared_with?.length ? (
                <>
                  <Users size={12} strokeWidth={1.7} aria-hidden className="shrink-0 opacity-70" />
                  <span>{note.shared_with.length + 1} people</span>
                </>
              ) : (
                <>
                  <Lock size={12} strokeWidth={1.7} aria-hidden className="shrink-0 opacity-70" />
                  <span>Private</span>
                </>
              )}
            </span>
          )}
          {client && (
            <span className="inline-flex items-center gap-1.5 min-w-0 text-[var(--color-text-muted)]">
              <Building2 size={12} strokeWidth={1.7} aria-hidden className="shrink-0 opacity-70" />
              <span className="truncate max-w-[9rem]">{client.name}</span>
            </span>
          )}
          {showFolder && folder && (
            <span className="inline-flex items-center gap-1.5 min-w-0 text-[var(--color-text-muted)]">
              <FolderIcon size={12} strokeWidth={1.7} aria-hidden className="shrink-0 opacity-70" />
              <span className="truncate max-w-[9rem]">{folder.name}</span>
            </span>
          )}
        </div>
      </Link>

      {onSelect && (
        // The native input is a transparent overlay filling a ~26px hit target;
        // the ring and check are painted on top and are pointer-transparent so
        // clicks reach it.
        <div
          className={cn(
            "absolute top-3.5 right-3.5 w-[26px] h-[26px] grid place-items-center transition-opacity duration-150",
            checkboxShown
              ? "opacity-100"
              : "opacity-0 group-hover:opacity-100 focus-within:opacity-100",
          )}
        >
          <input
            type="checkbox"
            checked={selected}
            aria-label={`Select ${title}`}
            data-shown={checkboxShown ? "true" : "false"}
            onClick={handleCheckboxClick}
            onChange={handleCheckboxChange}
            className="peer absolute inset-0 m-0 w-full h-full appearance-none opacity-0 cursor-pointer"
          />
          <span
            aria-hidden
            className={cn(
              "w-[18px] h-[18px] rounded-full border transition-colors pointer-events-none",
              "border-[var(--color-line-visible)] bg-[var(--color-surface)] peer-hover:border-[var(--color-text-muted)]",
              "peer-checked:border-[var(--color-accent-text)] peer-checked:bg-[var(--color-accent-text)]",
              "peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--color-accent-text)] peer-focus-visible:ring-offset-1 peer-focus-visible:ring-offset-[var(--color-surface)]",
            )}
          />
          <Check
            aria-hidden
            size={12}
            strokeWidth={3}
            className="absolute opacity-0 peer-checked:opacity-100 text-[var(--color-canvas)] pointer-events-none"
          />
        </div>
      )}
    </li>
  );
}
