import { dueLabel, dueState } from "./due";

// Shared formatting for copying follow-ups into a status email. Used by the
// Customer page and the Follow-ups group headers so both produce identical,
// paste-ready text.

export type FollowupLike = { text: string; owner: string; due: string };

/** One "- task (owner, due …)" line. */
export function followupLine(it: FollowupLike): string {
  const meta: string[] = [];
  if (it.owner.trim()) meta.push(it.owner.trim());
  if (it.due) meta.push(`due ${dueLabel(it.due)}`);
  return `- ${it.text.trim()}${meta.length ? ` (${meta.join(", ")})` : ""}`;
}

/** A titled block: "<heading> — open follow-ups" + one line per item. */
export function followupsText(heading: string, items: FollowupLike[]): string {
  return `${heading} — open follow-ups\n${items.map(followupLine).join("\n")}`;
}

/** Which due states a Follow-ups view shows. "week" = overdue through 7 days. */
export type DueFilter = "all" | "overdue" | "week" | "undated";
/** Owner filter: "" = everyone; otherwise an exact (trimmed, case-insensitive)
    owner; NO_OWNER = items nobody was assigned. */
export const NO_OWNER = "__none__";

export function filterFollowups<T extends FollowupLike>(items: T[], owner: string, due: DueFilter): T[] {
  return items.filter((it) => {
    const o = it.owner.trim().toLowerCase();
    if (owner === NO_OWNER ? o !== "" : owner !== "" && o !== owner.trim().toLowerCase()) return false;
    const st = dueState(it.due);
    switch (due) {
      case "overdue":
        return st === "overdue";
      case "week":
        return st === "overdue" || st === "today" || st === "soon" || (st === "later" && withinDays(it.due, 7));
      case "undated":
        return st === "none";
      default:
        return true;
    }
  });
}

function withinDays(due: string, days: number): boolean {
  const [y, m, d] = due.split("-").map(Number);
  const t = new Date(y, m - 1, d).getTime();
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  return t - today <= days * 86_400_000;
}

/** Distinct owners for the picker, most items first, display-cased as first seen. */
export function followupOwners(items: FollowupLike[]): { value: string; label: string; count: number }[] {
  const byKey = new Map<string, { label: string; count: number }>();
  for (const it of items) {
    const label = it.owner.trim();
    if (!label) continue;
    const key = label.toLowerCase();
    const e = byKey.get(key) ?? { label, count: 0 };
    e.count += 1;
    byKey.set(key, e);
  }
  return [...byKey.entries()]
    .map(([value, e]) => ({ value, label: e.label, count: e.count }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}
