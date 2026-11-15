import { dueLabel } from "./due";

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
