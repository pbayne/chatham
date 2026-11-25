import { memo, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Loader2, RefreshCw, Sparkles } from "lucide-react";
import { ipc, onSummary as onSummaryReady, onSummaryContentDelta, type FeedItem, type Note } from "../lib/ipc";
import { sanitizeMarkdownAngles } from "../lib/markdownSafe";
import { cn } from "../lib/cn";

// Per-item AI summary in the Feeds reader (docs/design/feeds-labs.md).
//
// - Article: summarised on demand from its text (or the page, when the feed
//   carries only a teaser) and stored on the feed item.
// - Episode: summarised from its transcript, which is a note — so the summary
//   IS the note's summary (podcast preset), written automatically once the
//   transcript and speakers land. Shown here read-only, live as it streams.

const Markdown = memo(function Markdown({ source }: { source: string }) {
  return <ReactMarkdown remarkPlugins={[remarkGfm]}>{sanitizeMarkdownAngles(source)}</ReactMarkdown>;
});

export function ItemSummary({
  item,
  onSummary,
}: {
  item: FeedItem;
  /** An article summary was written; the parent keeps its copy in sync. */
  onSummary: (itemId: string, summary: string) => void;
}) {
  const episode = item.enclosure_url !== "";
  // Episodes read their note; articles read the item.
  const [note, setNote] = useState<Note | null>(null);
  const [stream, setStream] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const streamKey = episode ? item.note_id : item.id;

  useEffect(() => {
    setStream("");
    setError(null);
    setBusy(false);
    setNote(null);
    if (!episode || !item.note_id) return;
    let alive = true;
    const load = () =>
      ipc
        .getNote(item.note_id!)
        .then((n) => alive && setNote(n))
        .catch(() => alive && setNote(null));
    void load();
    // The episode summary arrives after transcription; refresh when it does.
    let off: (() => void) | undefined;
    onSummaryReady((e) => {
      if (e.noteId === item.note_id) {
        setStream("");
        void load();
      }
    })
      .then((u) => (alive ? (off = u) : u()))
      .catch(() => {});
    return () => {
      alive = false;
      off?.();
    };
  }, [episode, item.id, item.note_id]);

  useEffect(() => {
    if (!streamKey) return;
    let alive = true;
    let off: (() => void) | undefined;
    onSummaryContentDelta((e) => {
      if (e.noteId === streamKey) setStream((s) => s + e.delta);
    })
      .then((u) => (alive ? (off = u) : u()))
      .catch(() => {});
    return () => {
      alive = false;
      off?.();
    };
  }, [streamKey]);

  const saved = episode ? (note?.summary ?? "") : item.summary;

  const summarize = async () => {
    setBusy(true);
    setError(null);
    setStream("");
    try {
      if (episode && item.note_id) {
        await ipc.summarizeNote(item.note_id);
        setNote(await ipc.getNote(item.note_id));
      } else {
        onSummary(item.id, await ipc.feedItemSummarize(item.id));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setStream("");
    }
  };

  // An untranscribed episode has nothing to summarise yet; the Transcribe
  // button above says what to do, so stay out of the way.
  if (episode && !item.note_id) return null;

  const streaming = stream.length > 0;
  const showing = streaming ? stream : saved;
  // Episode summaries are produced by the transcription pipeline; until then
  // the note exists but has no summary.
  const pending = episode && !saved && !streaming && !busy && !error;

  return (
    <section aria-label="Summary" className="nd-panelcard p-4 mb-6">
      <div className="flex items-center gap-2 mb-2">
        <Sparkles size={14} strokeWidth={1.7} className="text-[var(--color-interactive)]" />
        <h3 className="text-[13px] font-semibold flex-1">{episode ? "Episode summary" : "Summary"}</h3>
        {(saved || error) && !busy && !streaming && (
          <button
            type="button"
            onClick={summarize}
            className="nd-btn-icon nd-btn-icon-sm"
            title="Summarize again"
            aria-label="Summarize again"
          >
            <RefreshCw size={13} strokeWidth={1.7} />
          </button>
        )}
      </div>
      {showing ? (
        <div className={cn("prose-summary text-sm leading-relaxed", streaming && "opacity-90")}>
          <Markdown source={showing} />
        </div>
      ) : busy ? (
        <p className="text-sm text-[var(--color-text-muted)] inline-flex items-center gap-2">
          <Loader2 size={14} className="animate-spin" /> Summarizing…
        </p>
      ) : pending ? (
        <div className="flex items-center gap-3">
          <p className="text-sm text-[var(--color-text-muted)] flex-1">
            The summary is written once the transcript and speakers are done.
          </p>
          <button type="button" onClick={summarize} className="nd-btn shrink-0">
            Summarize now
          </button>
        </div>
      ) : (
        <div className="flex items-center gap-3">
          <p className="text-sm text-[var(--color-text-muted)] flex-1">
            {error ?? "Get the gist before you read: a one-line overview and the key points."}
          </p>
          <button type="button" onClick={summarize} className="nd-btn nd-btn-primary shrink-0">
            <Sparkles size={14} strokeWidth={1.7} /> Summarize
          </button>
        </div>
      )}
      {error && showing && <p className="text-[13px] text-[var(--color-danger)] mt-2">{error}</p>}
    </section>
  );
}
