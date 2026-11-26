import { useMemo, useState } from "react";
import { Check, Headphones, Loader2, Plus, Rss, Search } from "lucide-react";
import catalog from "../content/feedCatalog.json";
import { ipc, type Feed } from "../lib/ipc";
import { cn } from "../lib/cn";

// Feeds → Discover: a hand-picked catalog of popular feeds and podcasts
// (src/content/feedCatalog.json), seeded from awesome-rsshub-routes plus
// data, federal and podcast picks. Every entry is fetched and parsed by
// `feed_catalog_is_healthy` (src-tauri/src/feed_catalog_check.rs) before it
// ships, so nothing here is a dead link.

export type CatalogFeed = { name: string; url: string; blurb: string; kind: "rss" | "podcast" };
export type CatalogCategory = { name: string; blurb: string; feeds: CatalogFeed[] };
export const CATALOG = (catalog as { categories: CatalogCategory[] }).categories;

/** Loose URL identity: scheme, `www.` and a trailing slash don't make a
 *  different feed, so a subscription made by pasting matches its card. */
export function feedKey(url: string): string {
  return url
    .trim()
    .toLowerCase()
    .replace(/^https?:\/\//, "")
    .replace(/^www\./, "")
    .replace(/\/+$/, "");
}

export function Discover({
  subscribed,
  onSubscribed,
}: {
  subscribed: Feed[];
  onSubscribed: (f: Feed) => void | Promise<void>;
}) {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<string>("All");
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [failed, setFailed] = useState<Record<string, string>>({});
  const [bulk, setBulk] = useState<{ done: number; total: number } | null>(null);

  // Subscribed feeds match by their stored URL; the catalog URL may have
  // redirected on subscribe, so also remember what this session added.
  const [added, setAdded] = useState<Set<string>>(new Set());
  const have = useMemo(() => new Set([...subscribed.map((f) => feedKey(f.url)), ...added]), [subscribed, added]);
  const isSubscribed = (f: CatalogFeed) => have.has(feedKey(f.url));

  const q = query.trim().toLowerCase();
  const visible = CATALOG.filter((c) => category === "All" || c.name === category)
    .map((c) => ({
      ...c,
      feeds: c.feeds.filter((f) => !q || `${f.name} ${f.blurb}`.toLowerCase().includes(q)),
    }))
    .filter((c) => c.feeds.length > 0);

  const subscribe = async (f: CatalogFeed) => {
    setPending((p) => new Set(p).add(f.url));
    setFailed(({ [f.url]: _, ...rest }) => rest);
    try {
      const feed = await ipc.feedsAdd(f.url);
      setAdded((a) => new Set(a).add(feedKey(f.url)).add(feedKey(feed.url)));
      await onSubscribed(feed);
    } catch (e) {
      setFailed((m) => ({ ...m, [f.url]: String(e) }));
    } finally {
      setPending((p) => {
        const n = new Set(p);
        n.delete(f.url);
        return n;
      });
    }
  };

  // Subscribe to a whole category, one feed at a time so a slow host doesn't
  // stall the rest and the count reads honestly.
  const subscribeAll = async (c: CatalogCategory) => {
    const todo = c.feeds.filter((f) => !isSubscribed(f));
    setBulk({ done: 0, total: todo.length });
    for (const [i, f] of todo.entries()) {
      await subscribe(f);
      setBulk({ done: i + 1, total: todo.length });
    }
    setBulk(null);
  };

  return (
    <div className="flex-1 min-h-0 overflow-y-auto px-8 pb-16">
      <div className="max-w-[1080px]">
        <div className="flex items-center gap-2 flex-wrap mb-5 mt-1">
          <div className="relative">
            <Search
              size={14}
              strokeWidth={1.7}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-text-disabled)]"
            />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search suggestions"
              aria-label="Search suggested feeds"
              className="text-sm pl-8 pr-3 py-1.5 w-[220px] rounded-[var(--control-radius)] border border-[var(--color-line-visible)] bg-[var(--color-input-bg)] focus:outline-none focus:border-[var(--color-text-muted)]"
            />
          </div>
          <div role="tablist" aria-label="Categories" className="flex flex-wrap gap-1">
            {["All", ...CATALOG.map((c) => c.name)].map((name) => (
              <button
                key={name}
                type="button"
                role="tab"
                aria-selected={category === name}
                onClick={() => setCategory(name)}
                className={cn("nd-chip cursor-pointer", category === name && "is-active")}
              >
                {name}
              </button>
            ))}
          </div>
        </div>

        {visible.length === 0 && <p className="text-sm text-[var(--color-text-muted)]">No suggestions match “{query}”.</p>}

        {visible.map((c) => {
          const remaining = c.feeds.filter((f) => !isSubscribed(f)).length;
          return (
            <section key={c.name} aria-label={c.name} className="mb-8">
              <div className="flex items-baseline gap-3 mb-3">
                <h2 className="text-[15px] font-semibold text-[var(--color-text-display)]">{c.name}</h2>
                <p className="text-[13px] text-[var(--color-text-muted)] flex-1 truncate">{c.blurb}</p>
                {remaining > 1 && (
                  <button
                    type="button"
                    onClick={() => void subscribeAll(c)}
                    disabled={bulk !== null}
                    className="nd-btn shrink-0"
                  >
                    {bulk !== null ? `Adding ${bulk.done}/${bulk.total}…` : `Follow all ${remaining}`}
                  </button>
                )}
              </div>
              <ul className="grid grid-cols-[repeat(auto-fill,minmax(250px,1fr))] gap-2">
                {c.feeds.map((f) => {
                  const on = isSubscribed(f);
                  const busy = pending.has(f.url);
                  const err = failed[f.url];
                  return (
                    <li key={f.url} className="nd-panelcard p-3 flex items-start gap-2.5 min-w-0">
                      <span className="mt-0.5 shrink-0 text-[var(--color-text-muted)]">
                        {f.kind === "podcast" ? (
                          <Headphones size={15} strokeWidth={1.7} />
                        ) : (
                          <Rss size={15} strokeWidth={1.7} />
                        )}
                      </span>
                      <div className="min-w-0 flex-1">
                        <p className="text-sm font-medium truncate" title={f.url}>
                          {f.name}
                        </p>
                        <p className={cn("text-xs mt-0.5 line-clamp-2", err ? "text-[var(--color-danger)]" : "text-[var(--color-text-muted)]")}>
                          {err ?? f.blurb}
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() => void subscribe(f)}
                        disabled={on || busy}
                        aria-label={on ? `Following ${f.name}` : `Follow ${f.name}`}
                        title={on ? "Following" : "Follow"}
                        className={cn("nd-btn-icon nd-btn-icon-sm shrink-0", on && "text-[var(--color-interactive)]")}
                      >
                        {busy ? (
                          <Loader2 size={14} className="animate-spin" />
                        ) : on ? (
                          <Check size={14} strokeWidth={2} />
                        ) : (
                          <Plus size={14} strokeWidth={1.8} />
                        )}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          );
        })}
        <p className="text-xs text-[var(--color-text-disabled)] mt-2">
          Suggestions partly drawn from awesome-rsshub-routes (CC0). Know a feed that belongs here? Paste it in Add feed.
        </p>
      </div>
    </div>
  );
}
