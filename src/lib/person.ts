import type { Client, Note, OpenActionItem } from "./ipc";
import { byDue } from "./due";
import { extractSpeakerLabels } from "./speakers";

// Person intelligence (roadmap Phase D, docs/design/meeting-intelligence-roadmap.md):
// what one named speaker owes, when you last met, and under which customers —
// derived on the client from data already loaded (notes + open action items),
// so the Person page needs no new backend command.

export type PersonActivity = {
  /** Meetings they spoke in, newest first. */
  notes: Note[];
  /** Open action items whose owner is this person, soonest due first. */
  commitments: OpenActionItem[];
  /** created_at of their most recent meeting, or null. */
  lastSeen: number | null;
  /** Customers their meetings are filed under, most meetings first. */
  clients: { id: string; name: string; meetings: number }[];
};

/** How an extractor-written owner ("Kari", "kari nordvik.") relates to a
    speaker name: the full name, the first name only, or not at all. Trailing
    punctuation is dropped; the extractor writes names as spoken. */
export function ownerMatch(owner: string, name: string): "full" | "first" | null {
  const o = owner.trim().toLowerCase().replace(/[.,;:!?]+$/, "");
  const n = name.trim().toLowerCase();
  if (!o || !n) return null;
  if (o === n) return "full";
  const first = n.split(/\s+/)[0];
  return first.length > 1 && o === first ? "first" : null;
}

export function ownerMatches(owner: string, name: string): boolean {
  return ownerMatch(owner, name) !== null;
}

export function personActivity(
  name: string,
  notes: Note[],
  open: OpenActionItem[],
  clients: Client[],
): PersonActivity {
  const key = name.trim();
  const theirs = notes
    .filter((n) => n.transcript?.trim() && extractSpeakerLabels(n.transcript).some((l) => l.trim() === key))
    .sort((a, b) => b.created_at - a.created_at);
  const spokeIn = new Set(theirs.map((n) => n.id));
  const clientName = new Map(clients.map((c) => [c.id, c.name]));
  const counts = new Map<string, number>();
  for (const n of theirs) if (n.client_id) counts.set(n.client_id, (counts.get(n.client_id) ?? 0) + 1);
  return {
    notes: theirs,
    // A first-name-only owner is ambiguous (two Karis), so it counts only when
    // the item comes from a meeting this person spoke in.
    commitments: open
      .filter((i) => {
        const m = ownerMatch(i.owner, key);
        return m === "full" || (m === "first" && spokeIn.has(i.note_id));
      })
      .sort(byDue),
    lastSeen: theirs[0]?.created_at ?? null,
    clients: [...counts.entries()]
      .filter(([id]) => clientName.has(id))
      .map(([id, meetings]) => ({ id, name: clientName.get(id)!, meetings }))
      .sort((a, b) => b.meetings - a.meetings || a.name.localeCompare(b.name)),
  };
}
