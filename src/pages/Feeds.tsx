import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  CheckCheck,
  FileText,
  Download,
  ExternalLink,
  Headphones,
  Loader2,
  Plus,
  RefreshCw,
  Rss,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { ipc, type Feed, type FeedItem } from "../lib/ipc";
import { cn } from "../lib/cn";
import { useNotesStore } from "../lib/store";
import { ImportDialog } from "../components/ImportDialog";

// Feeds (Labs, docs/design/feeds-labs.md): an RSS reader whose podcast
// episodes download and transcribe into ordinary notes in a Podcasts folder.
// Three columns — feeds, items, reader — because each answers one question:
// which source, which story, what does it say.

/** "All feeds" is the absent feedId route param. */
const ALL = "";

export function formatDuration(s: number): string {
  if (!s) return "";
  const h = Math.floor(s / 3600);
  const m = Math.round((s % 3600) / 60);
  return h ? `${h} h ${m} min` : `${Math.max(1, m)} min`;
}

export function formatBytes(n: number): string {
  if (n < 1024 * 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
  if (n < 1024 * 1024 * 1024) return `${Math.round(n / (1024 * 1024))} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatWhen(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) });
}

export function Feeds() {
  const { feedId = ALL } = useParams<{ feedId: string }>();
  const navigate = useNavigate();
  const [feeds, setFeeds] = useState<Feed[] | null>(null);
  const [items, setItems] = useState<FeedItem[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [adding, setAdding] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // The episode whose transcribe dialog is open.
  const [transcribing, setTranscribing] = useState<FeedItem | null>(null);
  const upsertLocal = useNotesStore((s) => s.upsertLocal);
  const upsertFolder = useNotesStore((s) => s.upsertFolder);

  const loadFeeds = useCallback(async () => {
    try {
      const fs = await ipc.feedsList();
      setFeeds(Array.isArray(fs) ? fs : []);
    } catch {
      setFeeds([]);
    }
  }, []);

  const loadItems = useCallback(async () => {
    try {
      const is = await ipc.feedItemsList(feedId || undefined, unreadOnly);
      setItems(Array.isArray(is) ? is : []);
    } catch {
      setItems([]);
    }
  }, [feedId, unreadOnly]);

  useEffect(() => {
    void loadFeeds();
  }, [loadFeeds]);
  useEffect(() => {
    setSelectedId(null);
    void loadItems();
  }, [loadItems]);

  const feedById = useMemo(() => new Map((feeds ?? []).map((f) => [f.id, f])), [feeds]);
  const current = feedId ? feedById.get(feedId) : undefined;
  const selected = items?.find((i) => i.id === selectedId) ?? null;
  const totalUnread = (feeds ?? []).reduce((n, f) => n + f.unread, 0);

  const setReadLocal = useCallback((item: FeedItem, read: boolean) => {
    if (item.read === read) return;
    setItems((cur) => cur?.map((i) => (i.id === item.id ? { ...i, read } : i)) ?? cur);
    setFeeds(
      (cur) =>
        cur?.map((f) => (f.id === item.feed_id ? { ...f, unread: Math.max(0, f.unread + (read ? -1 : 1)) } : f)) ?? cur,
    );
  }, []);

  const select = useCallback(
    (item: FeedItem) => {
      setSelectedId(item.id);
      if (!item.read) {
        // Opening an item reads it — optimistic, reconciled on failure.
        setReadLocal(item, true);
        void ipc.feedItemSetRead(item.id, true).catch(() => {
          void loadItems();
          void loadFeeds();
        });
      }
    },
    [loadFeeds, loadItems, setReadLocal],
  );

  const toggleRead = useCallback(
    (item: FeedItem) => {
      setReadLocal(item, !item.read);
      void ipc.feedItemSetRead(item.id, !item.read).catch(() => {
        void loadItems();
        void loadFeeds();
      });
    },
    [loadFeeds, loadItems, setReadLocal],
  );

  const run = async (work: () => Promise<string | void>) => {
    setRefreshing(true);
    try {
      const msg = await work();
      if (msg) setNotice(msg);
      await Promise.all([loadFeeds(), loadItems()]);
    } catch (e) {
      setNotice(String(e));
    } finally {
      setRefreshing(false);
    }
  };

  const refresh = () =>
    run(async () => {
      const n = await ipc.feedsRefresh(feedId || undefined);
      return n === 0 ? "Nothing new." : `${n} new item${n === 1 ? "" : "s"}.`;
    });

  const markAllRead = () => run(() => ipc.feedItemsMarkAllRead(feedId || undefined));

  const importOpml = async () => {
    const path = await openDialog({ multiple: false, filters: [{ name: "OPML", extensions: ["opml", "xml"] }] });
    if (typeof path !== "string") return;
    await run(async () => {
      const r = await ipc.feedsImportOpml(path);
      return (
        `Imported ${r.added} feed${r.added === 1 ? "" : "s"}` +
        (r.failed.length ? ` · ${r.failed.length} couldn't be reached` : "") +
        "."
      );
    });
  };

  const exportOpml = async () => {
    const path = await saveDialog({ defaultPath: "chatham-feeds.opml", filters: [{ name: "OPML", extensions: ["opml"] }] });
    if (!path) return;
    try {
      await ipc.feedsExportOpml(path);
      setNotice("Exported your feeds.");
    } catch (e) {
      setNotice(String(e));
    }
  };

  // Download + transcribe. Resolves once the import is running; the note then
  // fills in live, so go there. Errors (busy recording, unsupported format,
  // download failure) surface inline in the dialog.
  const confirmTranscribe = async (language: string, expectedSpeakers: number | null) => {
    if (!transcribing) return;
    const { note, folder } = await ipc.feedItemTranscribe(transcribing.id, language, expectedSpeakers);
    upsertFolder(folder);
    upsertLocal(note);
    setTranscribing(null);
    navigate(`/note/${note.id}`);
  };

  const remove = async (f: Feed) => {
    if (!window.confirm(`Unsubscribe from “${f.title || f.url}”? Notes made from its episodes stay.`)) return;
    await run(async () => {
      await ipc.feedsRemove(f.id);
      if (feedId === f.id) navigate("/feeds");
    });
  };

  // j / k walk the list, m toggles read, o opens the original. Ignored while
  // typing so the add-feed field keeps its letters.
  const listRef = useRef<HTMLUListElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      if (e.metaKey || e.ctrlKey || e.altKey || adding || transcribing || !items?.length) return;
      const idx = items.findIndex((i) => i.id === selectedId);
      if (e.key === "j" || e.key === "k") {
        const next = items[Math.min(items.length - 1, Math.max(0, idx + (e.key === "j" ? 1 : -1)))] ?? items[0];
        select(next);
        listRef.current?.querySelector(`[data-item="${next.id}"]`)?.scrollIntoView?.({ block: "nearest" });
        e.preventDefault();
      } else if (e.key === "m" && selected) {
        toggleRead(selected);
      } else if (e.key === "o" && selected?.link) {
        void openExternal(selected.link);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [items, selectedId, selected, select, toggleRead, adding, transcribing]);

  const empty = feeds !== null && feeds.length === 0;

  return (
    <div className="h-full flex flex-col overflow-hidden nd-well">
      <div className="nd-well-bar shrink-0">
        <div className="w-full px-8 pt-14 pb-4">
          <div className="flex items-center gap-2.5 px-1 min-w-0">
            <Rss size={22} strokeWidth={1.6} className="shrink-0 text-[var(--color-interactive)]" />
            <h1 className="nd-heading truncate">{current?.title || "Feeds"}</h1>
            <span className="nd-chip shrink-0">Labs</span>
            {totalUnread > 0 && !current && (
              <span className="text-[14px] text-[var(--color-text-disabled)] tabular-nums shrink-0">{totalUnread} unread</span>
            )}
            <div className="flex-1" />
            {notice && (
              <span className="text-[13px] text-[var(--color-text-muted)] truncate max-w-[40ch]" role="status">
                {notice}
              </span>
            )}
            {!empty && (
              <>
                <button
                  type="button"
                  onClick={() => setUnreadOnly((v) => !v)}
                  aria-pressed={unreadOnly}
                  className={cn("nd-btn shrink-0", unreadOnly && "nd-btn-primary")}
                >
                  Unread only
                </button>
                <button
                  type="button"
                  onClick={markAllRead}
                  className="nd-btn-icon shrink-0"
                  title="Mark all read"
                  aria-label="Mark all read"
                >
                  <CheckCheck size={16} strokeWidth={1.7} />
                </button>
                <button
                  type="button"
                  onClick={refresh}
                  disabled={refreshing}
                  className="nd-btn-icon shrink-0"
                  title="Refresh"
                  aria-label="Refresh"
                >
                  <RefreshCw size={16} strokeWidth={1.7} className={cn(refreshing && "animate-spin")} />
                </button>
              </>
            )}
            <button type="button" onClick={() => setAdding(true)} className="nd-btn nd-btn-primary shrink-0">
              <Plus size={15} strokeWidth={1.8} />
              Add feed
            </button>
          </div>
        </div>
      </div>

      {empty ? (
        <EmptyState onAdd={() => setAdding(true)} onImport={importOpml} />
      ) : (
        <div className="flex-1 min-h-0 grid grid-cols-[200px_minmax(260px,340px)_minmax(0,1fr)] px-8 pb-6 pt-1">
          {/* Feeds */}
          <nav className="min-h-0 overflow-y-auto pr-3" aria-label="Feeds">
            <FeedRow
              active={feedId === ALL}
              onClick={() => navigate("/feeds")}
              icon={<Rss size={14} strokeWidth={1.7} />}
              label="All feeds"
              count={totalUnread}
            />
            {(feeds ?? []).map((f) => (
              <FeedRow
                key={f.id}
                active={feedId === f.id}
                onClick={() => navigate(`/feeds/${f.id}`)}
                icon={f.kind === "podcast" ? <Headphones size={14} strokeWidth={1.7} /> : <Rss size={14} strokeWidth={1.7} />}
                label={f.title || f.url}
                count={f.unread}
                error={f.error}
                onRemove={() => void remove(f)}
              />
            ))}
            <div className="mt-3 flex flex-col border-t border-[var(--color-line)] pt-3">
              <button type="button" onClick={importOpml} className="nd-navrow w-full text-[13px]">
                <Upload size={14} strokeWidth={1.7} /> Import OPML…
              </button>
              <button type="button" onClick={exportOpml} className="nd-navrow w-full text-[13px]">
                <Download size={14} strokeWidth={1.7} /> Export OPML…
              </button>
            </div>
          </nav>

          {/* Items */}
          <section className="min-h-0 overflow-y-auto nd-panelcard" aria-label="Items">
            {items === null ? (
              <p className="p-4 text-sm text-[var(--color-text-muted)]">Loading…</p>
            ) : items.length === 0 ? (
              <p className="p-4 text-sm text-[var(--color-text-muted)]">
                {unreadOnly ? "You're all caught up." : "No items yet — try Refresh."}
              </p>
            ) : (
              <ul ref={listRef}>
                {items.map((it) => {
                  const feed = feedById.get(it.feed_id);
                  const meta = [
                    !current && feed?.title,
                    formatWhen(it.published_at),
                    formatDuration(it.duration_s),
                    it.note_id && "Transcribed",
                  ]
                    .filter(Boolean)
                    .join(" · ");
                  return (
                    <li key={it.id} data-item={it.id}>
                      <button
                        type="button"
                        onClick={() => select(it)}
                        aria-current={it.id === selectedId ? "true" : undefined}
                        className={cn(
                          "w-full text-left px-4 py-3 border-b border-[var(--color-line)] transition-colors",
                          it.id === selectedId ? "bg-[var(--color-accent-soft)]" : "hover:bg-[var(--color-pill-hover)]",
                        )}
                      >
                        <div className="flex items-start gap-2">
                          <span
                            aria-label={it.read ? undefined : "Unread"}
                            className={cn(
                              "mt-[7px] shrink-0 w-1.5 h-1.5 rounded-full",
                              it.read ? "bg-transparent" : "bg-[var(--color-interactive)]",
                            )}
                          />
                          <div className="min-w-0 flex-1">
                            <p
                              className={cn(
                                "text-sm leading-snug line-clamp-2",
                                // The open item keeps full contrast even though
                                // opening it marked it read.
                                it.id === selectedId
                                  ? "text-[var(--color-text)]"
                                  : it.read
                                    ? "text-[var(--color-text-muted)]"
                                    : "font-medium",
                              )}
                            >
                              {it.title || "(untitled)"}
                            </p>
                            {meta && (
                              <p className="text-xs text-[var(--color-text-disabled)] mt-1 truncate tabular-nums">{meta}</p>
                            )}
                          </div>
                        </div>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          {/* Reader */}
          <article className="min-h-0 overflow-y-auto pl-8" aria-label="Reader">
            {selected ? (
              <Reader
                item={selected}
                feed={feedById.get(selected.feed_id)}
                onTranscribe={() => setTranscribing(selected)}
                onOpenNote={(id) => navigate(`/note/${id}`)}
              />
            ) : (
              <p className="pt-3 text-sm text-[var(--color-text-disabled)]">
                Pick an item. <kbd className="nd-kbd">j</kbd> <kbd className="nd-kbd">k</kbd> to move,{" "}
                <kbd className="nd-kbd">m</kbd> read/unread, <kbd className="nd-kbd">o</kbd> open original.
              </p>
            )}
          </article>
        </div>
      )}

      {transcribing && (
        <TranscribeDialog item={transcribing} onCancel={() => setTranscribing(null)} onConfirm={confirmTranscribe} />
      )}

      {adding && (
        <AddFeed
          onClose={() => setAdding(false)}
          onAdded={async (f) => {
            setAdding(false);
            setNotice(`Subscribed to ${f.title || f.url}.`);
            await loadFeeds();
            navigate(`/feeds/${f.id}`);
          }}
        />
      )}
    </div>
  );
}

function FeedRow({
  active,
  onClick,
  icon,
  label,
  count,
  error,
  onRemove,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  count: number;
  error?: string;
  onRemove?: () => void;
}) {
  return (
    <div className="group relative">
      <button
        type="button"
        onClick={onClick}
        aria-current={active ? "page" : undefined}
        className={cn("nd-navrow w-full", onRemove && "pr-8", active && "is-active")}
        title={error ? `Last refresh failed: ${error}` : label}
      >
        <span className={cn("shrink-0", error ? "text-[var(--color-danger)]" : "opacity-85")}>{icon}</span>
        <span className="flex-1 truncate text-left">{label}</span>
        {count > 0 && (
          <span className={cn("text-xs tabular-nums text-[var(--color-text-muted)]", onRemove && "group-hover:invisible")}>
            {count}
          </span>
        )}
      </button>
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          aria-label={`Unsubscribe from ${label}`}
          title="Unsubscribe"
          className="absolute right-1 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 focus-visible:opacity-100 nd-btn-icon nd-btn-icon-sm"
        >
          <Trash2 size={13} strokeWidth={1.7} />
        </button>
      )}
    </div>
  );
}

function Reader({
  item,
  feed,
  onTranscribe,
  onOpenNote,
}: {
  item: FeedItem;
  feed?: Feed;
  onTranscribe: () => void;
  onOpenNote: (noteId: string) => void;
}) {
  const byline = [feed?.title, item.author, formatWhen(item.published_at)].filter(Boolean).join(" · ");
  return (
    <div className="max-w-[68ch] pt-2 pb-16">
      {byline && <p className="text-xs text-[var(--color-text-muted)] mb-2">{byline}</p>}
      <h2 className="text-[22px] leading-tight font-semibold mb-3 text-[var(--color-text-display)]">
        {item.title || "(untitled)"}
      </h2>
      <div className="flex flex-wrap items-center gap-2 mb-6">
        {item.link && (
          <button type="button" onClick={() => void openExternal(item.link)} className="nd-btn">
            <ExternalLink size={14} strokeWidth={1.7} />
            Open original
          </button>
        )}
        {item.enclosure_url &&
          (item.note_id ? (
            <button type="button" onClick={() => onOpenNote(item.note_id!)} className="nd-btn nd-btn-primary">
              <FileText size={14} strokeWidth={1.7} />
              Open transcript
            </button>
          ) : (
            <button type="button" onClick={onTranscribe} className="nd-btn nd-btn-primary">
              <AudioLines size={14} strokeWidth={1.7} />
              Transcribe episode
            </button>
          ))}
        {item.enclosure_url && (
          <span className="inline-flex items-center gap-1.5 text-sm text-[var(--color-text-muted)]">
            <Headphones size={14} strokeWidth={1.7} />
            Episode{item.duration_s ? ` · ${formatDuration(item.duration_s)}` : ""}
            {item.enclosure_bytes > 0 ? ` · ${formatBytes(item.enclosure_bytes)}` : ""}
          </span>
        )}
      </div>
      {item.content_html ? (
        // Sanitized in Rust (feeds::sanitize_html): no scripts, event handlers,
        // iframes or images; links rel=noopener. Clicks on links open the
        // system browser instead of navigating the app's webview.
        <div
          className="prose-note feeds-article"
          onClick={(e) => {
            const a = (e.target as HTMLElement).closest("a");
            if (a?.href) {
              e.preventDefault();
              void openExternal(a.href);
            }
          }}
          dangerouslySetInnerHTML={{ __html: item.content_html }}
        />
      ) : (
        <p className="text-sm text-[var(--color-text-muted)]">
          This item has no text in the feed — open the original to read it.
        </p>
      )}
    </div>
  );
}

function EmptyState({ onAdd, onImport }: { onAdd: () => void; onImport: () => void }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center text-center px-8 pb-24">
      <Rss size={40} strokeWidth={1.3} className="text-[var(--color-text-disabled)] mb-3" />
      <p className="text-[15px] font-medium">Follow blogs and podcasts</p>
      <p className="text-sm text-[var(--color-text-muted)] mt-1 max-w-[46ch]">
        Paste a feed or a website address. A podcast episode can be transcribed into a note, where search, chat and
        summaries work on it like any meeting.
      </p>
      <div className="flex items-center gap-2 mt-5">
        <button type="button" onClick={onAdd} className="nd-btn nd-btn-primary">
          <Plus size={15} strokeWidth={1.8} /> Add feed
        </button>
        <button type="button" onClick={onImport} className="nd-btn">
          <Upload size={14} strokeWidth={1.7} /> Import OPML…
        </button>
      </div>
    </div>
  );
}

function AddFeed({ onClose, onAdded }: { onClose: () => void; onAdded: (f: Feed) => void | Promise<void> }) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await onAdded(await ipc.feedsAdd(url));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[18vh] bg-black/30" onClick={onClose}>
      <form
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
        className="nd-panelcard w-[min(520px,90vw)] p-5 shadow-lg"
        role="dialog"
        aria-modal="true"
        aria-label="Add a feed"
      >
        <div className="flex items-center mb-3">
          <h2 className="text-[15px] font-semibold flex-1">Add a feed</h2>
          <button type="button" onClick={onClose} className="nd-btn-icon nd-btn-icon-sm" aria-label="Close">
            <X size={14} />
          </button>
        </div>
        <input
          autoFocus
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/feed or a website"
          aria-label="Feed or website address"
          className="w-full text-sm px-3 py-2 rounded-[var(--control-radius)] border border-[var(--color-line-visible)] bg-[var(--color-input-bg)] focus:outline-none focus:border-[var(--color-text-muted)]"
        />
        {error && <p className="text-[13px] text-[var(--color-danger)] mt-2">{error}</p>}
        <div className="flex justify-end gap-2 mt-4">
          <button type="button" onClick={onClose} className="nd-btn">
            Cancel
          </button>
          <button type="submit" disabled={busy || !url.trim()} className="nd-btn nd-btn-primary">
            {busy ? <Loader2 size={14} className="animate-spin" /> : <Plus size={15} strokeWidth={1.8} />}
            {busy ? "Subscribing…" : "Subscribe"}
          </button>
        </div>
      </form>
    </div>
  );
}

/** The audio-import dialog (language + speakers), with live download progress
 *  as its busy label: a long episode takes a while to fetch before the import
 *  can start. */
function TranscribeDialog({
  item,
  onCancel,
  onConfirm,
}: {
  item: FeedItem;
  onCancel: () => void;
  onConfirm: (language: string, expectedSpeakers: number | null) => Promise<void>;
}) {
  const [progress, setProgress] = useState<{ received: number; total: number } | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    let alive = true;
    listen<{ item_id: string; received: number; total: number }>("feed_download_progress", (e) => {
      if (e.payload.item_id === item.id) setProgress({ received: e.payload.received, total: e.payload.total });
    })
      .then((u) => (alive ? (off = u) : u()))
      .catch(() => {});
    return () => {
      alive = false;
      off?.();
    };
  }, [item.id]);
  return (
    <ImportDialog
      path={item.enclosure_url}
      label={item.title || "Podcast episode"}
      title="Transcribe episode"
      confirmLabel="Download & transcribe"
      busyLabel={downloadLabel(progress)}
      onCancel={onCancel}
      onConfirm={onConfirm}
    />
  );
}

export function downloadLabel(p: { received: number; total: number } | null): string {
  if (!p) return "Starting download…";
  if (p.total > 0 && p.received >= p.total) return "Starting transcription…";
  if (p.total > 0) return `Downloading… ${Math.floor((p.received / p.total) * 100)}%`;
  return `Downloading… ${formatBytes(p.received)}`;
}
