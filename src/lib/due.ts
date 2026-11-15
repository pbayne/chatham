// Due-date helpers for action items (#followups). `due` is stored as an ISO
// date string "YYYY-MM-DD" (or "" for none). All comparisons are date-only in
// local time — a follow-up due "today" shouldn't flip to overdue at a UTC hour.

export type DueState = "none" | "overdue" | "today" | "soon" | "later";

/** Local YYYY-MM-DD for a Date (default: now). */
export function todayISO(d: Date = new Date()): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** Whole days from today to `due` (negative = past). null when `due` is unset. */
export function daysUntil(due: string): number | null {
  if (!due) return null;
  const [y, m, d] = due.split("-").map(Number);
  if (!y || !m || !d) return null;
  const target = new Date(y, m - 1, d).getTime();
  const [ty, tm, td] = todayISO().split("-").map(Number);
  const start = new Date(ty, tm - 1, td).getTime();
  return Math.round((target - start) / 86_400_000);
}

export function dueState(due: string): DueState {
  const n = daysUntil(due);
  if (n === null) return "none";
  if (n < 0) return "overdue";
  if (n === 0) return "today";
  if (n <= 3) return "soon";
  return "later";
}

/** Short human label: "Overdue", "Today", "Tomorrow", "in 3d", or a date. */
export function dueLabel(due: string): string {
  const n = daysUntil(due);
  if (n === null) return "";
  if (n < 0) return n === -1 ? "Yesterday" : `${-n}d overdue`;
  if (n === 0) return "Today";
  if (n === 1) return "Tomorrow";
  if (n <= 6) return `in ${n}d`;
  const [y, m, d] = due.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** CSS color var for a due state — overdue red, today/soon amber, else muted. */
export function dueColorVar(state: DueState): string {
  switch (state) {
    case "overdue":
      return "var(--color-danger, #e5484d)";
    case "today":
    case "soon":
      return "var(--color-warning, #d99a0b)";
    default:
      return "var(--color-text-muted)";
  }
}

/** Sort comparator for open items: dated-and-sooner first, undated last. */
export function byDue(a: { due: string }, b: { due: string }): number {
  const na = daysUntil(a.due);
  const nb = daysUntil(b.due);
  if (na === null && nb === null) return 0;
  if (na === null) return 1;
  if (nb === null) return -1;
  return na - nb;
}
