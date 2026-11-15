import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { Check, CheckCircle2, Circle, Copy, ListTodo } from "lucide-react";
import { ipc, type OpenActionItem } from "../lib/ipc";
import { byDue, dueColorVar, dueLabel, dueState } from "../lib/due";
import { followupsText } from "../lib/followups";

// Follow-ups (#followups): one running list of every open action item across
// all meetings, grouped by customer so "what do I owe Fisher Investments?" is
// one glance. Checking an item marks it done and drops it from the list. This
// is the daily driver — open it, work the list, done.

export function FollowUps() {
  const navigate = useNavigate();
  const [items, setItems] = useState<OpenActionItem[] | null>(null);

  const load = useCallback(() => {
    ipc.actionItemsOpen()
      .then((rs) => setItems(Array.isArray(rs) ? rs : []))
      .catch(() => setItems([]));
  }, []);

  useEffect(() => {
    load();
    const un = listen("action_items_changed", () => load());
    return () => {
      void un.then((f) => f());
    };
  }, [load]);

  const complete = (id: string) => {
    // Optimistic: drop it immediately, then persist.
    setItems((cur) => (cur ? cur.filter((i) => i.id !== id) : cur));
    void ipc.actionItemsUpdate(id, { done: true }).catch(load);
  };

  // Group by customer (client_name), with an "Unassigned" bucket last. Within a
  // group, keep the newest-note-first order the backend already applied.
  const groups = useMemo(() => {
    if (!items) return [];
    const byClient = new Map<string, { id: string | null; name: string; items: OpenActionItem[] }>();
    for (const it of items) {
      const key = it.client_id ?? "__none__";
      const name = it.client_name ?? "No customer";
      if (!byClient.has(key)) byClient.set(key, { id: it.client_id, name, items: [] });
      byClient.get(key)!.items.push(it);
    }
    const arr = [...byClient.values()];
    // Within each customer, dated-and-soonest first (overdue at the top).
    for (const g of arr) g.items.sort(byDue);
    // Customers alphabetical; the "No customer" bucket always last.
    arr.sort((a, b) => {
      if (a.name === "No customer") return 1;
      if (b.name === "No customer") return -1;
      return a.name.localeCompare(b.name);
    });
    return arr;
  }, [items]);

  const total = items?.length ?? 0;
  const overdue = useMemo(
    () => (items ?? []).filter((i) => dueState(i.due) === "overdue").length,
    [items],
  );

  const [copiedGroup, setCopiedGroup] = useState<string | null>(null);
  const copyGroup = (heading: string, groupItems: OpenActionItem[]) => {
    navigator.clipboard?.writeText(followupsText(heading, groupItems)).then(
      () => {
        setCopiedGroup(heading);
        window.setTimeout(() => setCopiedGroup((c) => (c === heading ? null : c)), 1500);
      },
      () => {},
    );
  };

  return (
    <div className="flex-1 min-h-0 overflow-y-auto">
      <div className="max-w-3xl mx-auto px-6 py-8">
        <div className="flex items-center gap-2.5 mb-1">
          <ListTodo size={20} strokeWidth={1.6} className="text-[var(--color-interactive)]" />
          <h1 className="text-xl font-semibold">Follow-ups</h1>
        </div>
        <p className="text-sm text-[var(--color-text-muted)] mb-6">
          {total === 0
            ? "You're all caught up — no open action items."
            : `${total} open item${total === 1 ? "" : "s"} across your meetings, by customer.`}
          {overdue > 0 && (
            <span style={{ color: dueColorVar("overdue") }}> · {overdue} overdue</span>
          )}
        </p>

        {items === null ? (
          <p className="text-sm text-[var(--color-text-muted)]">Loading…</p>
        ) : total === 0 ? (
          <div className="flex flex-col items-center justify-center py-20 text-center">
            <CheckCircle2 size={40} strokeWidth={1.3} className="text-[var(--color-success)] mb-3" />
            <p className="text-sm text-[var(--color-text-muted)]">
              Nothing to follow up on. Action items from your meetings show up here automatically.
            </p>
          </div>
        ) : (
          <div className="flex flex-col gap-7">
            {groups.map((g) => (
              <div key={g.name}>
                <h2 className="flex items-center text-xs font-semibold uppercase tracking-wide text-[var(--color-text-muted)] mb-2.5">
                  {g.id ? (
                    <button
                      type="button"
                      onClick={() => navigate(`/client/${g.id}`)}
                      className="hover:text-[var(--color-interactive)] transition-colors uppercase"
                    >
                      {g.name}
                    </button>
                  ) : (
                    g.name
                  )}
                  <span className="ml-2 font-normal normal-case">{g.items.length}</span>
                  <button
                    type="button"
                    onClick={() => copyGroup(g.name, g.items)}
                    className="ml-auto inline-flex items-center gap-1 normal-case font-normal hover:text-[var(--color-interactive)] transition-colors"
                    title={`Copy ${g.name} follow-ups for a status email`}
                  >
                    {copiedGroup === g.name ? (
                      <Check size={12} strokeWidth={1.8} />
                    ) : (
                      <Copy size={12} strokeWidth={1.7} />
                    )}
                    {copiedGroup === g.name ? "Copied" : "Copy"}
                  </button>
                </h2>
                <ul className="flex flex-col">
                  {g.items.map((it) => (
                    <li
                      key={it.id}
                      className="group flex items-start gap-3 py-2 border-b border-[var(--color-line)] last:border-0"
                    >
                      <button
                        type="button"
                        aria-label="Mark done"
                        onClick={() => complete(it.id)}
                        className="mt-0.5 text-[var(--color-text-muted)] hover:text-[var(--color-success)] transition-colors shrink-0"
                      >
                        <Circle size={17} strokeWidth={1.6} />
                      </button>
                      <div className="min-w-0 flex-1">
                        <p className="text-sm leading-snug">{it.text}</p>
                        <div className="flex flex-wrap items-center gap-x-2.5 gap-y-0.5 mt-0.5 text-xs text-[var(--color-text-muted)]">
                          {it.owner && <span>{it.owner}</span>}
                          {it.due && (
                            <span style={{ color: dueColorVar(dueState(it.due)) }}>
                              · {dueLabel(it.due)}
                            </span>
                          )}
                          <button
                            type="button"
                            onClick={() => navigate(`/note/${it.note_id}`)}
                            className="hover:text-[var(--color-interactive)] hover:underline truncate max-w-[16rem]"
                            title={it.note_title}
                          >
                            · {it.note_title || "Untitled note"}
                          </button>
                        </div>
                      </div>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
